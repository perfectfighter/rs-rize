use reqwest::{Client, Method};
use rize_service::{auth, router, AppState};
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    state: AppState,
    base: String,
    client: Client,
    clock: Arc<AtomicI64>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let clock = Arc::new(AtomicI64::new(1_792_238_400_000));
        let c = clock.clone();
        let state = AppState::new(
            ":memory:",
            HashMap::from([(
                "RIZE_ALLOWED_ORIGINS".into(),
                "http://localhost:5173".into(),
            )]),
        )
        .unwrap()
        .with_clock(Arc::new(move || c.load(Ordering::SeqCst)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            state,
            base,
            client: Client::new(),
            clock,
            task,
        }
    }
    fn actor(&self, email: &str) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        let db = self.state.db().unwrap();
        db.execute(
            "INSERT INTO users(id,email,name,password_hash,created_at) VALUES(?,?,?,?,?)",
            params![
                id,
                email,
                email.split('@').next().unwrap(),
                "synthetic-test-no-login",
                self.state.now_ms()
            ],
        )
        .unwrap();
        let token = auth::issue_session(&db, &id, self.state.now_ms()).unwrap();
        json!({"user":{"id":id,"email":email},"token":token})
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        actor: &Value,
    ) -> (u16, Value) {
        let mut call = self.client.request(
            Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", self.base),
        );
        if let Some(token) = actor["token"].as_str() {
            call = call.bearer_auth(token);
        }
        if let Some(body) = body {
            call = call.json(&body);
        }
        let response = call.send().await.unwrap();
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }
    async fn team(&self, actor: &Value) -> String {
        let (status, result) = self
            .call(
                "POST",
                "/v1/teams",
                Some(json!({"name":"Synthetic team"})),
                actor,
            )
            .await;
        assert_eq!(status, 201, "{result}");
        result["team"]["id"].as_str().unwrap().to_string()
    }
    async fn join(&self, owner: &Value, team: &str, actor: &Value, role: &str) {
        let (status, invite) = self
            .call(
                "POST",
                &format!("/v1/teams/{team}/invites"),
                Some(json!({"email":actor["user"]["email"],"role":role})),
                owner,
            )
            .await;
        assert_eq!(status, 201, "{invite}");
        let (status, result) = self
            .call(
                "POST",
                "/v1/teams/join",
                Some(json!({"code":invite["code"]})),
                actor,
            )
            .await;
        assert_eq!(status, 200, "{result}");
    }
}
fn entry(id: &str) -> Value {
    json!({"id":id,"start":"2026-10-17T04:00:00Z","end":"2026-10-17T05:00:00Z","description":"Reviewed work","status":"approved","category":"productive","clientId":"","projectId":"","taskId":"","billable":true,"source":"manual","durationMs":3_600_000})
}
fn change(kind: &str, id: &str, data: Value, version: i64) -> Value {
    json!({"kind":kind,"id":id,"data":data,"baseVersion":version})
}

