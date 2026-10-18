//! Bounded, explicitly configured provider access. Credentials never appear in errors.
use crate::{ApiError, ApiResult};
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::Duration};
use url::Url;

pub fn failure(status: u16, code: &str, message: &str) -> ApiError {
    ApiError {
        status,
        code: code.into(),
        message: message.into(),
    }
}
pub fn clean(value: &Value, max: usize) -> String {
    let s = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    };
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .chars()
        .take(max)
        .collect()
}
pub fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
pub fn iso(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|d| d.timestamp_millis())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().timestamp_millis())
        })
}
fn key(env: &HashMap<String, String>) -> ApiResult<Vec<u8>> {
    let bytes = STANDARD
        .decode(env.get("RIZE_SECRET_KEY").map(String::as_str).unwrap_or(""))
        .unwrap_or_default();
    if bytes.len() != 32 {
        return Err(failure(
            503,
            "SECRET_KEY_REQUIRED",
            "Set RIZE_SECRET_KEY to a base64 encoded 32-byte key before saving credentials.",
        ));
    }
    Ok(bytes)
}
pub fn secret_configured(env: &HashMap<String, String>) -> bool {
    key(env).is_ok()
}
pub fn seal(value: &Value, env: &HashMap<String, String>, scope: &str) -> ApiResult<Value> {
    let key = key(env)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| {
        failure(
            503,
            "SECRET_KEY_REQUIRED",
            "Invalid credential storage key.",
        )
    })?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut bytes = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: &serde_json::to_vec(value).unwrap_or_default(),
                aad: scope.as_bytes(),
            },
        )
        .map_err(|_| {
            failure(
                503,
                "SECRET_ENCRYPT_FAILED",
                "Credentials could not be encrypted.",
            )
        })?;
    let tag = bytes.split_off(bytes.len() - 16);
    Ok(
        json!({"v":1,"iv":STANDARD.encode(nonce),"tag":STANDARD.encode(tag),"ciphertext":STANDARD.encode(bytes)}),
    )
}
pub fn unseal(value: &Value, env: &HashMap<String, String>, scope: &str) -> ApiResult<Value> {
    let key = key(env)?;
    let bad = || {
        failure(
            503,
            "SECRET_DECRYPT_FAILED",
            "Stored credentials could not be decrypted with this server key.",
        )
    };
    let iv = STANDARD.decode(text(&value["iv"])).map_err(|_| bad())?;
    if iv.len() != 12 {
        return Err(bad());
    }
    let mut bytes = STANDARD
        .decode(text(&value["ciphertext"]))
        .map_err(|_| bad())?;
    bytes.extend(STANDARD.decode(text(&value["tag"])).map_err(|_| bad())?);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| bad())?;
    let clear = cipher
        .decrypt(
            iv.as_slice().into(),
            Payload {
                msg: &bytes,
                aad: scope.as_bytes(),
            },
        )
        .map_err(|_| bad())?;
    serde_json::from_slice(&clear).map_err(|_| bad())
}

