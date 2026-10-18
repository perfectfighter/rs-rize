//! Schema-1 accounting and calendar operations, ported from the v1.2 desktop domain.
//! Money is represented in currency minor units. Rates are integer millionths;
//! intermediate multiplication uses u128 and rounds half up exactly once.
use crate::Result;
use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const SAFE: u64 = 9_007_199_254_740_991;
fn object<'a>(v: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| format!("{name} must be an object."))
}
fn default<'a>(v: &'a Value, key: &str, fallback: &'a Value) -> &'a Value {
    v.get(key).filter(|x| !x.is_null()).unwrap_or(fallback)
}
fn text(v: &Value, name: &str, max: usize, required: bool) -> Result<String> {
    let s = v
        .as_str()
        .ok_or_else(|| format!("{name} must be valid text."))?;
    if s.encode_utf16().count() > max
        || s.chars()
            .any(|c| matches!(c as u32, 0..=8 | 11..=12 | 14..=31 | 127))
    {
        return Err(format!("{name} must be valid text."));
    }
    let s = s.trim().to_string();
    if required && s.is_empty() {
        return Err(format!("{name} is required."));
    }
    Ok(s)
}
fn txt(v: &Value, key: &str, name: &str, max: usize) -> Result<String> {
    text(
        default(v, key, &Value::String(String::new())),
        name,
        max,
        false,
    )
}
fn id(v: &Value) -> Result<String> {
    text(v, "ID", 160, true)
}
fn number(v: &Value, name: &str, min: f64, max: f64) -> Result<f64> {
    let n = v
        .as_f64()
        .ok_or_else(|| format!("{name} must be between {min} and {max}."))?;
    if !n.is_finite() || n < min || n > max {
        Err(format!("{name} must be between {min} and {max}."))
    } else {
        Ok(n)
    }
}
fn integer(v: &Value, name: &str, min: u64, max: u64) -> Result<u64> {
    let n = number(v, name, min as f64, max as f64)?;
    if n.fract() != 0.0 {
        Err(format!("{name} must be an integer."))
    } else {
        Ok(n as u64)
    }
}
fn uint(v: &Value, key: &str, name: &str, fallback: u64, max: u64) -> Result<u64> {
    integer(default(v, key, &json!(fallback)), name, 0, max)
}
fn array<'a>(v: &'a Value, name: &str, max: usize) -> Result<&'a Vec<Value>> {
    let a = v
        .as_array()
        .ok_or_else(|| format!("{name} must be an array of at most {max} items."))?;
    if a.len() > max {
        Err(format!("{name} must be an array of at most {max} items."))
    } else {
        Ok(a)
    }
}
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn n(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}
fn canonical(mut v: Value) -> Value {
    fn visit(v: &mut Value) {
        match v {
            Value::Array(a) => a.iter_mut().for_each(visit),
            Value::Object(m) => m.values_mut().for_each(visit),
            Value::Number(n) if n.is_f64() && n.as_f64().unwrap().fract() == 0. => {
                let f = n.as_f64().unwrap();
                if f.abs() <= SAFE as f64 {
                    *v = json!(f as i64)
                }
            }
            _ => (),
        }
    }
    visit(&mut v);
    v
}
fn new_id() -> String {
    Uuid::new_v4().to_string()
}
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
fn unique(a: Vec<Value>, name: &str) -> Result<Vec<Value>> {
    let mut seen = HashSet::new();
    for v in &a {
        if !seen.insert(s(v, "id").to_string()) {
            return Err(format!("Duplicate {name} ID."));
        }
    }
    Ok(a)
}
fn parse_date(v: &Value, name: &str) -> Result<String> {
    let s = v.as_str().unwrap_or("");
    if s.len() != 10
        || NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|d| d.format("%Y-%m-%d").to_string() != s)
            .unwrap_or(true)
    {
        Err(format!("{name} must be a valid YYYY-MM-DD date."))
    } else {
        Ok(s.to_string())
    }
}
fn period(start: &Value, end: &Value) -> Result<(String, String)> {
    let a = parse_date(start, "Start date")?;
    let b = parse_date(end, "End date")?;
    if b < a {
        return Err("End date must follow start date.".into());
    }
    Ok((a, b))
}
fn today(now: i64) -> Result<String> {
    DateTime::<Utc>::from_timestamp_millis(now)
        .map(|x| x.format("%Y-%m-%d").to_string())
        .ok_or("Invalid current timestamp.".into())
}