#[tokio::test]
async fn actual_http_accounts_sessions_hashes_and_expiry() {
    let f = Fixture::new().await;
    let(status,account)=f.call("POST","/v1/auth/register",Some(json!({"email":"Owner@Example.test","name":"Owner","password":"a long synthetic password"})),&Value::Null).await;
    assert_eq!(status, 201, "{account}");
    assert_eq!(account["user"]["email"], "owner@example.test");
    assert_eq!(account["token"].as_str().unwrap().len(), 43);
    let (hash, token_hash) = {
        let db = f.state.db().unwrap();
        (
            db.query_row("SELECT password_hash FROM users", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            db.query_row("SELECT token_hash FROM sessions", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        )
    };
    assert!(hash.starts_with("scrypt:32768:8:3:"));
    assert_ne!(token_hash, account["token"]);
    assert_eq!(f.call("GET", "/v1/me", None, &account).await.0, 200);
    let (status, login) = f
        .call(
            "POST",
            "/v1/auth/login",
            Some(json!({"email":"owner@example.test","password":"a long synthetic password"})),
            &Value::Null,
        )
        .await;
    assert_eq!(status, 200, "{login}");
    assert_ne!(login["token"], account["token"]);
    assert_eq!(
        f.call("POST", "/v1/auth/logout", Some(json!({})), &login)
            .await
            .0,
        200
    );
    assert_eq!(f.call("GET", "/v1/me", None, &login).await.0, 401);
    f.clock.fetch_add(auth::SESSION_TTL + 1, Ordering::SeqCst);
    assert_eq!(f.call("GET", "/v1/me", None, &account).await.0, 401);
}

#[tokio::test]
async fn actual_http_origins_body_bounds_and_registration_gates() {
    let f = Fixture::new().await;
    let denied = f
        .client
        .get(format!("{}/healthz", f.base))
        .header("Origin", "https://attacker.test")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let allowed = f
        .client
        .get(format!("{}/healthz", f.base))
        .header("Origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
    assert_eq!(
        allowed.headers()["access-control-allow-origin"],
        "http://localhost:5173"
    );
    assert_eq!(allowed.headers()["cache-control"], "no-store");
    let options = f
        .client
        .request(Method::OPTIONS, format!("{}/v1/me", f.base))
        .header("Origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();
    assert_eq!(options.status(), 204);
    for (data, status) in [("{".to_string(), 400), ("[]".to_string(), 400)] {
        let r = f
            .client
            .post(format!("{}/v1/auth/register", f.base))
            .header("content-type", "application/json")
            .body(data)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status);
    }
    // The server rejects an oversized declared body before accepting its bytes.
    // Send only headers so Windows cannot abort a still-uploading HTTP client
    // when the server returns 413 and closes the rejected request's connection.
    let authority = f.base.strip_prefix("http://").unwrap();
    let mut oversized = tokio::net::TcpStream::connect(authority).await.unwrap();
    let headers = format!(
        "POST /v1/auth/register HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        1024 * 1024 + 1
    );
    oversized.write_all(headers.as_bytes()).await.unwrap();
    let status_line = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            assert!(line.len() < 4096, "HTTP response status line exceeds limit");
            line.push(oversized.read_u8().await.unwrap());
        }
        String::from_utf8(line).unwrap()
    })
    .await
    .expect("The server must reject oversized Content-Length before receiving a body");
    let mut status_fields = status_line.split_whitespace();
    assert_eq!(status_fields.next(), Some("HTTP/1.1"), "{status_line:?}");
    assert_eq!(status_fields.next(), Some("413"), "{status_line:?}");
    drop(oversized);
    let weak = f
        .call(
            "POST",
            "/v1/auth/register",
            Some(json!({"email":"a@b.test","name":"A","password":"short"})),
            &Value::Null,
        )
        .await;
    assert_eq!(weak.0, 400);
    let mut rate_limited = false;
    for n in 0..21 {
        let result=f.call("POST","/v1/auth/register",Some(json!({"email":format!("weak{n}@example.test"),"name":"Weak","password":"short"})),&Value::Null).await;
        if result.0 == 429 {
            rate_limited = true;
            assert_eq!(result.1["code"], "RATE_LIMITED");
            break;
        }
    }
    assert!(rate_limited);
    assert!(rize_service::validate_binding("0.0.0.0", &HashMap::new()).is_err());
    assert!(AppState::new(
        ":memory:",
        HashMap::from([("RIZE_ALLOWED_ORIGINS".into(), "http://attacker.test".into())])
    )
    .is_err());
}

