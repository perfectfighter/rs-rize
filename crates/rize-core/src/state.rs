//! Schema 1 workspace storage and transactional commands, ported from the v1.2 desktop.
use crate::{business, timer, Result};
use chrono::{Local, TimeZone, Timelike};
use serde_json::{json, Map, Value};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 25 * 1024 * 1024;
const LOCAL_KINDS: [&str; 7] = [
    "clients",
    "projects",
    "tasks",
    "entries",
    "contracts",
    "allocations",
    "invoices",
];

pub fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{label} must be an object."))
}
pub fn text(value: &Value, label: &str, max: usize, empty: bool) -> Result<String> {
    let s = value.as_str().ok_or_else(|| {
        format!("{label} must be text of at most {max} characters without control characters.")
    })?;
    if s.encode_utf16().count() > max || s.chars().any(forbidden_control) {
        return Err(format!(
            "{label} must be text of at most {max} characters without control characters."
        ));
    }
    let result = s.trim_matches(js_whitespace).to_string();
    if !empty && result.is_empty() {
        return Err(format!("{label} is required."));
    }
    Ok(result)
}
fn forbidden_control(c: char) -> bool {
    matches!(c as u32,0..=8|11..=12|14..=31|127)
}
fn js_whitespace(c: char) -> bool {
    matches!(c as u32,9..=13|32|0xa0|0x1680|0x2000..=0x200a|0x2028|0x2029|0x202f|0x205f|0x3000|0xfeff)
}
pub fn number(value: &Value, label: &str, min: f64, max: f64) -> Result<f64> {
    let n = value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= min && *n <= max)
        .ok_or_else(|| format!("{label} must be between {min} and {max}."))?;
    Ok(n)
}
pub fn numeric(value: f64) -> Value {
    if value.is_finite()
        && value.fract() == 0.
        && value >= i64::MIN as f64
        && value < i64::MAX as f64
    {
        json!(value as i64)
    } else {
        json!(value)
    }
}
fn canonical_numbers(value: &mut Value) {
    match value {
        Value::Array(a) => a.iter_mut().for_each(canonical_numbers),
        Value::Object(m) => m.values_mut().for_each(canonical_numbers),
        Value::Number(n) if n.is_f64() => *value = numeric(n.as_f64().unwrap()),
        _ => {}
    }
}
pub fn boolean(value: &Value, label: &str) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| format!("{label} must be true or false."))
}
pub fn choice(value: &Value, choices: &[&str], label: &str) -> Result<String> {
    value
        .as_str()
        .filter(|s| choices.contains(s))
        .map(str::to_string)
        .ok_or_else(|| format!("{label} must be {}.", choices.join(", ")))
}
pub fn id(value: &Value, label: &str) -> Result<String> {
    text(value, label, 160, false)
}
pub fn list<'a>(value: &'a Value, label: &str, limit: usize) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .filter(|a| a.len() <= limit)
        .ok_or_else(|| format!("{label} must be an array with at most {limit} items."))
}
fn default<'a>(value: &'a Value, key: &str, fallback: &'a Value) -> &'a Value {
    value.get(key).filter(|v| !v.is_null()).unwrap_or(fallback)
}
pub fn iso(value: &Value, label: &str) -> Result<Value> {
    let failure = || format!("{label} must be a valid ISO timestamp.");
    let s = value.as_str().ok_or_else(failure)?;
    let b = s.as_bytes();
    if b.len() < 20
        || b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || b.get(10) != Some(&b'T')
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
    {
        return Err(failure());
    }
    for i in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !b[i].is_ascii_digit() {
            return Err(failure());
        }
    }
    if b[17] > b'5' {
        return Err(failure());
    }
    let mut pos = 19;
    if b.get(pos) == Some(&b'.') {
        pos += 1;
        let start = pos;
        while b.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1
        }
        if pos == start || pos - start > 3 {
            return Err(failure());
        }
    }
    let tail = &s[pos..];
    if tail != "Z" {
        let t = tail.as_bytes();
        if t.len() != 6
            || !matches!(t[0], b'+' | b'-')
            || t[3] != b':'
            || ![1, 2, 4, 5].iter().all(|i| t[*i].is_ascii_digit())
        {
            return Err(failure());
        }
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(s).map_err(|_| failure())?;
    if parsed.second() > 59 {
        return Err(failure());
    }
    Ok(json!(parsed
        .to_utc()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
}
fn keywords(value: Option<&Value>) -> Result<Value> {
    let empty = json!([]);
    let value = value.unwrap_or(&empty);
    let items = if let Some(s) = value.as_str() {
        s.split(',').map(|s| json!(s)).collect::<Vec<_>>()
    } else {
        list(value, "Keywords", 100)?.clone()
    };
    let mut result = Vec::new();
    for item in items {
        let word = text(&item, "Keyword", 120, true)?;
        if !word.is_empty() && !result.contains(&json!(word)) {
            result.push(json!(word))
        }
    }
    Ok(json!(result))
}
fn external_metadata(value: &Value) -> Result<Value> {
    let url = text(
        default(value, "externalUrl", &json!("")),
        "External URL",
        2000,
        true,
    )?;
    if !url.is_empty() {
        let parsed =
            url::Url::parse(&url).map_err(|_| "External URL must be a valid HTTPS URL.")?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("External URL must use HTTPS without credentials.".into());
        }
    }
    Ok(
        json!({"externalId":text(default(value,"externalId",&json!("")),"External ID",300,true)?,"externalProvider":text(default(value,"externalProvider",&json!("")),"External provider",80,true)?,"externalUrl":url}),
    )
}
fn extend(target: &mut Value, source: Value) {
    for (k, v) in source.as_object().unwrap() {
        target[k] = v.clone()
    }
}
pub fn default_workspace(_now_ms: i64) -> Value {
    json!({"schemaVersion":1,"workspaceId":uuid::Uuid::new_v4().to_string(),"onboarded":false,"demo":false,"settings":{"displayName":"You","trackingEnabled":false,"idleSeconds":300,"scheduleEnabled":false,"scheduleStart":"09:00","scheduleEnd":"18:00","workDays":[1,2,3,4,5]},"entries":[],"clients":[],"projects":[],"tasks":[],"rules":[],"sessions":[],"timer":null,"business":business::default_business(),"sharedIds":{"clients":[],"projects":[],"tasks":[],"entries":[],"contracts":[],"allocations":[],"invoices":[]}})
}
pub fn normalize_settings(value: &Value) -> Result<Value> {
    object(value, "Settings")?;
    let mut days = Vec::<i64>::new();
    for day in list(&value["workDays"], "Work days", 7)? {
        let n = number(day, "Work day", 0., 6.)?;
        if n.fract() != 0. {
            return Err("Work day must be an integer.".into());
        }
        if !days.contains(&(n as i64)) {
            days.push(n as i64)
        }
    }
    days.sort();
    let start = text(&value["scheduleStart"], "Schedule start", 5, false)?;
    let end = text(&value["scheduleEnd"], "Schedule end", 5, false)?;
    for s in [&start, &end] {
        let b = s.as_bytes();
        if b.len() != 5
            || b[2] != b':'
            || ![0, 1, 3, 4].iter().all(|i| b[*i].is_ascii_digit())
            || s[..2].parse::<u8>().unwrap() > 23
            || s[3..].parse::<u8>().unwrap() > 59
        {
            return Err("Schedule times must use HH:mm.".into());
        }
    }
    Ok(
        json!({"displayName":text(&value["displayName"],"Display name",80,false)?,"trackingEnabled":boolean(&value["trackingEnabled"],"Tracking enabled")?,"idleSeconds":number(&value["idleSeconds"],"Idle threshold",30.,3600.)?,"scheduleEnabled":boolean(&value["scheduleEnabled"],"Schedule enabled")?,"scheduleStart":start,"scheduleEnd":end,"workDays":days}),
    )
}
pub fn links(value: &Value, state: &Value) -> Result<Value> {
    let mut client = text(
        default(value, "clientId", &json!("")),
        "Client ID",
        160,
        true,
    )?;
    let mut project = text(
        default(value, "projectId", &json!("")),
        "Project ID",
        160,
        true,
    )?;
    let task = text(default(value, "taskId", &json!("")), "Task ID", 160, true)?;
    if !task.is_empty() {
        let found = state["tasks"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["id"] == task))
            .ok_or("The selected task does not exist.")?;
        if (!project.is_empty() && found["projectId"] != project)
            || (!client.is_empty() && found["clientId"] != client)
        {
            return Err("The task does not belong to the selected project or client.".into());
        }
        project = found["projectId"].as_str().unwrap_or("").into();
        client = found["clientId"].as_str().unwrap_or("").into();
    }
    if !project.is_empty() {
        let found = state["projects"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["id"] == project))
            .ok_or("The selected project does not exist.")?;
        if !client.is_empty() && found["clientId"] != client {
            return Err("The project does not belong to the selected client.".into());
        }
        client = found["clientId"].as_str().unwrap_or("").into();
    }
    if !client.is_empty()
        && !state["clients"]
            .as_array()
            .is_some_and(|a| a.iter().any(|c| c["id"] == client))
    {
        return Err("The selected client does not exist.".into());
    }
    Ok(json!({"clientId":client,"projectId":project,"taskId":task}))
}
pub fn normalize_entry(value: &Value, state: &Value) -> Result<Value> {
    object(value, "Time entry")?;
    let start = iso(&value["start"], "Entry start")?;
    let end = iso(&value["end"], "Entry end")?;
    let duration = timer::millis(&end)? - timer::millis(&start)?;
    if duration <= 0 {
        return Err("An entry must end after it starts.".into());
    }
    let mut labels = Vec::new();
    for label in list(default(value, "labelIds", &json!([])), "Entry labels", 100)? {
        let label = id(label, "Label ID")?;
        if !labels.contains(&label) {
            labels.push(label)
        }
    }
    let mut result = json!({"id":id(&value["id"],"ID")?,"start":start,"end":end,"durationMs":duration,"description":text(default(value,"description",&json!("")),"Description",2000,true)?,"app":text(default(value,"app",&json!("Manual entry")),"Application",300,true)?,"title":text(default(value,"title",&json!("")),"Window title",2000,true)?,"status":choice(default(value,"status",&json!("approved")),&["pending","approved"],"Entry status")?,"source":choice(default(value,"source",&json!("manual")),&["automatic","manual","timer"],"Entry source")?,"category":text(default(value,"category",&json!("neutral")),"Category",80,true)?,"billable":boolean(default(value,"billable",&json!(false)),"Billable")?,"labelIds":labels,"externalSource":text(default(value,"externalSource",&json!("")),"External source",80,true)?});
    extend(&mut result, links(value, state)?);
    extend(&mut result, external_metadata(value)?);
    Ok(result)
}
fn unique(items: Vec<Value>, label: &str) -> Result<Value> {
    let mut seen = HashSet::new();
    for item in &items {
        let ident = item["id"].as_str().unwrap();
        if !seen.insert(ident) {
            return Err(format!("Duplicate {label} ID: {ident}"));
        }
    }
    Ok(json!(items))
}
fn normalize_entity(state: &Value, kind: &str, value: &Value) -> Result<Value> {
    object(
        value,
        &format!("{}{}", kind[..1].to_uppercase(), &kind[1..]),
    )?;
    let mut result = json!({"id":id(&value["id"],"ID")?,"name":text(&value["name"],&format!("{}{} name",kind[..1].to_uppercase(),&kind[1..]),200,false)?});
    match kind {
        "client" => {
            result["hourlyRate"] = json!(number(
                default(value, "hourlyRate", &json!(0)),
                "Hourly rate",
                0.,
                1000000.
            )?);
        }
        "project" => {
            result["clientId"] =
                links(&json!({"clientId":value["clientId"]}), state)?["clientId"].clone();
            result["keywords"] = keywords(value.get("keywords"))?;
            extend(&mut result, external_metadata(value)?);
        }
        "task" => {
            let linked = links(
                &json!({"clientId":value["clientId"],"projectId":value["projectId"]}),
                state,
            )?;
            for k in ["clientId", "projectId"] {
                result[k] = linked[k].clone()
            }
            result["keywords"] = keywords(value.get("keywords"))?;
            result["done"] = json!(boolean(
                default(value, "done", &json!(false)),
                "Task complete"
            )?);
            extend(&mut result, external_metadata(value)?);
        }
        "rule" => {
            result["keyword"] = json!(text(&value["keyword"], "Rule keyword", 200, false)?);
            result["action"] = json!(choice(
                default(value, "action", &json!("productive")),
                &["productive", "neutral", "distracting", "exclude"],
                "Rule action"
            )?);
            extend(&mut result, links(value, state)?);
        }
        _ => return Err("Unknown entity kind.".into()),
    }
    Ok(result)
}
pub fn validate_snapshot(value: &Value) -> Result<Value> {
    object(value, "Workspace")?;
    if value["schemaVersion"].as_f64() != Some(1.) {
        return Err("Unsupported workspace schema version.".into());
    }
    let mut state = default_workspace(0);
    if let Some(w) = value.get("workspaceId") {
        state["workspaceId"] = json!(id(w, "Workspace ID")?)
    }
    state["onboarded"] = json!(boolean(&value["onboarded"], "Onboarding flag")?);
    state["demo"] = json!(boolean(&value["demo"], "Sample workspace flag")?);
    state["settings"] = normalize_settings(&value["settings"])?;
    if state["demo"] == true && state["settings"]["trackingEnabled"] == true {
        return Err("Automatic tracking is unavailable in a sample workspace.".into());
    }
    for (kind, plural, limit) in [
        ("client", "clients", 2000),
        ("project", "projects", 5000),
        ("task", "tasks", 10000),
        ("rule", "rules", 5000),
    ] {
        let empty = json!([]);
        let source = if kind == "rule" {
            value.get(plural).unwrap_or(&empty)
        } else {
            &value[plural]
        };
        let items = list(
            source,
            &format!("{}{}", plural[..1].to_uppercase(), &plural[1..]),
            limit,
        )?
        .iter()
        .map(|item| {
            if kind == "rule" {
                choice(
                    &item["action"],
                    &["productive", "neutral", "distracting", "exclude"],
                    "Rule action",
                )?;
            }
            normalize_entity(&state, kind, item)
        })
        .collect::<Result<Vec<_>>>()?;
        state[plural] = unique(items, kind)?;
    }
    state["entries"] = unique(
        list(&value["entries"], "Entries", 50000)?
            .iter()
            .map(|v| normalize_entry(v, &state))
            .collect::<Result<Vec<_>>>()?,
        "entry",
    )?;
    state["sessions"] = unique(
        list(&value["sessions"], "Sessions", 10000)?
            .iter()
            .map(|v| timer::normalize_session(v, &state))
            .collect::<Result<Vec<_>>>()?,
        "session",
    )?;
    if value.get("timer").is_none() {
        return Err("Timer must be an object.".into());
    }
    state["timer"] = timer::normalize_timer(&value["timer"], &state)?;
    state["business"] = business::validate_business(&value["business"], &state)?;
    let labels = state["business"]["labels"]
        .as_array()
        .ok_or("Invalid business labels.")?;
    for entry in state["entries"].as_array().unwrap() {
        for label in entry["labelIds"].as_array().unwrap() {
            if !labels.iter().any(|v| v["id"] == *label) {
                return Err("An entry label does not exist.".into());
            }
        }
    }
    if let Some(shared) = value.get("sharedIds") {
        object(shared, "Shared record IDs")?;
        for kind in LOCAL_KINDS {
            let mut ids = Vec::new();
            for val in list(
                default(shared, kind, &json!([])),
                "Shared record IDs",
                50000,
            )? {
                let ident = id(val, "ID")?;
                if !ids.contains(&ident) {
                    ids.push(ident)
                }
            }
            state["sharedIds"][kind] = json!(ids)
        }
    }
    canonical_numbers(&mut state);
    Ok(state)
}
fn plural(kind: &str) -> &str {
    match kind {
        "client" => "clients",
        "project" => "projects",
        "task" => "tasks",
        "rule" => "rules",
        _ => unreachable!(),
    }
}
pub fn modify_entity(state: &mut Value, kind: &str, payload: &Value) -> Result<()> {
    let ident = if payload
        .get("id")
        .is_some_and(|i| i.as_str().is_some_and(|s| !s.is_empty()))
    {
        id(&payload["id"], "ID")?
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    let p = plural(kind);
    let index = state[p]
        .as_array()
        .unwrap()
        .iter()
        .position(|v| v["id"] == ident);
    let mut value = index.map(|i| state[p][i].clone()).unwrap_or(json!({}));
    extend(&mut value, payload.clone());
    value["id"] = json!(ident);
    let entity = normalize_entity(state, kind, &value)?;
    if let Some(i) = index {
        state[p][i] = entity.clone()
    } else {
        state[p].as_array_mut().unwrap().push(entity.clone())
    }
    if kind == "project" || kind == "task" {
        let field = format!("{kind}Id");
        for key in if kind == "project" {
            vec!["tasks", "rules", "entries", "sessions"]
        } else {
            vec!["rules", "entries", "sessions"]
        } {
            for item in state[key].as_array_mut().unwrap() {
                if item[&field] == ident {
                    item["clientId"] = entity["clientId"].clone();
                    if kind == "task" {
                        item["projectId"] = entity["projectId"].clone()
                    }
                }
            }
        }
        if state["timer"][&field] == ident {
            state["timer"]["clientId"] = entity["clientId"].clone();
            if kind == "task" {
                state["timer"]["projectId"] = entity["projectId"].clone()
            }
        }
        if kind == "project" {
            for item in state["business"]["contracts"].as_array_mut().unwrap() {
                if item["projectId"] == ident {
                    item["clientId"] = entity["clientId"].clone()
                }
            }
        }
    }
    Ok(())
}
pub fn delete_entity(state: &mut Value, kind: &str, payload: &Value) -> Result<()> {
    let ident = id(&payload["id"], "ID")?;
    let p = plural(kind);
    if !state[p]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == ident)
    {
        return Err(format!("The {kind} does not exist."));
    }
    state[p]
        .as_array_mut()
        .unwrap()
        .retain(|v| v["id"] != ident);
    if kind == "rule" {
        return Ok(());
    }
    let field = format!("{kind}Id");
    for key in ["projects", "tasks", "rules", "entries", "sessions"] {
        for item in state[key].as_array_mut().unwrap() {
            if item[&field] == ident {
                item[&field] = json!("")
            }
        }
    }
    if state["timer"][&field] == ident {
        state["timer"][&field] = json!("")
    }
    if kind == "client" {
        for key in ["clientTerms", "contracts"] {
            state["business"][key]
                .as_array_mut()
                .unwrap()
                .retain(|v| v["clientId"] != ident)
        }
    }
    if kind == "project" {
        for key in ["contracts", "allocations"] {
            state["business"][key]
                .as_array_mut()
                .unwrap()
                .retain(|v| v["projectId"] != ident)
        }
    }
    Ok(())
}
fn sample_workspace(now_ms: i64) -> Result<Value> {
    let mut state = default_workspace(now_ms);
    state["onboarded"] = json!(true);
    state["demo"] = json!(true);
    state["settings"]["displayName"] = json!("Sample workspace");
    modify_entity(
        &mut state,
        "client",
        &json!({"id":"sample-client","name":"Acme Studio","hourlyRate":95}),
    )?;
    modify_entity(
        &mut state,
        "project",
        &json!({"id":"sample-project","name":"Website redesign","clientId":"sample-client","keywords":["Acme","Design"]}),
    )?;
    modify_entity(
        &mut state,
        "task",
        &json!({"id":"sample-task","name":"Design and development","clientId":"sample-client","projectId":"sample-project","keywords":["Figma","Visual Studio Code"]}),
    )?;
    let local = Local
        .timestamp_millis_opt(now_ms)
        .single()
        .ok_or("The current time is invalid.")?;
    for day in 0..5 {
        for block in 0..3 {
            let date = local.date_naive() - chrono::Duration::days(day);
            let naive = date
                .and_hms_opt(9 + block * 2, if block == 1 { 20 } else { 0 }, 0)
                .unwrap();
            let start = Local
                .from_local_datetime(&naive)
                .earliest()
                .ok_or("Sample date does not exist in local timezone.")?
                .timestamp_millis();
            let entry = json!({"id":format!("sample-{day}-{block}"),"start":timer::timestamp(start)?,"end":timer::timestamp(start+(70+block as i64*15)*60000)?,"description":(["Frontend implementation","Explore new concepts","Research and planning"][block as usize]),"app":(["Visual Studio Code","Figma","Google Chrome"][block as usize]),"title":"Acme · Website redesign","status":if block==2{"pending"}else{"approved"},"source":"automatic","category":"productive","clientId":"sample-client","projectId":"sample-project","taskId":"sample-task","billable":true});
            let clean = normalize_entry(&entry, &state)?;
            state["entries"].as_array_mut().unwrap().push(clean);
        }
    }
    Ok(state)
}
fn apply_command(mut state: Value, command: &Value, now_ms: i64) -> Result<Value> {
    object(command, "Command")?;
    let empty = json!({});
    let payload = command
        .get("payload")
        .filter(|v| !v.is_null())
        .unwrap_or(&empty);
    object(payload, "Command payload")?;
    let kind = text(&command["type"], "Command type", 80, false)?;
    match kind.as_str() {
        "workspace.create" => {
            let mut next = default_workspace(now_ms);
            next["onboarded"] = json!(true);
            next["settings"]["displayName"] = json!(text(
                default(payload, "displayName", &json!("You")),
                "Display name",
                80,
                false
            )?);
            return Ok(next);
        }
        "workspace.demo" => return sample_workspace(now_ms),
        "settings.update" => {
            let mut settings = state["settings"].clone();
            extend(&mut settings, payload.clone());
            state["settings"] = normalize_settings(&settings)?;
            if state["demo"] == true && state["settings"]["trackingEnabled"] == true {
                return Err("Create your own workspace before enabling automatic tracking.".into());
            }
            if !state["timer"].is_null() && state["settings"]["trackingEnabled"] == true {
                state["timer"]["autoCollected"] = json!(true)
            }
            return Ok(state);
        }
        _ => {}
    }
    if business::apply_business_command(
        &mut state,
        &json!({"type":kind,"payload":payload}),
        now_ms,
    )? {
        return Ok(state);
    }
    if let Some((entity, action)) = kind.split_once('.') {
        if ["client", "project", "task", "rule"].contains(&entity)
            && ["upsert", "delete"].contains(&action)
        {
            if action == "upsert" {
                modify_entity(&mut state, entity, payload)?
            } else {
                delete_entity(&mut state, entity, payload)?
            }
            return Ok(state);
        }
    }
    if kind == "entry.upsert" {
        let ident = if payload
            .get("id")
            .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            id(&payload["id"], "ID")?
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let index = state["entries"]
            .as_array()
            .unwrap()
            .iter()
            .position(|v| v["id"] == ident);
        if index.is_some() {
            business::assert_uninvoiced(&state, &[ident.clone()])?
        }
        let mut value = index
            .map(|i| state["entries"][i].clone())
            .unwrap_or(json!({}));
        extend(&mut value, payload.clone());
        value["id"] = json!(ident);
        let clean = normalize_entry(&value, &state)?;
        if let Some(i) = index {
            state["entries"][i] = clean
        } else {
            state["entries"].as_array_mut().unwrap().push(clean)
        }
        return Ok(state);
    }
    if kind == "entry.delete" || kind == "entry.review" {
        let ident = id(&payload["id"], "ID")?;
        let i = state["entries"]
            .as_array()
            .unwrap()
            .iter()
            .position(|v| v["id"] == ident)
            .ok_or("The time entry does not exist.")?;
        business::assert_uninvoiced(&state, &[ident])?;
        if kind == "entry.delete" {
            state["entries"].as_array_mut().unwrap().remove(i);
        } else {
            state["entries"][i]["status"] = json!(choice(
                &payload["status"],
                &["pending", "approved"],
                "Entry status"
            )?)
        }
        return Ok(state);
    }
    if timer::apply_timer_command(&mut state, &kind, payload, now_ms)? {
        return Ok(state);
    }
    Err(format!("Unknown command: {kind}"))
}