#[derive(Clone, Debug)]
pub struct Request {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Value>,
    pub form: Option<HashMap<String, String>>,
    pub allowed_origins: Vec<String>,
    pub timeout_ms: u64,
    pub max_bytes: usize,
}
impl Request {
    pub fn new(url: impl Into<String>, origin: &str) -> Self {
        Self {
            url: url.into(),
            method: "GET".into(),
            headers: HashMap::new(),
            body: None,
            form: None,
            allowed_origins: vec![origin.into()],
            timeout_ms: 20000,
            max_bytes: 2 * 1024 * 1024,
        }
    }
    pub fn post(mut self, body: Value) -> Self {
        self.method = "POST".into();
        self.body = Some(body);
        self
    }
    pub fn bearer(mut self, token: &str) -> Self {
        self.headers
            .insert("Authorization".into(), format!("Bearer {token}"));
        self
    }
}
#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: Request) -> ApiResult<Value>;
}
pub struct ReqwestTransport {
    client: reqwest::Client,
}
impl Default for ReqwestTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("TLS client construction"),
        }
    }
}
pub fn https_url(value: &str, origins: &[String]) -> ApiResult<Url> {
    let u = Url::parse(value)
        .map_err(|_| failure(503, "PROVIDER_CONFIG", "Provider URL is invalid."))?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
        || !origins
            .iter()
            .any(|o| o == u.origin().ascii_serialization().as_str())
    {
        return Err(failure(
            503,
            "PROVIDER_CONFIG",
            "Provider URL must use an allowed HTTPS origin.",
        ));
    }
    Ok(u)
}
#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, r: Request) -> ApiResult<Value> {
        https_url(&r.url, &r.allowed_origins)?;
        let method = reqwest::Method::from_bytes(r.method.as_bytes()).map_err(|_| {
            failure(
                503,
                "PROVIDER_CONFIG",
                "Provider request method is invalid.",
            )
        })?;
        let mut call = self
            .client
            .request(method, &r.url)
            .timeout(Duration::from_millis(r.timeout_ms))
            .header("Accept", "application/json");
        for (k, v) in r.headers {
            call = call.header(k, v);
        }
        if let Some(body) = r.body {
            call = call.json(&body);
        }
        if let Some(form) = r.form {
            call = call.form(&form);
        }
        let mut response = call.send().await.map_err(|_| {
            failure(
                502,
                "PROVIDER_UNREACHABLE",
                "Provider request failed or timed out.",
            )
        })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(failure(if status==429{429}else{502},if status==429{"PROVIDER_RATE_LIMIT"}else{"PROVIDER_REJECTED"},&format!("Provider rejected the request (HTTP {status}). Check credentials, scopes, plan access, and retry.")));
        }
        if response
            .content_length()
            .map(|n| n > r.max_bytes as u64)
            .unwrap_or(false)
        {
            return Err(failure(
                502,
                "PROVIDER_RESPONSE",
                "Provider response exceeds the size limit.",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            failure(
                502,
                "PROVIDER_RESPONSE",
                "Provider returned an unreadable response.",
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > r.max_bytes {
                return Err(failure(
                    502,
                    "PROVIDER_RESPONSE",
                    "Provider response exceeds the size limit.",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| failure(502, "PROVIDER_RESPONSE", "Provider returned invalid JSON."))
    }
}
// Validate before invoking even an injected transport.
pub async fn request(transport: &dyn Transport, r: Request) -> ApiResult<Value> {
    https_url(&r.url, &r.allowed_origins)?;
    transport.send(r).await
}
#[derive(Clone)]
pub struct AiConfig {
    pub base: String,
    pub origin: String,
    pub key: String,
    pub model: String,
}
pub fn ai_config(env: &HashMap<String, String>, embedding: bool) -> ApiResult<AiConfig> {
    let base = env.get("RIZE_AI_BASE_URL").filter(|s| !s.is_empty());
    let key = env.get("RIZE_AI_API_KEY").filter(|s| !s.is_empty());
    let model = env
        .get(if embedding {
            "RIZE_EMBEDDING_MODEL"
        } else {
            "RIZE_AI_MODEL"
        })
        .filter(|s| !s.is_empty());
    if base.is_none() || key.is_none() || model.is_none() {
        return Err(failure(
            503,
            "AI_NOT_CONFIGURED",
            if embedding {
                "Configure RIZE_AI_BASE_URL, RIZE_AI_API_KEY, and RIZE_EMBEDDING_MODEL for semantic suggestions."
            } else {
                "Configure RIZE_AI_BASE_URL, RIZE_AI_API_KEY, and RIZE_AI_MODEL to use AI."
            },
        ));
    }
    let u = Url::parse(base.unwrap())
        .map_err(|_| failure(503, "PROVIDER_CONFIG", "AI base URL is invalid."))?;
    let origin = u.origin().ascii_serialization();
    https_url(u.as_str(), &[origin.clone()])?;
    if u.query().is_some() {
        return Err(failure(
            503,
            "PROVIDER_CONFIG",
            "AI base URL must not include query parameters.",
        ));
    }
    Ok(AiConfig {
        base: u.as_str().trim_end_matches('/').into(),
        origin,
        key: key.unwrap().clone(),
        model: model.unwrap().clone(),
    })
}
pub async fn chat(
    env: &HashMap<String, String>,
    transport: &dyn Transport,
    messages: Value,
) -> ApiResult<Value> {
    let c = ai_config(env, false)?;
    let data=request(transport,Request::new(format!("{}/chat/completions",c.base),&c.origin).post(json!({"model":c.model,"messages":messages,"stream":false,"max_completion_tokens":1800})).bearer(&c.key)).await?;
    let answer = data["choices"][0]["message"]["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| failure(502, "AI_RESPONSE", "AI returned no text."))?;
    let mut out = json!({"text":answer.chars().take(30000).collect::<String>(),"model":c.model});
    if data.get("usage").is_some() {
        out["usage"] = json!({"promptTokens":data["usage"]["prompt_tokens"],"completionTokens":data["usage"]["completion_tokens"]});
    }
    Ok(out)
}
pub async fn embeddings(
    env: &HashMap<String, String>,
    transport: &dyn Transport,
    input: &[String],
) -> ApiResult<Vec<Vec<f64>>> {
    let c = ai_config(env, true)?;
    let mut req = Request::new(format!("{}/embeddings", c.base), &c.origin)
        .post(json!({"model":c.model,"input":input,"encoding_format":"float"}))
        .bearer(&c.key);
    req.max_bytes = 8 * 1024 * 1024;
    let data = request(transport, req).await?;
    let mut rows = data["data"].as_array().cloned().ok_or_else(|| {
        failure(
            502,
            "AI_RESPONSE",
            "Embedding provider returned invalid vectors.",
        )
    })?;
    rows.sort_by_key(|r| r["index"].as_u64().unwrap_or(u64::MAX));
    let mut vectors = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let numbers = row["embedding"].as_array().ok_or_else(|| {
            failure(
                502,
                "AI_RESPONSE",
                "Embedding provider returned invalid vectors.",
            )
        })?;
        if row["index"].as_u64() != Some(i as u64) || numbers.is_empty() || numbers.len() > 65536 {
            return Err(failure(
                502,
                "AI_RESPONSE",
                "Embedding provider returned invalid vectors.",
            ));
        }
        let vector = numbers
            .iter()
            .map(|n| {
                n.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
                    failure(
                        502,
                        "AI_RESPONSE",
                        "Embedding provider returned invalid vectors.",
                    )
                })
            })
            .collect::<ApiResult<Vec<_>>>()?;
        if vectors
            .first()
            .map(|v: &Vec<f64>| v.len() != vector.len())
            .unwrap_or(false)
        {
            return Err(failure(
                502,
                "AI_RESPONSE",
                "Embedding dimensions do not match.",
            ));
        }
        vectors.push(vector);
    }
    if vectors.len() != input.len() {
        return Err(failure(
            502,
            "AI_RESPONSE",
            "Embedding provider returned invalid vectors.",
        ));
    }
    Ok(vectors)
}
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let (dot, aa, bb) = a.iter().zip(b).fold((0., 0., 0.), |(d, aa, bb), (a, b)| {
        (d + a * b, aa + a * a, bb + b * b)
    });
    if aa > 0. && bb > 0. {
        dot / (aa * bb).sqrt()
    } else {
        0.
    }
}
pub fn integration_id(provider: &str, id: &str) -> String {
    format!(
        "{provider}_{}",
        &format!("{:x}", Sha256::digest(id.as_bytes()))[..24]
    )
}
fn origin(provider: &str) -> ApiResult<&'static str> {
    match provider {
        "linear" => Ok("https://api.linear.app"),
        "clickup" => Ok("https://api.clickup.com"),
        "asana" => Ok("https://app.asana.com"),
        _ => Err(failure(
            404,
            "PROVIDER_NOT_SUPPORTED",
            "Unsupported integration provider.",
        )),
    }
}
pub fn provider_config(provider: &str, body: &Value) -> ApiResult<Value> {
    origin(provider)?;
    let token = clean(&body["token"], 4000);
    if token.len() < 8 || text(&body["token"]).contains(['\r', '\n']) {
        return Err(failure(
            400,
            "INVALID_TOKEN",
            "Enter a valid provider token.",
        ));
    }
    let keys = match provider {
        "clickup" => vec!["listId", "workspaceId"],
        "asana" => vec!["projectId"],
        _ => vec!["teamId"],
    };
    let mut options = json!({});
    for k in keys {
        let v = clean(
            body.get(k)
                .or_else(|| body["options"].get(k))
                .unwrap_or(&Value::Null),
            120,
        );
        if !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(failure(400, "INVALID_OPTIONS", &format!("Invalid {k}.")));
        }
        options[k] = json!(v);
    }
    if provider == "clickup" && text(&options["listId"]).is_empty() {
        return Err(failure(
            400,
            "INVALID_OPTIONS",
            "ClickUp listId is required.",
        ));
    }
    if provider == "asana" && text(&options["projectId"]).is_empty() {
        return Err(failure(
            400,
            "INVALID_OPTIONS",
            "Asana projectId is required.",
        ));
    }
    Ok(json!({"token":token,"options":options}))
}
fn authorized(mut r: Request, provider: &str, config: &Value) -> Request {
    r.headers.insert(
        "Authorization".into(),
        if provider == "asana" {
            format!("Bearer {}", text(&config["token"]))
        } else {
            text(&config["token"]).into()
        },
    );
    r
}
pub async fn provider_tasks(
    provider: &str,
    config: &Value,
    transport: &dyn Transport,
) -> ApiResult<Value> {
    let base = origin(provider)?;
    let options = &config["options"];
    let mut tasks = Vec::new();
    let mut cursor = String::new();
    for page in 0..20 {
        let req=match provider{"linear"=>Request::new(format!("{base}/graphql"),base).post(json!({"query":"query RizeIssues($first:Int!,$after:String,$filter:IssueFilter){issues(first:$first,after:$after,filter:$filter){nodes{id title identifier url state{name type} project{id name}} pageInfo{hasNextPage endCursor}}}","variables":{"first":100,"after":if cursor.is_empty(){Value::Null}else{json!(cursor)},"filter":if text(&options["teamId"]).is_empty(){Value::Null}else{json!({"team":{"id":{"eq":options["teamId"]}}})}}})),"clickup"=>Request::new(format!("{base}/api/v2/list/{}/task?page={page}&include_closed=true&subtasks=true",text(&options["listId"])),base),_=>{let mut u=Url::parse(&format!("{base}/api/1.0/projects/{}/tasks",text(&options["projectId"]))).unwrap();u.query_pairs_mut().append_pair("limit","100").append_pair("opt_fields","name,completed,permalink_url");if !cursor.is_empty(){u.query_pairs_mut().append_pair("offset",&cursor);}Request::new(u.as_str(),base)}};
        let data = request(transport, authorized(req, provider, config)).await?;
        let done;
        if provider == "linear" {
            if data["errors"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false)
            {
                return Err(failure(
                    502,
                    "PROVIDER_REJECTED",
                    "Linear GraphQL rejected the query. Check token and team access.",
                ));
            }
            let rows = data["data"]["issues"]["nodes"].as_array().ok_or_else(|| {
                failure(
                    502,
                    "PROVIDER_RESPONSE",
                    "Linear returned an invalid issue response.",
                )
            })?;
            for t in rows {
                tasks.push(json!({"externalId":t["id"],"name":clean(&json!(format!("{} {}",text(&t["identifier"]),text(&t["title"]))),500),"complete":matches!(text(&t["state"]["type"]),"completed"|"canceled"),"url":clean(&t["url"],1000),"externalProjectId":t["project"]["id"].as_str().unwrap_or(""),"externalProjectName":clean(&t["project"]["name"],300)}));
            }
            done = data["data"]["issues"]["pageInfo"]["hasNextPage"] != true;
            cursor = text(&data["data"]["issues"]["pageInfo"]["endCursor"]).into();
        } else if provider == "clickup" {
            let rows = data["tasks"].as_array().ok_or_else(|| {
                failure(
                    502,
                    "PROVIDER_RESPONSE",
                    "ClickUp returned an invalid task response.",
                )
            })?;
            for t in rows {
                tasks.push(json!({"externalId":clean(&t["id"],200),"name":clean(&t["name"],500),"complete":matches!(text(&t["status"]["type"]),"closed"|"done"),"url":clean(&t["url"],1000)}));
            }
            done = data["last_page"] == true || rows.len() < 100;
        } else {
            let rows = data["data"].as_array().ok_or_else(|| {
                failure(
                    502,
                    "PROVIDER_RESPONSE",
                    "Asana returned an invalid task response.",
                )
            })?;
            for t in rows {
                tasks.push(json!({"externalId":clean(&t["gid"],200),"name":clean(&t["name"],500),"complete":t["completed"]==true,"url":clean(&t["permalink_url"],1000)}));
            }
            cursor = text(&data["next_page"]["offset"]).into();
            done = cursor.is_empty();
        }
        if done {
            return Ok(json!({"tasks":tasks,"truncated":false}));
        }
    }
    Ok(json!({"tasks":tasks,"truncated":true}))
}
pub async fn push_time(
    provider: &str,
    config: &Value,
    entry: &Value,
    external_id: &str,
    transport: &dyn Transport,
) -> ApiResult<String> {
    if provider == "linear" {
        return Err(failure(400,"TIME_EXPORT_UNSUPPORTED","Linear native time entry export is not supported; imported issues can be tagged locally."));
    }
    let start = timestamp(text(&entry["start"]));
    let end = timestamp(text(&entry["end"]));
    let duration = entry["durationMs"]
        .as_f64()
        .or_else(|| Some((end? - start?) as f64));
    if entry["status"] != "approved"
        || start.is_none()
        || end.is_none()
        || end <= start
        || duration.map(|d| !d.is_finite() || d <= 0.).unwrap_or(true)
    {
        return Err(failure(
            400,
            "ENTRY_NOT_APPROVED",
            "Only approved entries with a valid positive duration can be exported.",
        ));
    }
    let base = origin(provider)?;
    let duration = duration.unwrap();
    let req = if provider == "clickup" {
        if text(&config["options"]["workspaceId"]).is_empty() {
            return Err(failure(
                400,
                "INVALID_OPTIONS",
                "Reconnect ClickUp with workspaceId before exporting time.",
            ));
        }
        Request::new(format!("{base}/api/v2/team/{}/time_entries",text(&config["options"]["workspaceId"])),base).post(json!({"description":clean(&entry["description"],2000),"start":start,"duration":duration.round(),"billable":entry["billable"]!=false,"tid":external_id}))
    } else {
        if duration % 60000. != 0. {
            return Err(failure(400,"ASANA_WHOLE_MINUTES","Asana accepts whole minutes. Adjust or split this entry explicitly; export will not silently round."));
        }
        let mut u = Url::parse(&format!("{base}/api/1.0/tasks/")).unwrap();
        u.path_segments_mut()
            .unwrap()
            .push(external_id)
            .push("time_tracking_entries");
        Request::new(u.as_str(),base).post(json!({"data":{"duration_minutes":duration/60000.,"entered_on":text(&entry["start"]).chars().take(10).collect::<String>()}}))
    };
    let data = request(transport, authorized(req, provider, config)).await?;
    let id=data["data"].get("id").or_else(||data["data"].get("gid")).or_else(||data.get("id")).map(|v|clean(v,300)).filter(|s|!s.is_empty()).ok_or_else(||failure(502,"EXPORT_UNCERTAIN","Provider did not return a created time entry identifier. Check the vendor before retrying."))?;
    Ok(id)
}
#[derive(Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scope: String,
    pub authorize: String,
    pub token: String,
}
pub fn oauth_config(provider: &str, env: &HashMap<String, String>) -> ApiResult<OAuthConfig> {
    if !matches!(provider, "google" | "microsoft") {
        return Err(failure(
            404,
            "PROVIDER_NOT_SUPPORTED",
            "OAuth provider is unsupported.",
        ));
    }
    let prefix = if provider == "google" {
        "RIZE_GOOGLE_"
    } else {
        "RIZE_MICROSOFT_"
    };
    let invalid = || {
        failure(
            503,
            "OAUTH_NOT_CONFIGURED",
            "Configure a valid server public origin, OAuth client ID and client secret.",
        )
    };
    let u = Url::parse(
        env.get("RIZE_PUBLIC_ORIGIN")
            .or_else(|| env.get("RIZE_PUBLIC_URL"))
            .map(String::as_str)
            .unwrap_or(""),
    )
    .map_err(|_| invalid())?;
    let loopback = matches!(
        u.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
        || !(u.scheme() == "https" || u.scheme() == "http" && loopback)
    {
        return Err(invalid());
    }
    let client_id = env
        .get(&format!("{prefix}CLIENT_ID"))
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)?
        .clone();
    let client_secret = env
        .get(&format!("{prefix}CLIENT_SECRET"))
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)?
        .clone();
    let tenant = env
        .get("RIZE_MICROSOFT_TENANT")
        .map(String::as_str)
        .unwrap_or("common");
    if tenant.is_empty()
        || !tenant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(failure(
            503,
            "OAUTH_NOT_CONFIGURED",
            "Microsoft tenant configuration is invalid.",
        ));
    }
    Ok(OAuthConfig {
        client_id,
        client_secret,
        redirect_uri: format!(
            "{}/v1/oauth/{provider}/callback",
            u.origin().ascii_serialization()
        ),
        scope: if provider == "google" {
            "https://www.googleapis.com/auth/calendar.readonly".into()
        } else {
            "offline_access https://graph.microsoft.com/Calendars.Read".into()
        },
        authorize: if provider == "google" {
            "https://accounts.google.com/o/oauth2/v2/auth".into()
        } else {
            format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize")
        },
        token: if provider == "google" {
            "https://oauth2.googleapis.com/token".into()
        } else {
            format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token")
        },
    })
}
pub async fn oauth_tokens(
    provider: &str,
    env: &HashMap<String, String>,
    transport: &dyn Transport,
    mut fields: HashMap<String, String>,
    now: i64,
) -> ApiResult<Value> {
    let c = oauth_config(provider, env)?;
    fields.insert("client_id".into(), c.client_id);
    fields.insert("client_secret".into(), c.client_secret);
    if provider == "microsoft" {
        fields.insert("scope".into(), c.scope);
    }
    let origin = Url::parse(&c.token).unwrap().origin().ascii_serialization();
    let mut req = Request::new(c.token, &origin);
    req.method = "POST".into();
    req.form = Some(fields);
    let data = request(transport, req).await?;
    let access = text(&data["access_token"]);
    let expires = data["expires_in"]
        .as_f64()
        .or_else(|| text(&data["expires_in"]).parse().ok())
        .filter(|n| n.is_finite() && *n > 0.);
    if access.is_empty()
        || access.len() > 12000
        || !text(&data["token_type"]).eq_ignore_ascii_case("bearer")
        || expires.is_none()
    {
        return Err(failure(
            502,
            "OAUTH_RESPONSE",
            "OAuth provider returned invalid access credentials.",
        ));
    }
    let mut tokens =
        json!({"accessToken":access,"expiresAt":now+(expires.unwrap().min(604800.)*1000.)as i64});
    if let Some(refresh) = data["refresh_token"].as_str() {
        tokens["refreshToken"] = json!(refresh);
    }
    Ok(tokens)
}
pub fn calendar_range(options: &Value, now: i64) -> ApiResult<(String, String)> {
    let from = options["from"].as_str().and_then(timestamp).or_else(|| {
        if options["from"].is_null() {
            Some(now - 30 * 86400000)
        } else {
            None
        }
    });
    let to = options["to"].as_str().and_then(timestamp).or_else(|| {
        if options["to"].is_null() {
            Some(now + 90 * 86400000)
        } else {
            None
        }
    });
    if from.is_none() || to.is_none() || to <= from || to.unwrap() - from.unwrap() > 366 * 86400000
    {
        return Err(failure(
            400,
            "INVALID_RANGE",
            "Choose a calendar range up to 366 days.",
        ));
    }
    Ok((iso(from.unwrap()), iso(to.unwrap())))
}
pub fn canonical_zone(zone: &str) -> Option<String> {
    if zone.is_empty() {
        return Some("UTC".into());
    }
    if zone.parse::<chrono_tz::Tz>().is_ok() {
        return Some(zone.into());
    }
    static ZONES: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let zones = ZONES.get_or_init(|| {
        serde_json::from_str(include_str!("windows_zones.json"))
            .expect("checked Windows timezone mapping")
    });
    zones[zone]["iana"][0]
        .as_str()
        .filter(|s| s.parse::<chrono_tz::Tz>().is_ok())
        .map(String::from)
}
fn mapped_zone(zone: &str) -> String {
    canonical_zone(zone).unwrap_or_else(|| "UTC".into())
}
pub async fn calendar_events(
    provider: &str,
    token: &str,
    options: &Value,
    transport: &dyn Transport,
    now: i64,
) -> ApiResult<Value> {
    let (from, to) = calendar_range(options, now)?;
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut cursor = String::new();
    let mut next = String::new();
    for page in 0..20 {
        let mut req = if provider == "google" {
            let mut u =
                Url::parse("https://www.googleapis.com/calendar/v3/calendars/primary/events")
                    .unwrap();
            u.query_pairs_mut()
                .append_pair("timeMin", &from)
                .append_pair("timeMax", &to)
                .append_pair("singleEvents", "true")
                .append_pair("showDeleted", "false")
                .append_pair("maxResults", "100")
                .append_pair("orderBy", "startTime");
            if !cursor.is_empty() {
                u.query_pairs_mut().append_pair("pageToken", &cursor);
            }
            Request::new(u.as_str(), "https://www.googleapis.com")
        } else {
            let url = if next.is_empty() {
                let mut u = Url::parse("https://graph.microsoft.com/v1.0/me/calendar/calendarView")
                    .unwrap();
                u.query_pairs_mut()
                    .append_pair("startDateTime", &from)
                    .append_pair("endDateTime", &to)
                    .append_pair("$top", "100")
                    .append_pair(
                        "$select",
                        "id,subject,start,end,isAllDay,isCancelled,originalStartTimeZone",
                    );
                u.to_string()
            } else {
                next.clone()
            };
            let mut r = Request::new(url, "https://graph.microsoft.com");
            r.headers
                .insert("Prefer".into(), "outlook.timezone=\"UTC\"".into());
            r
        };
        req = req.bearer(token);
        let data = request(transport, req).await?;
        let rows = data[if provider == "google" {
            "items"
        } else {
            "value"
        }]
        .as_array()
        .ok_or_else(|| {
            failure(
                502,
                "PROVIDER_RESPONSE",
                if provider == "google" {
                    "Google Calendar returned an invalid event response."
                } else {
                    "Microsoft Graph returned an invalid event response."
                },
            )
        })?;
        for e in rows {
            if e["status"] == "cancelled" || e["isCancelled"] == true {
                continue;
            }
            let (all_day, start, end, zone, date, summary) = if provider == "google" {
                let all_day = e["start"]["date"].is_string();
                let start = if all_day {
                    format!("{}T00:00:00.000Z", text(&e["start"]["date"]))
                } else {
                    text(&e["start"]["dateTime"]).into()
                };
                let end = if all_day {
                    format!("{}T00:00:00.000Z", text(&e["end"]["date"]))
                } else {
                    text(&e["end"]["dateTime"]).into()
                };
                (
                    all_day,
                    start,
                    end,
                    e["start"]["timeZone"].as_str().unwrap_or("UTC").to_string(),
                    e["start"]["date"].clone(),
                    clean(&e["summary"], 500),
                )
            } else {
                if e["start"]["timeZone"].as_str().unwrap_or("UTC") != "UTC"
                    || e["end"]["timeZone"].as_str().unwrap_or("UTC") != "UTC"
                {
                    warnings.push(
                        "Skipped Microsoft event without the requested UTC timezone.".to_string(),
                    );
                    continue;
                }
                let utc = |s: &str| {
                    if timestamp(s).is_some() {
                        s.to_string()
                    } else {
                        format!("{s}Z")
                    }
                };
                let start = utc(text(&e["start"]["dateTime"]));
                let end = utc(text(&e["end"]["dateTime"]));
                let zone = mapped_zone(text(&e["originalStartTimeZone"]));
                let date = timestamp(&start)
                    .and_then(DateTime::<Utc>::from_timestamp_millis)
                    .map(|d| {
                        json!(d
                            .with_timezone(&zone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC))
                            .format("%Y-%m-%d")
                            .to_string())
                    })
                    .unwrap_or(Value::Null);
                (
                    e["isAllDay"] == true,
                    start,
                    end,
                    zone,
                    date,
                    clean(&e["subject"], 500),
                )
            };
            let start = timestamp(&start);
            let end = timestamp(&end);
            if start.is_none() || end.is_none() || end < start {
                warnings.push(format!("Skipped {provider} event with invalid dates."));
                continue;
            }
            let uid = clean(&e["id"], 1000);
            let mut event = json!({"id":integration_id(provider,&uid),"uid":uid,"summary":if summary.is_empty(){"Calendar event".into()}else{summary},"start":iso(start.unwrap()),"end":iso(end.unwrap()),"allDay":all_day,"timeZone":zone,"provider":provider});
            if all_day {
                event["date"] = date;
            }
            events.push(event);
        }
        cursor = text(&data["nextPageToken"]).into();
        next = text(&data["@odata.nextLink"]).into();
        if provider == "google" && cursor.is_empty() || provider != "google" && next.is_empty() {
            break;
        }
        if events.len() >= 1000 {
            warnings.push("Calendar import reached the 1,000-event limit. Choose a smaller date range for remaining events.".into());
            break;
        }
        if page == 19 {
            warnings.push("Calendar pagination limit reached. Choose a smaller range.".into());
        }
    }
    events.truncate(1000);
    Ok(json!({"proposals":calendar_proposals(&events),"events":events,"warnings":warnings}))
}
pub fn calendar_proposals(events: &[Value]) -> Vec<Value> {
    events.iter().filter_map(|e|{let start=timestamp(text(&e["start"]))?;let end=timestamp(text(&e["end"]))?;if e["allDay"]==true||end<=start{return None;}Some(json!({"externalId":e["id"],"description":e["summary"],"start":e["start"],"end":e["end"],"durationMs":end-start,"source":"calendar","status":"pending"}))}).collect()
}
