//! Strict iCalendar import with bounded recurrence expansion and explicit time zones.
use crate::{
    providers::{
        calendar_proposals, calendar_range, canonical_zone, clean, failure, integration_id, iso,
        timestamp,
    },
    ApiResult,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
#[derive(Clone, Debug)]
struct Property {
    name: String,
    params: BTreeMap<String, String>,
    value: String,
}
#[derive(Clone, Debug, Default)]
struct Event {
    props: Vec<Property>,
}
impl Event {
    fn get(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name == name)
    }
    fn val(&self, name: &str) -> &str {
        self.get(name).map(|p| p.value.as_str()).unwrap_or("")
    }
}
#[derive(Clone)]
struct CalendarTime {
    ms: i64,
    zone: String,
    all_day: bool,
    local: NaiveDateTime,
}
fn property_separator(line: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        if c == ':' && !quoted {
            return Some(i);
        }
    }
    None
}
fn unescape(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => result.push('\n'),
                Some(c) => result.push(c),
                None => result.push('\\'),
            }
        } else {
            result.push(c)
        }
    }
    result
}
fn parse_time(p: &Property, zone: &str) -> ApiResult<CalendarTime> {
    let date = p.params.get("VALUE").map(String::as_str) == Some("DATE") || p.value.len() == 8;
    let zone = if date {
        "UTC"
    } else if p.value.ends_with('Z') {
        "Etc/UTC"
    } else {
        p.params.get("TZID").map(String::as_str).unwrap_or(zone)
    };
    let zone=canonical_zone(zone.trim_matches('"')).ok_or_else(||failure(400,"INVALID_TIMEZONE","Calendar uses an unrecognized timezone. Convert it to a recognized IANA zone before importing."))?;
    let tz = zone.parse::<Tz>().map_err(|_| {
        failure(
            400,
            "INVALID_TIMEZONE",
            "Calendar uses an unrecognized timezone.",
        )
    })?;
    let value = p.value.trim_end_matches('Z');
    let local = if date {
        NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    } else {
        NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
            .ok()
            .or_else(|| NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M").ok())
    }
    .ok_or_else(|| {
        failure(
            400,
            "INVALID_CALENDAR",
            "Calendar could not be parsed. Check its date values and recurrence rules.",
        )
    })?;
    let date_time = tz.from_local_datetime(&local).earliest().ok_or_else(|| {
        failure(
            400,
            "INVALID_CALENDAR",
            "Calendar contains a nonexistent local time at a timezone transition.",
        )
    })?;
    Ok(CalendarTime {
        ms: date_time.timestamp_millis(),
        zone: zone.into(),
        all_day: date,
        local,
    })
}
fn duration(value: &str) -> Option<i64> {
    if !value.starts_with('P') {
        return None;
    }
    let mut digits = String::new();
    let mut time = false;
    let mut total = 0i64;
    for c in value.chars().skip(1) {
        if c == 'T' {
            time = true;
            continue;
        }
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let n = digits.parse::<i64>().ok()?;
        digits.clear();
        let factor = match c {
            'W' => 7 * 86400000,
            'D' => 86400000,
            'H' if time => 3600000,
            'M' if time => 60000,
            'S' if time => 1000,
            _ => return None,
        };
        total = total.checked_add(n.checked_mul(factor)?)?;
    }
    if digits.is_empty() {
        Some(total)
    } else {
        None
    }
}
fn event_bounds(e: &Event, zone: &str) -> ApiResult<(CalendarTime, i64)> {
    let start = parse_time(
        e.get("DTSTART").ok_or_else(|| {
            failure(
                400,
                "INVALID_CALENDAR",
                "Calendar event is missing a start date.",
            )
        })?,
        zone,
    )?;
    let finish = if let Some(end) = e.get("DTEND") {
        parse_time(end, zone)?.ms
    } else if let Some(span) = duration(e.val("DURATION")) {
        start.ms.saturating_add(span)
    } else {
        start.ms + if start.all_day { 86400000 } else { 0 }
    };
    Ok((start, finish))
}
fn recurrence_bad(value: &str) -> bool {
    value.split(';').any(|part| {
        matches!(part, "FREQ=SECONDLY" | "FREQ=MINUTELY" | "FREQ=HOURLY")
            || ["BYHOUR=", "BYMINUTE=", "BYSECOND="]
                .iter()
                .any(|p| part.starts_with(p) && part.contains(','))
    })
}
fn event_value(e: &Event, start: &CalendarTime, end: i64, uid: &str) -> Value {
    let summary = clean(&json!(unescape(e.val("SUMMARY"))), 500);
    let mut out = json!({"id":integration_id("calendar",&format!("{uid}|{}",start.ms)),"uid":uid,"summary":if summary.is_empty(){"Calendar event".into()}else{summary},"description":clean(&json!(unescape(e.val("DESCRIPTION"))),2000),"start":iso(start.ms),"end":iso(end),"allDay":start.all_day,"timeZone":start.zone});
    if start.all_day {
        out["date"] = json!(start.local.format("%Y-%m-%d").to_string());
    }
    out
}
pub fn parse_calendar(ics: &str, options: &Value, now: i64) -> ApiResult<Value> {
    if ics.len() > 1024 * 1024 || !ics.contains("BEGIN:VCALENDAR") {
        return Err(failure(
            400,
            "INVALID_CALENDAR",
            "Supply an iCalendar file of at most 1 MiB.",
        ));
    }
    let zone = options["timeZone"].as_str().unwrap_or("UTC");
    zone.parse::<Tz>().map_err(|_| {
        failure(
            400,
            "INVALID_TIMEZONE",
            "Supply an IANA timeZone for floating calendar times.",
        )
    })?;
    let (from, to) = calendar_range(options, now)?;
    let (from, to) = (timestamp(&from).unwrap(), timestamp(&to).unwrap());
    let mut lines: Vec<String> = Vec::new();
    for line in ics.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some(previous) = lines.last_mut() {
                previous.push_str(&line[1..]);
            }
        } else {
            lines.push(line.to_string());
        }
    }
    let mut source = Vec::new();
    let mut current: Option<Event> = None;
    let mut nested = 0;
    for line in lines {
        if line == "BEGIN:VEVENT" {
            if current.is_some() {
                return Err(failure(
                    400,
                    "INVALID_CALENDAR",
                    "Calendar event nesting is invalid.",
                ));
            }
            current = Some(Event::default());
            nested = 0;
            continue;
        }
        if line == "END:VEVENT" {
            if let Some(event) = current.take() {
                source.push(event);
            }
            if source.len() > 1000 {
                return Err(failure(
                    400,
                    "CALENDAR_TOO_LARGE",
                    "Calendar contains more than 1,000 events. Export a smaller calendar.",
                ));
            }
            continue;
        }
        if let Some(event) = current.as_mut() {
            if line.starts_with("BEGIN:") {
                nested += 1;
                continue;
            }
            if line.starts_with("END:") {
                nested -= 1;
                continue;
            }
            if nested > 0 {
                continue;
            }
            let Some(separator) = property_separator(&line) else {
                continue;
            };
            let (left, value) = (&line[..separator], &line[separator + 1..]);
            let mut names = left.split(';');
            let name = names.next().unwrap_or("").to_uppercase();
            let mut params = BTreeMap::new();
            for param in names {
                if let Some((k, v)) = param.split_once('=') {
                    params.insert(k.to_uppercase(), v.to_string());
                }
            }
            if name == "RRULE" && recurrence_bad(value) {
                return Err(failure(400,"CALENDAR_RECURRENCE_UNSUPPORTED","Subdaily recurrence is not supported by this import. Export expanded occurrences instead."));
            }
            event.props.push(Property {
                name,
                params,
                value: value.into(),
            });
        }
    }
    if current.is_some() {
        return Err(failure(
            400,
            "INVALID_CALENDAR",
            "Calendar event is incomplete.",
        ));
    }
    let mut bases = Vec::new();
    let mut overrides: BTreeMap<(String, i64), Event> = BTreeMap::new();
    for event in source {
        if let Some(recurrence) = event.get("RECURRENCE-ID") {
            let when = parse_time(recurrence, zone)?.ms;
            overrides.insert((event.val("UID").into(), when), event);
        } else {
            bases.push(event);
        }
    }
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut used = HashSet::new();
    for base in bases {
        if base.val("STATUS") == "CANCELLED" {
            continue;
        }
        let uid = clean(&json!(unescape(base.val("UID"))), 500);
        let (start, finish) = event_bounds(&base, zone)?;
        let span = finish - start.ms;
        if span < 0 {
            warnings.push(format!(
                "Skipped event {} with an invalid end.",
                uid.chars().take(80).collect::<String>()
            ));
            continue;
        }
        let mut occurrences = vec![start.ms];
        let rules = base
            .props
            .iter()
            .filter(|p| p.name == "RRULE")
            .collect::<Vec<_>>();
        if !rules.is_empty() {
            let dt_line = if start.zone == "UTC" {
                format!("DTSTART:{}Z", start.local.format("%Y%m%dT%H%M%S"))
            } else {
                format!(
                    "DTSTART;TZID={}:{}",
                    start.zone,
                    start.local.format("%Y%m%dT%H%M%S")
                )
            };
            let mut rule_text = dt_line;
            for rule in rules {
                rule_text.push_str("\nRRULE:");
                rule_text.push_str(&rule.value);
            }
            match rule_text.parse::<rrule::RRuleSet>() {
                Ok(set) => {
                    let after = DateTime::<Utc>::from_timestamp_millis(
                        from.saturating_sub(span).saturating_sub(1),
                    )
                    .unwrap()
                    .with_timezone(&rrule::Tz::UTC);
                    let before = DateTime::<Utc>::from_timestamp_millis(to.saturating_add(1))
                        .unwrap()
                        .with_timezone(&rrule::Tz::UTC);
                    let expanded = set.after(after).before(before).all(1001);
                    if expanded.limited && expanded.dates.len() >= 1001 {
                        return Err(failure(400,"CALENDAR_TOO_LARGE","Calendar range contains more than 1,000 occurrences. Choose a smaller date range."));
                    }
                    occurrences = expanded
                        .dates
                        .into_iter()
                        .map(|d| d.timestamp_millis())
                        .collect();
                }
                Err(_) => {
                    warnings.push(format!(
                        "Skipped recurrence {} because it could not be expanded.",
                        uid.chars().take(80).collect::<String>()
                    ));
                    continue;
                }
            }
        }
        let mut excluded = HashSet::new();
        for prop in base
            .props
            .iter()
            // v1.2's recurrence expander does not add standalone RDATE values.
            .filter(|p| p.name == "EXDATE")
        {
            for value in prop.value.split(',') {
                let mut part = prop.clone();
                part.value = value.into();
                let when = parse_time(&part, zone)?.ms;
                excluded.insert(when);
            }
        }
        occurrences.sort_unstable();
        occurrences.dedup();
        for when in occurrences {
            if excluded.contains(&when) {
                continue;
            }
            let key = (base.val("UID").to_string(), when);
            let event = if let Some(event) = overrides.get(&key) {
                used.insert(key.clone());
                event
            } else {
                &base
            };
            if event.val("STATUS") == "CANCELLED" {
                continue;
            }
            let (instance, end) = if std::ptr::eq(event, &base) {
                let tz = start.zone.parse::<Tz>().unwrap();
                let local = DateTime::<Utc>::from_timestamp_millis(when)
                    .unwrap()
                    .with_timezone(&tz)
                    .naive_local();
                (
                    CalendarTime {
                        ms: when,
                        zone: start.zone.clone(),
                        all_day: start.all_day,
                        local,
                    },
                    when + span,
                )
            } else {
                event_bounds(event, zone)?
            };
            if end < instance.ms {
                warnings.push(format!(
                    "Skipped event {} with an invalid end.",
                    uid.chars().take(80).collect::<String>()
                ));
                continue;
            }
            if instance.ms > to || end < from {
                continue;
            }
            events.push(event_value(event, &instance, end, &uid));
            if events.len() > 1000 {
                return Err(failure(400,"CALENDAR_TOO_LARGE","Calendar range contains more than 1,000 occurrences. Choose a smaller date range."));
            }
        }
    }
    for (key, event) in overrides {
        if used.contains(&key) || event.val("STATUS") == "CANCELLED" {
            continue;
        }
        let (start, end) = event_bounds(&event, zone)?;
        if start.ms <= to && end >= from && end >= start.ms {
            events.push(event_value(&event, &start, end, &clean(&json!(key.0), 500)));
            if events.len() > 1000 {
                return Err(failure(400,"CALENDAR_TOO_LARGE","Calendar range contains more than 1,000 occurrences. Choose a smaller date range."));
            }
        }
    }
    // DATE values remain date-only. Use UTC instants as a deterministic transport
    // representation; never turn all-day events into tracked-hour proposals.
    for event in &mut events {
        if event["allDay"] == true {
            event["timeZone"] = json!(zone);
        }
    }
    Ok(json!({"proposals":calendar_proposals(&events),"events":events,"warnings":warnings}))
}
