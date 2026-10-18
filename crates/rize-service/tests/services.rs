use async_trait::async_trait;
use base64::Engine;
use rize_service::{providers as p, services as s, ApiResult, AppState, Context};
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
const NOW: i64 = 1772323200000;
#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Vec<p::Request>>>,
    handler: Arc<dyn Fn(&p::Request) -> ApiResult<Value> + Send + Sync>,
}
#[async_trait]
impl p::Transport for Mock {
    async fn send(&self, r: p::Request) -> ApiResult<Value> {
        self.calls.lock().unwrap().push(r.clone());
        (self.handler)(&r)
    }
}
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
        ("RIZE_RUN_ROUTINES".into(), "1".into()),
        (
            "RIZE_PUBLIC_ORIGIN".into(),
            "https://rize-server.example.test".into(),
        ),
        ("RIZE_GOOGLE_CLIENT_ID".into(), "google-client".into()),
        (
            "RIZE_GOOGLE_CLIENT_SECRET".into(),
            "not-live-google-secret".into(),
        ),
        ("RIZE_MICROSOFT_CLIENT_ID".into(), "ms-client".into()),
        (
            "RIZE_MICROSOFT_CLIENT_SECRET".into(),
            "not-live-ms-secret".into(),
        ),
    ])
}
fn fixture(
    environment: HashMap<String, String>,
    handler: impl Fn(&p::Request) -> ApiResult<Value> + Send + Sync + 'static,
) -> (Context, Mock) {
    let mock = Mock {
        calls: Arc::new(Mutex::new(Vec::new())),
        handler: Arc::new(handler),
    };
    let mut state = AppState::new(":memory:", environment)
        .unwrap()
        .with_clock(Arc::new(|| NOW));
    state.transport = Arc::new(mock.clone());
    let team = uuid::Uuid::new_v4().to_string();
    {
        let db = state.db().unwrap();
        db.execute(
            "INSERT INTO users VALUES('u1','one@example.test','Test','unused',0)",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO teams VALUES(?,'Team',0)", [&team])
            .unwrap();
        db.execute("INSERT INTO members VALUES(?,'u1','owner',1,1,0)", [&team])
            .unwrap();
    }
    let ctx = state.context_for(&team, "u1").unwrap();
    (ctx, mock)
}
fn add(ctx: &Context, kind: &str, id: &str, data: Value) {
    ctx.state
        .db()
        .unwrap()
        .execute(
            "INSERT INTO records VALUES(?,?,?,'u1',?,1,0,?)",
            params![ctx.team_id, kind, id, data.to_string(), NOW],
        )
        .unwrap();
}
async fn call(ctx: &Context, path: &str, method: &str, body: Value) -> ApiResult<Value> {
    s::handle(
        ctx.clone(),
        method,
        &format!("/v1/teams/{}/services/{path}", ctx.team_id),
        body,
        HashMap::new(),
    )
    .await
    .map(|v| v.unwrap())
}
fn answer(_: &p::Request) -> ApiResult<Value> {
    Ok(json!({"choices":[{"message":{"content":"Evidence-based answer"}}]}))
}
#[tokio::test]
async fn consent_and_unconfigured_ai_are_truthful() {
    let (ctx, m) = fixture(HashMap::new(), answer);
    let status = call(&ctx, "status", "GET", json!({})).await.unwrap();
    assert_eq!(status["ai"]["configured"], false);
    assert_eq!(
        call(&ctx, "ai/chat", "POST", json!({"message":"Hi"}))
            .await
            .unwrap_err()
            .code,
        "CONSENT_REQUIRED"
    );
    assert_eq!(
        call(
            &ctx,
            "ai/chat",
            "POST",
            json!({"message":"Hi","consent":true})
        )
        .await
        .unwrap_err()
        .code,
        "AI_NOT_CONFIGURED"
    );
    assert!(m.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn ai_context_sends_only_approved_visible_work_descriptions() {
    let (ctx, m) = fixture(env(), answer);
    add(
        &ctx,
        "entries",
        "ok",
        json!({"status":"approved","description":"Reviewed design","start":"2026-03-01T00:00:00Z","end":"2026-03-01T00:10:00Z","durationMs":600000,"app":"secret-app","title":"private-title","password":"secret"}),
    );
    add(
        &ctx,
        "entries",
        "pending",
        json!({"status":"pending","description":"Never send this"}),
    );
    add(
        &ctx,
        "clients",
        "c",
        json!({"name":"Acme","hourlyRate":9999}),
    );
    let out = call(
        &ctx,
        "ai/chat",
        "POST",
        json!({"message":"Summarize","consent":true}),
    )
    .await
    .unwrap();
    assert_eq!(out["text"], "Evidence-based answer");
    let data = m.calls.lock().unwrap()[0].body.clone().unwrap().to_string();
    assert!(data.contains("Reviewed design"));
    for private in ["secret-app", "private-title", "Never send this", "9999"] {
        assert!(!data.contains(private), "{private}");
    }
}
#[tokio::test]
async fn embedding_tags_are_proposals_and_do_not_mutate_entries() {
    let (ctx, _) = fixture(env(), |_| {
        Ok(
            json!({"data":[{"index":0,"embedding":[1,0]},{"index":1,"embedding":[0,1]},{"index":2,"embedding":[1,0]}]}),
        )
    });
    add(
        &ctx,
        "entries",
        "e",
        json!({"status":"approved","description":"Design screens"}),
    );
    add(&ctx, "clients", "c", json!({"name":"Acme"}));
    add(
        &ctx,
        "projects",
        "p",
        json!({"name":"Design","clientId":"c"}),
    );
    let out = call(
        &ctx,
        "ai/tag",
        "POST",
        json!({"entryIds":["e"],"consent":true}),
    )
    .await
    .unwrap();
    assert_eq!(out["proposals"][0]["projectId"], "p");
    assert_eq!(out["proposals"][0]["confidence"], 1.);
    assert_eq!(out["requiresApproval"], true);
    assert_eq!(ctx.records("entries").unwrap()[0]["version"], 1);
}
#[tokio::test]
async fn reports_skills_routines_persist_with_versions_consent_and_owner_due_checks() {
    let (ctx, _) = fixture(env(), answer);
    let skill = call(
        &ctx,
        "ai/skills",
        "POST",
        json!({"name":"Brief","instructions":"Be concise"}),
    )
    .await
    .unwrap();
    let skill_id = skill["skill"]["id"].as_str().unwrap();
    assert_eq!(
        call(&ctx, "ai/skills", "GET", json!({})).await.unwrap()["skills"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(call(&ctx,"ai/routines","POST",json!({"name":"Daily","prompt":"Summarize","skillId":skill_id,"intervalMinutes":1440,"enabled":true})).await.unwrap_err().code,"CONSENT_REQUIRED");
    let routine=call(&ctx,"ai/routines","POST",json!({"name":"Daily","prompt":"Summarize","skillId":skill_id,"intervalMinutes":1440,"enabled":true,"consent":true})).await.unwrap();
    assert_eq!(s::tick_routines(ctx.clone()).await.unwrap()["ran"], 0);
    let id = routine["routine"]["id"].as_str().unwrap();
    let old = ctx.records("routines").unwrap().remove(0);
    let mut data = old["data"].clone();
    data["nextRunAt"] = json!(0);
    ctx.put_record("routines", id, data, Some(1)).unwrap();
    assert_eq!(s::tick_routines(ctx.clone()).await.unwrap()["ran"], 1);
    assert_eq!(s::tick_routines(ctx.clone()).await.unwrap()["ran"], 0);
    let list = call(&ctx, "ai/reports", "GET", json!({})).await.unwrap();
    let report = &list["reports"][0];
    let out = call(
        &ctx,
        "ai/reports",
        "POST",
        json!({"id":report["id"],"title":"Edited","text":"Reviewed by user","baseVersion":1}),
    )
    .await
    .unwrap();
    assert_eq!(out["report"]["version"], 2);
    assert_eq!(
        call(
            &ctx,
            "ai/reports",
            "POST",
            json!({"id":report["id"],"title":"Stale","text":"Old","baseVersion":1})
        )
        .await
        .unwrap_err()
        .status,
        409
    );
    call(
        &ctx,
        &format!("ai/reports/{}", report["id"].as_str().unwrap()),
        "DELETE",
        json!({"baseVersion":2}),
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn failed_routine_moves_due_time_and_stores_safe_error() {
    let mut e = HashMap::new();
    e.insert("RIZE_RUN_ROUTINES".into(), "1".into());
    let (ctx, _) = fixture(e, answer);
    add(
        &ctx,
        "routines",
        "r",
        json!({"name":"Report","prompt":"Review","enabled":true,"consent":true,"intervalMinutes":15,"nextRunAt":0}),
    );
    assert_eq!(s::tick_routines(ctx.clone()).await.unwrap()["ran"], 0);
    let r = &ctx.records("routines").unwrap()[0]["data"];
    assert!(r["lastError"].as_str().unwrap().contains("Configure"));
    assert!(r["nextRunAt"].as_i64().unwrap() > NOW);
}
#[tokio::test]
async fn integration_connect_encrypts_and_uncertain_export_does_not_retry() {
    let (ctx, m) = fixture(env(), |r| {
        if r.method == "POST" {
            return Err(p::failure(
                502,
                "PROVIDER_UNREACHABLE",
                "Provider request failed or timed out.",
            ));
        }
        Ok(json!({"tasks":[{"id":"t1","name":"Work","status":{"type":"open"}}],"last_page":true}))
    });
    let out = call(
        &ctx,
        "integrations/clickup/connect",
        "POST",
        json!({"token":"fake-token-secret","listId":"123","workspaceId":"456"}),
    )
    .await
    .unwrap();
    assert_eq!(out["integration"]["connected"], true);
    assert!(!out.to_string().contains("fake-token-secret"));
    assert!(!ctx
        .kv_get("integrations", "clickup")
        .unwrap()
        .unwrap()
        .to_string()
        .contains("fake-token-secret"));
    call(&ctx, "integrations/clickup/sync", "POST", json!({}))
        .await
        .unwrap();
    add(
        &ctx,
        "entries",
        "e",
        json!({"status":"approved","description":"Work","taskId":p::integration_id("clickup","t1"),"start":"2026-03-01T00:00:00Z","end":"2026-03-01T00:01:00Z","durationMs":60000}),
    );
    assert_eq!(
        call(
            &ctx,
            "integrations/clickup/sync",
            "POST",
            json!({"exportEntryIds":["e"],"consent":true})
        )
        .await
        .unwrap_err()
        .status,
        502
    );
    let out = call(
        &ctx,
        "integrations/clickup/sync",
        "POST",
        json!({"exportEntryIds":["e"],"consent":true}),
    )
    .await
    .unwrap();
    assert_eq!(out["skipped"], 1);
    assert!(out["warnings"][0].as_str().unwrap().contains("uncertain"));
    assert_eq!(
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}
#[tokio::test]
async fn nonadmin_cannot_connect_pull_or_disconnect() {
    let (mut ctx, _) = fixture(env(), answer);
    ctx.member["role"] = json!("member");
    for (path, method) in [
        ("integrations/asana/connect", "POST"),
        ("integrations/asana/sync", "POST"),
        ("integrations/asana", "DELETE"),
    ] {
        assert_eq!(
            call(
                &ctx,
                path,
                method,
                json!({"token":"fake-token","projectId":"123"})
            )
            .await
            .unwrap_err()
            .status,
            403
        );
    }
}
#[tokio::test]
async fn oauth_pkce_state_is_bound_single_use_and_refresh_token_encrypted() {
    let (ctx, m) = fixture(env(), |r| {
        assert_eq!(r.url, "https://oauth2.googleapis.com/token");
        assert_eq!(
            r.form.as_ref().unwrap().get("grant_type").unwrap(),
            "authorization_code"
        );
        Ok(
            json!({"access_token":"access-token","refresh_token":"refresh-token","token_type":"Bearer","expires_in":3600}),
        )
    });
    let start = call(&ctx, "integrations/google/oauth/start", "POST", json!({}))
        .await
        .unwrap();
    let u = url::Url::parse(start["authorizationUrl"].as_str().unwrap()).unwrap();
    let query = u.query_pairs().into_owned().collect::<HashMap<_, _>>();
    let state = query.get("state").unwrap();
    assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(
        s::resolve_oauth_state("google", state, NOW)
            .unwrap()
            .unwrap()["teamId"],
        ctx.team_id
    );
    assert!(s::resolve_oauth_state("microsoft", state, NOW)
        .unwrap()
        .is_none());
    assert!(s::resolve_oauth_state("google", state, NOW + 11 * 60000)
        .unwrap()
        .is_none());
    let mut wrong = ctx.clone();
    wrong.user["id"] = json!("intruder");
    let callback = HashMap::from([
        ("state".into(), state.clone()),
        ("code".into(), "code-from-provider".into()),
    ]);
    assert!(s::complete_oauth(wrong, "google", callback.clone())
        .await
        .is_err());
    assert!(m.calls.lock().unwrap().is_empty());
    s::complete_oauth(ctx.clone(), "google", callback.clone())
        .await
        .unwrap();
    assert!(s::complete_oauth(ctx.clone(), "google", callback)
        .await
        .is_err());
    let calls = m.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    use sha2::{Digest, Sha256};
    let verifier = calls[0]
        .form
        .as_ref()
        .unwrap()
        .get("code_verifier")
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes())),
        *query.get("code_challenge").unwrap()
    );
    let stored = ctx.kv_get("integrations", "google").unwrap().unwrap();
    assert!(!stored.to_string().contains("refresh-token"));
    assert_eq!(
        p::unseal(
            &stored["secret"],
            &ctx.state.env,
            &format!("{}:google", ctx.team_id)
        )
        .unwrap()["refreshToken"],
        "refresh-token"
    );
}
#[tokio::test]
async fn calendar_pull_refreshes_paginates_and_never_creates_time() {
    let count = Arc::new(Mutex::new(0));
    let c = count.clone();
    let (ctx, _) = fixture(env(), move |r| {
        if r.url == "https://oauth2.googleapis.com/token" {
            assert_eq!(
                r.form.as_ref().unwrap().get("refresh_token").unwrap(),
                "old-refresh"
            );
            return Ok(
                json!({"access_token":"fresh-access","token_type":"Bearer","expires_in":3600}),
            );
        }
        assert_eq!(
            r.headers.get("Authorization").unwrap(),
            "Bearer fresh-access"
        );
        let mut page = c.lock().unwrap();
        *page += 1;
        Ok(if *page == 1 {
            json!({"items":[{"id":"timed","summary":"Meeting","start":{"dateTime":"2026-03-03T09:00:00+05:30"},"end":{"dateTime":"2026-03-03T10:00:00+05:30"}}],"nextPageToken":"next-page"})
        } else {
            json!({"items":[{"id":"all-day","summary":"Holiday","start":{"date":"2026-03-08"},"end":{"date":"2026-03-09"}}]})
        })
    });
    ctx.kv_put("integrations","google",json!({"secret":p::seal(&json!({"accessToken":"expired","refreshToken":"old-refresh","expiresAt":0}),&ctx.state.env,&format!("{}:google",ctx.team_id)).unwrap(),"options":{"calendar":"primary"}})).unwrap();
    let result = call(
        &ctx,
        "integrations/google/sync",
        "POST",
        json!({"from":"2026-03-01","to":"2026-04-01"}),
    )
    .await
    .unwrap();
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
    assert_eq!(result["events"][0]["start"], "2026-03-03T03:30:00.000Z");
    assert_eq!(result["events"][1]["date"], "2026-03-08");
    assert_eq!(result["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(ctx.records("entries").unwrap().len(), 0);
    assert_eq!(ctx.records("context").unwrap().len(), 2);
}
#[tokio::test]
async fn imported_unicode_ids_decode_for_existing_service_delete_routes() {
    let (ctx, _) = fixture(env(), answer);
    let id = "skill a/+∑";
    add(
        &ctx,
        "skills",
        id,
        json!({"id":id,"name":"Imported","instructions":"Be brief"}),
    );
    let deleted = call(
        &ctx,
        "ai/skills/skill%20a%2F%2B%E2%88%91",
        "DELETE",
        json!({"baseVersion":1}),
    )
    .await
    .unwrap();
    assert_eq!(deleted["id"], id);
    assert!(ctx.records("skills").unwrap().is_empty());
    assert_eq!(
        call(&ctx, "ai/skills/bad%ZZ", "DELETE", json!({}))
            .await
            .unwrap_err()
            .status,
        400
    );
}
#[tokio::test]
async fn oauth_disconnect_cancels_pending_state_and_role_filters_hide_private_context() {
    let (ctx, _) = fixture(env(), answer);
    let start = call(&ctx, "integrations/google/oauth/start", "POST", json!({}))
        .await
        .unwrap();
    let url = url::Url::parse(start["authorizationUrl"].as_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    call(&ctx, "integrations/google", "DELETE", json!({}))
        .await
        .unwrap();
    assert!(s::resolve_oauth_state("google", &state, NOW)
        .unwrap()
        .is_none());
    {
        let db = ctx.state.db().unwrap();
        db.execute(
            "INSERT INTO users VALUES('u2','two@example.test','Other','unused',0)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO members VALUES(?,'u2','member',0,0,0)",
            [&ctx.team_id],
        )
        .unwrap();
        db.execute(
            "UPDATE members SET role='member' WHERE team_id=? AND user_id='u1'",
            [&ctx.team_id],
        )
        .unwrap();
        db.execute(
            "INSERT INTO records VALUES(?,'entries','other','u2',?,1,0,?)",
            params![
                ctx.team_id,
                json!({"status":"approved","description":"Other private entry"}).to_string(),
                NOW
            ],
        )
        .unwrap();
    }
    let member = ctx.state.context_for(&ctx.team_id, "u1").unwrap();
    let context = s::context(&member).unwrap();
    assert!(!context.to_string().contains("Other private entry"));
}
