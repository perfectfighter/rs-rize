//! End-to-end desktop cloud tests use the Rust HTTP service over real TCP.
//! Faults are inserted at the HTTP boundary, never by fabricating client success.
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rize_cloud::{CloudClient, SecureStorage};
use rize_core::state::Store;
use rize_service::{router, AppState};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Notify;

const NOW: i64 = 1_792_238_400_000;
struct SessionOnly;
impl SecureStorage for SessionOnly {
    fn available(&self) -> bool {
        false
    }
    fn protect(&self, _: &str) -> Result<Vec<u8>, String> {
        Err("Secure storage is unavailable.".into())
    }
    fn unprotect(&self, _: &[u8]) -> Result<String, String> {
        Err("Secure storage is unavailable.".into())
    }
}
#[derive(Default)]
struct Hooks {
    audit: Mutex<Vec<(String, String, String)>>,
    corrupt: AtomicBool,
    block: AtomicBool,
    entered: Notify,
    release: Notify,
}
async fn intercept(State(hooks): State<Arc<Hooks>>, request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
    hooks.audit.lock().unwrap().push((
        method.clone(),
        uri.clone(),
        String::from_utf8_lossy(&bytes).into_owned(),
    ));
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    if method == "GET" && uri.contains("/sync?") {
        if hooks.block.swap(false, Ordering::SeqCst) {
            hooks.entered.notify_one();
            hooks.release.notified().await;
        }
        if hooks.corrupt.load(Ordering::SeqCst) {
            let (mut parts, body) = response.into_parts();
            parts.headers.remove(axum::http::header::CONTENT_LENGTH);
            let bytes = to_bytes(body, 12 * 1024 * 1024).await.unwrap();
            let mut data: Value = serde_json::from_slice(&bytes).unwrap();
            data["records"].as_array_mut().unwrap().push(json!({"kind":"entries","id":"invalid-entry","ownerId":"owner","version":1,"deleted":false,"data":{"id":"invalid-entry","start":"invalid timestamp","end":"2026-10-17T10:00:00.000Z","status":"approved"}}));
            return Response::from_parts(parts, Body::from(serde_json::to_vec(&data).unwrap()));
        }
    }
    response
}
struct Fixture {
    origin: String,
    hooks: Arc<Hooks>,
    task: tokio::task::JoinHandle<()>,
    root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let state = AppState::new(":memory:", HashMap::new()).unwrap();
        Self::with_state(state).await
    }
    async fn with_state(state: AppState) -> Self {
        let hooks = Arc::new(Hooks::default());
        let app = router(state).layer(middleware::from_fn_with_state(hooks.clone(), intercept));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            origin,
            hooks,
            task,
            root: tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap(),
        }
    }
    fn device(&self, name: &str) -> Device {
        let store = Arc::new(Mutex::new(Store::new(None).unwrap()));
        mutate(&store, "workspace.create", json!({"displayName":name}));
        let file = self.root.path().join(format!("{name}.json"));
        let client = CloudClient::with_secure_storage(
            Some(file.clone()),
            store.clone(),
            Arc::new(SessionOnly),
        )
        .unwrap();
        Device {
            store,
            client,
            file,
        }
    }
    async fn authenticate(&self, device: &mut Device, register: bool) {
        self.authenticate_as(device, register, "synthetic-owner@example.test")
            .await;
    }
    async fn authenticate_as(&self, device: &mut Device, register: bool, email: &str) {
        run(
            &mut device.client,
            "configure",
            json!({"serverUrl":self.origin}),
        )
        .await;
        run(
            &mut device.client,
            if register { "register" } else { "login" },
            json!({"email":email,"password":"a long synthetic password","name":"Synthetic user"}),
        )
        .await;
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Device {
    store: Arc<Mutex<Store>>,
    client: CloudClient,
    file: PathBuf,
}
fn mutate(store: &Arc<Mutex<Store>>, kind: &str, payload: Value) -> Value {
    store
        .lock()
        .unwrap()
        .mutate(&json!({"type":kind,"payload":payload}), NOW)
        .unwrap()
}
async fn run(client: &mut CloudClient, action: &str, payload: Value) -> Value {
    client.execute(action, payload).await.unwrap()
}
async fn sync(client: &mut CloudClient) -> Value {
    run(client, "sync", json!({"consent":true})).await
}
fn entry(id: &str, status: &str, description: &str) -> Value {
    json!({"id":id,"start":"2026-10-17T09:00:00.123Z","end":"2026-10-17T09:30:00.456Z","description":description,"app":"Private source app","title":"A private foreground title","status":status,"source":"automatic","category":"productive"})
}

#[tokio::test]
async fn real_http_two_devices_privacy_conflicts_private_ids_and_workspace_binding() {
    let f = Fixture::new().await;
    let mut a = f.device("a");
    f.authenticate(&mut a, true).await;
    let team = run(
        &mut a.client,
        "team.create",
        json!({"name":"Synthetic team"}),
    )
    .await["result"]["team"]
        .clone();
    mutate(
        &a.store,
        "client.upsert",
        json!({"id":"client","name":"Client","hourlyRate":100}),
    );
    mutate(
        &a.store,
        "project.upsert",
        json!({"id":"project","name":"Project","clientId":"client"}),
    );
    let mut approved = entry("approved", "approved", "Reviewed work");
    approved["projectId"] = json!("project");
    mutate(&a.store, "entry.upsert", approved);
    mutate(
        &a.store,
        "entry.upsert",
        entry("pending", "pending", "Unreviewed private activity"),
    );
    assert_eq!(sync(&mut a.client).await["result"]["pushed"], 3);
    let posted = f
        .hooks
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, u, _)| m == "POST" && u.ends_with("/sync"))
        .map(|(_, _, b)| b.clone())
        .collect::<String>();
    for private in [
        "Private source app",
        "A private foreground title",
        "Unreviewed private activity",
    ] {
        assert!(!posted.contains(private));
    }
    let mut b = f.device("b");
    f.authenticate(&mut b, false).await;
    run(&mut b.client, "team.select", json!({"teamId":team["id"]})).await;
    mutate(
        &b.store,
        "entry.upsert",
        entry("local-pending", "pending", "Only on B"),
    );
    sync(&mut b.client).await;
    let state = b.store.lock().unwrap().get_state();
    let imported = state["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "approved")
        .unwrap();
    assert_eq!(imported["title"], "");
    assert_eq!(imported["app"], "Cloud sync");
    assert_eq!(state["projects"][0]["clientId"], "client");
    mutate(
        &a.store,
        "entry.upsert",
        json!({"id":"approved","description":"A revision"}),
    );
    sync(&mut a.client).await;
    mutate(
        &b.store,
        "entry.upsert",
        json!({"id":"approved","description":"B revision"}),
    );
    let conflict = sync(&mut b.client).await;
    assert_eq!(conflict["result"]["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(
        conflict["result"]["conflicts"][0]["remote"]["data"]["description"],
        "A revision"
    );
    run(&mut b.client,"sync",json!({"consent":true,"resolutions":[{"kind":"entries","id":"approved","strategy":"local"}]})).await;
    sync(&mut a.client).await;
    assert_eq!(
        a.store.lock().unwrap().get_state()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "approved")
            .unwrap()["description"],
        "B revision"
    );
    mutate(
        &b.store,
        "entry.upsert",
        json!({"id":"approved","description":"B second revision"}),
    );
    sync(&mut b.client).await;
    mutate(
        &a.store,
        "entry.upsert",
        json!({"id":"approved","description":"A second local revision"}),
    );
    sync(&mut a.client).await;
    run(&mut a.client,"sync",json!({"consent":true,"resolutions":[{"kind":"entries","id":"approved","strategy":"remote"}]})).await;
    assert!(a.client.status()["conflicts"]
        .as_array()
        .unwrap()
        .is_empty());
    // Same ID as cloud work cannot overwrite an unreviewed foreground entry.
    let mut c = f.device("c");
    f.authenticate(&mut c, false).await;
    run(&mut c.client, "team.select", json!({"teamId":team["id"]})).await;
    mutate(
        &c.store,
        "entry.upsert",
        entry("approved", "pending", "Private collision"),
    );
    let collision = sync(&mut c.client).await;
    assert_eq!(collision["result"]["conflicts"][0]["privateLocal"], true);
    assert!(c.client.execute("sync",json!({"consent":true,"resolutions":[{"kind":"entries","id":"approved","strategy":"remote"}]})).await.unwrap_err().contains("Review or rename"));
    mutate(
        &a.store,
        "entry.upsert",
        json!({"id":"approved","description":"Latest peer version"}),
    );
    sync(&mut a.client).await;
    assert_eq!(
        sync(&mut c.client).await["result"]["conflicts"][0]["remote"]["data"]["description"],
        "Latest peer version"
    );
    run(&mut a.client, "team.create", json!({"name":"Other team"})).await;
    assert!(a
        .client
        .execute("sync", json!({"consent":true}))
        .await
        .unwrap_err()
        .contains("another account or team"));
    assert!(a
        .client
        .execute("sync", json!({}))
        .await
        .unwrap_err()
        .contains("Approve synchronization"));
    assert!(!std::fs::read_to_string(&a.file)
        .unwrap()
        .contains("tokenCipher"));
    assert_eq!(
        CloudClient::new(Some(a.file.clone()), a.store.clone())
            .unwrap()
            .status()["authenticated"],
        false
    );
}

