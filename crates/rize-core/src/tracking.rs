//! Local foreground interval accounting. Only consecutive real samples may add time.
use chrono::{
    DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike,
    Utc,
};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub const DEFAULT_MAX_GAP_MS: i64 = 15_000;
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn parse_timestamp(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Utc.timestamp_millis_opt(n).single().map(|_| n);
    }
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|d| d.timestamp_millis())
}
fn iso(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .expect("validated timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn private_title(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?-u:\b)(?:incognito|inprivate|private\s+(?:browsing|window|mode))(?-u:\b)",
        )
        .unwrap()
    })
    .is_match(s)
}
/// Independently enforce the sampler's privacy boundaries when accepting imported samples.
pub fn clean_text(s: &str, limit: usize) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    let cleaned: String = s
        .chars()
        .filter(|c| !('\u{0}'..='\u{1f}').contains(c) && !('\u{7f}'..='\u{9f}').contains(c))
        .collect();
    let cleaned = URL
        .get_or_init(|| {
            Regex::new(r"(?i)(?-u:\b)(?:https?|ftp|file)://\S+|(?-u:\b)www\.[^\s]+").unwrap()
        })
        .replace_all(&cleaned, "[address]");
    // Match the baseline's UTF-16 size bound without ever emitting broken Unicode.
    let mut units = 0;
    cleaned
        .trim()
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= limit
        })
        .collect()
}
fn parse_time(v: &Value) -> Option<u32> {
    let s = v.as_str()?;
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' || ![b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let h = (b[0] - b'0') as u32 * 10 + (b[1] - b'0') as u32;
    let m = (b[3] - b'0') as u32 * 10 + (b[4] - b'0') as u32;
    (h < 24 && m < 60).then_some(h * 60 + m)
}
fn day_allowed(date: NaiveDate, settings: &Value) -> bool {
    let day = date.weekday().num_days_from_sunday() as u64;
    settings["workDays"]
        .as_array()
        .map(|a| a.iter().any(|v| v.as_u64() == Some(day)))
        .unwrap_or((1..=5).contains(&day))
}
pub fn is_scheduled(ms: i64, settings: &Value) -> bool {
    if settings["scheduleEnabled"] != true {
        return true;
    }
    let (Some(start), Some(end)) = (
        parse_time(&settings["scheduleStart"]),
        parse_time(&settings["scheduleEnd"]),
    ) else {
        return false;
    };
    let Some(date) = Local.timestamp_millis_opt(ms).single() else {
        return false;
    };
    let minute = date.hour() * 60 + date.minute();
    let day = date.date_naive();
    if start <= end {
        day_allowed(day, settings) && (start == end || (minute >= start && minute < end))
    } else if minute >= start {
        day_allowed(day, settings)
    } else {
        minute < end && day.pred_opt().is_some_and(|d| day_allowed(d, settings))
    }
}
fn local_millis(naive: NaiveDateTime) -> Option<i64> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(v) => Some(v.timestamp_millis()),
        LocalResult::Ambiguous(a, b) => Some(a.timestamp_millis().min(b.timestamp_millis())),
        LocalResult::None => {
            // JavaScript Date chooses the later clock time in a DST gap.
            let before = (1..=1440).find_map(|m| {
                Local
                    .from_local_datetime(&(naive - Duration::minutes(m)))
                    .earliest()
            });
            let after = (1..=1440).find_map(|m| {
                Local
                    .from_local_datetime(&(naive + Duration::minutes(m)))
                    .earliest()
            });
            let shift = after?.offset().local_minus_utc() - before?.offset().local_minus_utc();
            Local
                .from_local_datetime(&(naive + Duration::seconds(shift.into())))
                .earliest()
                .map(|v| v.timestamp_millis())
        }
    }
}
fn boundaries(start: i64, end: i64, settings: &Value) -> Vec<i64> {
    let mut result = vec![start, end];
    let Some(mut day) = Local
        .timestamp_millis_opt(start)
        .single()
        .map(|d| d.date_naive())
    else {
        return result;
    };
    while let Some(midnight) = day.and_hms_opt(0, 0, 0).and_then(local_millis) {
        if midnight > end {
            break;
        }
        if midnight > start && midnight < end {
            result.push(midnight);
        }
        if settings["scheduleEnabled"] == true {
            for field in ["scheduleStart", "scheduleEnd"] {
                if let Some(minutes) = parse_time(&settings[field]) {
                    if let Some(point) = day
                        .and_hms_opt(minutes / 60, minutes % 60, 0)
                        .and_then(local_millis)
                    {
                        if point > start && point < end {
                            result.push(point);
                        }
                    }
                }
            }
        }
        let Some(next) = day.succ_opt() else {
            break;
        };
        day = next;
    }
    result.sort_unstable();
    result.dedup();
    result
}
fn keyword_match(value: &Value, haystack: &str) -> bool {
    let words: Vec<&str> = if let Some(a) = value.as_array() {
        a.iter().filter_map(Value::as_str).collect()
    } else {
        text(value).split([',', '\n']).collect()
    };
    words.iter().any(|word| {
        let clean = clean_text(word, 256).to_lowercase();
        !clean.is_empty() && haystack.contains(&clean)
    })
}
fn links(entity: &Value, state: &Value) -> Value {
    let task = array(&state["tasks"])
        .iter()
        .find(|v| !text(&entity["taskId"]).is_empty() && v["id"] == entity["taskId"]);
    let project_id = task
        .map(|v| text(&v["projectId"]))
        .filter(|s| !s.is_empty())
        .unwrap_or(text(&entity["projectId"]));
    let project = array(&state["projects"])
        .iter()
        .find(|v| !project_id.is_empty() && text(&v["id"]) == project_id);
    let client_id = task
        .map(|v| text(&v["clientId"]))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            project
                .map(|v| text(&v["clientId"]))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(text(&entity["clientId"]));
    let client = array(&state["clients"])
        .iter()
        .find(|v| !client_id.is_empty() && text(&v["id"]) == client_id);
    json!({"clientId":client.map(|v| text(&v["id"])).unwrap_or(""), "projectId":project.map(|v| text(&v["id"])).unwrap_or(""), "taskId":task.map(|v| text(&v["id"])).unwrap_or("")})
}
fn classify(sample: &Sample, state: &Value) -> Option<Value> {
    let haystack = format!("{} {}", sample.app, sample.title).to_lowercase();
    let rules: Vec<&Value> = array(&state["rules"])
        .iter()
        .filter(|v| keyword_match(&json!([v["keyword"]]), &haystack))
        .collect();
    if rules.iter().any(|v| v["action"] == "exclude") {
        return None;
    }
    let rule = rules.first().copied();
    let category = if let Some(rule) = rule {
        match text(&rule["action"]) {
            "productive" => "productive",
            "distracting" => "distracting",
            _ => "neutral",
        }
    } else {
        static DISTRACTING: OnceLock<Regex> = OnceLock::new();
        static PRODUCTIVE: OnceLock<Regex> = OnceLock::new();
        if DISTRACTING.get_or_init(|| Regex::new(r"(?-u:\b)(?:netflix|tiktok|reddit|twitter|youtube|instagram)(?-u:\b)").unwrap()).is_match(&haystack) { "distracting" }
        else if PRODUCTIVE.get_or_init(|| Regex::new(r"(?-u:\b)(?:code|visual studio|webstorm|github|figma|notion|excel|word|zoom|teams|meet)(?-u:\b)").unwrap()).is_match(&haystack) { "productive" } else { "neutral" }
    };
    let entity = if let Some(rule) = rule {
        rule.clone()
    } else {
        let task = array(&state["tasks"])
            .iter()
            .find(|v| keyword_match(&v["keywords"], &haystack));
        let project = array(&state["projects"])
            .iter()
            .find(|v| keyword_match(&v["keywords"], &haystack));
        json!({"taskId":task.map(|v| text(&v["id"])).unwrap_or(""),"projectId":project.map(|v| text(&v["id"])).unwrap_or("")})
    };
    let mut result = links(&entity, state);
    result["category"] = json!(category);
    result["billable"] = json!(false);
    Some(result)
}
#[derive(Clone)]
struct Sample {
    ms: i64,
    app: String,
    title: String,
    available: bool,
    idle_seconds: f64,
}
impl Sample {
    fn from_value(v: &Value) -> Option<Self> {
        let mut title = clean_text(text(&v["title"]), 1024);
        if private_title(&title) {
            title = "[Private window]".into();
        }
        Some(Self {
            ms: parse_timestamp(&v["timestamp"])?,
            app: clean_text(text(&v["app"]), 256),
            title,
            available: v["available"] == true,
            idle_seconds: v["idleSeconds"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.)
                .unwrap_or(f64::INFINITY),
        })
    }
    fn usable(&self, state: &Value) -> bool {
        let cutoff = state["settings"]["idleSeconds"]
            .as_f64()
            .or_else(|| text(&state["settings"]["idleSeconds"]).parse().ok())
            .filter(|n| n.is_finite() && *n > 0.)
            .unwrap_or(300.);
        self.available && !self.app.is_empty() && self.idle_seconds < cutoff
    }
}
struct Active {
    entry: Value,
    signature: String,
    local_day: NaiveDate,
    end: i64,
}
pub struct TrackingEngine {
    previous: Option<Sample>,
    active: Option<Active>,
    max_gap_ms: i64,
}
impl Default for TrackingEngine {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_GAP_MS)
    }
}
impl TrackingEngine {
    pub fn new(max_gap_ms: i64) -> Self {
        Self {
            previous: None,
            active: None,
            max_gap_ms: if max_gap_ms > 0 {
                max_gap_ms
            } else {
                DEFAULT_MAX_GAP_MS
            },
        }
    }
    pub fn reset(&mut self) {
        self.previous = None;
        self.active = None;
    }
    pub fn consume(&mut self, sample: &Value, state: &Value) -> Vec<Value> {
        if state["onboarded"] != true
            || state["demo"] == true
            || state["settings"]["trackingEnabled"] != true
            || state["settings"]["trackingPaused"] == true
            || state["trackingPaused"] == true
        {
            self.reset();
            return vec![];
        }
        let Some(current) = Sample::from_value(sample) else {
            self.reset();
            return vec![];
        };
        if self
            .previous
            .as_ref()
            .is_some_and(|prior| current.ms <= prior.ms)
        {
            return vec![];
        }
        let prior = self.previous.replace(current.clone());
        if !current.usable(state) || classify(&current, state).is_none() {
            self.reset();
            return vec![];
        }
        let Some(prior) = prior.filter(|p| p.usable(state)) else {
            return vec![];
        };
        if current.ms - prior.ms > self.max_gap_ms {
            self.active = None;
            return vec![];
        }
        self.append(&prior, prior.ms, current.ms, state)
    }
    fn append(&mut self, sample: &Sample, start: i64, end: i64, state: &Value) -> Vec<Value> {
        let Some(metadata) = classify(sample, state) else {
            self.active = None;
            return vec![];
        };
        let signature = json!([sample.app, sample.title, metadata]).to_string();
        let mut result = vec![];
        for pair in boundaries(start, end, &state["settings"]).windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b <= a || !is_scheduled(a + (b - a) / 2, &state["settings"]) {
                self.active = None;
                continue;
            }
            let day = Local.timestamp_millis_opt(a).single().unwrap().date_naive();
            if !self.active.as_ref().is_some_and(|active| {
                active.signature == signature && active.end == a && active.local_day == day
            }) {
                let mut entry = json!({"id":format!("activity-{}",uuid::Uuid::new_v4()),"start":iso(a),"end":iso(b),"durationMs":b-a,
                    "description":if sample.title.is_empty() {&sample.app} else {&sample.title},"app":sample.app,"title":sample.title,"status":"pending","source":"automatic"});
                entry
                    .as_object_mut()
                    .unwrap()
                    .extend(metadata.as_object().unwrap().clone());
                self.active = Some(Active {
                    entry,
                    signature: signature.clone(),
                    local_day: day,
                    end: b,
                });
            } else {
                let active = self.active.as_mut().unwrap();
                active.end = b;
                active.entry["end"] = json!(iso(b));
                active.entry["durationMs"] =
                    json!(b - parse_timestamp(&active.entry["start"]).unwrap());
            }
            result.push(self.active.as_ref().unwrap().entry.clone());
        }
        result
    }
    /// A flush returns the known interval only; it never infers a tail to quit/lock time.
    pub fn flush(&mut self, timestamp: &str, _state: &Value) -> Vec<Value> {
        let entry = self.active.take().map(|a| a.entry);
        self.reset();
        if parse_timestamp(&json!(timestamp)).is_none() {
            return vec![];
        }
        entry.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Value {
        json!({"onboarded":true,"demo":false,"settings":{"trackingEnabled":true,"idleSeconds":300},"rules":[],"clients":[],"projects":[],"tasks":[]})
    }
    fn sample(ms: i64) -> Value {
        json!({"timestamp":iso(ms),"app":"Code","title":"Project","idleSeconds":0,"available":true})
    }
    fn time(y: i32, m: u32, d: u32, h: u32, n: u32, s: u32, ms: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, h, n, s)
            .single()
            .unwrap()
            .timestamp_millis()
            + ms as i64
    }
    #[test]
    fn explicit_opt_in_and_pause_reset() {
        for field in ["onboarded", "demo", "trackingEnabled", "trackingPaused"] {
            let mut s = state();
            match field {
                "onboarded" => s[field] = json!(false),
                "demo" => s[field] = json!(true),
                "trackingEnabled" => s["settings"][field] = json!(false),
                _ => s[field] = json!(true),
            };
            let mut e = TrackingEngine::default();
            assert!(e.consume(&sample(0), &s).is_empty());
            assert!(e.consume(&sample(5000), &s).is_empty());
        }
        let mut e = TrackingEngine::default();
        let mut s = state();
        e.consume(&sample(0), &s);
        s["timer"] = json!({"status":"paused"});
        assert_eq!(e.consume(&sample(5000), &s)[0]["durationMs"], 5000);
    }
    #[test]
    fn precise_intervals_stable_upsert_and_switches() {
        let mut e = TrackingEngine::default();
        let s = state();
        assert!(e.consume(&sample(123), &s).is_empty());
        let a = e.consume(&sample(5123), &s)[0].clone();
        let b = e.consume(&sample(10128), &s)[0].clone();
        assert_eq!(a["start"], "1970-01-01T00:00:00.123Z");
        assert_eq!(a["durationMs"], 5000);
        assert_eq!(a["id"], b["id"]);
        assert_eq!(b["durationMs"], 10005);
        assert!(b.get("signature").is_none());
        let mut next = sample(15128);
        next["app"] = json!("Word");
        next["title"] = json!("Document");
        assert_eq!(e.consume(&next, &s)[0]["app"], "Code");
        next["timestamp"] = json!(iso(20128));
        let c = e.consume(&next, &s)[0].clone();
        assert_eq!(c["app"], "Word");
        assert_ne!(c["id"], b["id"]);
    }
    #[test]
    fn unavailable_idle_sleep_and_reversed_samples_never_fabricate_time() {
        for patch in [
            json!({"idleSeconds":300}),
            json!({"idleSeconds":null}),
            json!({"available":false}),
            json!({"app":""}),
        ] {
            let mut e = TrackingEngine::default();
            let s = state();
            e.consume(&sample(0), &s);
            let mut bad = sample(5000);
            bad.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(e.consume(&bad, &s).is_empty());
            assert!(e.consume(&sample(10000), &s).is_empty());
            assert_eq!(e.consume(&sample(15000), &s)[0]["durationMs"], 5000);
        }
        let mut e = TrackingEngine::default();
        let s = state();
        e.consume(&sample(0), &s);
        assert!(e.consume(&sample(60000), &s).is_empty());
        assert_eq!(e.consume(&sample(65000), &s)[0]["durationMs"], 5000);
        assert!(e.consume(&sample(65000), &s).is_empty());
        assert!(e.consume(&sample(64000), &s).is_empty());
        assert_eq!(e.consume(&sample(70000), &s)[0]["durationMs"], 10000);
    }
    #[test]
    fn flush_has_no_quit_or_lock_tail() {
        let mut e = TrackingEngine::default();
        let s = state();
        e.consume(&sample(0), &s);
        let a = e.consume(&sample(5000), &s);
        assert_eq!(e.flush(&iso(90000), &s), a);
        assert!(e.flush(&iso(90000), &s).is_empty());
        assert!(e.consume(&sample(100000), &s).is_empty());
    }
    #[test]
    fn privacy_exclusion_and_ancestor_hierarchy() {
        let mut s = state();
        s["rules"] = json!([{"keyword":"Project","action":"productive"},{"keyword":"Project","action":"exclude"}]);
        let mut e = TrackingEngine::default();
        e.consume(&sample(0), &s);
        assert!(e.consume(&sample(5000), &s).is_empty());
        assert!(e.flush(&iso(10000), &s).is_empty());
        s["rules"] = json!([]);
        s["clients"] = json!([{"id":"c"}]);
        s["projects"] = json!([{"id":"p","clientId":"c","keywords":"Launch,Acme"}]);
        s["tasks"] = json!([{"id":"t","projectId":"p","keywords":["Launch"]}]);
        let mut a = sample(0);
        a["title"] = json!("Launch");
        e.consume(&a, &s);
        a["timestamp"] = json!(iso(5000));
        let row = e.consume(&a, &s)[0].clone();
        assert_eq!(
            json!([
                row["clientId"],
                row["projectId"],
                row["taskId"],
                row["category"]
            ]),
            json!(["c", "p", "t", "productive"])
        );
        s["rules"] = json!([{"keyword":"Launch","action":"distracting","projectId":"p"}]);
        e.reset();
        a["timestamp"] = json!(iso(0));
        e.consume(&a, &s);
        a["timestamp"] = json!(iso(5000));
        let row = e.consume(&a, &s)[0].clone();
        assert_eq!(row["taskId"], "");
        assert_eq!(row["category"], "distracting");
        s["rules"] = json!([{"keyword":"Launch","action":"neutral","clientId":"missing","projectId":"missing","taskId":"missing"}]);
        e.reset();
        a["timestamp"] = json!(iso(0));
        e.consume(&a, &s);
        a["timestamp"] = json!(iso(5000));
        let row = e.consume(&a, &s)[0].clone();
        assert_eq!(
            json!([row["clientId"], row["projectId"], row["taskId"]]),
            json!(["", "", ""])
        );
    }
    #[test]
    fn midnight_and_schedule_clip_exact_bounds() {
        let mut e = TrackingEngine::default();
        let mut s = state();
        let start = time(2026, 10, 19, 23, 59, 58, 123);
        let rows = {
            e.consume(&sample(start), &s);
            e.consume(&sample(start + 5000), &s)
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["durationMs"], 1877);
        assert_eq!(rows[1]["durationMs"], 3123);
        assert_ne!(rows[0]["id"], rows[1]["id"]);
        s["settings"] = json!({"trackingEnabled":true,"scheduleEnabled":true,"scheduleStart":"09:00","scheduleEnd":"17:00","workDays":[1]});
        e.reset();
        let start = time(2026, 10, 19, 8, 59, 58, 123);
        e.consume(&sample(start), &s);
        let rows = e.consume(&sample(start + 5000), &s);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["start"], iso(time(2026, 10, 19, 9, 0, 0, 0)));
        assert_eq!(rows[0]["durationMs"], 3123);
        e.reset();
        let sunday = time(2026, 10, 18, 10, 0, 0, 0);
        e.consume(&sample(sunday), &s);
        assert!(e.consume(&sample(sunday + 5000), &s).is_empty());
        s["settings"]["scheduleStart"] = json!("22:00");
        s["settings"]["scheduleEnd"] = json!("06:00");
        e.reset();
        let start = time(2026, 10, 20, 5, 59, 58, 0);
        e.consume(&sample(start), &s);
        assert_eq!(e.consume(&sample(start + 5000), &s)[0]["durationMs"], 2000);
    }
    #[test]
    fn sanitization_is_independent_and_bounded() {
        assert_eq!(
            clean_text(
                "Visit https://secret.test/account and www.private.test today",
                1024
            ),
            "Visit [address] and [address] today"
        );
        assert_eq!(clean_text("Ed\0itor\u{85}", 256), "Editor");
        assert_eq!(clean_text("😀😀", 3), "😀");
        let mut e = TrackingEngine::default();
        let s = state();
        let mut a = sample(0);
        a["app"] = json!("Br\0owser");
        a["title"] = json!("Secret - InPrivate");
        e.consume(&a, &s);
        a["timestamp"] = json!(iso(5000));
        let row = e.consume(&a, &s)[0].clone();
        assert_eq!(row["title"], "[Private window]");
        assert_eq!(row["description"], "[Private window]");
        assert_eq!(row["app"], "Browser");
    }
}

#[cfg(test)]
mod baseline_parity {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn golden_intervals_match_the_published_v1_2_engine() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/tracking-v1.2.json")).unwrap();
        for scenario in fixtures["scenarios"].as_array().unwrap() {
            let mut engine = TrackingEngine::default();
            let mut ids = HashMap::<String, String>::new();
            for step in scenario["steps"].as_array().unwrap() {
                let mut actual = if step.get("sample").is_some() {
                    engine.consume(&step["sample"], &scenario["state"])
                } else {
                    engine.flush(step["flush"].as_str().unwrap(), &scenario["state"])
                };
                for entry in &mut actual {
                    let id = entry["id"].as_str().unwrap().to_owned();
                    let next = format!("activity-{}", ids.len() + 1);
                    let normalized = ids.entry(id).or_insert(next).clone();
                    entry["id"] = json!(normalized);
                }
                assert_eq!(
                    json!(actual),
                    step["expected"],
                    "scenario {} sample {}",
                    scenario["name"],
                    step.get("sample").unwrap_or(&step["flush"])
                );
            }
        }
    }
}
