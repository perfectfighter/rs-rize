//! Native desktop connection and reviewed-work synchronization.
//!
//! Authentication is kept in memory unless the operating system can protect it.
//! Only approved work metadata crosses this boundary; raw foreground activity
//! never enters the shared-record representation.

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, SecondsFormat, Utc};
use rize_core::state::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub type Result<T> = std::result::Result<T, String>;
const KINDS: &[&str] = &[
    "clients",
    "projects",
    "tasks",
    "entries",
    "contracts",
    "allocations",
    "invoices",
];
const MAX_CONFIG: usize = 40 * 1024 * 1024;
const MAX_RESPONSE: usize = 12 * 1024 * 1024;
pub const MAX_SYNC_BODY: usize = 1024 * 1024;

/// Secure storage can be injected for deterministic tests. Production uses
/// current-user Windows DPAPI and has no plaintext fallback.
pub trait SecureStorage: Send + Sync {
    fn available(&self) -> bool;
    fn protect(&self, secret: &str) -> Result<Vec<u8>>;
    fn unprotect(&self, protected: &[u8]) -> Result<String>;
}

struct NativeSecureStorage;
impl SecureStorage for NativeSecureStorage {
    fn available(&self) -> bool {
        rize_native::secure_storage_available()
    }
    fn protect(&self, secret: &str) -> Result<Vec<u8>> {
        rize_native::protect_secret(secret)
    }
    fn unprotect(&self, protected: &[u8]) -> Result<String> {
        rize_native::unprotect_secret(protected)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SyncMeta {
    cursor: u64,
    versions: BTreeMap<String, u64>,
    baselines: BTreeMap<String, Option<String>>,
    owners: BTreeMap<String, Option<String>>,
    conflicts: Vec<Value>,
    last_sync_at: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Config {
    version: u32,
    server_url: String,
    user: Option<Value>,
    teams: Vec<Value>,
    team_id: String,
    auto_sync: bool,
    team_sync: BTreeMap<String, SyncMeta>,
    workspace_teams: BTreeMap<String, String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            server_url: String::new(),
            user: None,
            teams: vec![],
            team_id: String::new(),
            auto_sync: false,
            team_sync: BTreeMap::new(),
            workspace_teams: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OperationTicket {
    generation: u64,
    workspace_id: String,
}

/// Kept outside the shell's async client mutex. Reset must cancel here before
/// waiting for that mutex, so an in-flight HTTP response cannot mutate a reset
/// workspace. Capture a ticket before queueing commands behind the mutex too.
#[derive(Clone)]
pub struct CloudCancellation {
    generation: Arc<AtomicU64>,
    store: Arc<Mutex<Store>>,
}
impl CloudCancellation {
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    pub fn ticket(&self) -> Result<OperationTicket> {
        let generation = self.generation.load(Ordering::SeqCst);
        let state = self
            .store
            .lock()
            .map_err(|_| "Workspace lock is unavailable.")?
            .get_state();
        Ok(OperationTicket {
            generation,
            workspace_id: text(&state, "workspaceId").to_owned(),
        })
    }
}

pub struct CloudClient {
    file: Option<PathBuf>,
    store: Arc<Mutex<Store>>,
    config: Config,
    token: String,
    storage_message: String,
    secure: Arc<dyn SecureStorage>,
    client: reqwest::Client,
    cancellation: CloudCancellation,
    consent_generation: u64,
}

pub fn server_url(value: &str) -> Result<String> {
    let message =
        "Use an HTTPS server origin, or HTTP on localhost. Do not include credentials or a path.";
    if value.len() > 2000 || value.is_empty() {
        return Err("Enter a server URL.".into());
    }
    let url = url::Url::parse(value).map_err(|_| message.to_string())?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || url.host_str().is_none()
    {
        return Err(message.into());
    }
    Ok(url.origin().ascii_serialization())
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
fn truth(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(items) => Value::Object(
            items
                .iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect(),
        ),
        Value::Number(number) if number.is_f64() => {
            let number = number.as_f64().unwrap();
            if number.fract() == 0.0 && number >= i64::MIN as f64 && number < i64::MAX as f64 {
                json!(number as i64)
            } else {
                value.clone()
            }
        }
        other => other.clone(),
    }
}
fn encoded(value: &Value) -> String {
    serde_json::to_string(&canonical(value)).expect("JSON serialization cannot fail")
}
fn key_of(record: &Value) -> String {
    format!("{}:{}", text(record, "kind"), text(record, "id"))
}
fn timestamp_ms(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
}

pub fn work_data(kind: &str, data: &Value, id: &str) -> Value {
    if kind == "entries" {
        let duration = timestamp_ms(&data["end"])
            .zip(timestamp_ms(&data["start"]))
            .map(|(end, start)| end - start);
        let source = match text(data, "source") {
            "manual" => "manual",
            "automatic" => "automatic",
            "timer" => "timer",
            _ => "manual",
        };
        json!({"id":id,"start":data["start"],"end":data["end"],"description":text(data,"description"),"durationMs":duration,
            "category":if text(data,"category").is_empty(){"neutral"}else{text(data,"category")},"status":"approved","source":source,
            "clientId":text(data,"clientId"),"projectId":text(data,"projectId"),"taskId":text(data,"taskId"),"billable":truth(data,"billable")})
    } else {
        let mut value = data.as_object().cloned().unwrap_or_default();
        value.insert("id".into(), json!(id));
        Value::Object(value)
    }
}

pub fn local_records(state: &Value) -> BTreeMap<String, Value> {
    let mut records = BTreeMap::new();
    for kind in KINDS {
        let list = if matches!(*kind, "clients" | "projects" | "tasks" | "entries") {
            &state[*kind]
        } else {
            &state["business"][*kind]
        };
        for item in list.as_array().into_iter().flatten() {
            if *kind == "entries"
                && (text(item, "status") != "approved" || text(item, "category") == "break")
            {
                continue;
            }
            let id = text(item, "id");
            let record = json!({"kind":kind,"id":id,"data":work_data(kind,item,id)});
            records.insert(key_of(&record), record);
        }
    }
    records
}

pub fn sync_batches(changes: &[Value]) -> Result<Vec<Vec<Value>>> {
    let overhead = b"{\"changes\":[]}".len();
    let (mut batches, mut batch, mut bytes) = (vec![], vec![], overhead);
    for change in changes {
        let size = serde_json::to_vec(change).map_err(|e| e.to_string())?.len();
        if size + overhead > MAX_SYNC_BODY {
            return Err("One shared record exceeds the server request limit. Reduce its size before synchronizing.".into());
        }
        if !batch.is_empty() && (batch.len() >= 250 || bytes + size + 1 > MAX_SYNC_BODY) {
            batches.push(batch);
            batch = vec![];
            bytes = overhead;
        }
        bytes += size + usize::from(!batch.is_empty());
        batch.push(change.clone());
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    Ok(batches)
}

impl CloudClient {
    pub fn new(file: Option<PathBuf>, store: Arc<Mutex<Store>>) -> Result<Self> {
        Self::with_secure_storage(file, store, Arc::new(NativeSecureStorage))
    }
    pub fn with_secure_storage(
        file: Option<PathBuf>,
        store: Arc<Mutex<Store>>,
        secure: Arc<dyn SecureStorage>,
    ) -> Result<Self> {
        let cancellation = CloudCancellation {
            generation: Arc::new(AtomicU64::new(0)),
            store: store.clone(),
        };
        let mut client = Self {
            file,
            store,
            config: Config::default(),
            token: String::new(),
            storage_message: String::new(),
            secure,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|_| "Could not initialize the server connection.")?,
            cancellation,
            consent_generation: 0,
        };
        if client.file.as_ref().is_some_and(|f| f.exists()) {
            if client.restore().is_err() {
                client.config = Config::default();
                client.token.clear();
                client.storage_message="The cloud connection could not be restored. Configure it again; local work is preserved.".into();
            }
        }
        Ok(client)
    }
    fn restore(&mut self) -> Result<()> {
        let file = self.file.as_ref().unwrap();
        if fs::metadata(file).map_err(|e| e.to_string())?.len() > MAX_CONFIG as u64 {
            return Err("Cloud configuration is oversized.".into());
        }
        let bytes = fs::read(file).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_CONFIG {
            return Err("Cloud configuration is oversized.".into());
        }
        let saved: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if saved["version"].as_u64() != Some(1) {
            return Err("Unsupported cloud configuration version.".into());
        }
        let mut config: Config =
            serde_json::from_value(saved.clone()).map_err(|e| e.to_string())?;
        if !config.server_url.is_empty() {
            config.server_url = server_url(&config.server_url)?;
        }
        // Existing schema-1 baselines were serialized by JSON.stringify. Parse
        // their JSON once so harmless float/exponent formatting differences do
        // not turn a Rust migration into a false simultaneous-edit conflict.
        for meta in config.team_sync.values_mut() {
            for baseline in meta.baselines.values_mut() {
                if let Some(saved) = baseline {
                    if let Ok(value) = serde_json::from_str::<Value>(saved) {
                        *saved = encoded(&value);
                    }
                }
            }
        }
        if let Some(cipher) = saved["tokenCipher"].as_str() {
            if self.secure.available() {
                match STANDARD
                    .decode(cipher)
                    .map_err(|_| "Invalid protected credentials.".to_owned())
                    .and_then(|bytes| self.secure.unprotect(&bytes))
                {
                    Ok(token) => self.token = token,
                    Err(_) => {
                        self.storage_message =
                            "Sign in again: the saved session could not be unlocked on this device."
                                .into()
                    }
                }
            } else {
                self.storage_message =
                    "Sign in again: secure credential storage is unavailable.".into();
            }
        }
        self.config = config;
        Ok(())
    }
    pub fn cancel_handle(&self) -> CloudCancellation {
        self.cancellation.clone()
    }
    pub fn begin_operation(&self) -> Result<OperationTicket> {
        self.cancellation.ticket()
    }
    fn state(&self) -> Result<Value> {
        Ok(self
            .store
            .lock()
            .map_err(|_| "Workspace lock is unavailable.")?
            .get_state())
    }
    fn scope_for(&self, workspace_id: &str) -> String {
        format!(
            "{}:{}:{}",
            workspace_id,
            self.config
                .user
                .as_ref()
                .map(|v| text(v, "id"))
                .unwrap_or(""),
            self.config.team_id
        )
    }
    fn scope(&self) -> String {
        self.scope_for(
            &self
                .state()
                .ok()
                .map(|s| text(&s, "workspaceId").to_owned())
                .unwrap_or_default(),
        )
    }
    pub fn status(&self) -> Value {
        let current = self.config.team_sync.get(&self.scope());
        let authenticated = !self.token.is_empty();
        json!({"serverUrl":self.config.server_url,"configured":!self.config.server_url.is_empty(),"authenticated":authenticated,
            "user":if authenticated {self.config.user.clone()} else {None},"teams":if authenticated {self.config.teams.clone()} else {vec![]},"teamId":if authenticated {&self.config.team_id}else{""},
            "autoSync":authenticated && self.config.auto_sync && self.consent_generation==self.cancellation.generation.load(Ordering::SeqCst),
            "lastSyncAt":current.and_then(|c|c.last_sync_at.clone()),"conflicts":current.map(|c|c.conflicts.clone()).unwrap_or_default(),
            "credentialStorage":if self.secure.available(){"encrypted"}else{"session-only"},"message":self.storage_message})
    }
    fn persist(&self) -> Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut saved = serde_json::to_value(&self.config).map_err(|e| e.to_string())?;
        if !self.token.is_empty() && self.secure.available() {
            saved["tokenCipher"] = json!(STANDARD.encode(self.secure.protect(&self.token)?));
        }
        let serialized = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
        if serialized.len() > MAX_CONFIG {
            return Err("Cloud synchronization metadata exceeds 40 MiB. Export a backup and use a separate workspace before sharing more records.".into());
        }
        if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not save cloud settings: {e}"))?;
        }
        let temp = file.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(&temp)?;
            output.write_all(&serialized)?;
            output.sync_all()?;
            drop(output);
            fs::rename(&temp, file)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(temp);
            return Err(format!("Could not save cloud settings: {error}"));
        }
        Ok(())
    }
    pub fn pause_sync(&mut self) -> Result<()> {
        self.cancellation.cancel();
        self.config.auto_sync = false;
        self.persist()
    }
    fn guard(&self, ticket: &OperationTicket) -> Result<()> {
        if ticket.generation != self.cancellation.generation.load(Ordering::SeqCst)
            || text(&self.state()?, "workspaceId") != ticket.workspace_id
        {
            return Err("The workspace changed during synchronization. Start sync again in the current workspace.".into());
        }
        Ok(())
    }
    async fn request(
        &mut self,
        method: &str,
        route: &str,
        body: Option<&Value>,
        authenticated: bool,
    ) -> Result<Value> {
        if self.config.server_url.is_empty() {
            return Err("Configure your Rize Local server first.".into());
        }
        if authenticated && self.token.is_empty() {
            return Err("Sign in to your server first.".into());
        }
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "Invalid HTTP method.")?;
        let mut request = self
            .client
            .request(method, format!("{}{route}", self.config.server_url))
            .header("Accept", "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        if authenticated {
            request = request.bearer_auth(&self.token);
        }
        let mut response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "The server request timed out.".to_string()
            } else {
                "Could not connect to the server. Check the server address and connection."
                    .to_string()
            }
        })?;
        if response.status().is_redirection() {
            return Err(
                "The server redirected the request. Configure the final server origin directly."
                    .into(),
            );
        }
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err("Server response exceeds the workspace size limit.".into());
        }
        let mut raw = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            if e.is_timeout() {
                "The server request timed out."
            } else {
                "Could not read the server response."
            }
        })? {
            if raw.len() + chunk.len() > MAX_RESPONSE {
                return Err("Server response exceeds the workspace size limit.".into());
            }
            raw.extend_from_slice(&chunk);
        }
        let result: Value = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).map_err(|_| "The server returned an invalid response.")?
        };
        if !status.is_success() {
            if status.as_u16() == 401 && authenticated {
                self.token.clear();
                self.config.auto_sync = false;
                self.persist()?;
            }
            return Err(result["error"]
                .as_str()
                .map(|s| s.chars().take(500).collect())
                .unwrap_or_else(|| format!("Server request failed ({}).", status.as_u16())));
        }
        Ok(result)
    }
    fn team_route(&self, suffix: &str) -> Result<String> {
        if self.config.team_id.is_empty() {
            return Err("Select or create a team first.".into());
        }
        Ok(format!(
            "/v1/teams/{}{suffix}",
            component(&self.config.team_id)
        ))
    }
    async fn refresh_teams(&mut self) -> Result<Value> {
        let data = self.request("GET", "/v1/teams", None, true).await?;
        self.config.teams = data["teams"].as_array().cloned().unwrap_or_default();
        if !self.config.team_id.is_empty()
            && !self
                .config
                .teams
                .iter()
                .any(|t| text(t, "id") == self.config.team_id)
        {
            self.config.team_id.clear();
            self.config.auto_sync = false;
        }
        self.persist()?;
        Ok(data)
    }
    fn eligible(&self, kind: &str, record: &Value, current: &SyncMeta) -> bool {
        let team = self
            .config
            .teams
            .iter()
            .find(|v| text(v, "id") == self.config.team_id)
            .unwrap_or(&Value::Null);
        let role = if !text(team, "role").is_empty() {
            text(team, "role")
        } else if !text(&team["membership"], "role").is_empty() {
            text(&team["membership"], "role")
        } else {
            "viewer"
        };
        if role == "viewer" {
            return false;
        }
        if matches!(kind, "contracts" | "invoices") {
            return role == "owner"
                || truth(team, "finance")
                || truth(&team["membership"], "finance");
        }
        if kind == "allocations" {
            return role == "owner"
                || truth(team, "resourcing")
                || truth(&team["membership"], "resourcing");
        }
        if matches!(role, "owner" | "admin") {
            return true;
        }
        if kind == "entries" {
            return current
                .owners
                .get(&key_of(record))
                .and_then(|v| v.as_deref())
                .is_none_or(|owner| {
                    Some(owner) == self.config.user.as_ref().map(|u| text(u, "id"))
                });
        }
        false
    }
    fn merge(&self, records: &[Value], ticket: &OperationTicket) -> Result<Value> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Workspace lock is unavailable.")?;
        if ticket.generation != self.cancellation.generation.load(Ordering::SeqCst)
            || text(&store.get_state(), "workspaceId") != ticket.workspace_id
        {
            return Err("The workspace changed during synchronization. Start sync again in the current workspace.".into());
        }
        store.merge_shared(records, Utc::now().timestamp_millis())
    }
    async fn pull(
        &mut self,
        current: &mut SyncMeta,
        ticket: &OperationTicket,
        applied: &mut usize,
    ) -> Result<()> {
        let mut pages = 0;
        loop {
            let before = current.cursor;
            let route = self.team_route(&format!("/sync?since={}", current.cursor))?;
            let response = self.request("GET", &route, None, true).await?;
            self.guard(ticket)?;
            let state = self.state()?;
            let locals = local_records(&state);
            let private = private_ids(&state);
            let mut remote_records: BTreeMap<String, Value> = BTreeMap::new();
            for raw in response["records"].as_array().into_iter().flatten() {
                if let Some(record) = normalize_remote(raw) {
                    let key = key_of(&record);
                    if remote_records
                        .get(&key)
                        .is_none_or(|old| version(&record) > version(old))
                    {
                        remote_records.insert(key, record);
                    }
                }
            }
            let (mut accepted, mut metadata, mut conflicts) = (vec![], vec![], vec![]);
            for (key, remote) in remote_records {
                let local = locals.get(&key);
                let previous = current.baselines.get(&key);
                if let Some(existing) = current.conflicts.iter().find(|c| key_of(c) == key) {
                    if version(&remote) > version(&existing["remote"]) {
                        let mut conflict = existing.clone();
                        conflict["remote"] = remote;
                        conflicts.push(conflict);
                    }
                    continue;
                }
                if text(&remote, "kind") == "entries" && private.contains(text(&remote, "id")) {
                    conflicts.push(json!({"kind":remote["kind"],"id":remote["id"],"local":null,"remote":remote,"privateLocal":true,"reason":"An unreviewed or private local entry uses this ID. Review or rename it before resolving this collision."}));
                    continue;
                }
                let local_text = local.map(|l| encoded(&l["data"]));
                let remote_text = if truth(&remote, "deleted") {
                    None
                } else {
                    Some(encoded(&remote["data"]))
                };
                let local_changed = if let Some(previous) = previous {
                    &local_text != previous
                } else {
                    local.is_some()
                };
                if version(&remote) > *current.versions.get(&key).unwrap_or(&0)
                    && local_changed
                    && local_text != remote_text
                {
                    conflicts.push(json!({"kind":remote["kind"],"id":remote["id"],"local":local.map(|l|l["data"].clone()),"remote":remote,"reason":"Both this device and the server changed this record."}));
                    continue;
                }
                if version(&remote) <= *current.versions.get(&key).unwrap_or(&0) {
                    continue;
                }
                if !truth(&remote, "deleted") || previous.is_some() {
                    accepted.push(remote.clone());
                }
                metadata.push((key, remote, remote_text));
            }
            if !accepted.is_empty() {
                self.merge(&accepted, ticket)?;
                *applied += accepted.len();
            }
            for conflict in conflicts {
                let key = key_of(&conflict);
                if let Some(index) = current.conflicts.iter().position(|c| key_of(c) == key) {
                    current.conflicts[index] = conflict;
                } else {
                    current.conflicts.push(conflict);
                }
            }
            for (key, remote, remote_text) in metadata {
                current.versions.insert(key.clone(), version(&remote));
                current.baselines.insert(key.clone(), remote_text);
                current
                    .owners
                    .insert(key, remote["ownerId"].as_str().map(str::to_owned));
            }
            current.cursor = current.cursor.max(response["cursor"].as_u64().unwrap_or(0));
            self.config
                .team_sync
                .insert(self.scope_for(&ticket.workspace_id), current.clone());
            if !truth(&response, "hasMore") {
                break;
            }
            pages += 1;
            if current.cursor <= before || pages > 1000 {
                return Err("The server returned an invalid synchronization cursor.".into());
            }
        }
        Ok(())
    }
    async fn sync(&mut self, payload: &Value, ticket: &OperationTicket) -> Result<(Value, Value)> {
        if !truth(payload, "consent") {
            return Err(
                "Approve synchronization of reviewed time and shared work records first.".into(),
            );
        }
        if truth(&self.state()?, "demo") {
            return Err("Create a real workspace before synchronizing.".into());
        }
        self.guard(ticket)?;
        self.refresh_teams().await?;
        self.guard(ticket)?;
        if self.config.team_id.is_empty() {
            return Err("Select or create a team first.".into());
        }
        let workspace_id = text(&self.state()?, "workspaceId").to_owned();
        let target = format!(
            "{}:{}:{}",
            self.config.server_url,
            self.config
                .user
                .as_ref()
                .map(|u| text(u, "id"))
                .unwrap_or(""),
            self.config.team_id
        );
        if self
            .config
            .workspace_teams
            .get(&workspace_id)
            .is_some_and(|binding| binding != &target)
        {
            return Err("This workspace is synchronized with another account or team. Create a separate workspace before sharing here.".into());
        }
        self.config.workspace_teams.insert(workspace_id, target);
        let scope = self.scope();
        let mut current = self
            .config
            .team_sync
            .get(&scope)
            .cloned()
            .unwrap_or_default();
        let (mut applied, mut pushed, mut skipped) = (0usize, 0usize, vec![]);
        for resolution in payload["resolutions"].as_array().into_iter().flatten() {
            let Some(index) = current
                .conflicts
                .iter()
                .position(|c| c["kind"] == resolution["kind"] && c["id"] == resolution["id"])
            else {
                continue;
            };
            let conflict = current.conflicts[index].clone();
            let strategy = text(resolution, "strategy");
            if !matches!(strategy, "local" | "remote") {
                return Err("Choose the local or remote conflict version.".into());
            }
            if text(&conflict, "kind") == "entries"
                && private_ids(&self.state()?).contains(text(&conflict, "id"))
            {
                return Err(
                    "Review or rename the private local entry before resolving this ID collision."
                        .into(),
                );
            }
            if conflict["remote"].is_object() {
                if strategy == "remote" {
                    self.merge(&[conflict["remote"].clone()], ticket)?;
                    applied += 1;
                }
                let remote = &conflict["remote"];
                let key = key_of(remote);
                current.versions.insert(key.clone(), version(remote));
                current
                    .owners
                    .insert(key.clone(), remote["ownerId"].as_str().map(str::to_owned));
                current.baselines.insert(
                    key,
                    if truth(remote, "deleted") {
                        None
                    } else {
                        Some(encoded(&remote["data"]))
                    },
                );
            }
            current.conflicts.remove(index);
        }
        self.config.team_sync.insert(scope.clone(), current.clone());
        self.pull(&mut current, ticket, &mut applied).await?;
        let state = self.state()?;
        let locals = local_records(&state);
        let private = private_ids(&state);
        let blocked: BTreeSet<String> = current.conflicts.iter().map(key_of).collect();
        let mut pending = vec![];
        // Preserve the baseline's dependency order, even though the lookup map
        // is sorted: referenced clients/projects/tasks must precede entries.
        for kind in KINDS {
            for (key, record) in locals
                .iter()
                .filter(|(_, record)| text(record, "kind") == *kind)
            {
                if blocked.contains(key)
                    || current
                        .baselines
                        .get(key)
                        .is_some_and(|b| b.as_ref() == Some(&encoded(&record["data"])))
                {
                    continue;
                }
                if !self.eligible(text(record, "kind"), record, &current) {
                    skipped.push(json!({"kind":record["kind"],"id":record["id"],"reason":"Your team role cannot publish changes to this record."}));
                    continue;
                }
                let mut change = record.clone();
                change["baseVersion"] = json!(current.versions.get(key).copied().unwrap_or(0));
                pending.push(change);
            }
        }
        for (key, baseline) in &current.baselines {
            if locals.contains_key(key) || blocked.contains(key) || baseline.is_none() {
                continue;
            }
            let Some((kind, id)) = key.split_once(':') else {
                continue;
            };
            if kind == "entries" && private.contains(id) {
                continue;
            }
            let record = json!({"kind":kind,"id":id});
            if KINDS.contains(&kind) && self.eligible(kind, &record, &current) {
                pending.push(json!({"kind":kind,"id":id,"baseVersion":current.versions.get(key),"deleted":true}));
            }
        }
        for changes in sync_batches(&pending)? {
            self.guard(ticket)?;
            let route = self.team_route("/sync")?;
            let response = self
                .request("POST", &route, Some(&json!({"changes":changes})), true)
                .await?;
            self.guard(ticket)?;
            for raw in response["records"].as_array().into_iter().flatten() {
                let Some(remote) = normalize_remote(raw) else {
                    continue;
                };
                let key = key_of(&remote);
                current.versions.insert(key.clone(), version(&remote));
                current.baselines.insert(
                    key.clone(),
                    if truth(&remote, "deleted") {
                        None
                    } else {
                        Some(encoded(&remote["data"]))
                    },
                );
                current
                    .owners
                    .insert(key, remote["ownerId"].as_str().map(str::to_owned));
                pushed += 1;
            }
            for conflict in response["conflicts"].as_array().into_iter().flatten() {
                let key = key_of(conflict);
                let remote = normalize_remote(if conflict["current"].is_object() {
                    &conflict["current"]
                } else {
                    &conflict["remote"]
                });
                current.conflicts.push(json!({"kind":conflict["kind"],"id":conflict["id"],"local":locals.get(&key).map(|l|l["data"].clone()),"remote":remote,"reason":conflict["error"].as_str().or(conflict["message"].as_str()).unwrap_or("The server has a different record version.")}));
            }
            self.config.team_sync.insert(scope.clone(), current.clone());
            // POST cursors do not acknowledge unseen concurrent team changes.
        }
        self.pull(&mut current, ticket, &mut applied).await?;
        self.guard(ticket)?;
        current.last_sync_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        self.config.team_sync.insert(scope, current.clone());
        self.persist()?;
        Ok((
            json!({"pushed":pushed,"applied":applied,"skipped":skipped,"conflicts":current.conflicts,"cursor":current.cursor,"lastSyncAt":current.last_sync_at}),
            self.state()?,
        ))
    }
    pub async fn execute(&mut self, action: &str, payload: Value) -> Result<Value> {
        let ticket = self.begin_operation()?;
        self.execute_with_ticket(action, payload, ticket).await
    }
    pub async fn execute_with_ticket(
        &mut self,
        action: &str,
        payload: Value,
        ticket: OperationTicket,
    ) -> Result<Value> {
        if !payload.is_object() {
            return Err("Invalid cloud command.".into());
        }
        if action == "sync" || (action == "sync.auto" && truth(&payload, "enabled")) {
            if self.guard(&ticket).is_err() {
                return Err("The workspace changed before synchronization could start. Approve sync again in the current workspace.".into());
            }
        }
        let mut workspace = None;
        let result = match action {
            "status" => self.status(),
            "configure" => {
                let next = server_url(text(&payload, "serverUrl"))?;
                if next != self.config.server_url {
                    self.token.clear();
                    self.config.user = None;
                    self.config.teams.clear();
                    self.config.team_id.clear();
                    self.config.auto_sync = false;
                    self.config.team_sync.clear();
                }
                self.config.server_url = next;
                self.storage_message.clear();
                self.persist()?;
                self.status()
            }
            "register" | "login" => {
                let data = self
                    .request("POST", &format!("/v1/auth/{action}"), Some(&payload), false)
                    .await?;
                if !data["token"].is_string() || text(&data["user"], "id").is_empty() {
                    return Err("The server did not return a valid session.".into());
                }
                self.token = text(&data, "token").into();
                self.config.user = Some(data["user"].clone());
                self.storage_message.clear();
                self.refresh_teams().await?;
                json!({"user":data["user"]})
            }
            "logout" => {
                let request = if !self.token.is_empty() {
                    self.request("POST", "/v1/auth/logout", Some(&json!({})), true)
                        .await
                        .map(|_| ())
                } else {
                    Ok(())
                };
                self.token.clear();
                self.config.user = None;
                self.config.teams.clear();
                self.config.team_id.clear();
                self.config.auto_sync = false;
                self.persist()?;
                request?;
                json!({"signedOut":true})
            }
            "teams" => self.refresh_teams().await?,
            "team.create" | "team.join" => {
                let route = if action == "team.create" {
                    "/v1/teams"
                } else {
                    "/v1/teams/join"
                };
                let data = self.request("POST", route, Some(&payload), true).await?;
                self.refresh_teams().await?;
                if !text(&data["team"], "id").is_empty() {
                    self.config.team_id = text(&data["team"], "id").into();
                }
                self.config.auto_sync = false;
                self.persist()?;
                data
            }
            "team.select" => {
                self.refresh_teams().await?;
                if !self
                    .config
                    .teams
                    .iter()
                    .any(|t| t["id"] == payload["teamId"])
                {
                    return Err("This account cannot access that team.".into());
                }
                self.config.team_id = text(&payload, "teamId").into();
                self.config.auto_sync = false;
                self.persist()?;
                self.status()
            }
            "team.members" => {
                let route = self.team_route("/members")?;
                self.request("GET", &route, None, true).await?
            }
            "team.invite" => {
                let route = self.team_route("/invites")?;
                self.request("POST", &route, Some(&payload), true).await?
            }
            "team.member.update" => {
                let route =
                    self.team_route(&format!("/members/{}", component(text(&payload, "userId"))))?;
                let mut patch = payload.clone();
                patch.as_object_mut().unwrap().remove("userId");
                let data = self.request("PATCH", &route, Some(&patch), true).await?;
                self.refresh_teams().await?;
                data
            }
            "team.member.remove" => {
                let route =
                    self.team_route(&format!("/members/{}", component(text(&payload, "userId"))))?;
                let data = self.request("DELETE", &route, None, true).await?;
                self.refresh_teams().await?;
                data
            }
            "analytics" => {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                for key in ["startDate", "endDate", "timezone"] {
                    if let Some(value) = payload[key].as_str() {
                        query.append_pair(key, value);
                    }
                }
                if !payload["capacityHours"].is_null() {
                    query.append_pair(
                        "capacityHours",
                        &payload["capacityHours"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| payload["capacityHours"].to_string()),
                    );
                }
                let query = query.finish();
                let route = self.team_route(&format!(
                    "/analytics{}",
                    if query.is_empty() {
                        String::new()
                    } else {
                        format!("?{query}")
                    }
                ))?;
                self.request("GET", &route, None, true).await?
            }
            "sync" => {
                let (result, state) = self.sync(&payload, &ticket).await?;
                workspace = Some(state);
                result
            }
            "sync.auto" => {
                let enabled = truth(&payload, "enabled");
                if enabled && !truth(&payload, "consent") {
                    return Err("Approve background synchronization first.".into());
                }
                if enabled
                    && (self.token.is_empty()
                        || self.config.team_id.is_empty()
                        || truth(&self.state()?, "demo"))
                {
                    return Err("Select a team in a real workspace first.".into());
                }
                self.config.auto_sync = enabled;
                self.consent_generation = self.cancellation.generation.load(Ordering::SeqCst);
                self.persist()?;
                self.status()
            }
            _ => {
                let (method, suffix) = service_route(action, &payload)?;
                let route = self.team_route(&format!("/services{suffix}"))?;
                self.request(
                    method,
                    &route,
                    if matches!(method, "GET" | "DELETE") {
                        None
                    } else {
                        Some(&payload)
                    },
                    true,
                )
                .await?
            }
        };
        let mut envelope = json!({"cloud":self.status(),"result":result});
        if let Some(workspace) = workspace {
            envelope["workspace"] = workspace;
        }
        Ok(envelope)
    }
}

