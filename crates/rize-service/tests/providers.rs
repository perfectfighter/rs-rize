use async_trait::async_trait;
use base64::Engine;
use rize_service::{calendar, mcp, providers as p, ApiResult};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
fn env() -> HashMap<String, String> {
    HashMap::from([
        (
            "RIZE_SECRET_KEY".into(),
            base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        ),
        (
            "RIZE_AI_BASE_URL".into(),
            "https://ai.example.test/v1".into(),
        ),
        ("RIZE_AI_API_KEY".into(), "not-a-live-key".into()),
        ("RIZE_AI_MODEL".into(), "test-model".into()),
        ("RIZE_EMBEDDING_MODEL".into(), "test-embedding".into()),
    ])
}
struct Fake {
    reply: Value,
    calls: Arc<Mutex<Vec<p::Request>>>,
}
#[async_trait]
impl p::Transport for Fake {
    async fn send(&self, r: p::Request) -> ApiResult<Value> {
        self.calls.lock().unwrap().push(r);
        Ok(self.reply.clone())
    }
}
fn fake(reply: Value) -> Fake {
    Fake {
        reply,
        calls: Arc::new(Mutex::new(Vec::new())),
    }
}
#[test]
fn credential_vault_is_scope_bound_and_compatible_shape() {
    let env = env();
    let sealed = p::seal(&json!({"token":"sensitive-token"}), &env, "team1:clickup").unwrap();
    assert!(!sealed.to_string().contains("sensitive-token"));
    assert_eq!(
        p::unseal(&sealed, &env, "team1:clickup").unwrap(),
        json!({"token":"sensitive-token"})
    );
    assert_eq!(
        p::unseal(&sealed, &env, "team2:clickup").unwrap_err().code,
        "SECRET_DECRYPT_FAILED"
    );
    assert_eq!(
        p::seal(&json!({}), &HashMap::new(), "scope")
            .unwrap_err()
            .code,
        "SECRET_KEY_REQUIRED"
    );
    assert_eq!(sealed["v"], 1);
}
#[tokio::test]
async fn https_and_origin_validation_happens_before_injected_transport() {
    let f = fake(json!({"ok":true}));
    assert!(p::request(
        &f,
        p::Request::new("http://api.clickup.com/x", "http://api.clickup.com")
    )
    .await
    .is_err());
    assert!(p::request(
        &f,
        p::Request::new("https://other.test/", "https://api.clickup.com")
    )
    .await
    .is_err());
    assert!(p::request(
        &f,
        p::Request::new("https://api.clickup.com/x", "https://api.clickup.com")
    )
    .await
    .is_ok());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn ai_no_configuration_or_invalid_embeddings_never_fakes_output() {
    let f = fake(json!({}));
    assert_eq!(
        p::chat(&HashMap::new(), &f, json!([]))
            .await
            .unwrap_err()
            .code,
        "AI_NOT_CONFIGURED"
    );
    assert!(f.calls.lock().unwrap().is_empty());
    let f = fake(json!({"data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[0,1]}]}));
    assert_eq!(
        p::embeddings(&env(), &f, &["A".into(), "B".into()])
            .await
            .unwrap_err()
            .code,
        "AI_RESPONSE"
    );
}
#[tokio::test]
async fn ai_chat_and_embeddings_use_only_explicit_payloads() {
    let f = fake(json!({"choices":[{"message":{"content":"Evidence-based answer"}}]}));
    assert_eq!(
        p::chat(
            &env(),
            &f,
            json!([{"role":"user","content":"Reviewed work"}])
        )
        .await
        .unwrap()["text"],
        "Evidence-based answer"
    );
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls[0].url, "https://ai.example.test/v1/chat/completions");
    assert_eq!(calls[0].body.as_ref().unwrap()["stream"], false);
    assert_eq!(calls[0].body.as_ref().unwrap()["model"], "test-model");
}
#[tokio::test]
async fn clickup_import_and_asana_whole_minutes_preserve_contract() {
    let f = fake(
        json!({"tasks":[{"id":"t1","name":"Work","status":{"type":"open"}}],"last_page":true}),
    );
    let c = p::provider_config(
        "clickup",
        &json!({"token":"fake-token","listId":"123","workspaceId":"456"}),
    )
    .unwrap();
    assert_eq!(
        p::provider_tasks("clickup", &c, &f).await.unwrap()["tasks"][0]["externalId"],
        "t1"
    );
    assert!(p::provider_config(
        "clickup",
        &json!({"token":"fake-token","listId":"../invalid"})
    )
    .is_err());
    let entry = json!({"status":"approved","start":"2026-03-01T00:00:00Z","end":"2026-03-01T00:00:30Z","durationMs":30000});
    assert_eq!(
        p::push_time(
            "asana",
            &json!({"token":"fake","options":{}}),
            &entry,
            "task1",
            &f
        )
        .await
        .unwrap_err()
        .code,
        "ASANA_WHOLE_MINUTES"
    );
    assert_eq!(
        p::push_time("linear", &json!({}), &entry, "task1", &f)
            .await
            .unwrap_err()
            .code,
        "TIME_EXPORT_UNSUPPORTED"
    );
    let f = fake(json!({"data":{"gid":"remote"}}));
    let entry = json!({"status":"approved","start":"2026-03-01T00:00:00Z","end":"2026-03-01T00:02:00Z","durationMs":120000});
    p::push_time(
        "asana",
        &json!({"token":"fake","options":{}}),
        &entry,
        "task1",
        &f,
    )
    .await
    .unwrap();
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls[0].body.as_ref().unwrap()["data"]["duration_minutes"],
        2.
    );
}
fn ics(events: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}\r\nEND:VCALENDAR")
}
fn range() -> Value {
    json!({"from":"2026-03-01","to":"2026-04-01"})
}
#[test]
fn calendar_retains_dst_all_day_and_no_duration_fabrication() {
    let data=ics("BEGIN:VEVENT\r\nUID:daily\r\nDTSTART;TZID=America/New_York:20260307T090000\r\nDTEND;TZID=America/New_York:20260307T100000\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:allday\r\nDTSTART;VALUE=DATE:20260308\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:instant\r\nDTSTART:20260308T160000Z\r\nSUMMARY:Reminder\r\nEND:VEVENT");
    let result = calendar::parse_calendar(&data, &range(), 0).unwrap();
    let daily = result["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["uid"] == "daily")
        .collect::<Vec<_>>();
    assert_eq!(daily.len(), 3);
    assert_eq!(daily[0]["start"], "2026-03-07T14:00:00.000Z");
    assert_eq!(daily[1]["start"], "2026-03-08T13:00:00.000Z");
    assert_eq!(result["proposals"].as_array().unwrap().len(), 3);
    let instant = result["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["uid"] == "instant")
        .unwrap();
    assert_eq!(instant["start"], instant["end"]);
}
#[test]
fn calendar_floating_exception_overrides_and_unrecognized_zones() {
    let data=ics("BEGIN:VEVENT\nUID:series\nDTSTART;TZID=America/New_York:20260307T090000\nDTEND;TZID=America/New_York:20260307T100000\nRRULE:FREQ=DAILY;COUNT=3\nEXDATE;TZID=America/New_York:20260308T090000\nSUMMARY:Base\nEND:VEVENT\nBEGIN:VEVENT\nUID:series\nRECURRENCE-ID;TZID=America/New_York:20260309T090000\nDTSTART;TZID=America/New_York:20260309T110000\nDTEND;TZID=America/New_York:20260309T120000\nSUMMARY:Moved\nEND:VEVENT");
    let result = calendar::parse_calendar(&data, &range(), 0).unwrap();
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
    assert_eq!(result["events"][1]["summary"], "Moved");
    assert_eq!(result["events"][1]["start"], "2026-03-09T15:00:00.000Z");
    let floating = ics(
        "BEGIN:VEVENT\nUID:floating\nDTSTART:20260303T090000\nDTEND:20260303T100000\nEND:VEVENT",
    );
    let mut opts = range();
    opts["timeZone"] = json!("Asia/Kolkata");
    assert_eq!(
        calendar::parse_calendar(&floating, &opts, 0).unwrap()["events"][0]["start"],
        "2026-03-03T03:30:00.000Z"
    );
    let bad = ics("BEGIN:VEVENT\nUID:x\nDTSTART;TZID=UnknownZone:20260303T090000\nEND:VEVENT");
    assert_eq!(
        calendar::parse_calendar(&bad, &range(), 0)
            .unwrap_err()
            .code,
        "INVALID_TIMEZONE"
    );
    let bad = ics("BEGIN:VEVENT\nUID:x\nDTSTART:20260303T090000Z\nRRULE:FREQ=SECONDLY\nEND:VEVENT");
    assert_eq!(
        calendar::parse_calendar(&bad, &range(), 0)
            .unwrap_err()
            .code,
        "CALENDAR_RECURRENCE_UNSUPPORTED"
    );
}
struct McpFake;
#[async_trait]
impl mcp::Backend for McpFake {
    async fn get(&self, _: &mcp::Configuration, path: &str) -> ApiResult<Value> {
        assert_eq!(path, "sync?since=0");
        Ok(
            json!({"records":[{"kind":"entries","id":"approved","data":{"status":"approved","description":"Reviewed","app":"private-app","title":"private-title","start":"2026-03-01T00:00:00Z"}},{"kind":"entries","id":"pending","data":{"status":"pending","description":"private-pending"}}]}),
        )
    }
}
#[tokio::test]
async fn mcp_is_read_only_initialized_and_redacts_metadata() {
    let env = HashMap::from([
        ("RIZE_SESSION_TOKEN".into(), "secret-session".into()),
        ("RIZE_TEAM_ID".into(), "team1".into()),
    ]);
    let mut m = mcp::Mcp::new(&env, McpFake).unwrap();
    assert_eq!(
        m.handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .await
            .unwrap()["error"]["code"],
        -32000
    );
    assert_eq!(m.handle(&json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":mcp::VERSION}})).await.unwrap()["result"]["protocolVersion"],mcp::VERSION);
    let out=m.handle(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"approved_time_entries","arguments":{}}})).await.unwrap();
    assert!(out.to_string().contains("Reviewed"));
    assert!(!out.to_string().contains("private-"));
    assert_eq!(m.handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"delete_everything"}})).await.unwrap()["error"]["code"],-32602);
}
#[test]
fn rust_decrypts_node_v12_aes_gcm_fixture() {
    let envelope = json!({"v":1,"iv":"AAECAwQFBgcICQoL","tag":"UFtcXZzn2RkXTMUFcb6wlQ==","ciphertext":"Y6OdH3Zst2tNkIP1miwZiIQR2eB0G1pfebpO1uehCCej"});
    assert_eq!(
        p::unseal(&envelope, &env(), "team1:clickup").unwrap(),
        json!({"token":"fixture-only-not-live"})
    );
}
#[tokio::test]
async fn microsoft_calendar_keeps_requested_utc_and_original_all_day_zone() {
    let f = fake(
        json!({"value":[{"id":"ms-event","subject":"Meeting","start":{"dateTime":"2026-03-09T13:00:00.0000000","timeZone":"UTC"},"end":{"dateTime":"2026-03-09T14:00:00.0000000","timeZone":"UTC"},"isAllDay":false},{"id":"ms-holiday","subject":"Holiday","start":{"dateTime":"2026-03-07T18:30:00","timeZone":"UTC"},"end":{"dateTime":"2026-03-08T18:30:00","timeZone":"UTC"},"isAllDay":true,"originalStartTimeZone":"India Standard Time"}]}),
    );
    let out = p::calendar_events("microsoft", "mock-token", &range(), &f, 0)
        .await
        .unwrap();
    assert_eq!(out["events"][1]["date"], "2026-03-08");
    assert_eq!(out["events"][1]["timeZone"], "Asia/Calcutta");
    assert_eq!(out["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.calls.lock().unwrap()[0].headers.get("Prefer").unwrap(),
        "outlook.timezone=\"UTC\""
    );
}
#[test]
fn calendar_matches_published_node_v12_golden_contract() {
    // Generated against 98f56dba6138280b64a7ea57c02b91b73a268b8a with TZ=UTC,
    // node-ical 0.27.3. No Node runtime is required to run these Rust tests.
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/calendar_v12.json")).unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let actual =
            calendar::parse_calendar(fixture["ics"].as_str().unwrap(), &fixture["options"], 0)
                .unwrap();
        assert_eq!(
            actual, fixture["expected"],
            "calendar fixture {}",
            fixture["name"]
        );
    }
}
