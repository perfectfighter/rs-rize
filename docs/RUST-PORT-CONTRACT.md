# Rust migration contract

Port the published v1.2 baseline `98f56dba6138280b64a7ea57c02b91b73a268b8a`
from `perfectfighter/rize-local-public`. The reference is checked out at
`../rize-v1.2`. No new product features or redesign. Retain the React interface,
replace Electron/Node application and service logic with Rust, and preserve schema
version 1 and existing HTTP/renderer contracts.

## Ownership

- Root: workspace manifests, `src-tauri`, frontend bridge, npm/build configuration,
  GitHub publication, final docs and integration fixes.
- State agent: `crates/rize-core/src/state.rs`, `timer.rs`, tests for these modules.
- Billing agent: `crates/rize-core/src/business.rs`, accounting tests.
- Native agent: `crates/rize-core/src/tracking.rs`, `crates/rize-native` sources/tests.
- Accounts agent: `crates/rize-service/src/lib.rs`, `main.rs`, `auth.rs`, `db.rs`,
  `workspace.rs`, account/sync HTTP tests.
- Providers agent: service `providers.rs`, `services.rs`, `calendar.rs`, `mcp.rs`,
  provider tests and configuration notes. Coordinate Context with accounts agent.
- Cloud agent: `crates/rize-cloud` Rust sources and integration tests.

Only root edits manifests. Request extra dependencies; avoid parallel Cargo.lock
edits. Work on owned files, coordinate interfaces, do not commit or publish.

## Shared conventions

Use serde_json::Value for the existing JSON boundaries, idiomatic typed internal
structures where useful. Core `pub type Result<T> = std::result::Result<T,String>`.
Errors must be actionable, preserve drafts/data, and never echo credentials.
Timestamp strings ISO UTC with millisecond precision; injected now_ms i64 permits
deterministic tests. Preserve exact intervals and existing currency minor units.
Never replace unsupported functionality with a fabricated successful result.

Core exports modules `state`, `timer`, `business`, `tracking` from lib.rs (root).
State API (coordinate adjustments early):

```
Store::new(file: Option<PathBuf>) -> Result<Store>
Store::from_value(value: Value, file: Option<PathBuf>) -> Result<Store>
Store::get_state(&self) -> Value
Store::mutate(&mut self, command: &Value, now_ms: i64) -> Result<Value>
Store::flush(&mut self, now_ms: i64) -> Result<()>
Store::export_snapshot(&self, now_ms: i64) -> Result<Value>
Store::import_data(&mut self, value: Value, now_ms: i64) -> Result<Value>
Store::merge_shared(&mut self, records: &[Value], now_ms: i64) -> Result<Value>
Store::upsert_activity(&mut self, entries: &[Value], now_ms: i64) -> Result<Value>
state::default_workspace(now_ms: i64) -> Value
state::validate_snapshot(value: &Value) -> Result<Value>
state::to_csv(state: &Value) -> String
```

Business API:

```
default_business() -> Value
validate_business(value: &Value, state: &Value) -> Result<Value>
apply_business_command(state: &mut Value, command: &Value, now_ms:i64) -> Result<bool>
financial_summary(state:&Value, query:&Value) -> Result<Value>
invoice_html(invoice:&Value) -> Result<String>
validate_invoice(invoice:&Value) -> Result<Value>
assert_invoice_transition(old:Option<&Value>, new:Option<&Value>) -> Result<()>
assert_invoice_entries(invoice:&Value, state:&Value) -> Result<()>
```

State delegates business commands and entry split/merge/bulk/regenerate to business.
Business and state can call public helpers across modules; coordinate validation.
Tracking API `TrackingEngine::default()`, `consume(&mut self,sample:&Value,state:&Value)
-> Vec<Value>`, `flush(&mut self,timestamp:&str,state:&Value)->Vec<Value>`, `reset()`.
Native sampler returns only allowed sanitized foreground metadata and idle seconds.

## Service

Axum/Tokio HTTP service and rusqlite durable database. All existing `/v1` routes,
authorization/privacy constraints, byte limits, pagination, optimistic versions,
financial invariants, scheduler, OAuth, and configuration gates retained.
Core server API `AppState::new(db_path:&str,env:HashMap<String,String>) -> ApiResult<AppState>`
and `router(state:AppState) -> axum::Router`. State cloneable, DB Arc<Mutex<Connection>>.
`ApiError { pub status:u16, pub code:String, pub message:String }` implements IntoResponse.
Alias `ApiResult<T>`; functions validate errors with same HTTP codes as baseline.

Accounts and providers coordinate owned Context interface immediately. Proposed
cloneable Context has AppState, user:Value, team_id:String, member:Value. Methods:
authorize(action), records(kind)->Vec<Value>, put_record(kind,id,data,base_version),
delete_record(kind,id,base_version), kv_get(namespace,key), kv_put(namespace,key,value),
now_ms(), env(key). No DB lock held over outbound provider awaits. Providers handler
`handle(ctx:Context,method:&str,path:&str,body:Value,query:HashMap<String,String>)
-> ApiResult<Option<Value>>`. Public OAuth callbacks and routine ticks exposed for
accounts router. Transport injectable for tests; provider-owned trait+real reqwest
implementation, coordinate shared `AppState.transport:Arc<dyn Transport>`.

## Cloud client

Rust client contains all existing desktop cloud actions (including OAuth start,
analytics query filters, CRUD reports/skills/routines, sync). Store is supplied
through `Arc<Mutex<Store>>` but no lock held over awaits. API:

```
CloudClient::new(file:Option<PathBuf>, store:Arc<Mutex<Store>>) -> Result<CloudClient>
CloudClient::status(&self) -> Value
CloudClient::pause_sync(&mut self) -> Result<()>
CloudClient::execute(&mut self, action:&str,payload:Value) -> async Result<Value>
```

Persistence of tokens only through Windows DPAPI (native module helper) or an
injected secure-store abstraction; otherwise session-only, never plaintext.
Preserve conflict review, byte-sized sync batches, cursor paging, workspace/account/
team binding, reset cancellation, private pending-ID safety and no implicit consent.
Frontend continues window.rize narrow API; root bridge maps to Tauri commands/events.

## Verification and publishing

Prefer meaningful Rust unit/integration tests, actual HTTP service tests and
cross-language golden parity fixtures derived from baseline. Existing pure React
DOM tests may stay for interface regressions. Do not ship old Node/Electron backend
as a runtime fallback. No credentials or test user data in commits. User authorized
new public repository `perfectfighter/rs-rize`; original remains untouched.
