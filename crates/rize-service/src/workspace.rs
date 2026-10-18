use crate::{auth::clean_text, db::transaction, ApiError, ApiResult, Context};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const KINDS: &[&str] = &[
    "clients",
    "projects",
    "tasks",
    "entries",
    "contracts",
    "allocations",
    "invoices",
    "skills",
    "routines",
    "reports",
    "context",
];
pub fn authorize(member: &Value, action: &str) -> bool {
    let role = member["role"].as_str().unwrap_or("");
    match action {
        "read" => !role.is_empty(),
        "admin" => matches!(role, "owner" | "admin"),
        "write" => !role.is_empty() && role != "viewer",
        "finance" => {
            role != "viewer" && (role == "owner" || member["finance"].as_bool() == Some(true))
        }
        "resourcing" => {
            role != "viewer" && (role == "owner" || member["resourcing"].as_bool() == Some(true))
        }
        _ => false,
    }
}
fn can_read(member: &Value, user: &str, kind: &str, owner: &str) -> bool {
    if matches!(kind, "contracts" | "invoices") && !authorize(member, "finance")
        || kind == "allocations" && !authorize(member, "resourcing")
    {
        return false;
    }
    !matches!(
        kind,
        "entries" | "skills" | "routines" | "reports" | "context"
    ) || authorize(member, "admin")
        || owner == user
}
fn can_write(member: &Value, user: &str, kind: &str, existing: Option<&Record>) -> bool {
    if !authorize(member, "write") {
        return false;
    }
    match kind {
        "contracts" | "invoices" => authorize(member, "finance"),
        "allocations" => authorize(member, "resourcing"),
        _ if authorize(member, "admin") => true,
        "entries" | "skills" | "routines" | "reports" | "context" => {
            existing.map(|r| r.owner == user).unwrap_or(true)
        }
        _ => false,
    }
}
pub fn iso(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or(DateTime::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
#[derive(Clone)]
struct Record {
    kind: String,
    id: String,
    owner: String,
    data: Value,
    version: i64,
    deleted: bool,
    updated: i64,
    cursor: i64,
}
impl Record {
    fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        let text: String = r.get("data")?;
        Ok(Self {
            kind: r.get("kind")?,
            id: r.get("id")?,
            owner: r.get("owner_id")?,
            data: serde_json::from_str(&text).unwrap_or(Value::Null),
            version: r.get("version")?,
            deleted: r.get::<_, i64>("deleted")? != 0,
            updated: r.get("updated_at")?,
            cursor: r.get("cursor").unwrap_or(0),
        })
    }
    fn canonical(&self) -> Value {
        json!({"kind":self.kind,"id":self.id,"data":if self.deleted {Value::Null}else{self.data.clone()},"version":self.version,"ownerId":self.owner,"deleted":self.deleted,"updatedAt":iso(self.updated)})
    }
}
fn find(db: &Connection, team: &str, kind: &str, id: &str) -> ApiResult<Option<Record>> {
    Ok(db
        .query_row(
            "SELECT * FROM records WHERE team_id=? AND kind=? AND id=?",
            params![team, kind, id],
            Record::from_row,
        )
        .optional()?)
}
fn cursor(db: &Connection, team: &str) -> ApiResult<i64> {
    Ok(db.query_row(
        "SELECT COALESCE(MAX(cursor),0) FROM events WHERE team_id=?",
        [team],
        |r| r.get(0),
    )?)
}
fn identifier(value: &str) -> ApiResult<String> {
    clean_text(&json!(value), "Record ID", 160, false)
}
fn safe_integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|n| n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0)
            .map(|n| n as i64)
    })
}
pub fn valid_json(value: &Value, depth: usize) -> ApiResult<()> {
    if depth > 12 {
        return Err(ApiError::bad("Record nesting exceeds the limit."));
    }
    match value {
        Value::String(s) => {
            clean_text(&json!(s), "Record text", 32768, true)?;
        }
        Value::Array(a) => {
            if a.len() > 2000 {
                return Err(ApiError::bad("Record arrays are too large."));
            }
            for x in a {
                valid_json(x, depth + 1)?;
            }
        }
        Value::Object(o) => {
            if o.len() > 100 {
                return Err(ApiError::bad("Record contains too many fields."));
            }
            for (k, v) in o {
                if ["__proto__", "constructor", "prototype"].contains(&k.as_str()) {
                    return Err(ApiError::bad("Invalid record field."));
                }
                clean_text(&json!(k), "Record field", 80, false)?;
                valid_json(v, depth + 1)?;
            }
        }
        _ => (),
    }
    Ok(())
}
fn parse_iso(v: &Value) -> ApiResult<i64> {
    let s = v
        .as_str()
        .ok_or_else(|| ApiError::bad("Entry timestamps must contain a timezone offset."))?;
    static FORMAT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let format=FORMAT.get_or_init(||regex::Regex::new(r"^\d{4}-\d\d-\d\dT([01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d{1,3})?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$").expect("static timestamp format"));
    if !format.is_match(s) {
        return Err(ApiError::bad(
            "Entry timestamps must contain a timezone offset.",
        ));
    }
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .map_err(|_| ApiError::bad("Entry timestamps must contain a timezone offset."))
}
pub fn normalize_data(kind: &str, id: &str, value: &Value) -> ApiResult<Value> {
    if !KINDS.contains(&kind) {
        return Err(ApiError::bad("Unknown record kind."));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| ApiError::bad("Record data must be an object."))?;
    valid_json(value, 0)?;
    if serde_json::to_vec(value)
        .map_err(|_| ApiError::bad("Invalid record."))?
        .len()
        > 128 * 1024
    {
        return Err(ApiError::new(
            413,
            "A record cannot exceed 128 KiB.",
            "TOO_LARGE",
        ));
    }
    if obj.get("id").map(|v| v != id).unwrap_or(false) {
        return Err(ApiError::bad("Record IDs must match."));
    }
    if obj.contains_key("ownerId") || obj.contains_key("teamId") {
        return Err(ApiError::bad("Ownership is assigned by the server."));
    }
    let mut data = value.clone();
    data["id"] = json!(id);
    if kind == "entries" {
        const FIELDS: &[&str] = &[
            "id",
            "start",
            "end",
            "durationMs",
            "duration",
            "description",
            "status",
            "source",
            "category",
            "clientId",
            "projectId",
            "taskId",
            "billable",
            "labelIds",
            "externalId",
            "externalSource",
        ];
        if obj.keys().any(|k| !FIELDS.contains(&k.as_str())) {
            return Err(ApiError::new(400,"Share only approved time descriptions and tags. Raw application, window, URL, and device metadata are forbidden.","PRIVATE_ACTIVITY"));
        }
        if value["status"] != "approved" {
            return Err(ApiError::new(
                400,
                "Only approved entries may be shared.",
                "APPROVAL_REQUIRED",
            ));
        }
        let start = parse_iso(&value["start"])?;
        let end = parse_iso(&value["end"])?;
        let duration = end - start;
        if duration <= 0 || duration > 7 * 86_400_000 {
            return Err(ApiError::bad(
                "Entry duration must be positive and no longer than seven days.",
            ));
        }
        if obj
            .get("durationMs")
            .map(|v| v.as_f64() != Some(duration as f64))
            .unwrap_or(false)
            || obj
                .get("duration")
                .map(|v| v.as_f64() != Some(duration as f64 / 1000.0))
                .unwrap_or(false)
        {
            return Err(ApiError::bad("Entry duration must match its interval."));
        }
        if obj
            .get("billable")
            .map(|v| !v.is_boolean())
            .unwrap_or(false)
        {
            return Err(ApiError::bad("Billable must be a boolean."));
        }
        for field in [
            "description",
            "category",
            "clientId",
            "projectId",
            "taskId",
            "source",
            "externalId",
            "externalSource",
        ] {
            if let Some(v) = obj.get(field) {
                clean_text(
                    v,
                    field,
                    if field == "description" { 2000 } else { 160 },
                    true,
                )?;
            }
        }
        if let Some(labels) = obj.get("labelIds") {
            if !labels
                .as_array()
                .map(|a| {
                    a.len() <= 100
                        && a.iter().all(|v| {
                            v.as_str()
                                .map(|s| s.encode_utf16().count() <= 160)
                                .unwrap_or(false)
                        })
                })
                .unwrap_or(false)
            {
                return Err(ApiError::bad("Invalid entry label IDs."));
            }
        }
        data["start"] = json!(iso(start));
        data["end"] = json!(iso(end));
        data["durationMs"] = json!(duration);
    } else if kind == "invoices" {
        data = rize_core::business::validate_invoice(&data).map_err(|_| {
            ApiError::new(
                400,
                "The invoice snapshot violates accounting constraints.",
                "FINANCIAL_INVARIANT",
            )
        })?;
    }
    Ok(data)
}
fn financial_snapshot(db: &Connection, team: &str) -> ApiResult<Value> {
    let mut state = json!({"clients":[],"projects":[],"tasks":[],"entries":[],"business":rize_core::business::default_business()});
    let mut statement=db.prepare("SELECT * FROM records WHERE team_id=? AND deleted=0 AND kind IN ('clients','projects','tasks','entries','contracts','allocations','invoices')")?;
    let rows = statement
        .query_map([team], Record::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    for row in rows {
        if ["clients", "projects", "tasks", "entries"].contains(&row.kind.as_str()) {
            state[&row.kind].as_array_mut().unwrap().push(row.data);
        } else if ["contracts", "allocations", "invoices"].contains(&row.kind.as_str()) {
            state["business"][&row.kind]
                .as_array_mut()
                .unwrap()
                .push(row.data);
        }
    }
    let next = state["business"]["invoices"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["sequence"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    state["business"]["nextInvoiceNumber"] = json!(next.max(1));
    Ok(state)
}
fn capture_accounting(db: &Connection, team: &str, changes: &[Value]) -> ApiResult<Vec<Record>> {
    let mut statement =
        db.prepare("SELECT * FROM records WHERE team_id=? AND kind='invoices' AND deleted=0")?;
    let mut rows = statement
        .query_map([team], Record::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    for change in changes.iter().filter(|c| c["kind"] == "entries") {
        if let Some(row) =
            find(db, team, "entries", change["id"].as_str().unwrap_or(""))?.filter(|r| !r.deleted)
        {
            rows.push(row);
        }
    }
    Ok(rows)
}
fn validate_accounting(
    db: &Connection,
    team: &str,
    changes: &[Value],
    before: &[Record],
) -> ApiResult<()> {
    let mut state = financial_snapshot(db, team)?;
    state["business"]=rize_core::business::validate_business(&state["business"],&state).map_err(|_|ApiError::new(400,"Shared financial records violate invoice, contract, allocation, or source-time constraints. Review the change and retry.","FINANCIAL_INVARIANT"))?;
    let invoices = state["business"]["invoices"].as_array().unwrap();
    for change in changes
        .iter()
        .filter(|c| c["kind"] == "invoices" && c["deleted"] != true)
    {
        if let Some(current) = invoices.iter().find(|v| v["id"] == change["id"]) {
            let previous = before
                .iter()
                .find(|r| r.kind == "invoices" && r.id == change["id"].as_str().unwrap_or(""));
            if current["status"] != "void"
                && previous
                    .map(|v| v.data["status"] == "draft")
                    .unwrap_or(true)
            {
                rize_core::business::assert_invoice_entries(current, &state).map_err(|_| {
                    ApiError::new(
                        400,
                        "Shared financial records violate source-time constraints.",
                        "FINANCIAL_INVARIANT",
                    )
                })?;
            }
        }
    }
    for change in changes.iter().filter(|c| c["kind"] == "entries") {
        let id = change["id"].as_str().unwrap_or("");
        let Some(previous) = before.iter().find(|r| r.kind == "entries" && r.id == id) else {
            continue;
        };
        let locked = before
            .iter()
            .filter(|r| r.kind == "invoices" && r.data["status"] != "void")
            .any(|old| {
                old.data["lines"]
                    .as_array()
                    .map(|a| a.iter().any(|l| l["entryId"] == id))
                    .unwrap_or(false)
                    && invoices.iter().any(|now| {
                        now["id"] == old.id
                            && now["status"] != "void"
                            && now["lines"]
                                .as_array()
                                .map(|a| a.iter().any(|l| l["entryId"] == id))
                                .unwrap_or(false)
                    })
            });
        if locked
            && find(db, team, "entries", id)?
                .map(|r| r.deleted || r.data != previous.data)
                .unwrap_or(true)
        {
            return Err(ApiError::new(409,"Shared time reserved by an active invoice cannot change. Ask a finance member to void or revise the draft first.","BILLED_TIME_LOCKED"));
        }
    }
    Ok(())
}
fn put(
    db: &Connection,
    ctx: &Context,
    kind: &str,
    id: &str,
    data: &Value,
    base: Option<i64>,
    deleted: bool,
) -> ApiResult<Value> {
    identifier(id)?;
    if !KINDS.contains(&kind) {
        return Err(ApiError::bad("Unknown record kind."));
    }
    let existing = find(db, &ctx.team_id, kind, id)?;
    if !can_write(&ctx.member, ctx.user_id(), kind, existing.as_ref()) {
        return Err(ApiError::new(
            403,
            "Your team role does not permit changing this record.",
            "FORBIDDEN",
        ));
    }
    let current = existing.as_ref().map(|r| r.version).unwrap_or(0);
    let base = base.unwrap_or(current);
    if base < 0 || base > 9_007_199_254_740_991 {
        return Err(ApiError::bad("Base version must be a nonnegative integer."));
    }
    if base != current {
        return Err(ApiError::new(
            409,
            "Record changed on another device. Resolve the conflict before retrying.",
            "CONFLICT",
        ));
    }
    if existing.is_none() && deleted {
        return Err(ApiError::bad("Cannot delete a record that does not exist."));
    }
    if existing.is_none()
        && db.query_row(
            "SELECT COUNT(*) FROM records WHERE team_id=?",
            [&ctx.team_id],
            |r| r.get::<_, i64>(0),
        )? >= 50_000
    {
        return Err(ApiError::new(
            409,
            "This team has reached its 50,000-record limit.",
            "RECORD_LIMIT",
        ));
    }
    let normalized = if deleted {
        json!({})
    } else {
        normalize_data(kind, id, data)?
    };
    if kind == "invoices" {
        rize_core::business::assert_invoice_transition(existing.as_ref().filter(|r|!r.deleted).map(|r|&r.data),if deleted{None}else{Some(&normalized)}).map_err(|_|ApiError::new(409,"Invoice accounting snapshots and payment history are immutable after sending. Void an invoice instead of deleting it.","INVOICE_IMMUTABLE"))?;
    }
    let owner = existing
        .as_ref()
        .map(|r| r.owner.as_str())
        .unwrap_or(ctx.user_id());
    let version = current + 1;
    let now = ctx.now_ms();
    let text = normalized.to_string();
    db.execute("INSERT INTO records(team_id,kind,id,owner_id,data,version,deleted,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(team_id,kind,id) DO UPDATE SET data=excluded.data,version=excluded.version,deleted=excluded.deleted,updated_at=excluded.updated_at",params![ctx.team_id,kind,id,owner,text,version,deleted as i64,now])?;
    db.execute("INSERT INTO events(team_id,kind,id,owner_id,data,version,deleted,updated_at) VALUES(?,?,?,?,?,?,?,?)",params![ctx.team_id,kind,id,owner,text,version,deleted as i64,now])?;
    Ok(find(db, &ctx.team_id, kind, id)?.unwrap().canonical())
}

impl Context {
    pub fn records(&self, kind: &str) -> ApiResult<Vec<Value>> {
        if !KINDS.contains(&kind) {
            return Err(ApiError::bad("Unknown record kind."));
        }
        let db = self.state.db()?;
        let mut s = db.prepare(
            "SELECT * FROM records WHERE team_id=? AND kind=? AND deleted=0 ORDER BY id",
        )?;
        let rows = s
            .query_map(params![self.team_id, kind], Record::from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter(|r| can_read(&self.member, self.user_id(), &r.kind, &r.owner))
            .map(|r| r.canonical())
            .collect())
    }
    pub fn put_record(
        &self,
        kind: &str,
        id: &str,
        data: Value,
        base: Option<i64>,
    ) -> ApiResult<Value> {
        self.mutate_record(kind, id, data, base, false)
    }
    pub fn delete_record(&self, kind: &str, id: &str, base: Option<i64>) -> ApiResult<Value> {
        self.mutate_record(kind, id, Value::Null, base, true)
    }
    fn mutate_record(
        &self,
        kind: &str,
        id: &str,
        data: Value,
        base: Option<i64>,
        deleted: bool,
    ) -> ApiResult<Value> {
        let db = self.state.db()?;
        transaction(&db, || {
            let changes = [json!({"kind":kind,"id":id,"deleted":deleted})];
            let before = capture_accounting(&db, &self.team_id, &changes)?;
            let row = put(&db, self, kind, id, &data, base, deleted)?;
            validate_accounting(&db, &self.team_id, &changes, &before)?;
            Ok(row)
        })
    }
    pub fn sync(&self, changes: &Value) -> ApiResult<Value> {
        let changes = changes
            .as_array()
            .filter(|a| a.len() <= 500)
            .ok_or_else(|| ApiError::bad("Send at most 500 changes per sync."))?;
        let db = self.state.db()?;
        let mut seen = HashSet::new();
        for c in changes {
            let id = c["id"]
                .as_str()
                .ok_or_else(|| ApiError::bad("Invalid change."))?;
            identifier(id)?;
            let kind = c["kind"].as_str().unwrap_or("");
            if !KINDS.contains(&kind)
                || safe_integer(&c["baseVersion"])
                    .map(|n| n < 0 || n > 9_007_199_254_740_991)
                    .unwrap_or(true)
                || c.get("deleted").map(|v| !v.is_boolean()).unwrap_or(false)
            {
                return Err(ApiError::bad(
                    "Changes require a kind, ID, baseVersion, and optional deleted boolean.",
                ));
            }
            if !seen.insert(format!("{kind}:{id}")) {
                return Err(ApiError::bad(
                    "A record may appear only once in a sync batch.",
                ));
            }
            let existing = find(&db, &self.team_id, kind, id)?;
            if !can_write(&self.member, self.user_id(), kind, existing.as_ref()) {
                return Err(ApiError::new(
                    403,
                    "Your role does not permit one or more changes.",
                    "FORBIDDEN",
                ));
            }
            if c["deleted"] != true {
                normalize_data(kind, id, &c["data"])?;
            }
        }
        transaction(&db, || {
            let before = capture_accounting(&db, &self.team_id, changes)?;
            let mut records = Vec::new();
            let mut conflicts = Vec::new();
            let mut applied = Vec::new();
            for c in changes {
                let kind = c["kind"].as_str().unwrap();
                let id = c["id"].as_str().unwrap();
                let row = find(&db, &self.team_id, kind, id)?;
                let base = safe_integer(&c["baseVersion"]).unwrap();
                if row.as_ref().map(|r| r.version).unwrap_or(0) != base {
                    conflicts.push(json!({"kind":kind,"id":id,"baseVersion":base,"current":row.as_ref().filter(|r|can_read(&self.member,self.user_id(),kind,&r.owner)).map(Record::canonical)}));
                } else {
                    records.push(put(
                        &db,
                        self,
                        kind,
                        id,
                        &c["data"],
                        Some(base),
                        c["deleted"] == true,
                    )?);
                    applied.push(c.clone());
                }
            }
            validate_accounting(&db, &self.team_id, &applied, &before)?;
            Ok(json!({"cursor":cursor(&db,&self.team_id)?,"records":records,"conflicts":conflicts}))
        })
    }
    pub fn pull(&self, since: i64) -> ApiResult<Value> {
        if !(0..=9_007_199_254_740_991).contains(&since) {
            return Err(ApiError::bad("Sync cursor must be a nonnegative integer."));
        }
        let db = self.state.db()?;
        if since > cursor(&db, &self.team_id)? {
            return Err(ApiError::new(
                409,
                "Cursor belongs to another team or a newer database. Restart a full sync.",
                "INVALID_CURSOR",
            ));
        }
        let mut s = db.prepare(
            "SELECT * FROM events WHERE team_id=? AND cursor>? ORDER BY cursor LIMIT 1001",
        )?;
        let rows = s
            .query_map(params![self.team_id, since], Record::from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let page = &rows[..rows.len().min(1000)];
        Ok(
            json!({"cursor":page.last().map(|r|r.cursor).unwrap_or(since),"records":page.iter().filter(|r|can_read(&self.member,self.user_id(),&r.kind,&r.owner)).map(Record::canonical).collect::<Vec<_>>(),"hasMore":rows.len()>1000}),
        )
    }
    pub fn kv_get(&self, namespace: &str, key: &str) -> ApiResult<Option<Value>> {
        clean_text(&json!(namespace), "Namespace", 100, false)?;
        clean_text(&json!(key), "Key", 300, false)?;
        let db = self.state.db()?;
        let text: Option<String> = db
            .query_row(
                "SELECT value FROM kv WHERE team_id=? AND namespace=? AND key=?",
                params![self.team_id, namespace, key],
                |r| r.get(0),
            )
            .optional()?;
        text.map(|s| serde_json::from_str(&s).map_err(|_| ApiError::internal()))
            .transpose()
    }
    pub fn kv_put(&self, namespace: &str, key: &str, value: Value) -> ApiResult<()> {
        clean_text(&json!(namespace), "Namespace", 100, false)?;
        clean_text(&json!(key), "Key", 300, false)?;
        valid_json(&value, 0)?;
        let text = value.to_string();
        if text.len() > 1024 * 1024 {
            return Err(ApiError::new(
                413,
                "Stored service value exceeds 1 MiB.",
                "TOO_LARGE",
            ));
        }
        self.state.db()?.execute("INSERT INTO kv(team_id,namespace,key,value) VALUES(?,?,?,?) ON CONFLICT(team_id,namespace,key) DO UPDATE SET value=excluded.value",params![self.team_id,namespace,key,text])?;
        Ok(())
    }
}

pub fn split_days(mut start: i64, end: i64, tz: Tz) -> Vec<(String, i64, i64)> {
    let day = |ms| {
        Utc.timestamp_millis_opt(ms)
            .single()
            .unwrap()
            .with_timezone(&tz)
            .format("%Y-%m-%d")
            .to_string()
    };
    let mut result = Vec::new();
    while start < end {
        let key = day(start);
        if day(end - 1) == key {
            result.push((key, start, end));
            break;
        }
        let mut lo = start + 1;
        let mut hi = end.min(start + 36 * 3_600_000);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if day(mid) == key {
                lo = mid + 1
            } else {
                hi = mid
            }
        }
        result.push((key, start, lo));
        start = lo;
    }
    result
}
fn union_duration(intervals: &mut [(i64, i64)]) -> i64 {
    intervals.sort_by_key(|v| v.0);
    let mut end = i64::MIN;
    let mut total = 0;
    for (start, stop) in intervals {
        if *stop > end {
            total += *stop - end.max(*start);
            end = *stop;
        }
    }
    total
}
pub fn analytics(ctx: &Context, query: &HashMap<String, String>) -> ApiResult<Value> {
    let timezone = query.get("timezone").map(String::as_str).unwrap_or("UTC");
    let tz: Tz = timezone
        .parse()
        .map_err(|_| ApiError::bad("Unknown IANA timezone."))?;
    let now = ctx.now_ms();
    let default_end = iso(now)[..10].to_string();
    let default_start = iso(now - 6 * 86_400_000)[..10].to_string();
    let start = query.get("startDate").unwrap_or(&default_start);
    let end = query.get("endDate").unwrap_or(&default_end);
    let parse = |s: &str| {
        if s.len() == 10
            && s.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .filter(|d| d.format("%Y-%m-%d").to_string() == s)
        } else {
            None
        }
    };
    let a = parse(start).ok_or_else(|| {
        ApiError::bad("Analytics dates must form a range no longer than one year.")
    })?;
    let b = parse(end).ok_or_else(|| {
        ApiError::bad("Analytics dates must form a range no longer than one year.")
    })?;
    let days = (b - a).num_days();
    if !(0..=366).contains(&days) {
        return Err(ApiError::bad(
            "Analytics dates must form a range no longer than one year.",
        ));
    }
    let capacity = query
        .get("capacityHours")
        .map(|s| s.parse::<f64>().unwrap_or(f64::NAN))
        .unwrap_or(40.0);
    if !capacity.is_finite() || !(1.0..=168.0).contains(&capacity) {
        return Err(ApiError::bad(
            "Weekly capacity must be between 1 and 168 hours.",
        ));
    }
    let db = ctx.state.db()?;
    let mut grouped: BTreeMap<(String, String), (Vec<(i64, i64)>, Vec<(i64, i64)>, usize)> =
        BTreeMap::new();
    let mut entries =
        db.prepare("SELECT * FROM records WHERE team_id=? AND kind='entries' AND deleted=0")?;
    let entries = entries
        .query_map([&ctx.team_id], Record::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    for row in entries {
        let from = parse_iso(&row.data["start"])?;
        let to = parse_iso(&row.data["end"])?;
        for (day, s, e) in split_days(from, to, tz) {
            if day < *start || day > *end {
                continue;
            }
            let group = grouped.entry((day, row.owner.clone())).or_default();
            group.0.push((s, e));
            if row.data["billable"] == true {
                group.1.push((s, e));
            }
            group.2 += 1;
        }
    }
    let timesheet:Vec<Value>=grouped.into_iter().map(|((date,user),(mut intervals,mut billable,count))|json!({"userId":user,"date":date,"durationMs":union_duration(&mut intervals),"billableMs":union_duration(&mut billable),"entryCount":count})).collect();
    let mut s=db.prepare("SELECT m.user_id,u.name FROM members m JOIN users u ON u.id=m.user_id WHERE m.team_id=? ORDER BY m.joined_at,m.user_id")?;
    let members = s
        .query_map([&ctx.team_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let utilization:Vec<Value>=members.into_iter().map(|(id,name)|{let rows:Vec<_>=timesheet.iter().filter(|r|r["userId"]==id).collect();let duration:i64=rows.iter().map(|r|r["durationMs"].as_i64().unwrap()).sum();let billable:i64=rows.iter().map(|r|r["billableMs"].as_i64().unwrap()).sum();let cap=capacity*(days+1) as f64/7.0*3_600_000.0;json!({"userId":id,"name":name,"durationMs":duration,"billableMs":billable,"capacityMs":cap,"utilization":duration as f64/cap})}).collect();
    let duration: i64 = timesheet
        .iter()
        .map(|r| r["durationMs"].as_i64().unwrap())
        .sum();
    let billable: i64 = timesheet
        .iter()
        .map(|r| r["billableMs"].as_i64().unwrap())
        .sum();
    Ok(
        json!({"timezone":timezone,"startDate":start,"endDate":end,"timesheet":timesheet,"utilization":utilization,"totals":{"durationMs":duration,"billableMs":billable},"capacityBasis":"Weekly capacity prorated across calendar days; overlapping entries count once per member."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daylight_and_privacy_validation() {
        let parts = split_days(
            DateTime::parse_from_rfc3339("2026-03-08T05:00:00Z")
                .unwrap()
                .timestamp_millis(),
            DateTime::parse_from_rfc3339("2026-03-09T05:00:00Z")
                .unwrap()
                .timestamp_millis(),
            chrono_tz::America::New_York,
        );
        assert_eq!(
            parts.iter().map(|p| p.2 - p.1).collect::<Vec<_>>(),
            vec![23 * 3_600_000, 3_600_000]
        );
        assert_eq!(
            normalize_data(
                "entries",
                "id",
                &json!({"status":"approved","app":"private"})
            )
            .unwrap_err()
            .code,
            "PRIVATE_ACTIVITY"
        );
    }
}