pub struct Store {
    state: Value,
    file: Option<PathBuf>,
}
impl Store {
    pub fn new(file: Option<PathBuf>) -> Result<Self> {
        if let Some(path) = &file {
            if path.as_os_str().is_empty() {
                return Err("Workspace file path must be a non-empty string.".into());
            }
            if path.exists() {
                let loaded = (|| {
                    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_BYTES as u64 {
                        return Err("Workspace file exceeds 25 MiB.".into());
                    }
                    let bytes = fs::read(path).map_err(|e| e.to_string())?;
                    let value = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("Invalid workspace JSON: {e}"))?;
                    let state = validate_snapshot(&value)?;
                    if state["timer"]["status"] == "running" {
                        return Err(
                            "A running timer snapshot must be paused before restoring it.".into(),
                        );
                    }
                    Ok(state)
                })();
                return loaded
                    .map(|state| Self { state, file })
                    .map_err(|e: String| {
                        format!("Cannot open workspace; the original file was preserved: {e}")
                    });
            }
        }
        Ok(Self {
            state: default_workspace(chrono::Utc::now().timestamp_millis()),
            file,
        })
    }
    pub fn from_value(value: Value, file: Option<PathBuf>) -> Result<Self> {
        let state = validate_snapshot(&value)?;
        if state["timer"]["status"] == "running" {
            return Err("A running timer snapshot must be paused before restoring it.".into());
        }
        Ok(Self { state, file })
    }
    pub fn get_state(&self) -> Value {
        self.state.clone()
    }
    fn persist(&self, next: &Value, now_ms: i64) -> Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let parent = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let save = (|| -> std::io::Result<()> {
            fs::create_dir_all(parent)?;
            let temp = parent.join(format!(
                ".{}.{}.{}.tmp",
                file.file_name().unwrap_or_default().to_string_lossy(),
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let result = (|| {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut out = options.open(&temp)?;
                let bytes = serde_json::to_vec_pretty(
                    &timer::checkpoint(next, now_ms).map_err(std::io::Error::other)?,
                )
                .map_err(std::io::Error::other)?;
                if bytes.len() > MAX_BYTES {
                    return Err(std::io::Error::other("Workspace file exceeds 25 MiB."));
                }
                out.write_all(&bytes)?;
                out.sync_all()?;
                drop(out);
                fs::rename(&temp, file)?;
                #[cfg(unix)]
                {
                    if let Ok(directory) = std::fs::File::open(parent) {
                        let _ = directory.sync_all();
                    }
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temp);
            }
            result
        })();
        save.map_err(|e| format!("Could not save workspace: {e}"))
    }
    fn commit(&mut self, next: Value, now_ms: i64) -> Result<Value> {
        timer::timestamp(now_ms)?;
        let clean = validate_snapshot(&next)?;
        self.persist(&clean, now_ms)?;
        self.state = clean;
        Ok(self.state.clone())
    }
    pub fn mutate(&mut self, command: &Value, now_ms: i64) -> Result<Value> {
        timer::timestamp(now_ms)?;
        let next = apply_command(self.state.clone(), command, now_ms)?;
        self.commit(next, now_ms)
    }
    pub fn flush(&mut self, now_ms: i64) -> Result<()> {
        timer::timestamp(now_ms)?;
        self.persist(&self.state, now_ms)
    }
    pub fn export_snapshot(&self, now_ms: i64) -> Result<Value> {
        timer::checkpoint(&self.state, now_ms)
    }
    pub fn import_data(&mut self, value: Value, now_ms: i64) -> Result<Value> {
        let parsed = if let Some(s) = value.as_str() {
            if s.len() > MAX_BYTES {
                return Err("Workspace import exceeds 25 MiB.".into());
            }
            serde_json::from_str(s).map_err(|_| "The selected file is not valid JSON.")?
        } else {
            value
        };
        let clean = validate_snapshot(&parsed)?;
        if clean["timer"]["status"] == "running" {
            return Err(
                "Imported timers must be paused to avoid counting time since export.".into(),
            );
        }
        self.commit(clean, now_ms)
    }
    pub fn upsert_activity(&mut self, entries: &[Value], now_ms: i64) -> Result<Value> {
        if entries.len() > 1000 {
            return Err("Activity entries must be an array with at most 1000 items.".into());
        }
        let mut draft = self.state.clone();
        for incoming in entries {
            object(incoming, "Activity entry")?;
            let index = draft["entries"]
                .as_array()
                .unwrap()
                .iter()
                .position(|v| v["id"] == incoming["id"]);
            if index.is_some_and(|i| {
                draft["entries"][i]["status"] == "approved"
                    || draft["entries"][i]["source"] != "automatic"
            }) {
                continue;
            }
            let mut safe = incoming.clone();
            safe["source"] = json!("automatic");
            safe["status"] = json!("pending");
            for field in ["app", "title", "description"] {
                if let Some(s) = safe[field].as_str() {
                    safe[field] = json!(s
                        .chars()
                        .filter(|c| !forbidden_control(*c))
                        .collect::<String>())
                }
            }
            let clean = normalize_entry(&safe, &draft)?;
            if let Some(i) = index {
                draft["entries"][i] = clean
            } else {
                draft["entries"].as_array_mut().unwrap().push(clean)
            }
        }
        if !draft["timer"].is_null() && draft["settings"]["trackingEnabled"] == true {
            draft["timer"]["autoCollected"] = json!(true)
        }
        self.commit(draft, now_ms)
    }
    pub fn merge_shared(&mut self, records: &[Value], now_ms: i64) -> Result<Value> {
        if records.len() > 50000 {
            return Err("Shared records must be an array with at most 50000 items.".into());
        }
        let mut incoming = Vec::new();
        for r in records {
            object(r, "Shared record")?;
            let kind = choice(
                &r["kind"],
                &[
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
                ],
                "Shared kind",
            )?;
            let mut r = r.clone();
            r["kind"] = json!(kind);
            r["id"] = json!(id(&r["id"], "ID")?);
            r["deleted"] = json!(boolean(
                default(&r, "deleted", &json!(false)),
                "Deleted flag"
            )?);
            if LOCAL_KINDS.contains(&kind.as_str()) {
                incoming.push(r)
            }
        }
        let mut draft = self.state.clone();
        incoming.sort_by_key(|r| LOCAL_KINDS.iter().position(|k| r["kind"] == *k).unwrap());
        for record in incoming.iter().filter(|r| r["deleted"] != true) {
            let kind = record["kind"].as_str().unwrap();
            let ident = record["id"].as_str().unwrap();
            object(&record["data"], "Shared record data")?;
            let mut data = record["data"].clone();
            data["id"] = json!(ident);
            let existing = items(&draft, kind)
                .iter()
                .find(|v| v["id"] == ident)
                .cloned();
            if kind == "entries" {
                if existing.as_ref().is_some_and(|e| e["status"] == "pending") {
                    continue;
                }
                let mut safe = existing.clone().unwrap_or(json!({}));
                extend(&mut safe, data);
                safe["status"] = json!("approved");
                safe["app"] = existing
                    .as_ref()
                    .map(|e| e["app"].clone())
                    .unwrap_or(json!("Cloud sync"));
                safe["title"] = existing
                    .as_ref()
                    .map(|e| e["title"].clone())
                    .unwrap_or(json!(""));
                if !["manual", "automatic", "timer"]
                    .iter()
                    .any(|s| safe["source"] == *s)
                {
                    safe["source"] = existing
                        .as_ref()
                        .map(|e| e["source"].clone())
                        .unwrap_or(json!("manual"));
                }
                let clean = normalize_entry(&safe, &draft)?;
                put_item(&mut draft, kind, ident, clean);
            } else if ["clients", "projects", "tasks"].contains(&kind) {
                modify_entity(&mut draft, &kind[..kind.len() - 1], &data)?;
            } else {
                if kind == "invoices" {
                    let mut b = draft["business"].clone();
                    b["invoices"] = json!([data]);
                    let sequence = b["invoices"][0]["sequence"].as_u64().unwrap_or(0) + 1;
                    let current = b["nextInvoiceNumber"].as_u64().unwrap_or(1);
                    b["nextInvoiceNumber"] = json!(current.max(sequence));
                    data = business::validate_business(&b, &draft)?["invoices"][0].clone();
                    business::assert_invoice_transition(existing.as_ref(), Some(&data))?;
                }
                put_item(&mut draft, kind, ident, data);
            }
            if !draft["sharedIds"][kind]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == ident)
            {
                draft["sharedIds"][kind]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(ident))
            }
        }
        for record in incoming.iter().rev().filter(|r| r["deleted"] == true) {
            let kind = record["kind"].as_str().unwrap();
            let ident = record["id"].as_str().unwrap();
            if !draft["sharedIds"][kind]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == ident)
            {
                continue;
            }
            let existing = items(&draft, kind)
                .iter()
                .find(|v| v["id"] == ident)
                .cloned();
            if existing.is_none() {
                draft["sharedIds"][kind]
                    .as_array_mut()
                    .unwrap()
                    .retain(|v| v != ident);
                continue;
            }
            let existing = existing.unwrap();
            if kind == "entries"
                && (existing["status"] == "pending"
                    || business::assert_uninvoiced(&draft, &[ident.into()]).is_err())
            {
                continue;
            }
            if ["clients", "projects", "tasks"].contains(&kind) {
                let entity = &kind[..kind.len() - 1];
                let field = format!("{entity}Id");
                let mut dependent = false;
                for key in ["entries", "sessions", "projects", "tasks", "rules"] {
                    dependent |= draft[key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v[&field] == ident)
                }
                for key in ["contracts", "allocations"] {
                    dependent |= draft["business"][key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v[&field] == ident)
                }
                dependent |= draft["timer"][&field] == ident;
                if dependent {
                    continue;
                }
                delete_entity(&mut draft, entity, &json!({"id":ident}))?;
            } else if kind == "invoices" {
                continue;
            } else {
                items_mut(&mut draft, kind).retain(|v| v["id"] != ident)
            }
            draft["sharedIds"][kind]
                .as_array_mut()
                .unwrap()
                .retain(|v| v != ident);
        }
        let maximum = draft["business"]["invoices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["sequence"].as_u64().unwrap_or(0) + 1)
            .max()
            .unwrap_or(1);
        draft["business"]["nextInvoiceNumber"] =
            json!(maximum.max(draft["business"]["nextInvoiceNumber"].as_u64().unwrap_or(1)));
        self.commit(draft, now_ms)
    }
}
fn items<'a>(state: &'a Value, kind: &str) -> &'a Vec<Value> {
    if ["contracts", "allocations", "invoices"].contains(&kind) {
        state["business"][kind].as_array().unwrap()
    } else {
        state[kind].as_array().unwrap()
    }
}
fn items_mut<'a>(state: &'a mut Value, kind: &str) -> &'a mut Vec<Value> {
    if ["contracts", "allocations", "invoices"].contains(&kind) {
        state["business"][kind].as_array_mut().unwrap()
    } else {
        state[kind].as_array_mut().unwrap()
    }
}
fn put_item(state: &mut Value, kind: &str, ident: &str, data: Value) {
    let items = items_mut(state, kind);
    if let Some(i) = items.iter().position(|v| v["id"] == ident) {
        items[i] = data
    } else {
        items.push(data)
    }
}

