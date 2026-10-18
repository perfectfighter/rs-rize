//! Active intervals are stored explicitly: pauses and application downtime are never work.
use crate::{
    state::{boolean, choice, id, iso, links, number, object, text},
    Result,
};
use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::{json, Value};

pub fn timestamp(ms: i64) -> Result<String> {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or_else(|| "The current time is invalid.".into())
}
pub fn millis(value: &Value) -> Result<i64> {
    let s = value.as_str().ok_or("Timestamp must be text.")?;
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .map_err(|_| "Timestamp must be a valid ISO timestamp.".into())
}
pub fn segment_duration(segments: &[Value]) -> Result<i64> {
    segments.iter().try_fold(0i64, |sum, s| {
        Ok(sum + millis(&s["end"])? - millis(&s["start"])?)
    })
}
pub fn normalize_segments(
    value: Option<&Value>,
    started_at: &Value,
    ended_at: Option<&Value>,
) -> Result<Vec<Value>> {
    let empty = Vec::new();
    let segments = match value {
        None | Some(Value::Null) => &empty,
        Some(v) => v
            .as_array()
            .filter(|a| a.len() <= 10000)
            .ok_or("Timer segments must be an array with at most 10000 items.")?,
    };
    let mut previous = millis(started_at)?;
    let mut result = Vec::new();
    for s in segments {
        object(s, "Timer segment")?;
        let start = iso(&s["start"], "Segment start")?;
        let end = iso(&s["end"], "Segment end")?;
        let start_ms = millis(&start)?;
        let end_ms = millis(&end)?;
        if start_ms < previous
            || end_ms <= start_ms
            || ended_at
                .map(millis)
                .transpose()?
                .map(|e| end_ms > e)
                .unwrap_or(false)
        {
            return Err("Timer segments must be ordered, non-overlapping active intervals.".into());
        }
        previous = end_ms;
        result.push(json!({"start":start,"end":end}));
    }
    Ok(result)
}
fn metadata(value: &Value, state: &Value) -> Result<Value> {
    let mut result = json!({"id":id(&value["id"],"ID")?,"kind":choice(&value["kind"],&["focus","meeting","break"],"Session type")?,"goal":text(value.get("goal").filter(|v|!v.is_null()).unwrap_or(&json!("")),"Session goal",2000,true)?,"targetMs":number(&value["targetMs"],"Session target",60000.,86400000.)?,"autoCollected":boolean(value.get("autoCollected").filter(|v|!v.is_null()).unwrap_or(&Value::Bool(false)),"Automatic collection flag")?});
    for (k, v) in links(value, state)?.as_object().unwrap() {
        result[k] = v.clone();
    }
    Ok(result)
}
pub fn normalize_timer(value: &Value, state: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    object(value, "Timer")?;
    let started = iso(&value["startedAt"], "Session start")?;
    let status = choice(&value["status"], &["running", "paused"], "Timer status")?;
    let resumed = if status == "running" {
        iso(&value["resumedAt"], "Resume time")?
    } else {
        Value::Null
    };
    let segments = normalize_segments(
        value.get("segments"),
        &started,
        if resumed.is_null() {
            None
        } else {
            Some(&resumed)
        },
    )?;
    let elapsed = segment_duration(&segments)?;
    if !resumed.is_null() && millis(&resumed)? < millis(&started)? {
        return Err("Resume time cannot precede session start.".into());
    }
    if value
        .get("elapsedMs")
        .is_some_and(|e| e.as_f64() != Some(elapsed as f64))
    {
        return Err("Timer elapsed time does not match its active segments.".into());
    }
    let mut result = metadata(value, state)?;
    result["status"] = json!(status);
    result["startedAt"] = started;
    result["resumedAt"] = resumed;
    result["elapsedMs"] = json!(elapsed);
    result["segments"] = json!(segments);
    Ok(result)
}
pub fn normalize_session(value: &Value, state: &Value) -> Result<Value> {
    object(value, "Session")?;
    let started = iso(&value["startedAt"], "Session start")?;
    let ended = iso(&value["endedAt"], "Session end")?;
    if millis(&ended)? < millis(&started)? {
        return Err("Session end cannot precede its start.".into());
    }
    let segments = normalize_segments(value.get("segments"), &started, Some(&ended))?;
    let elapsed = segment_duration(&segments)?;
    if value
        .get("elapsedMs")
        .is_some_and(|e| e.as_f64() != Some(elapsed as f64))
    {
        return Err("Session elapsed time does not match its active segments.".into());
    }
    let mut result = metadata(value, state)?;
    result["startedAt"] = started;
    result["endedAt"] = ended;
    result["elapsedMs"] = json!(elapsed);
    result["segments"] = json!(segments);
    Ok(result)
}
pub fn close_active_segment(timer: &mut Value, now_ms: i64) -> Result<()> {
    if timer["status"] != "running" {
        return Ok(());
    }
    let start = millis(&timer["resumedAt"])?;
    let end = now_ms.max(start);
    if end > start {
        let segment = json!({"start":timer["resumedAt"],"end":timestamp(end)?});
        timer["segments"]
            .as_array_mut()
            .ok_or("Invalid timer segments.")?
            .push(segment);
    }
    timer["elapsedMs"] = json!(segment_duration(
        timer["segments"]
            .as_array()
            .ok_or("Invalid timer segments.")?
    )?);
    timer["status"] = json!("paused");
    timer["resumedAt"] = Value::Null;
    Ok(())
}
pub fn checkpoint(state: &Value, now_ms: i64) -> Result<Value> {
    timestamp(now_ms)?;
    let mut copy = state.clone();
    if !copy["timer"].is_null() {
        close_active_segment(&mut copy["timer"], now_ms)?;
    }
    Ok(copy)
}
/// Split at the machine's local calendar midnight, preserving every millisecond.
pub fn split_at_midnight(segment: &Value) -> Result<Vec<Value>> {
    let mut cursor = millis(&segment["start"])?;
    let end = millis(&segment["end"])?;
    let mut result = Vec::new();
    while cursor < end {
        let local = Local
            .timestamp_millis_opt(cursor)
            .single()
            .ok_or("The current time is invalid.")?;
        let next_date = local
            .date_naive()
            .succ_opt()
            .ok_or("Timer date is out of range.")?;
        // Rare time zones move clocks at midnight. Find the first valid instant on the next date.
        let mut next = None;
        for minute in 0..=180 {
            let naive = next_date.and_hms_opt(0, 0, 0).unwrap() + chrono::Duration::minutes(minute);
            match Local.from_local_datetime(&naive) {
                chrono::LocalResult::Single(x) => {
                    next = Some(x.timestamp_millis());
                    break;
                }
                chrono::LocalResult::Ambiguous(a, b) => {
                    next = Some(a.timestamp_millis().min(b.timestamp_millis()));
                    break;
                }
                chrono::LocalResult::None => {}
            }
        }
        let boundary = end.min(next.ok_or("Cannot determine local midnight.")?);
        if boundary <= cursor {
            return Err("Cannot determine local midnight.".into());
        }
        result.push(json!({"start":timestamp(cursor)?,"end":timestamp(boundary)?}));
        cursor = boundary;
    }
    Ok(result)
}
pub fn apply_timer_command(
    state: &mut Value,
    kind: &str,
    payload: &Value,
    now_ms: i64,
) -> Result<bool> {
    if !kind.starts_with("timer.") {
        return Ok(false);
    }
    timestamp(now_ms)?;
    if kind == "timer.start" {
        if !state["timer"].is_null() {
            return Err("Stop the current session before starting another.".into());
        }
        let session_kind = choice(
            payload
                .get("kind")
                .filter(|v| !v.is_null())
                .unwrap_or(&json!("focus")),
            &["focus", "meeting", "break"],
            "Session type",
        )?;
        let minutes = match session_kind.as_str() {
            "focus" => 45,
            "meeting" => 30,
            _ => 5,
        };
        let mut timer = json!({"id":uuid::Uuid::new_v4().to_string(),"kind":session_kind,"goal":payload.get("goal").filter(|v|!v.is_null()).cloned().unwrap_or(json!("")),"targetMs":payload.get("targetMs").filter(|v|!v.is_null()).cloned().unwrap_or(json!(minutes*60000)),"status":"running","startedAt":timestamp(now_ms)?,"resumedAt":timestamp(now_ms)?,"elapsedMs":0,"segments":[],"autoCollected":state["settings"]["trackingEnabled"]});
        for (k, v) in links(payload, state)?.as_object().unwrap() {
            timer[k] = v.clone();
        }
        state["timer"] = normalize_timer(&timer, state)?;
        return Ok(true);
    }
    if state["timer"].is_null() {
        return Err("There is no active session.".into());
    }
    let mut timer = state["timer"].clone();
    match kind {
        "timer.pause" => {
            if timer["status"] != "running" {
                return Err("The session is already paused.".into());
            }
            close_active_segment(&mut timer, now_ms)?;
        }
        "timer.resume" => {
            if timer["status"] != "paused" {
                return Err("The session is already running.".into());
            }
            let last = timer["segments"]
                .as_array()
                .unwrap()
                .last()
                .map(|s| &s["end"])
                .unwrap_or(&timer["startedAt"]);
            timer["resumedAt"] = json!(timestamp(now_ms.max(millis(last)?))?);
            timer["status"] = json!("running");
            if state["settings"]["trackingEnabled"] == true {
                timer["autoCollected"] = json!(true)
            }
        }
        "timer.extend" => {
            let minutes = number(&payload["minutes"], "Additional minutes", 0.1, 1440.)?;
            timer["targetMs"] = json!(number(
                &json!(timer["targetMs"].as_f64().unwrap() + minutes * 60000.),
                "Session target",
                60000.,
                86400000.
            )?);
        }
        "timer.update" => {
            for field in [
                "goal",
                "kind",
                "targetMs",
                "clientId",
                "projectId",
                "taskId",
            ] {
                if let Some(v) = payload.get(field) {
                    timer[field] = v.clone();
                }
            }
            timer = normalize_timer(&timer, state)?;
        }
        "timer.stop" => {
            close_active_segment(&mut timer, now_ms)?;
            let last = timer["segments"]
                .as_array()
                .unwrap()
                .last()
                .map(|s| &s["end"])
                .unwrap_or(&timer["startedAt"]);
            let end = now_ms.max(millis(last)?).max(millis(&timer["startedAt"])?);
            let mut session = timer.clone();
            session.as_object_mut().unwrap().remove("status");
            session.as_object_mut().unwrap().remove("resumedAt");
            session["endedAt"] = json!(timestamp(end)?);
            let normalized = normalize_session(&session, state)?;
            state["sessions"].as_array_mut().unwrap().push(normalized);
            if timer["autoCollected"] != true && timer["kind"] != "break" {
                let mut segments = Vec::new();
                for segment in timer["segments"].as_array().unwrap() {
                    segments.extend(split_at_midnight(segment)?);
                }
                for (index, mut entry) in segments.into_iter().enumerate() {
                    let k = timer["kind"].as_str().unwrap();
                    let default_goal = format!("{}{} session", k[..1].to_uppercase(), &k[1..]);
                    entry["id"] = json!(format!(
                        "{}:segment:{}",
                        timer["id"].as_str().unwrap(),
                        index
                    ));
                    entry["description"] = if timer["goal"].as_str().unwrap().is_empty() {
                        json!(default_goal)
                    } else {
                        timer["goal"].clone()
                    };
                    entry["app"] = json!("Rize Local");
                    entry["title"] = timer["goal"].clone();
                    entry["status"] = json!("approved");
                    entry["source"] = json!("timer");
                    entry["category"] = timer["kind"].clone();
                    for key in ["clientId", "projectId", "taskId"] {
                        entry[key] = timer[key].clone();
                    }
                    entry["billable"] = json!(false);
                    let normalized = crate::state::normalize_entry(&entry, state)?;
                    state["entries"].as_array_mut().unwrap().push(normalized);
                }
            }
            state["timer"] = Value::Null;
            return Ok(true);
        }
        _ => return Err(format!("Unknown command: {kind}")),
    }
    state["timer"] = timer;
    Ok(true)
}