// Frozen to the baseline Node/ICU ISO-currency support, including obsolete codes
// accepted by v1.2 backups. No FX conversion is performed.
pub fn currency_digits(code: &str) -> Result<u32> {
    const CODES:&str="AED AFN ALL AMD ANG AOA ARS AUD AWG AZN BAM BBD BDT BGN BHD BIF BMD BND BOB BRL BSD BTN BWP BYN BZD CAD CDF CHF CLP CNY COP CRC CUC CUP CVE CZK DJF DKK DOP DZD EGP ERN ETB EUR FJD FKP GBP GEL GHS GIP GMD GNF GTQ GYD HKD HNL HRK HTG HUF IDR ILS INR IQD IRR ISK JMD JOD JPY KES KGS KHR KMF KPW KRW KWD KYD KZT LAK LBP LKR LRD LSL LYD MAD MDL MGA MKD MMK MNT MOP MRU MUR MVR MWK MXN MYR MZN NAD NGN NIO NOK NPR NZD OMR PAB PEN PGK PHP PKR PLN PYG QAR RON RSD RUB RWF SAR SBD SCR SDG SEK SGD SHP SLE SLL SOS SRD SSP STN SVC SYP SZL THB TJS TMT TND TOP TRY TTD TWD TZS UAH UGX USD UYU UZS VES VND VUV WST XAF XCD XCG XDR XOF XPF XSU YER ZAR ZMW ZWG ZWL";
    if !CODES.split(' ').any(|x| x == code) {
        return Err("Choose a supported ISO currency.".into());
    }
    Ok(if "AFN ALL BIF CLP COP DJF GNF HUF IDR IQD IRR ISK JPY KMF KPW KRW LAK LBP MGA MMK PKR PYG RWF SLL SOS SYP UGX VND VUV XAF XOF XPF YER".split(' ').any(|x|x==code){0}else if "BHD JOD KWD LYD OMR TND".split(' ').any(|x|x==code){3}else{2})
}
fn currency(v: &Value) -> Result<String> {
    let c = v.as_str().ok_or("Choose a supported ISO currency.")?;
    currency_digits(c)?;
    Ok(c.into())
}
fn rate(v: &Value, name: &str) -> Result<f64> {
    let r = number(v, name, 0., 1_000_000.)?;
    if (r * 1e6 - (r * 1e6).round()).abs() > 0.0001 {
        return Err(format!("{name} supports at most six decimal places."));
    }
    Ok(r)
}
fn ratio(n: u128, d: u128) -> Result<u64> {
    if d == 0 {
        return Err("Invalid money calculation.".into());
    }
    let x = n
        .checked_add(d / 2)
        .ok_or("Money amount exceeds safe limits.")?
        / d;
    if x > SAFE as u128 {
        return Err("Money amount exceeds safe limits.".into());
    }
    Ok(x as u64)
}
pub fn charge(duration_ms: f64, hourly_rate: f64, digits: u32) -> Result<u64> {
    if !duration_ms.is_finite()
        || duration_ms < 0.
        || !hourly_rate.is_finite()
        || hourly_rate < 0.
        || digits > 3
    {
        return Err("Invalid money calculation.".into());
    }
    let micro = (hourly_rate * 1e6).round() as u128;
    let ms = duration_ms.round() as u128;
    let value = micro
        .checked_mul(ms)
        .and_then(|x| x.checked_mul(10u128.pow(digits)))
        .ok_or("Money amount exceeds safe limits.")?;
    ratio(value, 3_600_000_000_000)
}
fn reference(v: &Value, all: &[Value], name: &str, required: bool) -> Result<String> {
    let empty = json!("");
    let t = text(if v.is_null() { &empty } else { v }, name, 160, required)?;
    if !t.is_empty() && !all.iter().any(|x| s(x, "id") == t) {
        return Err(format!("{name} does not exist."));
    }
    Ok(t)
}
pub fn default_business() -> Value {
    json!({"profile":{"name":"","address":"","email":"","paymentInstructions":"","invoicePrefix":"INV-"},"clientTerms":[],"contracts":[],"invoices":[],"allocations":[],"labels":[],"nextInvoiceNumber":1})
}
fn profile(v: &Value) -> Result<Value> {
    object(v, "Business profile")?;
    Ok(
        json!({"name":txt(v,"name","Business name",200)?,"address":txt(v,"address","Business address",2000)?,"email":txt(v,"email","Business email",320)?,"paymentInstructions":txt(v,"paymentInstructions","Payment instructions",4000)?,"invoicePrefix":text(default(v,"invoicePrefix",&json!("INV-")),"Invoice prefix",30,true)?}),
    )
}
fn terms(v: &Value, state: &Value) -> Result<Value> {
    object(v, "Client terms")?;
    Ok(
        json!({"id":id(&v["id"])?,"clientId":reference(&v["clientId"],items(state,"clients"),"Client",true)?,"contact":txt(v,"contact","Contact",200)?,"email":txt(v,"email","Email",320)?,"address":txt(v,"address","Address",2000)?,"currency":currency(default(v,"currency",&json!("USD")))?,"paymentDays":uint(v,"paymentDays","Payment days",30,365)?}),
    )
}
fn contract(v: &Value, state: &Value) -> Result<Value> {
    object(v, "Contract")?;
    let client = reference(&v["clientId"], items(state, "clients"), "Client", true)?;
    let project = reference(&v["projectId"], items(state, "projects"), "Project", false)?;
    if !project.is_empty()
        && items(state, "projects")
            .iter()
            .find(|x| s(x, "id") == project)
            .map(|x| s(x, "clientId") != client)
            .unwrap_or(false)
    {
        return Err("Contract project must belong to the selected client.".into());
    }
    let kind = s(v, "type");
    if !["hourly", "retainer", "hybrid"].contains(&kind) {
        return Err("Contract type must be hourly, retainer, or hybrid.".into());
    }
    let start = parse_date(&v["startDate"], "Contract start")?;
    let end = if truthy(&v["endDate"]) {
        parse_date(&v["endDate"], "Contract end")?
    } else {
        String::new()
    };
    if !end.is_empty() && end < start {
        return Err("Contract end must follow start.".into());
    }
    if kind != "hourly" && end.is_empty() {
        return Err("Retainer and hybrid contracts need a fixed end date.".into());
    }
    Ok(
        json!({"id":id(&v["id"])?,"clientId":client,"projectId":project,"type":kind,"hourlyRate":rate(default(v,"hourlyRate",&json!(0)),"Hourly rate")?,"retainerMinor":uint(v,"retainerMinor","Retainer amount",0,SAFE)?,"includedHours":number(default(v,"includedHours",&json!(0)),"Included hours",0.,100000.)?,"budgetHours":number(default(v,"budgetHours",&json!(0)),"Budget hours",0.,1000000.)?,"costRate":rate(default(v,"costRate",&json!(0)),"Cost rate")?,"startDate":start,"endDate":end,"currency":currency(default(v,"currency",&json!("USD")))?}),
    )
}
fn allocation(v: &Value, state: &Value) -> Result<Value> {
    object(v, "Allocation")?;
    let week = parse_date(&v["week"], "Allocation week")?;
    if NaiveDate::parse_from_str(&week, "%Y-%m-%d")
        .unwrap()
        .weekday()
        .number_from_monday()
        != 1
    {
        return Err("Allocation week must be a Monday.".into());
    }
    Ok(
        json!({"id":id(&v["id"])?,"memberId":id(&v["memberId"])?,"memberName":text(&v["memberName"],"Member name",200,true)?,"projectId":reference(&v["projectId"],items(state,"projects"),"Project",true)?,"week":week,"hours":number(&v["hours"],"Allocated hours",0.,168.)?,"capacityHours":number(default(v,"capacityHours",&json!(40)),"Weekly capacity",0.,168.)?}),
    )
}
fn label(v: &Value) -> Result<Value> {
    object(v, "Label")?;
    let keys = if let Some(s) = v["keywords"].as_str() {
        s.split(',').map(|x| json!(x)).collect()
    } else {
        array(default(v, "keywords", &json!([])), "Label keywords", 100)?.clone()
    };
    let mut keywords = Vec::new();
    for k in &keys {
        let t = text(k, "Label keyword", 120, false)?;
        if !t.is_empty() && !keywords.contains(&t) {
            keywords.push(t)
        }
    }
    let color = text(
        default(v, "color", &json!("#7660bd")),
        "Label color",
        7,
        false,
    )?;
    if color.len() != 7
        || !color.starts_with('#')
        || !color[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err("Label color must be a six-digit hex color.".into());
    }
    Ok(
        json!({"id":id(&v["id"])?,"name":text(&v["name"],"Label name",200,true)?,"color":color,"keywords":keywords}),
    )
}
fn invoice_totals(lines: &[Value], tax: u64, discount: u64) -> Result<(u64, u64, u64)> {
    let subtotal = lines.iter().try_fold(0u64, |a, l| {
        a.checked_add(u(l, "amountMinor"))
            .filter(|x| *x <= SAFE)
            .ok_or("Subtotal must be a safe integer.")
    })?;
    if discount > subtotal {
        return Err("Discount cannot exceed subtotal.".into());
    }
    let taxable = subtotal - discount;
    let tax_minor = ratio(taxable as u128 * tax as u128, 10000)?;
    let total = taxable
        .checked_add(tax_minor)
        .filter(|x| *x <= SAFE)
        .ok_or("Invoice total exceeds safe limits.")?;
    Ok((subtotal, tax_minor, total))
}

