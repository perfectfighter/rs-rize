//! Read-only MCP 2025-11-25 over newline-delimited JSON-RPC stdio.
use crate::{
    providers::{clean, failure, text},
    ApiResult,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use url::Url;
pub const VERSION: &str = "2025-11-25";
pub fn tools() -> Value {
    json!([
{"name":"approved_time_entries","description":"Read approved shared time descriptions and tags for a date range. No raw window activity.","inputSchema":{"type":"object","properties":{"from":{"type":"string","description":"Inclusive YYYY-MM-DD"},"to":{"type":"string","description":"Inclusive YYYY-MM-DD"}},"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}},
{"name":"search_work","description":"Search visible project and task names in the selected team.","inputSchema":{"type":"object","properties":{"query":{"type":"string","maxLength":200}},"required":["query"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}},
{"name":"team_analytics","description":"Read server-authorized aggregate team time and utilization.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}])
}
#[derive(Clone)]
pub struct Configuration {
    pub base: String,
    pub team_id: String,
    pub token: String,
}
pub fn configuration(env: &HashMap<String, String>) -> ApiResult<Configuration> {
    let u = Url::parse(
        env.get("RIZE_SERVER_URL")
            .map(String::as_str)
            .unwrap_or("http://127.0.0.1:4791"),
    )
    .map_err(|_| failure(400, "MCP_CONFIG", "Invalid RIZE_SERVER_URL."))?;
    if !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || !(u.scheme() == "https"
            || u.scheme() == "http"
                && matches!(
                    u.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                ))
    {
        return Err(failure(
            400,
            "MCP_CONFIG",
            "RIZE_SERVER_URL requires HTTPS or loopback HTTP.",
        ));
    }
    let token = env
        .get("RIZE_SESSION_TOKEN")
        .filter(|s| !s.is_empty() && !s.contains(['\r', '\n']));
    let team = env.get("RIZE_TEAM_ID").filter(|s| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    if token.is_none() || team.is_none() {
        return Err(failure(
            400,
            "MCP_CONFIG",
            "Set RIZE_SESSION_TOKEN and RIZE_TEAM_ID.",
        ));
    }
    Ok(Configuration {
        base: u.as_str().trim_end_matches('/').into(),
        team_id: team.unwrap().clone(),
        token: token.unwrap().clone(),
    })
}
#[async_trait]
pub trait Backend: Send + Sync {
    async fn get(&self, configuration: &Configuration, path: &str) -> ApiResult<Value>;
}
pub struct HttpBackend {
    client: reqwest::Client,
}
impl Default for HttpBackend {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .expect("TLS client construction"),
        }
    }
}
#[async_trait]
impl Backend for HttpBackend {
    async fn get(&self, c: &Configuration, path: &str) -> ApiResult<Value> {
        let mut res = self
            .client
            .get(format!("{}/v1/teams/{}/{}", c.base, c.team_id, path))
            .bearer_auth(&c.token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| failure(502, "MCP_BACKEND", "Backend unavailable."))?;
        if !res.status().is_success() {
            return Err(failure(
                502,
                "MCP_BACKEND",
                &format!("Backend access failed (HTTP {}).", res.status().as_u16()),
            ));
        }
        const LIMIT: usize = 2 * 1024 * 1024;
        if res
            .content_length()
            .map(|s| s > LIMIT as u64)
            .unwrap_or(false)
        {
            return Err(failure(502, "MCP_BACKEND", "Backend response too large."));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|_| failure(502, "MCP_BACKEND", "Backend unavailable."))?
        {
            if bytes.len() + chunk.len() > LIMIT {
                return Err(failure(502, "MCP_BACKEND", "Backend response too large."));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| failure(502, "MCP_BACKEND", "Backend returned invalid JSON."))
    }
}
pub struct Mcp<B: Backend> {
    initialized: bool,
    configuration: Configuration,
    backend: B,
}
impl<B: Backend> Mcp<B> {
    pub fn new(env: &HashMap<String, String>, backend: B) -> ApiResult<Self> {
        Ok(Self {
            initialized: false,
            configuration: configuration(env)?,
            backend,
        })
    }
    pub async fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let reply = |result: Value| json!({"jsonrpc":"2.0","id":id,"result":result});
        let error = |code: i64, message: &str| json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}});
        if !message.is_object() || message["jsonrpc"] != "2.0" || !message["method"].is_string() {
            return Some(error(-32600, "Invalid JSON-RPC request."));
        }
        if message.get("id").is_none() {
            return None;
        }
        let method = text(&message["method"]);
        if method == "initialize" {
            if !message["params"]["protocolVersion"].is_string() {
                return Some(error(-32602, "protocolVersion is required."));
            }
            self.initialized = true;
            return Some(reply(
                json!({"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"rize-local-readonly","version":"1.2.0"},"instructions":"Read-only access to approved shared time and configured work records. This server supports MCP 2025-11-25."}),
            ));
        }
        if method == "ping" {
            return Some(reply(json!({})));
        }
        if !self.initialized {
            return Some(error(
                -32000,
                "Initialize the MCP session before requesting tools.",
            ));
        }
        if method == "tools/list" {
            return Some(reply(json!({"tools":tools()})));
        }
        if method != "tools/call" {
            return Some(error(-32601, "Method not found."));
        }
        let name = text(&message["params"]["name"]);
        if !matches!(
            name,
            "approved_time_entries" | "search_work" | "team_analytics"
        ) {
            return Some(error(-32602, "Unknown tool."));
        }
        let empty = json!({});
        let args = message["params"].get("arguments").unwrap_or(&empty);
        let Some(args) = args.as_object() else {
            return Some(error(-32602, "Arguments must be an object."));
        };
        if name == "team_analytics" && !args.is_empty() {
            return Some(error(-32602, "This tool accepts no arguments."));
        }
        let date_valid = |v: &Value| {
            v.as_str()
                .map(|s| {
                    s.len() == 10
                        && s.char_indices().all(|(i, c)| {
                            if i == 4 || i == 7 {
                                c == '-'
                            } else {
                                c.is_ascii_digit()
                            }
                        })
                })
                .unwrap_or(false)
        };
        if name == "approved_time_entries"
            && (args.keys().any(|k| k != "from" && k != "to")
                || ["from", "to"]
                    .iter()
                    .any(|k| args.get(*k).map(|v| !date_valid(v)).unwrap_or(false)))
        {
            return Some(error(-32602, "Dates must be YYYY-MM-DD."));
        }
        if name == "search_work"
            && (args.keys().any(|k| k != "query")
                || args
                    .get("query")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().count() > 200)
                    .unwrap_or(true))
        {
            return Some(error(-32602, "Provide query of at most 200 characters."));
        }
        let result = self.call_tool(name, args).await;
        Some(match result {
            Ok(value) => {
                reply(json!({"content":[{"type":"text","text":value.to_string()}],"isError":false}))
            }
            Err(e) => reply(json!({"content":[{"type":"text","text":e.message}],"isError":true})),
        })
    }
    async fn call_tool(
        &self,
        name: &str,
        args: &serde_json::Map<String, Value>,
    ) -> ApiResult<Value> {
        if name == "team_analytics" {
            return self.backend.get(&self.configuration, "analytics").await;
        }
        let data = self
            .backend
            .get(&self.configuration, "sync?since=0")
            .await?;
        let records = data["records"].as_array().cloned().unwrap_or_default();
        let records = records.iter().filter(|r| r["deleted"] != true);
        if name == "approved_time_entries" {
            let from = args.get("from").and_then(Value::as_str);
            let to = args.get("to").and_then(Value::as_str);
            Ok(json!(records.filter(|r|r["kind"]=="entries"&&r["data"]["status"]=="approved"&&from.map(|d|text(&r["data"]["start"]).get(..10).map(|s|s>=d).unwrap_or(false)).unwrap_or(true)&&to.map(|d|text(&r["data"]["start"]).get(..10).map(|s|s<=d).unwrap_or(false)).unwrap_or(true)).take(500).map(|r|json!({"id":r["id"],"description":clean(&r["data"]["description"],1500),"start":r["data"]["start"],"end":r["data"]["end"],"durationMs":r["data"]["durationMs"],"clientId":r["data"]["clientId"],"projectId":r["data"]["projectId"],"taskId":r["data"]["taskId"],"ownerId":r["ownerId"]})).collect::<Vec<_>>()))
        } else {
            let q = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            Ok(json!(records.filter(|r|matches!(text(&r["kind"]),"projects"|"tasks")&&text(&r["data"]["name"]).to_lowercase().contains(&q)).take(100).map(|r|json!({"kind":r["kind"],"id":r["id"],"name":clean(&r["data"]["name"],500),"clientId":r["data"]["clientId"],"projectId":r["data"]["projectId"]})).collect::<Vec<_>>()))
        }
    }
}
async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(
    input: &mut R,
) -> std::io::Result<Option<(Vec<u8>, bool)>> {
    const LIMIT: usize = 1024 * 1024;
    let mut bytes = Vec::new();
    let mut oversized = false;
    loop {
        let available = input.fill_buf().await?;
        if available.is_empty() {
            return Ok(if bytes.is_empty() && !oversized {
                None
            } else {
                Some((bytes, oversized))
            });
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let consumed = newline.map(|i| i + 1).unwrap_or(available.len());
        let remaining = LIMIT.saturating_sub(bytes.len());
        bytes.extend_from_slice(&available[..consumed.min(remaining)]);
        if consumed > remaining {
            oversized = true;
        }
        input.consume(consumed);
        if newline.is_some() {
            return Ok(Some((bytes, oversized)));
        }
    }
}
pub async fn run_stdio() -> ApiResult<()> {
    let mut mcp = Mcp::new(&std::env::vars().collect(), HttpBackend::default())?;
    let mut input = tokio::io::BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    while let Some((bytes, oversized)) = read_message(&mut input)
        .await
        .map_err(|_| failure(500, "MCP_IO", "Could not read standard input."))?
    {
        let result = if oversized {
            Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Message too large."}}),
            )
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(message) => mcp.handle(&message).await,
                Err(_) => Some(
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error."}}),
                ),
            }
        };
        if let Some(result) = result {
            output
                .write_all(format!("{result}\n").as_bytes())
                .await
                .map_err(|_| failure(500, "MCP_IO", "Could not write standard output."))?;
            output
                .flush()
                .await
                .map_err(|_| failure(500, "MCP_IO", "Could not flush standard output."))?;
        }
    }
    Ok(())
}