#[tokio::test]
async fn actual_http_invites_bind_email_roles_and_revoke_members() {
    let f = Fixture::new().await;
    let owner = f.actor("owner@example.test");
    let member = f.actor("member@example.test");
    let outsider = f.actor("other@example.test");
    let team = f.team(&owner).await;
    let (_, inv) = f
        .call(
            "POST",
            &format!("/v1/teams/{team}/invites"),
            Some(json!({"email":member["user"]["email"]})),
            &owner,
        )
        .await;
    assert_eq!(
        f.call(
            "POST",
            "/v1/teams/join",
            Some(json!({"code":inv["code"]})),
            &outsider
        )
        .await
        .1["code"],
        "INVALID_INVITE"
    );
    assert_eq!(
        f.call(
            "POST",
            "/v1/teams/join",
            Some(json!({"code":inv["code"]})),
            &member
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.call(
            "POST",
            "/v1/teams/join",
            Some(json!({"code":inv["code"]})),
            &member
        )
        .await
        .1["code"],
        "INVALID_INVITE"
    );
    let owner_route = format!(
        "/v1/teams/{team}/members/{}",
        owner["user"]["id"].as_str().unwrap()
    );
    assert_eq!(
        f.call("DELETE", &owner_route, Some(json!({})), &owner)
            .await
            .1["code"],
        "LAST_OWNER"
    );
    let member_route = format!(
        "/v1/teams/{team}/members/{}",
        member["user"]["id"].as_str().unwrap()
    );
    assert_eq!(
        f.call(
            "PATCH",
            &member_route,
            Some(json!({"role":"owner"})),
            &member
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call(
            "PATCH",
            &member_route,
            Some(json!({"finance":true,"resourcing":true})),
            &owner
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.call("DELETE", &member_route, Some(json!({})), &owner)
            .await
            .0,
        200
    );
    assert_eq!(
        f.call("GET", &format!("/v1/teams/{team}/sync"), None, &member)
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn actual_http_sync_is_private_atomic_versioned_and_team_scoped() {
    let f = Fixture::new().await;
    let owner = f.actor("owner@example.test");
    let member = f.actor("member@example.test");
    let viewer = f.actor("viewer@example.test");
    let other = f.actor("other@example.test");
    let team = f.team(&owner).await;
    let other_team = f.team(&other).await;
    f.join(&owner, &team, &member, "member").await;
    f.join(&owner, &team, &viewer, "viewer").await;
    let route = format!("/v1/teams/{team}/sync");
    let (status, pushed) = f
        .call(
            "POST",
            &route,
            Some(json!({"changes":[change("entries","work",entry("work"),0)]})),
            &owner,
        )
        .await;
    assert_eq!(status, 200, "{pushed}");
    assert_eq!(pushed["records"][0]["ownerId"], owner["user"]["id"]);
    assert_eq!(pushed["records"][0]["version"], 1);
    assert_eq!(f.call("GET", &route, None, &other).await.0, 404);
    assert_eq!(
        f.call("GET", &format!("/v1/teams/{other_team}/sync"), None, &other)
            .await
            .1["records"],
        json!([])
    );
    assert_eq!(
        f.call("GET", &route, None, &member).await.1["records"],
        json!([])
    );
    let stale = f
        .call(
            "POST",
            &route,
            Some(json!({"changes":[change("entries","work",entry("work"),0)]})),
            &owner,
        )
        .await;
    assert_eq!(stale.1["conflicts"][0]["current"]["version"], 1);
    assert_eq!(
        f.call(
            "POST",
            &route,
            Some(json!({"changes":[change("entries","work",entry("work"),1)]})),
            &member
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call(
            "POST",
            &route,
            Some(json!({"changes":[change("entries","mine",entry("mine"),0)]})),
            &viewer
        )
        .await
        .0,
        403
    );
    let mut private = entry("raw");
    private["title"] = json!("private window");
    let result=f.call("POST",&route,Some(json!({"changes":[change("clients","rollback",json!({"name":"Must not persist"}),0),change("entries","raw",private,0)]})),&owner).await;
    assert_eq!(result.1["code"], "PRIVATE_ACTIVITY");
    assert_eq!(
        f.call("GET", &route, None, &owner).await.1["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let deleted = f
        .call(
            "POST",
            &route,
            Some(
                json!({"changes":[{"kind":"entries","id":"work","baseVersion":1,"deleted":true}]}),
            ),
            &owner,
        )
        .await;
    assert_eq!(deleted.1["records"][0]["deleted"], true);
    assert!(deleted.1["records"][0]["data"].is_null());
}

#[tokio::test]
async fn actual_http_sync_pages_without_skipping_hidden_events() {
    let f = Fixture::new().await;
    let owner = f.actor("owner@example.test");
    let member = f.actor("member@example.test");
    let team = f.team(&owner).await;
    f.join(&owner, &team, &member, "member").await;
    let route = format!("/v1/teams/{team}/sync");
    for offset in [0, 500, 1000] {
        let changes: Vec<_> = (offset..if offset == 1000 { 1001 } else { offset + 500 })
            .map(|n| {
                let id = format!("client-{n}");
                change("clients", &id, json!({"name":id}), 0)
            })
            .collect();
        let (status, result) = f
            .call("POST", &route, Some(json!({"changes":changes})), &owner)
            .await;
        assert_eq!(status, 200, "{result}");
    }
    let (status, hidden) = f
        .call(
            "POST",
            &route,
            Some(json!({"changes":[change("entries","owner-only",entry("owner-only"),0)]})),
            &owner,
        )
        .await;
    assert_eq!(status, 200, "{hidden}");
    let (_, first) = f.call("GET", &route, None, &member).await;
    assert_eq!(first["records"].as_array().unwrap().len(), 1000);
    assert_eq!(first["hasMore"], true);
    let (_, last) = f
        .call(
            "GET",
            &format!("{route}?since={}", first["cursor"]),
            None,
            &member,
        )
        .await;
    assert_eq!(last["records"].as_array().unwrap().len(), 1);
    assert_eq!(last["hasMore"], false);
    assert_eq!(last["records"][0]["id"], "client-1000");
    assert_eq!(last["cursor"], hidden["cursor"]);
    let (_, empty) = f
        .call(
            "GET",
            &format!("{route}?since={}", last["cursor"]),
            None,
            &member,
        )
        .await;
    assert_eq!(empty["records"], json!([]));
    let invalid = f
        .call("GET", &format!("{route}?since=999999"), None, &owner)
        .await;
    assert_eq!(invalid.1["code"], "INVALID_CURSOR");
}

#[tokio::test]
async fn actual_http_analytics_split_local_midnight_union_and_hide_descriptions() {
    let f = Fixture::new().await;
    let owner = f.actor("owner@example.test");
    let team = f.team(&owner).await;
    let mut night = entry("night");
    night["start"] = json!("2026-10-16T17:30:00Z");
    night["end"] = json!("2026-10-16T19:30:00Z");
    night["durationMs"] = json!(7_200_000);
    let mut overlap = entry("overlap");
    overlap["start"] = json!("2026-10-16T18:45:00Z");
    overlap["end"] = json!("2026-10-16T19:15:00Z");
    overlap["durationMs"] = json!(1_800_000);
    let(status,result)=f.call("POST",&format!("/v1/teams/{team}/sync"),Some(json!({"changes":[change("entries","night",night,0),change("entries","overlap",overlap,0)]})),&owner).await;
    assert_eq!(status, 200, "{result}");
    let(status,result)=f.call("GET",&format!("/v1/teams/{team}/analytics?startDate=2026-10-16&endDate=2026-10-17&timezone=Asia%2FKolkata"),None,&owner).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["totals"]["durationMs"], 7_200_000);
    assert_eq!(result["timesheet"][0]["durationMs"], 3_600_000);
    assert_eq!(result["timesheet"][1]["durationMs"], 3_600_000);
    assert!(!result.to_string().contains("Reviewed work"));
    assert_eq!(
        f.call(
            "GET",
            &format!("/v1/teams/{team}/analytics?timezone=Invalid/Zone"),
            None,
            &owner
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn actual_http_financial_snapshots_lock_sources_and_preserve_payment_history() {
    let f = Fixture::new().await;
    let owner = f.actor("owner@example.test");
    let team = f.team(&owner).await;
    let route = format!("/v1/teams/{team}/sync");
    // Golden snapshot produced by published v1.2 invoice.create, using synthetic data.
    let invoice: Value =
        serde_json::from_str(include_str!("fixtures/accounting-invoice.json")).unwrap();
    let mut work = entry("owner-entry");
    work["clientId"] = json!("client");
    let(status,result)=f.call("POST",&route,Some(json!({"changes":[change("clients","client",json!({"name":"Synthetic client","hourlyRate":100}),0),change("entries","owner-entry",work.clone(),0),change("invoices","invoice",invoice.clone(),0)]})),&owner).await;
    assert_eq!(status, 200, "{result}");
    let mut edited = work.clone();
    edited["description"] = json!("Cannot rewrite reserved time");
    let locked = f
        .call(
            "POST",
            &route,
            Some(json!({"changes":[change("entries","owner-entry",edited.clone(),1)]})),
            &owner,
        )
        .await;
    assert_eq!(locked.1["code"], "BILLED_TIME_LOCKED", "{}", locked.1);
    let mut duplicate = invoice.clone();
    duplicate["id"] = json!("duplicate");
    duplicate["number"] = json!("INV-0002");
    duplicate["sequence"] = json!(2);
    let rolled_back=f.call("POST",&route,Some(json!({"changes":[change("clients","rollback",json!({"name":"Must roll back"}),0),change("invoices","duplicate",duplicate,0)]})),&owner).await;
    assert_eq!(rolled_back.1["code"], "FINANCIAL_INVARIANT");
    let after = f.call("GET", &route, None, &owner).await;
    assert_eq!(after.1["records"].as_array().unwrap().len(), 3);
    let mut sent = invoice.clone();
    sent["status"] = json!("sent");
    assert_eq!(
        f.call(
            "POST",
            &route,
            Some(json!({"changes":[change("invoices","invoice",sent.clone(),1)]})),
            &owner
        )
        .await
        .0,
        200
    );
    let mut rewritten = sent.clone();
    rewritten["notes"] = json!("Change frozen note");
    assert_eq!(
        f.call(
            "POST",
            &route,
            Some(json!({"changes":[change("invoices","invoice",rewritten,2)]})),
            &owner
        )
        .await
        .1["code"],
        "INVOICE_IMMUTABLE"
    );
    assert_eq!(f.call("POST",&route,Some(json!({"changes":[{"kind":"invoices","id":"invoice","baseVersion":2,"deleted":true}]})),&owner).await.1["code"],"INVOICE_IMMUTABLE");
    let mut paid = sent.clone();
    paid["payments"] =
        json!([{"id":"payment-1","amountMinor":1000,"date":"2026-10-17","reference":"Bank 1"}]);
    paid["paidMinor"] = json!(1000);
    paid["outstandingMinor"] = json!(9000);
    let (status, result) = f
        .call(
            "POST",
            &route,
            Some(json!({"changes":[change("invoices","invoice",paid.clone(),2)]})),
            &owner,
        )
        .await;
    assert_eq!(status, 200, "{result}");
    let mut forged = paid.clone();
    forged["payments"][0]["reference"] = json!("Forged reference");
    assert_eq!(
        f.call(
            "POST",
            &route,
            Some(json!({"changes":[change("invoices","invoice",forged,3)]})),
            &owner
        )
        .await
        .1["code"],
        "INVOICE_IMMUTABLE"
    );
    // Paid invoices cannot be voided or have their payment history removed.
    paid["status"] = json!("void");
    paid["voidReason"] = json!("Synthetic cancellation");
    assert_eq!(
        f.call(
            "POST",
            &route,
            Some(json!({"changes":[change("invoices","invoice",paid,3)]})),
            &owner
        )
        .await
        .1["code"],
        "FINANCIAL_INVARIANT"
    );
    // An unpaid invoice may be voided and its source edited in the same transaction.
    let team2 = f.team(&owner).await;
    let route2 = format!("/v1/teams/{team2}/sync");
    let(status,result)=f.call("POST",&route2,Some(json!({"changes":[change("clients","client",json!({"name":"Synthetic client","hourlyRate":100}),0),change("entries","owner-entry",work,0),change("invoices","invoice",invoice.clone(),0)]})),&owner).await;
    assert_eq!(status, 200, "{result}");
    let mut voided = invoice;
    voided["status"] = json!("void");
    voided["voidReason"] = json!("Synthetic cancellation");
    let(status,result)=f.call("POST",&route2,Some(json!({"changes":[change("entries","owner-entry",edited,1),change("invoices","invoice",voided,1)]})),&owner).await;
    assert_eq!(status, 200, "{result}");
}