pub fn validate_invoice(v: &Value) -> Result<Value> {
    invoice_validated(v).map(canonical)
}
fn invoice_validated(v: &Value) -> Result<Value> {
    object(v, "Invoice")?;
    let code = currency(&v["currency"])?;
    let precision = currency_digits(&code)?;
    if v["currencyDigits"].as_u64() != Some(precision as u64) {
        return Err("Invoice currency precision is invalid.".into());
    }
    let status = s(v, "status");
    if !["draft", "sent", "paid", "void"].contains(&status) {
        return Err("Invoice status is invalid.".into());
    }
    let mut lines = Vec::new();
    for raw in array(&v["lines"], "Invoice lines", 50000)? {
        object(raw, "Invoice line")?;
        let entry = txt(raw, "entryId", "Entry ID", 160)?;
        let quantity = number(&raw["quantityHours"], "Quantity hours", 0., 1000000.)?;
        let duration = if raw.get("durationMs").is_none() {
            (quantity * 3600000.).round() as u64
        } else {
            integer(&raw["durationMs"], "Line duration", 0, SAFE)?
        };
        let r = rate(&raw["rate"], "Hourly rate")?;
        let amount = integer(&raw["amountMinor"], "Line amount", 0, SAFE)?;
        let kind = default(raw, "kind", &json!("time"))
            .as_str()
            .unwrap_or("")
            .to_string();
        if !["time", "retainer"].contains(&kind.as_str()) {
            return Err("Invoice line type is invalid.".into());
        }
        if kind == "time" && (entry.is_empty() || duration == 0) {
            return Err("Time invoice lines need a source entry and positive duration.".into());
        }
        if kind == "retainer"
            && (!entry.is_empty()
                || duration != 0
                || quantity != 0.
                || r != 0.
                || s(raw, "contractId").is_empty())
        {
            return Err("Retainer invoice lines need a contract and no time quantity.".into());
        }
        if kind == "time" && amount != charge(duration as f64, r, precision)? {
            return Err("Invoice line amount does not match its time and rate.".into());
        }
        if kind == "time" && (quantity - duration as f64 / 3600000.).abs() > 1e-9 {
            return Err("Invoice hours do not match duration.".into());
        }
        lines.push(json!({"id":id(&raw["id"])?,"entryId":entry,"contractId":txt(raw,"contractId","Contract ID",160)?,"description":text(&raw["description"],"Line description",2000,false)?,"kind":kind,"quantityHours":quantity,"durationMs":duration,"rate":r,"amountMinor":amount}));
    }
    let lines = unique(lines, "invoice line")?;
    let tax = uint(v, "taxBps", "Tax basis points", 0, 10000)?;
    let discount = uint(v, "discountMinor", "Discount", 0, SAFE)?;
    let (subtotal, tax_minor, total) = invoice_totals(&lines, tax, discount)?;
    for (key, expected) in [
        ("subtotalMinor", subtotal),
        ("taxMinor", tax_minor),
        ("totalMinor", total),
    ] {
        if v[key].as_u64() != Some(expected) {
            return Err(format!("Invoice {key} does not match its line items."));
        }
    }
    let issue = parse_date(&v["issueDate"], "Issue date")?;
    let due = parse_date(&v["dueDate"], "Due date")?;
    if due < issue {
        return Err("Due date cannot precede issue date.".into());
    }
    let mut payments = Vec::new();
    for p in array(default(v, "payments", &json!([])), "Payments", 10000)? {
        payments.push(json!({"id":id(&p["id"])?,"amountMinor":integer(&p["amountMinor"],"Payment amount",1,SAFE)?,"date":parse_date(&p["date"],"Payment date")?,"reference":txt(p,"reference","Payment reference",300)?}));
    }
    let payments = unique(payments, "payment")?;
    let paid = payments.iter().try_fold(0u64, |a, p| {
        a.checked_add(u(p, "amountMinor"))
            .filter(|x| *x <= SAFE)
            .ok_or("Paid amount exceeds safe limits.")
    })?;
    if paid > total {
        return Err("Payments cannot exceed invoice total.".into());
    }
    if v["paidMinor"].as_u64() != Some(paid) || v["outstandingMinor"].as_u64() != Some(total - paid)
    {
        return Err("Invoice payment balance is invalid.".into());
    }
    if status == "draft" && paid != 0 {
        return Err("Draft invoices cannot contain payments.".into());
    }
    if status == "paid" && paid != total {
        return Err("Paid invoice must be paid in full.".into());
    }
    if status == "sent" && paid == total {
        return Err("A fully settled invoice must have paid status.".into());
    }
    if status == "void" && paid != 0 {
        return Err("Paid invoices cannot be voided.".into());
    }
    let (start, end) = period(&v["startDate"], &v["endDate"])?;
    let issuer = profile(&v["issuer"])?;
    let bill = &v["billTo"];
    Ok(
        json!({"id":id(&v["id"])?,"number":text(&v["number"],"Invoice number",80,true)?,"sequence":integer(&v["sequence"],"Invoice sequence",1,SAFE)?,"clientId":id(&v["clientId"])?,"clientName":text(&v["clientName"],"Client name",200,true)?,"billTo":{"contact":txt(bill,"contact","Bill-to contact",200)?,"email":txt(bill,"email","Bill-to email",320)?,"address":txt(bill,"address","Bill-to address",2000)?},"issuer":issuer,"currency":code,"currencyDigits":precision,"startDate":start,"endDate":end,"issueDate":issue,"dueDate":due,"paymentDays":uint(v,"paymentDays","Payment days",30,365)?,"status":status,"lines":lines,"subtotalMinor":subtotal,"taxBps":tax,"taxMinor":tax_minor,"discountMinor":discount,"totalMinor":total,"payments":payments,"paidMinor":paid,"outstandingMinor":total-paid,"notes":txt(v,"notes","Invoice notes",4000)?,"paymentInstructions":txt(v,"paymentInstructions","Payment instructions",4000)?,"voidReason":text(default(v,"voidReason",&json!("")),"Void reason",2000,status=="void")?}),
    )
}
pub fn assert_invoice_transition(previous: Option<&Value>, next: Option<&Value>) -> Result<()> {
    let Some(before) = previous else {
        return next.map(validate_invoice).transpose().map(|_| ());
    };
    let after = next.ok_or(
        "Invoice snapshots cannot be deleted. Void the invoice to preserve its number and history.",
    )?;
    let before = validate_invoice(before)?;
    let after = validate_invoice(after)?;
    if ["id", "number", "sequence"]
        .iter()
        .any(|k| before[k] != after[k])
    {
        return Err("Invoice identity and number cannot change.".into());
    }
    if s(&before, "status") == "draft" {
        return Ok(());
    }
    let mut a = before.clone();
    let mut b = after.clone();
    for key in [
        "status",
        "payments",
        "paidMinor",
        "outstandingMinor",
        "voidReason",
    ] {
        a.as_object_mut().unwrap().remove(key);
        b.as_object_mut().unwrap().remove(key);
    }
    if a != b {
        return Err("A sent invoice cannot change its frozen accounting fields.".into());
    }
    if s(&after, "status") == "draft" {
        return Err("A sent invoice cannot become a draft.".into());
    }
    if s(&before, "status") == "void"
        && (s(&after, "status") != "void" || before["voidReason"] != after["voidReason"])
    {
        return Err("A void invoice cannot be reopened or rewritten.".into());
    }
    for (i, p) in items(&before, "payments").iter().enumerate() {
        if items(&after, "payments").get(i) != Some(p) {
            return Err(
                "Recorded invoice payments cannot be removed, reordered, or rewritten.".into(),
            );
        }
    }
    Ok(())
}
fn in_period(e: &Value, start: &str, end: &str) -> bool {
    let day = s(e, "start").get(..10).unwrap_or("");
    day >= start && day <= end
}
pub fn assert_invoice_entries(v: &Value, state: &Value) -> Result<()> {
    let inv = validate_invoice(v)?;
    if !items(state, "clients")
        .iter()
        .any(|c| c["id"] == inv["clientId"])
    {
        return Err("Invoice source client does not exist.".into());
    }
    let mut durations: HashMap<String, u64> = HashMap::new();
    for line in items(&inv, "lines") {
        if s(line, "kind") != "time" {
            continue;
        }
        let e = items(state, "entries")
            .iter()
            .find(|e| e["id"] == line["entryId"])
            .ok_or("Invoice time must come from approved billable entries for its client.")?;
        if s(e, "status") != "approved" || e["billable"] != true || e["clientId"] != inv["clientId"]
        {
            return Err(
                "Invoice time must come from approved billable entries for its client.".into(),
            );
        }
        if !in_period(e, s(&inv, "startDate"), s(&inv, "endDate")) {
            return Err("Invoice time falls outside its selected date range.".into());
        }
        let d = durations.entry(s(e, "id").into()).or_default();
        *d = d
            .checked_add(u(line, "durationMs"))
            .ok_or("Invoice duration exceeds safe limits.")?;
    }
    if durations.is_empty() {
        return Err("An invoice needs approved billable source time.".into());
    }
    for (key, d) in durations {
        if items(state, "entries")
            .iter()
            .find(|e| s(e, "id") == key)
            .map(|e| u(e, "durationMs"))
            != Some(d)
        {
            return Err(
                "Invoice time quantities must equal the complete source entry duration.".into(),
            );
        }
    }
    Ok(())
}