fn private_ids(state: &Value) -> BTreeSet<String> {
    state["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| text(e, "status") != "approved" || text(e, "category") == "break")
        .map(|e| text(e, "id").to_owned())
        .collect()
}
fn version(record: &Value) -> u64 {
    record["version"].as_u64().unwrap_or(0)
}
fn normalize_remote(record: &Value) -> Option<Value> {
    if !record.is_object()
        || !KINDS.contains(&text(record, "kind"))
        || !record["id"].is_string()
        || version(record) == 0
        || version(record) > 9_007_199_254_740_991
    {
        return None;
    }
    let mut remote = record.clone();
    let deleted = truth(record, "deleted");
    remote["deleted"] = json!(deleted);
    remote["data"] = if deleted {
        Value::Null
    } else {
        work_data(text(record, "kind"), &record["data"], text(record, "id"))
    };
    Some(remote)
}
fn component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
fn service_route(action: &str, payload: &Value) -> Result<(&'static str, String)> {
    let route = match action {
        "services.status" => ("GET", "/status"),
        "ai.chat" => ("POST", "/ai/chat"),
        "ai.tag" => ("POST", "/ai/tag"),
        "ai.report" => ("POST", "/ai/report"),
        "ai.reports.list" => ("GET", "/ai/reports"),
        "ai.reports.save" => ("POST", "/ai/reports"),
        "ai.skills.list" => ("GET", "/ai/skills"),
        "ai.skills.save" => ("POST", "/ai/skills"),
        "ai.routines.list" => ("GET", "/ai/routines"),
        "ai.routines.save" => ("POST", "/ai/routines"),
        "integrations.list" => ("GET", "/integrations"),
        "calendar.import" => ("POST", "/calendar/import"),
        "ai.skills.delete" | "ai.routines.delete" | "ai.reports.delete" => {
            return Ok((
                "DELETE",
                format!(
                    "/ai/{}/{}",
                    action.split('.').nth(1).unwrap(),
                    component(text(payload, "id"))
                ),
            ))
        }
        "integrations.connect" | "integrations.sync" | "integrations.disconnect" => {
            let provider = text(payload, "provider");
            if !matches!(
                provider,
                "linear" | "clickup" | "asana" | "google" | "microsoft"
            ) {
                return Err("Choose a supported integration.".into());
            }
            return Ok((
                if action == "integrations.disconnect" {
                    "DELETE"
                } else {
                    "POST"
                },
                format!(
                    "/integrations/{provider}{}",
                    if action == "integrations.connect" {
                        "/connect"
                    } else if action == "integrations.sync" {
                        "/sync"
                    } else {
                        ""
                    }
                ),
            ));
        }
        "integrations.oauth.start" => {
            let provider = text(payload, "provider");
            if !matches!(provider, "google" | "microsoft") {
                return Err("Choose a supported OAuth provider.".into());
            }
            return Ok(("POST", format!("/integrations/{provider}/oauth/start")));
        }
        _ => return Err(format!("Unknown cloud action: {action}")),
    };
    Ok((route.0, route.1.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_rules() {
        assert_eq!(
            server_url("http://127.0.0.1:4791/").unwrap(),
            "http://127.0.0.1:4791"
        );
        assert_eq!(
            server_url("http://[::1]:4791").unwrap(),
            "http://[::1]:4791"
        );
        for bad in [
            "http://example.test",
            "https://user:pass@example.test",
            "https://example.test/path",
            "https://example.test?x=1",
            "file:///etc/passwd",
        ] {
            assert!(server_url(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn privacy_allowlist() {
        let state = json!({"entries":[{"id":"reviewed","start":"2026-10-17T09:00:00.123Z","end":"2026-10-17T09:30:00.456Z","app":"Secret app","title":"Private title","description":"Reviewed work","status":"approved","category":"productive","source":"automatic"},{"id":"pending","status":"pending","description":"Private activity"},{"id":"break","status":"approved","category":"break"}]});
        let records = local_records(&state);
        assert_eq!(records.len(), 1);
        let encoded = encoded(&json!(records));
        assert!(!encoded.contains("Private"));
        assert!(!encoded.contains("Secret"));
        assert_eq!(records["entries:reviewed"]["data"]["durationMs"], 1_800_333);
    }
    #[test]
    fn unicode_batches_limit_bytes() {
        let changes:Vec<Value>=(0..300).map(|id|json!({"kind":"entries","id":id.to_string(),"data":{"description":"अ".repeat(4000)},"baseVersion":0})).collect();
        let batches = sync_batches(&changes).unwrap();
        assert!(batches.len() > 3);
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), 300);
        assert!(batches.iter().all(|b| b.len() <= 250
            && serde_json::to_vec(&json!({"changes":b})).unwrap().len() <= MAX_SYNC_BODY));
        assert!(sync_batches(&[json!({"description":"x".repeat(MAX_SYNC_BODY)})]).is_err());
    }
    #[test]
    fn service_routes_are_explicit() {
        assert_eq!(
            service_route("integrations.disconnect", &json!({"provider":"google"})).unwrap(),
            ("DELETE", "/integrations/google".into())
        );
        assert_eq!(
            service_route("ai.reports.delete", &json!({"id":"../other"})).unwrap(),
            ("DELETE", "/ai/reports/..%2Fother".into())
        );
        assert!(service_route("integrations.oauth.start", &json!({"provider":"linear"})).is_err());
    }
    #[test]
    fn oversized_and_unknown_configuration_preserve_the_workspace() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let file = root.path().join("cloud.json");
        let store = Arc::new(Mutex::new(Store::new(None).unwrap()));
        let before = store.lock().unwrap().get_state();
        let oversized = std::fs::File::create(&file).unwrap();
        oversized.set_len(MAX_CONFIG as u64 + 1).unwrap();
        drop(oversized);
        let client = CloudClient::new(Some(file.clone()), store.clone()).unwrap();
        assert_eq!(client.status()["authenticated"], false);
        assert!(text(&client.status(), "message").contains("could not be restored"));
        assert_eq!(store.lock().unwrap().get_state(), before);
        std::fs::write(&file,r#"{"version":99,"serverUrl":"https://example.test","token":"never-restore-plaintext"}"#).unwrap();
        let client = CloudClient::new(Some(file), store).unwrap();
        assert_eq!(client.status()["configured"], false);
        assert_eq!(client.status()["authenticated"], false);
    }
    #[test]
    fn queued_ticket_is_invalidated_before_the_store_is_replaced() {
        let store = Arc::new(Mutex::new(Store::new(None).unwrap()));
        let mut client = CloudClient::new(None, store).unwrap();
        let ticket = client.begin_operation().unwrap();
        client.pause_sync().unwrap();
        assert!(client
            .guard(&ticket)
            .unwrap_err()
            .contains("workspace changed"));
    }
    #[test]
    fn numeric_baselines_use_javascript_number_equivalence() {
        assert_eq!(
            encoded(&json!({"rate":0.0,"count":1.0})),
            encoded(&json!({"rate":0,"count":1}))
        );
    }
}