#[tokio::test]
async fn real_http_pagination_byte_batches_failed_merge_and_reset_cancellation() {
    let f = Fixture::new().await;
    let mut a = f.device("large-a");
    f.authenticate(&mut a, true).await;
    let team = run(&mut a.client, "team.create", json!({"name":"Large team"})).await["result"]
        ["team"]
        .clone();
    let mut state = a.store.lock().unwrap().get_state();
    state["clients"]=json!((0..1001).map(|i|json!({"id":format!("client-{i}"),"name":format!("Client {i}"),"hourlyRate":0,"keywords":[]})).collect::<Vec<_>>());
    state["entries"] = json!((0..300)
        .map(|i| entry(&format!("entry-{i}"), "approved", &"अ".repeat(2000)))
        .collect::<Vec<_>>());
    a.store.lock().unwrap().import_data(state, NOW).unwrap();
    assert_eq!(sync(&mut a.client).await["result"]["pushed"], 1301);
    {
        let audit = f.hooks.audit.lock().unwrap();
        let bodies: Vec<_> = audit
            .iter()
            .filter(|(m, u, _)| m == "POST" && u.ends_with("/sync"))
            .collect();
        assert!(bodies.len() >= 6);
        assert!(bodies.iter().all(|(_, _, body)| body.len() <= 1024 * 1024));
    }
    let mut b = f.device("large-b");
    f.authenticate(&mut b, false).await;
    run(&mut b.client, "team.select", json!({"teamId":team["id"]})).await;
    f.hooks.corrupt.store(true, Ordering::SeqCst);
    let before = b.store.lock().unwrap().get_state();
    assert!(b
        .client
        .execute("sync", json!({"consent":true}))
        .await
        .unwrap_err()
        .contains("ISO timestamp"));
    assert_eq!(b.store.lock().unwrap().get_state(), before);
    assert!(b.client.status()["lastSyncAt"].is_null());
    f.hooks.corrupt.store(false, Ordering::SeqCst);
    sync(&mut b.client).await;
    assert_eq!(
        b.store.lock().unwrap().get_state()["clients"]
            .as_array()
            .unwrap()
            .len(),
        1001
    );
    assert_eq!(
        b.store.lock().unwrap().get_state()["entries"]
            .as_array()
            .unwrap()
            .len(),
        300
    );
    assert!(f
        .hooks
        .audit
        .lock()
        .unwrap()
        .iter()
        .any(|(_, uri, _)| uri.ends_with("/sync?since=1000")));
    // The reset handle remains usable while execute owns the mutable client.
    let cancel = b.client.cancel_handle();
    let pending = cancel.ticket().unwrap();
    let store = b.store.clone();
    let hooks = f.hooks.clone();
    hooks.block.store(true, Ordering::SeqCst);
    let reset = async move {
        hooks.entered.notified().await;
        cancel.cancel();
        mutate(
            &store,
            "workspace.create",
            json!({"displayName":"Fresh workspace"}),
        );
        hooks.release.notify_one();
    };
    let (result, ()) = tokio::join!(b.client.execute("sync", json!({"consent":true})), reset);
    assert!(result.unwrap_err().contains("workspace changed"));
    assert!(b.store.lock().unwrap().get_state()["entries"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(b
        .client
        .execute_with_ticket("sync", json!({"consent":true}), pending.clone())
        .await
        .unwrap_err()
        .contains("workspace changed"));
    assert!(b
        .client
        .execute_with_ticket("sync.auto", json!({"enabled":true,"consent":true}), pending)
        .await
        .unwrap_err()
        .contains("workspace changed"));
    assert_eq!(b.client.status()["autoSync"], false);
}

struct TestSecure;
impl SecureStorage for TestSecure {
    fn available(&self) -> bool {
        true
    }
    fn protect(&self, secret: &str) -> Result<Vec<u8>, String> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };
        let cipher = Aes256Gcm::new_from_slice(&[23u8; 32]).unwrap();
        let id = uuid::Uuid::new_v4();
        let nonce = &id.as_bytes()[..12];
        let mut output = nonce.to_vec();
        output.extend(
            cipher
                .encrypt(Nonce::from_slice(nonce), secret.as_bytes())
                .map_err(|_| "protect failed")?,
        );
        Ok(output)
    }
    fn unprotect(&self, bytes: &[u8]) -> Result<String, String> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };
        if bytes.len() < 12 {
            return Err("Invalid protected test token".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&[23u8; 32]).unwrap();
        String::from_utf8(
            cipher
                .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
                .map_err(|_| "unprotect failed")?,
        )
        .map_err(|_| "Invalid UTF-8".into())
    }
}
#[tokio::test]
async fn injected_secure_storage_restores_encrypted_session_and_logout_erases_it() {
    let f = Fixture::new().await;
    let mut device = f.device("secure");
    device.client = CloudClient::with_secure_storage(
        Some(device.file.clone()),
        device.store.clone(),
        Arc::new(TestSecure),
    )
    .unwrap();
    f.authenticate(&mut device, true).await;
    run(
        &mut device.client,
        "team.create",
        json!({"name":"Secure team"}),
    )
    .await;
    run(
        &mut device.client,
        "sync.auto",
        json!({"enabled":true,"consent":true}),
    )
    .await;
    let saved = std::fs::read_to_string(&device.file).unwrap();
    let value: Value = serde_json::from_str(&saved).unwrap();
    let cipher = STANDARD
        .decode(value["tokenCipher"].as_str().unwrap())
        .unwrap();
    let token = TestSecure.unprotect(&cipher).unwrap();
    assert!(!saved.contains(&token));
    let mut restored = CloudClient::with_secure_storage(
        Some(device.file.clone()),
        device.store.clone(),
        Arc::new(TestSecure),
    )
    .unwrap();
    assert_eq!(restored.status()["authenticated"], true);
    assert_eq!(restored.status()["autoSync"], true);
    assert_eq!(
        run(&mut restored, "team.members", json!({})).await["result"]["members"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    run(&mut restored, "logout", json!({})).await;
    let empty = CloudClient::with_secure_storage(
        Some(device.file.clone()),
        device.store.clone(),
        Arc::new(TestSecure),
    )
    .unwrap();
    assert_eq!(empty.status()["authenticated"], false);
    assert!(!std::fs::read_to_string(device.file)
        .unwrap()
        .contains("tokenCipher"));
}

#[derive(Default)]
struct ProviderAudit(Mutex<Vec<rize_service::providers::Request>>);
#[async_trait::async_trait]
impl rize_service::providers::Transport for ProviderAudit {
    async fn send(
        &self,
        request: rize_service::providers::Request,
    ) -> rize_service::ApiResult<Value> {
        let url = request.url.clone();
        self.0.lock().unwrap().push(request);
        if url == "https://oauth2.googleapis.com/token" {
            Ok(
                json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh","token_type":"Bearer","expires_in":3600}),
            )
        } else {
            Err(rize_service::ApiError::new(
                502,
                "Unexpected synthetic provider request.",
                "TEST_TRANSPORT",
            ))
        }
    }
}
#[tokio::test]
async fn cloud_actions_preserve_oauth_pkce_filters_member_permissions_and_service_crud() {
    let transport = Arc::new(ProviderAudit::default());
    let env = HashMap::from([
        (
            "RIZE_PUBLIC_ORIGIN".into(),
            "https://rize.example.test".into(),
        ),
        ("RIZE_GOOGLE_CLIENT_ID".into(), "synthetic-id".into()),
        (
            "RIZE_GOOGLE_CLIENT_SECRET".into(),
            "synthetic-client-secret".into(),
        ),
        ("RIZE_SECRET_KEY".into(), STANDARD.encode([17u8; 32])),
    ]);
    let mut state = AppState::new(":memory:", env).unwrap();
    state.transport = transport.clone();
    let f = Fixture::with_state(state).await;
    let mut owner = f.device("routing-owner");
    f.authenticate(&mut owner, true).await;
    run(
        &mut owner.client,
        "team.create",
        json!({"name":"Routing team"}),
    )
    .await;
    mutate(
        &owner.store,
        "client.upsert",
        json!({"id":"client","name":"Client","hourlyRate":100}),
    );
    let mut time = entry("owner-time", "approved", "Owner billable work");
    time["clientId"] = json!("client");
    time["billable"] = json!(true);
    mutate(&owner.store, "entry.upsert", time);
    mutate(
        &owner.store,
        "invoice.create",
        json!({"clientId":"client","startDate":"2026-10-01","endDate":"2026-10-31"}),
    );
    sync(&mut owner.client).await;
    let mut finance = f.device("routing-finance");
    f.authenticate_as(&mut finance, true, "synthetic-finance@example.test")
        .await;
    let invite = run(
        &mut owner.client,
        "team.invite",
        json!({"email":"synthetic-finance@example.test","role":"member"}),
    )
    .await;
    run(
        &mut finance.client,
        "team.join",
        json!({"code":invite["result"]["code"]}),
    )
    .await;
    let finance_id = finance.client.status()["user"]["id"].clone();
    let updated = run(
        &mut owner.client,
        "team.member.update",
        json!({"userId":finance_id,"finance":true,"resourcing":true}),
    )
    .await;
    assert_eq!(updated["result"]["member"]["finance"], true);
    sync(&mut finance.client).await;
    let shared = finance.store.lock().unwrap().get_state();
    assert!(shared["entries"].as_array().unwrap().is_empty());
    assert_eq!(shared["business"]["invoices"].as_array().unwrap().len(), 1);
    assert_eq!(
        shared["business"]["invoices"][0]["lines"][0]["entryId"],
        "owner-time"
    );
    let analytics=run(&mut owner.client,"analytics",json!({"startDate":"2026-10-16","endDate":"2026-10-17","timezone":"Asia/Kolkata","capacityHours":32})).await;
    assert_eq!(analytics["result"]["timezone"], "Asia/Kolkata");
    assert!(f
        .hooks
        .audit
        .lock()
        .unwrap()
        .iter()
        .any(|(_, u, _)| u.contains("capacityHours=32") && u.contains("Asia%2FKolkata")));
    let oauth = run(
        &mut owner.client,
        "integrations.oauth.start",
        json!({"provider":"google"}),
    )
    .await;
    let url = url::Url::parse(oauth["result"]["authorizationUrl"].as_str().unwrap()).unwrap();
    let pairs: HashMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        url.origin().ascii_serialization(),
        "https://accounts.google.com"
    );
    assert_eq!(pairs["code_challenge_method"], "S256");
    let response = reqwest::Client::new()
        .get(format!("{}/v1/oauth/google/callback", f.origin))
        .query(&[
            ("state", &pairs["state"]),
            ("code", &"synthetic-code".to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["connected"], true);
    assert!(!body.to_string().contains("synthetic-access"));
    assert!(transport.0.lock().unwrap()[0]
        .form
        .as_ref()
        .unwrap()
        .contains_key("code_verifier"));
    let connected = run(&mut owner.client, "integrations.list", json!({})).await;
    assert!(connected["result"]["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["provider"] == "google" && i["connected"] == true));
    assert_eq!(
        run(
            &mut owner.client,
            "integrations.disconnect",
            json!({"provider":"google"})
        )
        .await["result"]["disconnected"],
        true
    );
    let skill = run(
        &mut owner.client,
        "ai.skills.save",
        json!({"name":"Weekly review","instructions":"Summarize approved work only."}),
    )
    .await["result"]["skill"]
        .clone();
    assert_eq!(
        run(&mut owner.client, "ai.skills.list", json!({})).await["result"]["skills"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let routine=run(&mut owner.client,"ai.routines.save",json!({"name":"Review","prompt":"Summarize my work","skillId":skill["id"],"intervalMinutes":60,"enabled":false})).await["result"]["routine"].clone();
    assert_eq!(
        run(&mut owner.client, "ai.routines.list", json!({})).await["result"]["routines"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        run(
            &mut owner.client,
            "ai.routines.delete",
            json!({"id":routine["id"]})
        )
        .await["result"]["deleted"],
        true
    );
    assert_eq!(
        run(
            &mut owner.client,
            "ai.skills.delete",
            json!({"id":skill["id"]})
        )
        .await["result"]["deleted"],
        true
    );
    assert!(owner
        .client
        .execute(
            "ai.chat",
            json!({"message":"Summarize my work","consent":false})
        )
        .await
        .is_err());
    assert_eq!(
        run(
            &mut owner.client,
            "team.member.remove",
            json!({"userId":finance_id})
        )
        .await["result"]["ok"],
        true
    );
    assert!(
        run(&mut finance.client, "teams", json!({})).await["result"]["teams"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