pub fn validate_business(v: &Value, state: &Value) -> Result<Value> {
    let base = default_business();
    let v = if v.is_null() { &base } else { v };
    object(v, "Business data")?;
    let mut out = default_business();
    out["profile"] = profile(default(v, "profile", &base["profile"]))?;
    for (key, name, max) in [
        ("clientTerms", "client terms", 2000),
        ("contracts", "contract", 10000),
        ("invoices", "invoice", 50000),
        ("allocations", "allocation", 50000),
        ("labels", "label", 5000),
    ] {
        let mut a = Vec::new();
        for x in array(default(v, key, &json!([])), name, max)? {
            a.push(match key {
                "clientTerms" => terms(x, state)?,
                "contracts" => contract(x, state)?,
                "invoices" => validate_invoice(x)?,
                "allocations" => allocation(x, state)?,
                _ => label(x)?,
            });
        }
        out[key] = json!(unique(a, name)?);
    }
    out["nextInvoiceNumber"] = json!(integer(
        default(v, "nextInvoiceNumber", &json!(1)),
        "Next invoice number",
        1,
        SAFE
    )?);
    let mut capacity = HashMap::new();
    let mut scopes = HashSet::new();
    for a in items(&out, "allocations") {
        let key = (s(a, "memberId"), s(a, "week"));
        if let Some(old) = capacity.insert(key, n(a, "capacityHours")) {
            if old != n(a, "capacityHours") {
                return Err("A member must have one consistent weekly capacity.".into());
            }
        }
        if !scopes.insert((key, s(a, "projectId"))) {
            return Err("Use one allocation per member, project, and week.".into());
        }
    }
    let mut clients = HashSet::new();
    for t in items(&out, "clientTerms") {
        if !clients.insert(s(t, "clientId")) {
            return Err("Only one set of payment terms is allowed per client.".into());
        }
    }
    let (mut numbers, mut sequences, mut reserved, mut retained) = (
        HashSet::new(),
        HashSet::new(),
        HashSet::new(),
        HashSet::new(),
    );
    for inv in items(&out, "invoices") {
        if !numbers.insert(s(inv, "number")) || !sequences.insert(u(inv, "sequence")) {
            return Err("Invoice numbers cannot be reused.".into());
        }
        if u(inv, "sequence") >= u(&out, "nextInvoiceNumber") {
            return Err("Next invoice number must exceed every issued number.".into());
        }
        if s(inv, "status") != "void" {
            let mut this = HashSet::new();
            for line in items(inv, "lines") {
                if s(line, "kind") == "retainer"
                    && !retained.insert((
                        s(line, "contractId"),
                        s(inv, "startDate"),
                        s(inv, "endDate"),
                    ))
                {
                    return Err("A fixed contract fee cannot appear on multiple active invoices for its period.".into());
                }
                if !s(line, "entryId").is_empty() {
                    this.insert(s(line, "entryId"));
                }
            }
            for eid in this {
                if !reserved.insert(eid) {
                    return Err("A time entry cannot appear on multiple active invoices.".into());
                }
            }
        }
    }
    let cs = items(&out, "contracts");
    for (i, a) in cs.iter().enumerate() {
        for b in &cs[i + 1..] {
            let ae = if s(a, "endDate").is_empty() {
                "9999-12-31"
            } else {
                s(a, "endDate")
            };
            let be = if s(b, "endDate").is_empty() {
                "9999-12-31"
            } else {
                s(b, "endDate")
            };
            if a["clientId"] == b["clientId"]
                && a["projectId"] == b["projectId"]
                && s(a, "startDate") <= be
                && s(b, "startDate") <= ae
            {
                return Err("Contracts for the same scope cannot overlap.".into());
            }
        }
    }
    Ok(out)
}
fn contract_for<'a>(state: &'a Value, e: &Value) -> Option<&'a Value> {
    let day = s(e, "start").get(..10).unwrap_or("");
    let mut general = None;
    for c in items(&state["business"], "contracts") {
        if c["clientId"] == e["clientId"]
            && (s(c, "projectId").is_empty() || c["projectId"] == e["projectId"])
            && s(c, "startDate") <= day
            && (s(c, "endDate").is_empty() || s(c, "endDate") >= day)
        {
            if !s(c, "projectId").is_empty() {
                return Some(c);
            }
            if general.is_none() {
                general = Some(c)
            }
        }
    }
    general
}
fn time_line(
    e: &Value,
    c: Option<&Value>,
    duration: u64,
    rate: f64,
    digits: u32,
    included: bool,
) -> Result<Value> {
    Ok(
        json!({"id":format!("{}:{}",s(e,"id"),if included{"included"}else{"time"}),"entryId":s(e,"id"),"contractId":c.map(|c|s(c,"id")).unwrap_or(""),"description":if included{format!("{} (included in retainer)",if s(e,"description").is_empty(){"Tracked time"}else{s(e,"description")})}else{if s(e,"description").is_empty(){"Tracked time"}else{s(e,"description")}.to_string()},"kind":"time","quantityHours":duration as f64/3600000.,"durationMs":duration,"rate":rate,"amountMinor":charge(duration as f64,rate,digits)?}),
    )
}
fn invoice_draft(state: &Value, p: &Value, now: i64, previous: Option<&Value>) -> Result<Value> {
    let b = &state["business"];
    let client = items(state, "clients")
        .iter()
        .find(|c| c["id"] == p["clientId"])
        .ok_or("Select an existing client.")?;
    let (start, end) = period(&p["startDate"], &p["endDate"])?;
    let terms = items(b, "clientTerms")
        .iter()
        .find(|t| t["clientId"] == client["id"]);
    let code = currency(default(
        p,
        "currency",
        terms.map(|t| &t["currency"]).unwrap_or(&json!("USD")),
    ))?;
    let precision = currency_digits(&code)?;
    let tax = uint(p, "taxBps", "Tax basis points", 0, 10000)?;
    let discount = uint(p, "discountMinor", "Discount", 0, SAFE)?;
    let other = items(b, "invoices")
        .iter()
        .filter(|i| {
            s(i, "status") != "void" && previous.map(|p| p["id"] != i["id"]).unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let occupied = other
        .iter()
        .flat_map(|i| items(i, "lines"))
        .filter_map(|l| {
            let eid = s(l, "entryId");
            if eid.is_empty() {
                None
            } else {
                Some(eid)
            }
        })
        .collect::<HashSet<_>>();
    let mut entries = items(state, "entries")
        .iter()
        .filter(|e| {
            e["clientId"] == client["id"]
                && s(e, "status") == "approved"
                && e["billable"] == true
                && in_period(e, &start, &end)
                && !occupied.contains(s(e, "id"))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| s(e, "start"));
    let mut used: HashMap<String, u64> = HashMap::new();
    for inv in &other {
        for l in items(inv, "lines") {
            if !s(l, "contractId").is_empty() && s(l, "kind") == "time" {
                *used.entry(s(l, "contractId").into()).or_default() += u(l, "durationMs")
            }
        }
    }
    let (mut lines, mut added) = (Vec::new(), HashSet::new());
    for e in entries {
        let c = contract_for(state, e);
        if c.map(|c| s(c, "currency") != code).unwrap_or(false) {
            return Err(
                "Selected entries use a contract in another currency. Create a separate invoice."
                    .into(),
            );
        }
        let mut r = c
            .map(|c| n(c, "hourlyRate"))
            .unwrap_or(n(client, "hourlyRate"));
        if let Some(c) = c.filter(|c| ["retainer", "hybrid"].contains(&s(c, "type"))) {
            if start != s(c, "startDate") || end != s(c, "endDate") {
                return Err("Retainer and hybrid invoices must use the exact fixed contract start and end dates.".into());
            }
            let line_id = format!("retainer:{}:{start}:{end}", s(c, "id"));
            if added.insert(s(c, "id")) {
                if !other
                    .iter()
                    .any(|i| items(i, "lines").iter().any(|l| s(l, "id") == line_id))
                {
                    lines.push(json!({"id":line_id,"entryId":"","contractId":s(c,"id"),"description":format!("{} fee ({start} – {end})",if s(c,"type")=="hybrid"{"Hybrid"}else{"Retainer"}),"kind":"retainer","quantityHours":0,"durationMs":0,"rate":0,"amountMinor":u(c,"retainerMinor")}));
                }
            }
            if s(c, "type") == "retainer" {
                r = 0.
            }
        }
        let mut duration = u(e, "durationMs");
        if let Some(c) = c.filter(|c| s(c, "type") == "hybrid") {
            let prev = *used.get(s(c, "id")).unwrap_or(&0);
            let included = ((n(c, "includedHours") * 3600000.).round() as u64).saturating_sub(prev);
            used.insert(s(c, "id").into(), prev + duration);
            let chargeable = duration.saturating_sub(included);
            if chargeable != duration {
                lines.push(time_line(
                    e,
                    Some(c),
                    duration - chargeable,
                    0.,
                    precision,
                    true,
                )?);
                duration = chargeable;
            }
        }
        if duration > 0 || !lines.iter().any(|l| l["entryId"] == e["id"]) {
            lines.push(time_line(e, c, duration, r, precision, false)?);
        }
    }
    if lines.is_empty() {
        return Err("There are no uninvoiced approved billable entries in this date range.".into());
    }
    let (subtotal, tax_minor, total) = invoice_totals(&lines, tax, discount)?;
    let issue = parse_date(default(p, "issueDate", &json!(today(now)?)), "Issue date")?;
    let days = integer(
        default(
            p,
            "paymentDays",
            terms.map(|t| &t["paymentDays"]).unwrap_or(&json!(30)),
        ),
        "Payment days",
        0,
        365,
    )?;
    let due = NaiveDate::parse_from_str(&issue, "%Y-%m-%d")
        .unwrap()
        .checked_add_days(Days::new(days))
        .ok_or("Due date exceeds valid calendar.")?
        .format("%Y-%m-%d")
        .to_string();
    let sequence = previous
        .map(|p| u(p, "sequence"))
        .unwrap_or(u(b, "nextInvoiceNumber"));
    let identity = previous
        .map(|p| s(p, "id").to_string())
        .unwrap_or_else(new_id);
    let number = previous
        .map(|p| s(p, "number").to_string())
        .unwrap_or_else(|| format!("{}{:05}", s(&b["profile"], "invoicePrefix"), sequence));
    Ok(
        json!({"id":identity,"number":number,"sequence":sequence,"clientId":s(client,"id"),"clientName":s(client,"name"),"billTo":{"contact":terms.map(|t|s(t,"contact")).unwrap_or(""),"email":terms.map(|t|s(t,"email")).unwrap_or(""),"address":terms.map(|t|s(t,"address")).unwrap_or("")},"issuer":b["profile"],"currency":code,"currencyDigits":precision,"startDate":start,"endDate":end,"issueDate":issue,"dueDate":due,"paymentDays":days,"status":"draft","lines":lines,"subtotalMinor":subtotal,"taxMinor":tax_minor,"totalMinor":total,"taxBps":tax,"discountMinor":discount,"payments":[],"paidMinor":0,"outstandingMinor":total,"notes":txt(p,"notes","Invoice notes",4000)?,"paymentInstructions":b["profile"]["paymentInstructions"],"voidReason":""}),
    )
}

fn merged(a: &Value, b: &Value) -> Value {
    let mut out = a.as_object().cloned().unwrap_or_default();
    if let Some(b) = b.as_object() {
        out.extend(b.clone());
    }
    Value::Object(out)
}
pub fn assert_uninvoiced(state: &Value, ids: &[String]) -> Result<()> {
    if items(&state["business"], "invoices").iter().any(|inv| {
        s(inv, "status") != "void"
            && items(inv, "lines")
                .iter()
                .any(|l| ids.iter().any(|id| id == s(l, "entryId")))
    }) {
        Err(
            "An entry belongs to an active invoice. Void that invoice before editing the entry."
                .into(),
        )
    } else {
        Ok(())
    }
}
fn timestamp(v: &Value, name: &str) -> Result<(String, i64)> {
    let valid = crate::state::iso(v, name)?;
    let input = valid.as_str().unwrap();
    let dt = DateTime::parse_from_rfc3339(input)
        .map_err(|_| format!("{name} must be a valid ISO timestamp."))?;
    Ok((input.to_string(), dt.timestamp_millis()))
}
fn advanced_entry(state: &mut Value, kind: &str, p: &Value) -> Result<bool> {
    if kind == "entry.split" {
        let target = id(&p["id"])?;
        let index = items(state, "entries")
            .iter()
            .position(|e| s(e, "id") == target)
            .ok_or("The time entry does not exist.")?;
        assert_uninvoiced(state, &[target])?;
        let entry = state["entries"][index].clone();
        let (at, ms) = timestamp(&p["at"], "Split time")?;
        if ms <= timestamp(&entry["start"], "Entry start")?.1
            || ms >= timestamp(&entry["end"], "Entry end")?.1
        {
            return Err("Split time must be inside the entry.".into());
        }
        let first = crate::state::normalize_entry(
            &merged(&entry, &json!({"id":new_id(),"end":at})),
            state,
        )?;
        let second = crate::state::normalize_entry(
            &merged(&entry, &json!({"id":new_id(),"start":at})),
            state,
        )?;
        state["entries"]
            .as_array_mut()
            .unwrap()
            .splice(index..=index, [first, second]);
        return Ok(true);
    }
    if kind == "entry.merge" || kind == "entry.bulk" {
        let mut ids = Vec::new();
        for x in array(&p["ids"], "Selected entries", 1000)? {
            let x = id(x)?;
            if !ids.contains(&x) {
                ids.push(x)
            }
        }
        if ids.is_empty() || (kind == "entry.merge" && ids.len() < 2) {
            return Err("Select the required time entries.".into());
        }
        let mut selected = Vec::new();
        for id in &ids {
            selected.push(
                items(state, "entries")
                    .iter()
                    .find(|e| s(e, "id") == id)
                    .ok_or("A selected time entry does not exist.")?
                    .clone(),
            )
        }
        assert_uninvoiced(state, &ids)?;
        if kind == "entry.bulk" {
            let patch = object(&p["patch"], "Bulk entry changes")?;
            let allowed = [
                "description",
                "clientId",
                "projectId",
                "taskId",
                "status",
                "category",
                "billable",
                "labelIds",
            ];
            if patch.keys().any(|k| !allowed.contains(&k.as_str())) {
                return Err("Bulk changes contain an unsupported field.".into());
            }
            for entry in &selected {
                let normalized = crate::state::normalize_entry(&merged(entry, &p["patch"]), state)?;
                let index = items(state, "entries")
                    .iter()
                    .position(|e| e["id"] == entry["id"])
                    .unwrap();
                state["entries"][index] = normalized;
            }
        } else {
            selected.sort_by_key(|e| s(e, "start").to_string());
            let first = &selected[0];
            for pair in selected.windows(2) {
                if timestamp(&pair[0]["end"], "Entry end")?.1
                    != timestamp(&pair[1]["start"], "Entry start")?.1
                {
                    return Err(
                        "Merged entries must be exactly contiguous, without gaps or overlaps."
                            .into(),
                    );
                }
                let fields = [
                    "clientId",
                    "projectId",
                    "taskId",
                    "status",
                    "source",
                    "category",
                    "billable",
                    "app",
                    "title",
                    "externalId",
                    "externalSource",
                ];
                let mut labels = items(&pair[1], "labelIds").to_vec();
                let mut first_labels = items(first, "labelIds").to_vec();
                labels.sort_by_key(Value::to_string);
                first_labels.sort_by_key(Value::to_string);
                if fields.iter().any(|k| pair[1][k] != first[k]) || labels != first_labels {
                    return Err("Merged entries must have the same tags, status, source, application, and labels.".into());
                }
            }
            let mut descriptions = Vec::new();
            for e in &selected {
                let d = s(e, "description").to_string();
                if !d.is_empty() && !descriptions.contains(&d) {
                    descriptions.push(d)
                }
            }
            let value = crate::state::normalize_entry(
                &merged(
                    first,
                    &json!({"id":new_id(),"end":selected.last().unwrap()["end"],"description":descriptions.join("\n")}),
                ),
                state,
            )?;
            state["entries"]
                .as_array_mut()
                .unwrap()
                .retain(|e| !ids.iter().any(|i| i == s(e, "id")));
            state["entries"].as_array_mut().unwrap().push(value);
        }
        return Ok(true);
    }
    if kind == "entry.regenerate" {
        let target = id(&p["id"])?;
        let index = items(state, "entries")
            .iter()
            .position(|e| s(e, "id") == target)
            .ok_or("The time entry does not exist.")?;
        assert_uninvoiced(state, &[target])?;
        let e = state["entries"][index].clone();
        let haystack = format!(
            "{}\n{}\n{}",
            s(&e, "app"),
            s(&e, "title"),
            s(&e, "description")
        )
        .to_lowercase();
        let mut patch = json!({});
        if let Some(rule) = items(state, "rules")
            .iter()
            .find(|r| haystack.contains(&s(r, "keyword").to_lowercase()))
        {
            patch = json!({"category":rule["action"],"clientId":rule["clientId"],"projectId":rule["projectId"],"taskId":rule["taskId"]});
            if s(rule, "action") == "exclude" {
                patch["status"] = json!("pending")
            }
        }
        let mut labels = items(&e, "labelIds").to_vec();
        for label in items(&state["business"], "labels") {
            if items(label, "keywords")
                .iter()
                .any(|k| haystack.contains(&k.as_str().unwrap_or("").to_lowercase()))
                && !labels.contains(&label["id"])
            {
                labels.push(label["id"].clone());
            }
        }
        patch["labelIds"] = json!(labels);
        state["entries"][index] = crate::state::normalize_entry(&merged(&e, &patch), state)?;
        return Ok(true);
    }
    Ok(false)
}
/// Returns false for commands owned by the general state module. Every handled
/// command is transactional even when this helper is called independently.
pub fn apply_business_command(state: &mut Value, command: &Value, now: i64) -> Result<bool> {
    let kind = command["type"].as_str().unwrap_or("");
    let p = default(command, "payload", &json!({})).clone();
    object(&p, "Business command")?;
    let mut candidate = state.clone();
    if candidate["business"].is_null() {
        candidate["business"] = default_business()
    }
    let handled = apply_inner(&mut candidate, kind, &p, now)?;
    if handled {
        candidate["business"] = validate_business(&candidate["business"], &candidate)?;
        *state = candidate;
    }
    Ok(handled)
}
fn apply_inner(state: &mut Value, kind: &str, p: &Value, now: i64) -> Result<bool> {
    if advanced_entry(state, kind, p)? {
        return Ok(true);
    }
    if kind == "business.profile.update" {
        state["business"]["profile"] = profile(&merged(&state["business"]["profile"], p))?;
        return Ok(true);
    }
    if kind == "allocation.import" {
        let mut incoming = Vec::new();
        for item in array(&p["allocations"], "Allocation import", 10000)? {
            let target = if truthy(&item["id"]) {
                id(&item["id"])?
            } else {
                items(&state["business"], "allocations")
                    .iter()
                    .find(|a| {
                        a["memberId"] == item["memberId"]
                            && a["projectId"] == item["projectId"]
                            && a["week"] == item["week"]
                    })
                    .map(|a| s(a, "id").to_string())
                    .unwrap_or_else(new_id)
            };
            incoming.push(allocation(&merged(item, &json!({"id":target})), state)?);
        }
        let incoming = unique(incoming, "allocation")?;
        let mut capacities = HashMap::new();
        for item in &incoming {
            let key = (s(item, "memberId"), s(item, "week"));
            if let Some(old) = capacities.insert(key, n(item, "capacityHours")) {
                if old != n(item, "capacityHours") {
                    return Err("Imported member capacity must be consistent for each week.".into());
                }
            }
        }
        for item in incoming {
            upsert(&mut state["business"]["allocations"], item.clone());
            for peer in state["business"]["allocations"].as_array_mut().unwrap() {
                if peer["memberId"] == item["memberId"] && peer["week"] == item["week"] {
                    peer["capacityHours"] = item["capacityHours"].clone();
                }
            }
        }
        return Ok(true);
    }
    for (prefix, key) in [
        ("business.clientTerms", "clientTerms"),
        ("contract", "contracts"),
        ("allocation", "allocations"),
        ("label", "labels"),
    ] {
        if kind == format!("{prefix}.delete") {
            let target = id(&p["id"])?;
            if !items(&state["business"], key)
                .iter()
                .any(|x| s(x, "id") == target)
            {
                return Err("The business record does not exist.".into());
            }
            state["business"][key]
                .as_array_mut()
                .unwrap()
                .retain(|x| s(x, "id") != target);
            if key == "labels" {
                for entry in state["entries"].as_array_mut().unwrap() {
                    if let Some(labels) = entry["labelIds"].as_array_mut() {
                        labels.retain(|x| x.as_str() != Some(target.as_str()));
                    }
                }
            }
            return Ok(true);
        }
        if kind == format!("{prefix}.upsert") {
            let target = if truthy(&p["id"]) {
                id(&p["id"])?
            } else if key == "clientTerms" {
                items(&state["business"], key)
                    .iter()
                    .find(|t| t["clientId"] == p["clientId"])
                    .map(|t| s(t, "id").to_string())
                    .unwrap_or_else(new_id)
            } else {
                new_id()
            };
            let old = items(&state["business"], key)
                .iter()
                .find(|x| s(x, "id") == target)
                .cloned()
                .unwrap_or(json!({}));
            let value = merged(&merged(&old, p), &json!({"id":target}));
            let normalized = match key {
                "clientTerms" => terms(&value, state)?,
                "contracts" => contract(&value, state)?,
                "allocations" => allocation(&value, state)?,
                _ => label(&value)?,
            };
            if key == "allocations" {
                for peer in state["business"][key].as_array_mut().unwrap() {
                    if peer["memberId"] == normalized["memberId"]
                        && peer["week"] == normalized["week"]
                    {
                        peer["capacityHours"] = normalized["capacityHours"].clone();
                    }
                }
            }
            upsert(&mut state["business"][key], normalized);
            return Ok(true);
        }
    }
    if kind == "invoice.create" {
        let inv = invoice_draft(state, p, now, None)?;
        state["business"]["invoices"]
            .as_array_mut()
            .unwrap()
            .push(inv);
        let next = u(&state["business"], "nextInvoiceNumber")
            .checked_add(1)
            .filter(|x| *x <= SAFE)
            .ok_or("Invoice sequence exceeds safe limits.")?;
        state["business"]["nextInvoiceNumber"] = json!(next);
        return Ok(true);
    }
    if kind.starts_with("invoice.") {
        let target = id(&p["id"])?;
        let index = items(&state["business"], "invoices")
            .iter()
            .position(|i| s(i, "id") == target)
            .ok_or("Invoice does not exist.")?;
        let mut invoice = state["business"]["invoices"][index].clone();
        match kind {
            "invoice.update" => {
                if s(&invoice, "status") != "draft" {
                    return Err(
                        "Sent invoices are immutable; void and issue a new invoice instead.".into(),
                    );
                }
                invoice = invoice_draft(state, &merged(&invoice, p), now, Some(&invoice))?
            }
            "invoice.markSent" => {
                if s(&invoice, "status") != "draft" {
                    return Err("Only draft invoices can be marked sent.".into());
                }
                invoice["status"] = json!(if u(&invoice, "totalMinor") == 0 {
                    "paid"
                } else {
                    "sent"
                });
            }
            "invoice.payment" => {
                if !["sent", "paid"].contains(&s(&invoice, "status")) {
                    return Err("Payments require a sent invoice.".into());
                }
                let amount = integer(&p["amountMinor"], "Payment amount", 1, SAFE)?;
                if amount > u(&invoice, "outstandingMinor") {
                    return Err("Payment cannot exceed outstanding balance.".into());
                }
                let payment = json!({"id":new_id(),"amountMinor":amount,"date":parse_date(default(p,"date",&json!(today(now)?)),"Payment date")?,"reference":txt(p,"reference","Payment reference",300)?});
                invoice["payments"].as_array_mut().unwrap().push(payment);
                invoice["paidMinor"] = json!(u(&invoice, "paidMinor") + amount);
                invoice["outstandingMinor"] = json!(u(&invoice, "outstandingMinor") - amount);
                if u(&invoice, "outstandingMinor") == 0 {
                    invoice["status"] = json!("paid");
                }
            }
            "invoice.void" => {
                if s(&invoice, "status") == "void" {
                    return Err("Invoice is already void.".into());
                }
                if u(&invoice, "paidMinor") != 0 {
                    return Err("Paid invoices cannot be voided.".into());
                }
                invoice["status"] = json!("void");
                invoice["voidReason"] = json!(text(
                    default(p, "reason", &json!("")),
                    "Void reason",
                    2000,
                    true
                )?);
            }
            _ => return Err(format!("Unknown command: {kind}")),
        }
        state["business"]["invoices"][index] = invoice;
        return Ok(true);
    }
    Ok(false)
}
fn upsert(a: &mut Value, v: Value) {
    if let Some(index) = a
        .as_array()
        .unwrap()
        .iter()
        .position(|x| x["id"] == v["id"])
    {
        a[index] = v
    } else {
        a.as_array_mut().unwrap().push(v)
    }
}

#[derive(Default)]
struct Group {
    project: String,
    client: String,
    code: String,
    total: f64,
    billable: f64,
    revenue: i128,
    cost: i128,
    budget: f64,
    budget_ids: HashSet<String>,
}
#[derive(Default)]
struct CurrencyTotal {
    code: String,
    total: f64,
    billable: f64,
    revenue: i128,
    cost: i128,
    profit: i128,
    invoiced: i128,
    outstanding: i128,
}
fn safe_money(n: i128) -> Result<i64> {
    if n.unsigned_abs() > SAFE as u128 {
        Err("Financial summary exceeds safe money limits.".into())
    } else {
        Ok(n as i64)
    }
}
fn group_index(groups: &mut Vec<Group>, e: &Value, code: &str) -> usize {
    let (project, client) = (s(e, "projectId"), s(e, "clientId"));
    if let Some(i) = groups
        .iter()
        .position(|g| g.project == project && g.client == client && g.code == code)
    {
        i
    } else {
        groups.push(Group {
            project: project.into(),
            client: client.into(),
            code: code.into(),
            ..Default::default()
        });
        groups.len() - 1
    }
}
fn total_index(totals: &mut Vec<CurrencyTotal>, code: &str) -> usize {
    if let Some(i) = totals.iter().position(|g| g.code == code) {
        i
    } else {
        totals.push(CurrencyTotal {
            code: code.into(),
            ..Default::default()
        });
        totals.len() - 1
    }
}
pub fn financial_summary(state: &Value, query: &Value) -> Result<Value> {
    let mut clean = state.clone();
    clean["business"] = validate_business(&state["business"], state)?;
    let (start, end) = period(
        default(query, "startDate", &json!("1970-01-01")),
        default(query, "endDate", &json!("9999-12-31")),
    )?;
    let (mut groups, mut totals) = (Vec::<Group>::new(), Vec::<CurrencyTotal>::new());
    let mut used: HashMap<String, f64> = HashMap::new();
    let mut entries = items(&clean, "entries")
        .iter()
        .filter(|e| s(e, "status") == "approved")
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        s(a, "start")
            .cmp(s(b, "start"))
            .then(s(a, "id").cmp(s(b, "id")))
    });
    for e in &entries {
        let c = contract_for(&clean, e);
        let terms = items(&clean["business"], "clientTerms")
            .iter()
            .find(|t| t["clientId"] == e["clientId"]);
        let code = c
            .map(|c| s(c, "currency"))
            .or(terms.map(|t| s(t, "currency")))
            .unwrap_or("USD");
        let precision = currency_digits(code)?;
        let hours = n(e, "durationMs") / 3600000.;
        let mut revenue = 0;
        if e["billable"] == true {
            if c.map(|c| s(c, "type") == "hourly").unwrap_or(true) {
                let r = c.map(|c| n(c, "hourlyRate")).unwrap_or_else(|| {
                    items(&clean, "clients")
                        .iter()
                        .find(|x| x["id"] == e["clientId"])
                        .map(|x| n(x, "hourlyRate"))
                        .unwrap_or(0.)
                });
                revenue = charge(n(e, "durationMs"), r, precision)?;
            } else if let Some(c) = c.filter(|c| s(c, "type") == "hybrid") {
                let previous = *used.get(s(c, "id")).unwrap_or(&0.);
                let extra = (previous + hours - n(c, "includedHours")).max(0.)
                    - (previous - n(c, "includedHours")).max(0.);
                revenue = charge((extra * 3600000.).round(), n(c, "hourlyRate"), precision)?;
                used.insert(s(c, "id").into(), previous + hours);
            }
        }
        if !in_period(e, &start, &end) {
            continue;
        }
        let i = group_index(&mut groups, e, code);
        let g = &mut groups[i];
        g.total += hours;
        if e["billable"] == true {
            g.billable += hours
        }
        g.revenue += revenue as i128;
        g.cost += charge(
            n(e, "durationMs"),
            c.map(|c| n(c, "costRate")).unwrap_or(0.),
            precision,
        )? as i128;
        if let Some(c) = c.filter(|c| !s(c, "projectId").is_empty()) {
            if g.budget_ids.insert(s(c, "id").into()) {
                g.budget += n(c, "budgetHours")
            }
        }
    }
    for c in items(&clean["business"], "contracts").iter().filter(|c| {
        s(c, "type") != "hourly"
            && s(c, "startDate") >= start.as_str()
            && s(c, "startDate") <= end.as_str()
    }) {
        let i = group_index(&mut groups, c, s(c, "currency"));
        let g = &mut groups[i];
        g.revenue += u(c, "retainerMinor") as i128;
        if !s(c, "projectId").is_empty() && g.budget_ids.insert(s(c, "id").into()) {
            g.budget += n(c, "budgetHours")
        }
    }
    let mut projects = Vec::new();
    for g in groups {
        let used = entries
            .iter()
            .filter(|e| {
                s(e, "projectId") == g.project
                    && contract_for(&clean, e)
                        .map(|c| g.budget_ids.contains(s(c, "id")))
                        .unwrap_or(false)
            })
            .map(|e| n(e, "durationMs") / 3600000.)
            .sum::<f64>();
        let remaining = if g.budget != 0. {
            json!(g.budget - used)
        } else {
            Value::Null
        };
        let profit = g.revenue - g.cost;
        let i = total_index(&mut totals, &g.code);
        let t = &mut totals[i];
        t.total += g.total;
        t.billable += g.billable;
        t.revenue += g.revenue;
        t.cost += g.cost;
        t.profit += profit;
        projects.push(json!({"projectId":g.project,"projectName":items(&clean,"projects").iter().find(|p|s(p,"id")==g.project).map(|p|s(p,"name")).unwrap_or("Unassigned"),"clientId":g.client,"clientName":items(&clean,"clients").iter().find(|c|s(c,"id")==g.client).map(|c|s(c,"name")).unwrap_or("Unassigned"),"currency":g.code,"currencyDigits":currency_digits(&g.code)?,"totalHours":g.total,"billableHours":g.billable,"revenueMinor":safe_money(g.revenue)?,"costMinor":safe_money(g.cost)?,"profitMinor":safe_money(profit)?,"budgetHours":g.budget,"budgetRemainingHours":remaining}));
    }
    for inv in items(&clean["business"], "invoices").iter().filter(|i| {
        ["sent", "paid"].contains(&s(i, "status"))
            && s(i, "issueDate") >= start.as_str()
            && s(i, "issueDate") <= end.as_str()
    }) {
        let i = total_index(&mut totals, s(inv, "currency"));
        totals[i].invoiced += u(inv, "totalMinor") as i128;
        totals[i].outstanding += u(inv, "outstandingMinor") as i128;
    }
    let mut contracts = Vec::new();
    for c in items(&clean["business"], "contracts").iter().filter(|c| {
        s(c, "startDate") <= end.as_str()
            && (s(c, "endDate").is_empty() || s(c, "endDate") >= start.as_str())
    }) {
        let total = entries
            .iter()
            .filter(|e| {
                contract_for(&clean, e)
                    .map(|x| x["id"] == c["id"])
                    .unwrap_or(false)
            })
            .map(|e| n(e, "durationMs") / 3600000.)
            .sum::<f64>();
        contracts.push(json!({"contractId":c["id"],"clientId":c["clientId"],"projectId":c["projectId"],"currency":c["currency"],"totalHours":total,"budgetHours":c["budgetHours"],"budgetRemainingHours":if n(c,"budgetHours")!=0.{json!(n(c,"budgetHours")-total)}else{Value::Null}}));
    }
    let mut currencies = Vec::new();
    for t in totals {
        currencies.push(json!({"currency":t.code,"currencyDigits":currency_digits(&t.code)?,"totalHours":t.total,"billableHours":t.billable,"revenueMinor":safe_money(t.revenue)?,"costMinor":safe_money(t.cost)?,"profitMinor":safe_money(t.profit)?,"invoicedMinor":safe_money(t.invoiced)?,"outstandingMinor":safe_money(t.outstanding)?}));
    }
    Ok(
        json!({"startDate":start,"endDate":end,"currencies":currencies,"projects":projects,"contracts":contracts,"notes":["Estimates use approved entries and contract rates; currencies are reported separately without conversion.","Retainer/hybrid contracts have one fixed period. The fixed fee is recognized on its start date; included hours span its entire period. No recurrence or proration is assumed.","Invoice selection uses the UTC date on which each entry starts."]}),
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn nl(s: &str) -> String {
    escape(s).replace('\n', "<br>")
}
fn money(minor: u64, code: &str, digits: u32) -> String {
    let scale = 10u64.pow(digits);
    let whole = minor / scale;
    let raw = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i) % 3 == 0 {
            grouped.push(',')
        }
        grouped.push(c)
    }
    let value = if digits == 0 {
        grouped
    } else {
        format!(
            "{grouped}.{:0width$}",
            minor % scale,
            width = digits as usize
        )
    };
    let prefix = match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "CNY" => "CN¥",
        "CAD" => "CA$",
        "AUD" => "A$",
        "HKD" => "HK$",
        "NZD" => "NZ$",
        "TWD" => "NT$",
        "INR" => "₹",
        "KRW" => "₩",
        "BRL" => "R$",
        "ILS" => "₪",
        "MXN" => "MX$",
        "PHP" => "₱",
        "VND" => "₫",
        "XCD" => "EC$",
        "XAF" => "FCFA",
        "XCG" => "Cg.",
        "XOF" => "F\u{202f}CFA",
        "XPF" => "CFPF",
        _ => code,
    };
    escape(&format!(
        "{prefix}{}{value}",
        if prefix == code || ["XAF", "XCG", "XOF", "XPF"].contains(&code) {
            "\u{a0}"
        } else {
            ""
        }
    ))
}
// JavaScript toFixed(4) rounds the exact binary value toward the larger
// positive integer on a tie (unlike Rust formatting's ties-to-even).
fn fixed_hours(hours: f64) -> String {
    let bits = hours.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023 - 52;
    let mantissa = (bits & ((1u64 << 52) - 1)) | if exponent == -1075 { 0 } else { 1u64 << 52 };
    let numerator = mantissa as u128 * 10000;
    let rounded = if hours == 0. || exponent < -127 {
        0
    } else if exponent >= 0 {
        numerator << exponent
    } else {
        let denominator = 1u128 << (-exponent);
        (numerator + denominator / 2) / denominator
    };
    format!("{}.{:04}", rounded / 10000, rounded % 10000)
}
pub fn invoice_html(value: &Value) -> Result<String> {
    let inv = validate_invoice(value)?;
    let code = s(&inv, "currency");
    let digits = u(&inv, "currencyDigits") as u32;
    let m = |key: &str| money(u(&inv, key), code, digits);
    let mut rows = String::new();
    for l in items(&inv, "lines") {
        let hours = if s(l, "kind") == "retainer" {
            "—".into()
        } else {
            fixed_hours(n(l, "quantityHours"))
        };
        let rate = if s(l, "kind") == "retainer" {
            "—".into()
        } else {
            money(charge(3600000., n(l, "rate"), digits)?, code, digits)
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{hours}</td><td>{rate}</td><td>{}</td></tr>",
            nl(s(l, "description")),
            money(u(l, "amountMinor"), code, digits)
        ));
    }
    let payments = if items(&inv, "payments").is_empty() {
        String::new()
    } else {
        let mut h = "<section><h3>Payments</h3>".to_string();
        for p in items(&inv, "payments") {
            h.push_str(&format!(
                "<p>{} · {} · {}</p>",
                escape(s(p, "date")),
                money(u(p, "amountMinor"), code, digits),
                escape(s(p, "reference"))
            ));
        }
        h.push_str("</section>");
        h
    };
    let void = if s(&inv, "status") == "void" {
        format!("<p>Void reason: {}</p>", nl(s(&inv, "voidReason")))
    } else {
        String::new()
    };
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><title>Invoice {number}</title><style>body{{font:14px Arial,sans-serif;color:#20202a;padding:36px;line-height:1.5}}h1{{font-size:30px}}header,.parties{{display:flex;justify-content:space-between;gap:36px}}section{{margin:24px 0}}.parties>div{{width:48%}}table{{width:100%;border-collapse:collapse}}th,td{{text-align:left;border-bottom:1px solid #ddd;padding:10px}}th:last-child,td:last-child{{text-align:right}}.totals{{margin-left:auto;width:320px}}.totals p{{display:flex;justify-content:space-between}}.status{{font-weight:bold;text-transform:uppercase}}footer{{margin-top:40px;color:#555}}@page{{margin:18mm}}tr{{break-inside:avoid}}</style></head><body><header><div><h1>Invoice {number}</h1><p class="status">{status}</p></div><div>Issued: {issue}<br>Due: {due}<br>Currency: {code}</div></header><section class="parties"><div><strong>From</strong><p>{issuer}<br>{address}<br>{email}</p></div><div><strong>Bill to</strong><p>{client}<br>{contact}<br>{bill_address}<br>{bill_email}</p></div></section><table><thead><tr><th>Description</th><th>Hours</th><th>Rate</th><th>Amount</th></tr></thead><tbody>{rows}</tbody></table><section class="totals"><p><span>Subtotal</span><span>{subtotal}</span></p><p><span>Discount</span><span>{discount}</span></p><p><span>Tax ({tax_percent}%)</span><span>{tax}</span></p><p><strong>Total</strong><strong>{total}</strong></p><p><span>Paid</span><span>{paid}</span></p><p><strong>Outstanding</strong><strong>{outstanding}</strong></p></section>{payments}<section>{notes}</section><footer>{instructions}{void}</footer></body></html>"#,
        number = escape(s(&inv, "number")),
        status = escape(s(&inv, "status")),
        issue = escape(s(&inv, "issueDate")),
        due = escape(s(&inv, "dueDate")),
        code = escape(code),
        issuer = nl(s(&inv["issuer"], "name")),
        address = nl(s(&inv["issuer"], "address")),
        email = escape(s(&inv["issuer"], "email")),
        client = escape(s(&inv, "clientName")),
        contact = escape(s(&inv["billTo"], "contact")),
        bill_address = nl(s(&inv["billTo"], "address")),
        bill_email = escape(s(&inv["billTo"], "email")),
        subtotal = m("subtotalMinor"),
        discount = m("discountMinor"),
        tax_percent = n(&inv, "taxBps") / 100.,
        tax = m("taxMinor"),
        total = m("totalMinor"),
        paid = m("paidMinor"),
        outstanding = m("outstandingMinor"),
        notes = nl(s(&inv, "notes")),
        instructions = nl(s(&inv, "paymentInstructions"))
    ))
}

#[cfg(test)]
mod formatting_tests {
    use super::*;
    #[test]
    fn all_baseline_currency_patterns_match_icu() {
        let g: Value =
            serde_json::from_str(include_str!("../tests/fixtures/business-v1.2-golden.json"))
                .unwrap();
        for (code, f) in g["currencyFormats"].as_object().unwrap() {
            let digits = f["digits"].as_u64().unwrap() as u32;
            assert_eq!(currency_digits(code).unwrap(), digits, "{code}");
            assert_eq!(
                money(123456, code, digits),
                f["formatted"].as_str().unwrap(),
                "{code}"
            );
        }
    }
    #[test]
    fn hours_format_matches_javascript_half_up_binary_ties() {
        assert_eq!(fixed_hours(0.03125), "0.0313");
        assert_eq!(fixed_hours(0.09375), "0.0938");
        assert_eq!(fixed_hours(0.), "0.0000");
        assert_eq!(fixed_hours(1.23445), "1.2345");
    }
}
