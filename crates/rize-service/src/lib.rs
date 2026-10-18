pub mod auth;
pub mod calendar;
pub mod db;
pub mod mcp;
pub mod providers;
pub mod services;
pub mod workspace;

use axum::{
    body::{to_bytes, Body},
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use tokio::sync::Semaphore;
use workspace::{authorize, iso};

pub type ApiResult<T> = std::result::Result<T, ApiError>;
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
}
impl ApiError {
    pub fn new(status: u16, message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(400, message, "INVALID_REQUEST")
    }
    pub fn internal() -> Self {
        Self::new(
            500,
            "The server could not complete this request.",
            "INTERNAL_ERROR",
        )
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ApiError {}
impl From<rusqlite::Error> for ApiError {
    fn from(_: rusqlite::Error) -> Self {
        Self::internal()
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error":self.message,"code":self.code})),
        )
            .into_response()
    }
}
#[derive(Clone)]
pub struct AppState {
    pub connection: Arc<Mutex<Connection>>,
    pub env: Arc<HashMap<String, String>>,
    pub transport: Arc<dyn providers::Transport>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    origins: Arc<HashSet<String>>,
    limits: Arc<Mutex<HashMap<String, (usize, i64)>>>,
    auth_slots: Arc<Semaphore>,
    routines_busy: Arc<AtomicBool>,
}
impl AppState {
    pub fn new(db_path: &str, env: HashMap<String, String>) -> ApiResult<Self> {
        let mut origins = HashSet::new();
        for raw in env
            .get("RIZE_ALLOWED_ORIGINS")
            .map(String::as_str)
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            validate_origin(raw,false).map_err(|_|ApiError::bad("RIZE_ALLOWED_ORIGINS must contain exact HTTPS origins, or loopback HTTP origins."))?;
            origins.insert(raw.to_string());
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(db::open(db_path)?)),
            env: Arc::new(env),
            transport: Arc::new(providers::ReqwestTransport::default()),
            clock: Arc::new(|| chrono::Utc::now().timestamp_millis()),
            origins: Arc::new(origins),
            limits: Arc::new(Mutex::new(HashMap::new())),
            auth_slots: Arc::new(Semaphore::new(4)),
            routines_busy: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }
    pub fn db(&self) -> ApiResult<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| ApiError::internal())
    }
    pub fn now_ms(&self) -> i64 {
        (self.clock)()
    }
    fn limited(&self, key: String, limit: usize, window: i64) -> ApiResult<()> {
        let now = self.now_ms();
        let mut limits = self.limits.lock().map_err(|_| ApiError::internal())?;
        if limits.len() > 10_000 {
            limits.retain(|_, v| v.1 > now);
            if limits.len() > 10_000 {
                if let Some(k) = limits.keys().next().cloned() {
                    limits.remove(&k);
                }
            }
        }
        let item = limits.entry(key).or_insert((0, now + window));
        if item.1 <= now {
            *item = (0, now + window);
        }
        item.0 += 1;
        if item.0 > limit {
            return Err(ApiError::new(
                429,
                "Too many requests. Try again later.",
                "RATE_LIMITED",
            ));
        }
        Ok(())
    }
    pub fn context_for(&self, team: &str, user_id: &str) -> ApiResult<Context> {
        let db = self.db()?;
        let user=db.query_row("SELECT id,email,name FROM users WHERE id=?",[user_id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"email":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?}))).optional()?.ok_or_else(||ApiError::new(404,"Account not found.","NOT_FOUND"))?;
        let member = member_for(&db, team, user_id)?;
        Ok(Context {
            state: self.clone(),
            user,
            team_id: team.to_string(),
            member,
        })
    }
    pub async fn tick_services(&self) -> ApiResult<Value> {
        if self.routines_busy.swap(true, Ordering::SeqCst) {
            return Ok(json!({"skipped":true}));
        }
        struct Reset(Arc<AtomicBool>);
        impl Drop for Reset {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _reset = Reset(self.routines_busy.clone());
        let rows = {
            let db = self.db()?;
            let mut s=db.prepare("SELECT t.id,m.user_id FROM teams t JOIN members m ON m.team_id=t.id WHERE m.role!='viewer' ORDER BY m.joined_at,m.user_id")?;
            let rows = s
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let mut results = Vec::new();
        for (team, user) in rows {
            let ctx = self.context_for(&team, &user)?;
            match services::tick_routines(ctx).await{Ok(result)=>results.push(json!({"teamId":team,"userId":user,"result":result})),Err(_)=>results.push(json!({"teamId":team,"userId":user,"error":"Routine execution failed; inspect the routine status."}))}
        }
        Ok(json!({"results":results}))
    }
}
#[derive(Clone)]
pub struct Context {
    pub state: AppState,
    pub user: Value,
    pub team_id: String,
    pub member: Value,
}
impl Context {
    pub fn authorize(&self, action: &str) -> bool {
        authorize(&self.member, action)
    }
    pub fn require(&self, action: &str) -> ApiResult<()> {
        if self.authorize(action) {
            Ok(())
        } else {
            Err(ApiError::new(
                403,
                "Your team role does not permit this action.",
                "FORBIDDEN",
            ))
        }
    }
    pub fn user_id(&self) -> &str {
        self.user["id"].as_str().unwrap_or("")
    }
    pub fn now_ms(&self) -> i64 {
        self.state.now_ms()
    }
    pub fn env(&self, key: &str) -> Option<String> {
        self.state.env.get(key).cloned()
    }
}
pub fn is_loopback(host: &str) -> bool {
    ["localhost", "127.0.0.1", "::1", "[::1]"].contains(&host)
}
pub fn validate_origin(origin: &str, https_only: bool) -> ApiResult<()> {
    let url = reqwest::Url::parse(origin).map_err(|_| ApiError::bad("Invalid origin."))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.origin().ascii_serialization() != origin
        || url.scheme() != "https"
            && (https_only || url.scheme() != "http" || !is_loopback(url.host_str().unwrap_or("")))
    {
        return Err(ApiError::bad(
            "Origin must be an exact HTTPS origin, or an allowed loopback HTTP origin.",
        ));
    }
    Ok(())
}
pub fn validate_binding(host: &str, env: &HashMap<String, String>) -> ApiResult<()> {
    if !is_loopback(host) {
        let origin=env.get("RIZE_PUBLIC_ORIGIN").ok_or_else(||ApiError::bad("External binding requires RIZE_PUBLIC_ORIGIN=https://your-host and a TLS reverse proxy."))?;
        validate_origin(origin, true)
            .map_err(|_| ApiError::bad("RIZE_PUBLIC_ORIGIN must be an exact HTTPS origin."))?;
    }
    Ok(())
}
fn member_for(db: &Connection, team: &str, user: &str) -> ApiResult<Value> {
    db.query_row("SELECT role,finance,resourcing,joined_at FROM members WHERE team_id=? AND user_id=?",params![team,user],|r|Ok(json!({"role":r.get::<_,String>(0)?,"finance":r.get::<_,i64>(1)?!=0,"resourcing":r.get::<_,i64>(2)?!=0,"joinedAt":iso(r.get(3)?),"userId":user}))).optional()?.ok_or_else(||ApiError::new(404,"Team not found.","NOT_FOUND"))
}
fn member_result(db: &Connection, team: &str, user: &str) -> ApiResult<Value> {
    let mut member = member_for(db, team, user)?;
    let (name, email) = db.query_row("SELECT name,email FROM users WHERE id=?", [user], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    member["name"] = json!(name);
    member["email"] = json!(email);
    Ok(member)
}
fn team_result(db: &Connection, team: &str, user: &str) -> ApiResult<Value> {
    Ok(db.query_row("SELECT t.id,t.name,m.role,m.finance,m.resourcing FROM teams t JOIN members m ON m.team_id=t.id WHERE t.id=? AND m.user_id=?",params![team,user],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?,"finance":r.get::<_,i64>(3)?!=0,"resourcing":r.get::<_,i64>(4)?!=0})))?)
}
fn id_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn router(state: AppState) -> Router {
    Router::new().fallback(any(dispatch)).with_state(state)
}
async fn dispatch(State(state): State<AppState>, req: Request<Body>) -> Response {
    let origin = req
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let allowed = origin
        .as_ref()
        .map(|s| state.origins.contains(s))
        .unwrap_or(false);
    let mut response = if origin.is_some() && !allowed {
        ApiError::new(403, "This origin is not allowed.", "ORIGIN_DENIED").into_response()
    } else if req.method() == "OPTIONS" {
        if !allowed {
            ApiError::new(403, "This origin is not allowed.", "ORIGIN_DENIED").into_response()
        } else {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                "Access-Control-Allow-Methods",
                HeaderValue::from_static("GET,POST,PATCH,DELETE,OPTIONS"),
            );
            response.headers_mut().insert(
                "Access-Control-Allow-Headers",
                HeaderValue::from_static("Authorization,Content-Type"),
            );
            response
                .headers_mut()
                .insert("Access-Control-Max-Age", HeaderValue::from_static("600"));
            response
        }
    } else {
        match route(state, req).await {
            Ok((status, body)) => (
                StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                Json(body),
            )
                .into_response(),
            Err(error) => error.into_response(),
        }
    };
    let headers = response.headers_mut();
    headers.insert("Cache-Control", HeaderValue::from_static("no-store"));
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "Content-Security-Policy",
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert("Referrer-Policy", HeaderValue::from_static("no-referrer"));
    if allowed {
        if let Ok(value) = HeaderValue::from_str(origin.as_ref().unwrap()) {
            headers.insert("Access-Control-Allow-Origin", value);
            headers.insert("Vary", HeaderValue::from_static("Origin"));
        }
    }
    response
}
async fn read_body(headers: &HeaderMap, body: Body, method: &str) -> ApiResult<Value> {
    if !["POST", "PATCH", "PUT", "DELETE"].contains(&method) {
        return Ok(json!({}));
    }
    if headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v != "identity")
        .unwrap_or(false)
    {
        return Err(ApiError::new(
            415,
            "Compressed requests are not supported.",
            "INVALID_REQUEST",
        ));
    }
    if headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n > 1024 * 1024)
        .unwrap_or(false)
    {
        return Err(ApiError::new(
            413,
            "Request body exceeds 1 MiB.",
            "TOO_LARGE",
        ));
    }
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        to_bytes(body, 1024 * 1024),
    )
    .await
    .map_err(|_| ApiError::new(408, "Request body timed out.", "REQUEST_TIMEOUT"))?
    .map_err(|_| ApiError::new(413, "Request body exceeds 1 MiB.", "TOO_LARGE"))?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    let content = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if !content.eq_ignore_ascii_case("application/json") {
        return Err(ApiError::new(
            415,
            "Send application/json.",
            "INVALID_REQUEST",
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad("Request body must be a JSON object."))?;
    if !value.is_object() {
        return Err(ApiError::bad("Request body must be a JSON object."));
    }
    Ok(value)
}
async fn route(state: AppState, req: Request<Body>) -> ApiResult<(u16, Value)> {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|v| v.0.ip().to_string())
        .unwrap_or_else(|| "local".into());
    state.limited(format!("general:{ip}"), 600, 60_000)?;
    if req.headers().len() > 60 {
        return Err(ApiError::new(431, "Too many request headers.", "TOO_LARGE"));
    }
    let method = req.method().as_str().to_string();
    let url = reqwest::Url::parse(&format!("http://localhost{}", req.uri()))
        .map_err(|_| ApiError::bad("Invalid request URL."))?;
    let path = url.path().to_string();
    let query: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if path == "/healthz" && method == "GET" {
        return Ok((
            200,
            json!({"ok":true,"service":"rize-local","accounts":"self-hosted","version":1}),
        ));
    }
    if method == "GET" && path.starts_with("/v1/oauth/") && path.ends_with("/callback") {
        let provider = path
            .trim_start_matches("/v1/oauth/")
            .trim_end_matches("/callback");
        if ["google", "microsoft"].contains(&provider) {
            let pending = services::resolve_oauth_state(
                provider,
                query.get("state").map(String::as_str).unwrap_or(""),
                state.now_ms(),
            )?
            .ok_or_else(|| {
                ApiError::new(
                    400,
                    "OAuth state is invalid or expired. Start the connection again.",
                    "INVALID_OAUTH_STATE",
                )
            })?;
            let ctx = state.context_for(
                pending["teamId"].as_str().unwrap_or(""),
                pending["userId"].as_str().unwrap_or(""),
            )?;
            ctx.require("admin")?;
            return Ok((200, services::complete_oauth(ctx, provider, query).await?));
        }
    }
    if ["/v1/auth/register", "/v1/auth/login"].contains(&path.as_str()) && method == "POST" {
        state.limited(format!("auth:{ip}"), 20, 15 * 60_000)?;
    }
    let headers = req.headers().clone();
    let body = read_body(&headers, req.into_body(), &method).await?;
    if ["/v1/auth/register", "/v1/auth/login"].contains(&path.as_str()) && method == "POST" {
        let slot = state.auth_slots.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(
                429,
                "Authentication is busy. Try again shortly.",
                "RATE_LIMITED",
            )
        })?;
        let email = auth::email(&body["email"])?;
        state.limited(format!("auth-email:{email}"), 10, 15 * 60_000)?;
        let secret = auth::password(&body["password"])?;
        let register = path.ends_with("/register");
        if register {
            if state
                .env
                .get("RIZE_DISABLE_REGISTRATION")
                .map(String::as_str)
                == Some("1")
            {
                return Err(ApiError::new(
                    403,
                    "Registration is disabled on this server.",
                    "REGISTRATION_DISABLED",
                ));
            }
            let name = auth::clean_text(&body["name"], "Name", 80, false)?;
            let hash = tokio::task::spawn_blocking(move || {
                let _slot = slot;
                auth::password_hash(&secret)
            })
            .await
            .map_err(|_| ApiError::internal())??;
            let id = uuid::Uuid::new_v4().to_string();
            let user = json!({"id":id,"email":email,"name":name});
            let now = state.now_ms();
            let db = state.db()?;
            let token = db::transaction(&db, || {
                if db
                    .query_row("SELECT id FROM users WHERE email=?", [&email], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?
                    .is_some()
                {
                    return Err(ApiError::new(
                        409,
                        "An account with this email already exists.",
                        "ACCOUNT_EXISTS",
                    ));
                }
                db.execute(
                    "INSERT INTO users(id,email,name,password_hash,created_at) VALUES(?,?,?,?,?)",
                    params![id, email, name, hash, now],
                )?;
                auth::issue_session(&db, &id, now)
            })?;
            return Ok((
                201,
                json!({"token":token,"user":user,"expiresAt":iso(now+auth::SESSION_TTL)}),
            ));
        }
        let row = {
            let db = state.db()?;
            db.query_row(
                "SELECT id,email,name,password_hash FROM users WHERE email=?",
                [&email],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
        };
        let hash = row.as_ref().map(|r| r.3.clone());
        let valid = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            auth::check_password(&secret, hash.as_deref())
        })
        .await
        .map_err(|_| ApiError::internal())??;
        if !valid || row.is_none() {
            return Err(ApiError::new(
                401,
                "Email or password is incorrect.",
                "INVALID_CREDENTIALS",
            ));
        }
        let (id, email, name, _) = row.unwrap();
        let now = state.now_ms();
        let token = auth::issue_session(&*state.db()?, &id, now)?;
        return Ok((
            200,
            json!({"token":token,"user":{"id":id,"email":email,"name":name},"expiresAt":iso(now+auth::SESSION_TTL)}),
        ));
    }
    let user = auth::authenticated(
        &*state.db()?,
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        state.now_ms(),
    )?;
    let user_id = user["id"].as_str().unwrap();
    if path == "/v1/auth/logout" && method == "POST" {
        let token = headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches("Bearer ");
        state.db()?.execute(
            "DELETE FROM sessions WHERE token_hash=?",
            [auth::digest(token)],
        )?;
        return Ok((200, json!({"ok":true})));
    }
    if path == "/v1/me" && method == "GET" {
        return Ok((200, json!({"user":user})));
    }
    if path == "/v1/teams" && method == "GET" {
        let db = state.db()?;
        let mut s=db.prepare("SELECT t.id FROM teams t JOIN members m ON m.team_id=t.id WHERE m.user_id=? ORDER BY t.created_at,t.id")?;
        let ids = s
            .query_map([user_id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let teams = ids
            .iter()
            .map(|id| team_result(&db, id, user_id))
            .collect::<ApiResult<Vec<_>>>()?;
        return Ok((200, json!({"teams":teams})));
    }
    if path == "/v1/teams" && method == "POST" {
        let name = auth::clean_text(&body["name"], "Team name", 100, false)?;
        let db = state.db()?;
        if db.query_row(
            "SELECT COUNT(*) FROM members WHERE user_id=?",
            [user_id],
            |r| r.get::<_, i64>(0),
        )? >= 50
        {
            return Err(ApiError::bad("Account team limit reached."));
        }
        let id = uuid::Uuid::new_v4().to_string();
        db::transaction(&db, || {
            db.execute(
                "INSERT INTO teams(id,name,created_at) VALUES(?,?,?)",
                params![id, name, state.now_ms()],
            )?;
            db.execute("INSERT INTO members(team_id,user_id,role,finance,resourcing,joined_at) VALUES(?,?,'owner',1,1,?)",params![id,user_id,state.now_ms()])?;
            Ok(())
        })?;
        return Ok((201, json!({"team":team_result(&db,&id,user_id)?})));
    }
    if path == "/v1/teams/join" && method == "POST" {
        state.limited(format!("join:{user_id}"), 30, 3_600_000)?;
        let code = auth::clean_text(&body["code"], "Invitation code", 100, false)?;
        let db = state.db()?;
        let team = db::transaction(&db, || {
            let invitation=db.query_row("SELECT team_id,email,role,inviter_id,expires_at,used_at FROM invites WHERE code_hash=?",[auth::digest(&code)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<i64>>(5)?))).optional()?.filter(|r|r.5.is_none()&&r.4>state.now_ms()&&r.1==user["email"].as_str().unwrap_or("")).ok_or_else(||ApiError::new(400,"Invitation is invalid, expired, used, or addressed to another email.","INVALID_INVITE"))?;
            let (team, _, role, inviter, _, _) = invitation;
            if db
                .query_row(
                    "SELECT 1 FROM members WHERE team_id=? AND user_id=?",
                    params![team, user_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .is_some()
            {
                return Err(ApiError::new(
                    409,
                    "You already belong to this team.",
                    "ALREADY_MEMBER",
                ));
            }
            let member = member_for(&db, &team, &inviter).map_err(|_| {
                ApiError::new(
                    400,
                    "Invitation no longer has a permitted inviter.",
                    "INVALID_INVITE",
                )
            })?;
            if !authorize(&member, "admin") || role == "admin" && member["role"] != "owner" {
                return Err(ApiError::new(
                    400,
                    "Invitation no longer has a permitted inviter.",
                    "INVALID_INVITE",
                ));
            }
            db.execute("INSERT INTO members(team_id,user_id,role,finance,resourcing,joined_at) VALUES(?,?,?,0,0,?)",params![team,user_id,role,state.now_ms()])?;
            db.execute(
                "UPDATE invites SET used_at=? WHERE code_hash=?",
                params![state.now_ms(), auth::digest(&code)],
            )?;
            Ok(team)
        })?;
        return Ok((200, json!({"team":team_result(&db,&team,user_id)?})));
    }
    let Some(rest) = path.strip_prefix("/v1/teams/") else {
        return Err(ApiError::new(404, "Endpoint not found.", "NOT_FOUND"));
    };
    let (team, route) = rest.split_once('/').unwrap_or((rest, ""));
    if !id_path(team) {
        return Err(ApiError::new(404, "Endpoint not found.", "NOT_FOUND"));
    }
    let member = member_for(&*state.db()?, team, user_id)?;
    if route == "members" && method == "GET" {
        let db = state.db()?;
        let mut s =
            db.prepare("SELECT user_id FROM members WHERE team_id=? ORDER BY joined_at,user_id")?;
        let ids = s
            .query_map([team], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((
            200,
            json!({"members":ids.iter().map(|id|member_result(&db,team,id)).collect::<ApiResult<Vec<_>>>()?}),
        ));
    }
    if route == "invites" && method == "POST" {
        if !authorize(&member, "admin") {
            return Err(ApiError::new(
                403,
                "Only team owners and admins can invite members.",
                "FORBIDDEN",
            ));
        }
        state.limited(format!("invites:{user_id}"), 50, 3_600_000)?;
        let email = auth::email(&body["email"])?;
        let role = body["role"].as_str().unwrap_or("member");
        if !["admin", "member", "viewer"].contains(&role)
            || role == "admin" && member["role"] != "owner"
        {
            return Err(ApiError::new(
                403,
                "You cannot invite that role.",
                "FORBIDDEN",
            ));
        }
        if email == user["email"] {
            return Err(ApiError::bad("You already belong to this team."));
        }
        let code = auth::random_token(24);
        let expires = state.now_ms() + auth::SESSION_TTL;
        state.db()?.execute("INSERT INTO invites(code_hash,team_id,email,role,inviter_id,expires_at) VALUES(?,?,?,?,?,?)",params![auth::digest(&code),team,email,role,user_id,expires])?;
        return Ok((
            201,
            json!({"code":code,"expiresAt":iso(expires),"invite":{"email":email,"role":role}}),
        ));
    }
    if let Some(target) = route.strip_prefix("members/").filter(|s| id_path(s)) {
        if ["PATCH", "DELETE"].contains(&method.as_str()) {
            let db = state.db()?;
            let target_member = member_for(&db, team, target)?;
            return db::transaction(&db, || {
                if method == "DELETE" {
                    if target != user_id
                        && (!authorize(&member, "admin")
                            || member["role"] != "owner"
                                && ["owner", "admin"]
                                    .contains(&target_member["role"].as_str().unwrap_or("")))
                    {
                        return Err(ApiError::new(
                            403,
                            "Your role cannot remove that member.",
                            "FORBIDDEN",
                        ));
                    }
                    if target_member["role"] == "owner"
                        && db.query_row(
                            "SELECT COUNT(*) FROM members WHERE team_id=? AND role='owner'",
                            [team],
                            |r| r.get::<_, i64>(0),
                        )? <= 1
                    {
                        return Err(ApiError::new(
                            409,
                            "A team must retain an owner.",
                            "LAST_OWNER",
                        ));
                    }
                    db.execute(
                        "DELETE FROM members WHERE team_id=? AND user_id=?",
                        params![team, target],
                    )?;
                    db.execute(
                        "DELETE FROM invites WHERE team_id=? AND inviter_id=? AND used_at IS NULL",
                        params![team, target],
                    )?;
                    return Ok((200, json!({"ok":true})));
                }
                if !authorize(&member, "admin") {
                    return Err(ApiError::new(
                        403,
                        "Only team owners and admins can manage members.",
                        "FORBIDDEN",
                    ));
                }
                let object = body.as_object().unwrap();
                if object.is_empty()
                    || object
                        .keys()
                        .any(|k| !["role", "finance", "resourcing"].contains(&k.as_str()))
                {
                    return Err(ApiError::bad(
                        "Only role, finance, and resourcing may be changed.",
                    ));
                }
                let role = body
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| target_member["role"].as_str().unwrap());
                if !["owner", "admin", "member", "viewer"].contains(&role)
                    || body
                        .get("role")
                        .map(|v| !v.is_string() && !v.is_null())
                        .unwrap_or(false)
                {
                    return Err(ApiError::bad("Unknown role."));
                }
                if target_member["role"] == "owner"
                    && role != "owner"
                    && db.query_row(
                        "SELECT COUNT(*) FROM members WHERE team_id=? AND role='owner'",
                        [team],
                        |r| r.get::<_, i64>(0),
                    )? <= 1
                {
                    return Err(ApiError::new(
                        409,
                        "A team must retain an owner.",
                        "LAST_OWNER",
                    ));
                }
                if target == user_id {
                    return Err(ApiError::new(
                        403,
                        "Another owner must change your own membership.",
                        "SELF_EDIT",
                    ));
                }
                if member["role"] != "owner"
                    && (["owner", "admin"].contains(&target_member["role"].as_str().unwrap_or(""))
                        || ["owner", "admin"].contains(&role)
                        || object.contains_key("finance")
                        || object.contains_key("resourcing"))
                {
                    return Err(ApiError::new(
                        403,
                        "Only owners may change powerful roles or resource permissions.",
                        "FORBIDDEN",
                    ));
                }
                if ["finance", "resourcing"]
                    .iter()
                    .any(|key| object.get(*key).map(|v| !v.is_boolean()).unwrap_or(false))
                {
                    return Err(ApiError::bad("Resource permissions must be booleans."));
                }
                let permission = |key: &str| {
                    if role == "owner" {
                        true
                    } else if role == "viewer" {
                        false
                    } else {
                        body.get(key)
                            .and_then(Value::as_bool)
                            .unwrap_or(target_member[key] == true)
                    }
                };
                db.execute("UPDATE members SET role=?,finance=?,resourcing=? WHERE team_id=? AND user_id=?",params![role,permission("finance") as i64,permission("resourcing") as i64,team,target])?;
                Ok((200, json!({"member":member_result(&db,team,target)?})))
            });
        }
    }
    let ctx = Context {
        state: state.clone(),
        user: user.clone(),
        team_id: team.to_string(),
        member,
    };
    if route == "sync" && method == "GET" {
        let raw = query.get("since").map(String::as_str).unwrap_or("0");
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ApiError::bad("Invalid sync cursor."));
        }
        let since = raw
            .parse::<i64>()
            .map_err(|_| ApiError::bad("Invalid sync cursor."))?;
        return Ok((200, ctx.pull(since)?));
    }
    if route == "sync" && method == "POST" {
        return Ok((200, ctx.sync(&body["changes"])?));
    }
    if route == "analytics" && method == "GET" {
        return Ok((200, workspace::analytics(&ctx, &query)?));
    }
    if route.starts_with("services/") {
        if let Some(result) = services::handle(ctx, &method, &path, body, query).await? {
            return Ok((200, result));
        }
    }
    Err(ApiError::new(404, "Endpoint not found.", "NOT_FOUND"))
}