pub fn to_csv(state: &Value) -> String {
    to_csv_checked(state).unwrap_or_default()
}
/// ECMAScript `toFixed` rounds exact binary halfway values away from zero.
/// Rust's formatter uses ties-to-even, which changes legacy CSV amounts such as 0.625.
pub fn fixed_decimal(value: f64, digits: u32) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value.abs() >= 1e21 {
        return value.to_string();
    }
    let bits = value.abs().to_bits();
    let fraction = bits & ((1u64 << 52) - 1);
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = if exponent == 0 {
        fraction as u128
    } else {
        (fraction | (1u64 << 52)) as u128
    };
    let exponent = if exponent == 0 {
        -1074
    } else {
        exponent - 1023 - 52
    };
    let numerator = mantissa * 10u128.pow(digits);
    let rounded = if exponent >= 0 {
        numerator.checked_shl(exponent as u32).unwrap_or(u128::MAX)
    } else {
        let shift = (-exponent) as u32;
        if shift >= 128 {
            0
        } else {
            let denominator = 1u128 << shift;
            (numerator + denominator / 2) / denominator
        }
    };
    let denominator = 10u128.pow(digits);
    let sign = if value < 0. { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{rounded}")
    } else {
        format!(
            "{sign}{}.{:0width$}",
            rounded / denominator,
            rounded % denominator,
            width = digits as usize
        )
    }
}
fn js_number(value: f64) -> String {
    if value == 0. {
        return "0".into();
    }
    if value.abs() < 1e-6 || value.abs() >= 1e21 {
        let scientific = format!("{value:e}");
        if let Some((mantissa, power)) = scientific.split_once('e') {
            let exponent = power.parse::<i32>().unwrap_or(0);
            return format!(
                "{mantissa}e{}{exponent}",
                if exponent >= 0 { "+" } else { "" }
            );
        }
        scientific
    } else {
        value.to_string()
    }
}
pub fn to_csv_checked(state: &Value) -> Result<String> {
    let clean = validate_snapshot(state)?;
    let mut rows = vec![vec![
        "ID",
        "Start",
        "End",
        "Duration seconds",
        "Description",
        "Application",
        "Window title",
        "Status",
        "Source",
        "Category",
        "Client",
        "Project",
        "Task",
        "Billable",
        "Hourly rate USD",
        "Amount USD",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>()];
    let name = |kind: &str, ident: &Value| {
        clean[kind]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == *ident)
            .and_then(|v| v["name"].as_str())
            .unwrap_or("")
            .to_string()
    };
    for entry in clean["entries"].as_array().unwrap() {
        let rate = clean["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == entry["clientId"])
            .and_then(|v| v["hourlyRate"].as_f64())
            .unwrap_or(0.);
        let duration = entry["durationMs"].as_f64().unwrap();
        let mut row = ["id", "start", "end"]
            .iter()
            .map(|k| entry[k].as_str().unwrap_or("").to_string())
            .collect::<Vec<_>>();
        row.push(js_number(duration / 1000.));
        for key in [
            "description",
            "app",
            "title",
            "status",
            "source",
            "category",
        ] {
            row.push(entry[key].as_str().unwrap_or("").to_string())
        }
        row.extend([
            name("clients", &entry["clientId"]),
            name("projects", &entry["projectId"]),
            name("tasks", &entry["taskId"]),
            if entry["billable"] == true {
                "Yes".into()
            } else {
                "No".into()
            },
            js_number(rate),
            if entry["billable"] == true {
                fixed_decimal(duration / 3600000. * rate, 2)
            } else {
                "0.00".into()
            },
        ]);
        rows.push(row);
    }
    let quote = |s: &String| {
        let trimmed = s.trim_start_matches(js_whitespace);
        let dangerous = trimmed
            .chars()
            .next()
            .is_some_and(|c| matches!(c, '=' | '+' | '@' | '-'))
            || s.chars().next().is_some_and(|c| matches!(c, '\t' | '\r'));
        let cell = if dangerous {
            format!("'{s}")
        } else {
            s.clone()
        };
        format!("\"{}\"", cell.replace('"', "\"\""))
    };
    Ok(format!(
        "\u{feff}{}\r\n",
        rows.iter()
            .map(|r| r.iter().map(quote).collect::<Vec<_>>().join(","))
            .collect::<Vec<_>>()
            .join("\r\n")
    ))
}
