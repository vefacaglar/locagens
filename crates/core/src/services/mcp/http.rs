//! Streamable HTTP MCP client (spec 2025-03-26 / 2025-06-18): per-server
//! session with the `initialize` handshake, `Mcp-Session-Id` and
//! `MCP-Protocol-Version` headers, JSON or SSE replies, re-initialization when
//! the server forgets the session (404) and `DELETE` on shutdown.
//!
//! Like Codex's rmcp client this speaks only the streamable transport; the
//! legacy HTTP+SSE transport (GET stream + POST endpoint event) is not
//! implemented. Auth is static headers / bearer env var from the config, else
//! a stored MCP OAuth token (see `oauth.rs`).

use super::oauth::{self, TokenStore};
use super::{truncate_chars, unwrap_rpc_response, McpServerConfig, MCP_CLIENT_NAME, MCP_CLIENT_VERSION};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const HTTP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Bound for one request including a streamed SSE reply.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
const DELETE_TIMEOUT: Duration = Duration::from_secs(3);
/// Present in every "sign in required" error; `probe_server` keys off it.
pub(super) const LOGIN_HINT: &str = "lcgn mcp login";

/// Transports served by this client.
pub(super) fn is_remote(transport: &str) -> bool {
    matches!(transport, "http" | "sse" | "streamable-http")
}

// ----------------------------------------------------------------------
// Headers
// ----------------------------------------------------------------------

/// Static headers from the config: `http_headers`, `env_http_headers`
/// (header -> env var name, skipped when unset/empty) and
/// `bearer_token_env_var` (sent as `Authorization: Bearer`).
pub(super) fn static_headers(
    config: &McpServerConfig,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, value) in config.http_headers.iter().flatten() {
        out.push((name.clone(), value.clone()));
    }
    // Only user-scope config may turn environment variables into request
    // headers: a project's `mcp.json` could otherwise send any secret of
    // the user's environment to any URL it names.
    if config.scope == "project" {
        return out;
    }
    for (name, var) in config.env_http_headers.iter().flatten() {
        if let Some(value) = env(var).filter(|v| !v.is_empty()) {
            out.push((name.clone(), value));
        }
    }
    if let Some(var) = config.bearer_token_env_var.as_deref() {
        if let Some(token) = env(var).filter(|v| !v.is_empty()) {
            out.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
    }
    out
}

fn insert_header(map: &mut HeaderMap, name: &str, value: &str, sensitive: bool) -> Result<(), String> {
    let header_name =
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| format!("Invalid MCP header name '{name}'."))?;
    let mut header_value =
        HeaderValue::from_str(value).map_err(|_| format!("Invalid value for MCP header '{name}'."))?;
    header_value.set_sensitive(sensitive);
    map.insert(header_name, header_value);
    Ok(())
}

// ----------------------------------------------------------------------
// SSE parsing
// ----------------------------------------------------------------------

/// Incremental `text/event-stream` parser yielding the `data` of each event.
#[derive(Default)]
pub(super) struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    /// Feeds bytes; returns the data payload of every completed event.
    pub(super) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend(chunk.iter().copied().filter(|b| *b != b'\r'));
        let mut events = Vec::new();
        while let Some(end) = self.buffer.windows(2).position(|w| w == b"\n\n") {
            let raw: Vec<u8> = self.buffer.drain(..end + 2).collect();
            let text = String::from_utf8_lossy(&raw);
            let data: Vec<&str> = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                events.push(data.join("\n"));
            }
        }
        events
    }
}

/// A payload is one JSON-RPC message or a batch array of them.
fn messages_from(data: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Array(items)) => items,
        Ok(value) => vec![value],
        Err(_) => Vec::new(),
    }
}

fn is_response_to(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
        && message.get("method").is_none()
        && (message.get("result").is_some() || message.get("error").is_some())
}

// ----------------------------------------------------------------------
// Session
// ----------------------------------------------------------------------

enum HttpError {
    /// 404 while holding a session id: the server dropped the session.
    SessionExpired,
    /// 401 from the server.
    Unauthorized,
    Other(String),
}

impl From<String> for HttpError {
    fn from(message: String) -> Self {
        HttpError::Other(message)
    }
}

pub(super) struct HttpSession {
    client: reqwest::Client,
    url: String,
    store: TokenStore,
    session_id: Option<String>,
    protocol_version: String,
    initialized: bool,
    next_id: u64,
}

impl HttpSession {
    pub(super) fn new(url: &str, store: TokenStore) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            url: url.to_string(),
            store,
            session_id: None,
            protocol_version: HTTP_PROTOCOL_VERSION.to_string(),
            initialized: false,
            next_id: 1,
        }
    }

    fn reset(&mut self) {
        self.session_id = None;
        self.initialized = false;
        self.protocol_version = HTTP_PROTOCOL_VERSION.to_string();
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn headers(&self, config: &McpServerConfig) -> Result<HeaderMap, HttpError> {
        let mut map = HeaderMap::new();
        insert_header(&mut map, "Accept", "application/json, text/event-stream", false)?;
        insert_header(&mut map, "Content-Type", "application/json", false)?;
        let mut has_authorization = false;
        for (name, value) in static_headers(config, &|var| std::env::var(var).ok()) {
            has_authorization |= name.eq_ignore_ascii_case("authorization");
            insert_header(&mut map, &name, &value, true)?;
        }
        if !has_authorization {
            if let Some(token) = oauth::access_token(&self.store, config, false).await? {
                insert_header(&mut map, "Authorization", &format!("Bearer {token}"), true)?;
            }
        }
        if let Some(id) = &self.session_id {
            insert_header(&mut map, "Mcp-Session-Id", id, false)?;
        }
        if self.initialized {
            insert_header(&mut map, "MCP-Protocol-Version", &self.protocol_version, false)?;
        }
        Ok(map)
    }

    /// POSTs one JSON-RPC message and checks the HTTP status.
    async fn post(&mut self, config: &McpServerConfig, body: &Value) -> Result<reqwest::Response, HttpError> {
        let headers = self.headers(config).await?;
        let response = self
            .client
            .post(&self.url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|err| HttpError::Other(format!("MCP HTTP request to '{}' failed: {err}", self.url)))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(HttpError::Unauthorized);
        }
        if status == reqwest::StatusCode::NOT_FOUND && self.session_id.is_some() {
            return Err(HttpError::SessionExpired);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(HttpError::Other(format!(
                "MCP HTTP endpoint returned {status}: {}",
                truncate_chars(&text, 400)
            )));
        }
        if let Some(id) = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session_id = Some(id.to_string());
        }
        Ok(response)
    }

    /// Answers a server-initiated request: `ping` gets `{}`, anything else
    /// "method not found". Failures are ignored (the reply is best effort).
    async fn reply_to_server_request(&mut self, config: &McpServerConfig, message: &Value) {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let reply = if message.get("method").and_then(Value::as_str) == Some("ping") {
            json!({ "jsonrpc": "2.0", "id": id, "result": {} })
        } else {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })
        };
        let _ = self.post(config, &reply).await;
    }

    /// Reads the reply to request `id` from a JSON or SSE response body.
    async fn read_reply(
        &mut self,
        config: &McpServerConfig,
        response: reqwest::Response,
        id: u64,
    ) -> Result<Value, HttpError> {
        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/event-stream"));
        if !is_sse {
            let text = response
                .text()
                .await
                .map_err(|err| HttpError::Other(format!("Failed to read MCP response: {err}")))?;
            return messages_from(&text)
                .into_iter()
                .find(|m| is_response_to(m, id))
                .ok_or_else(|| HttpError::Other(format!("MCP HTTP endpoint '{}' returned no JSON-RPC response.", self.url)));
        }
        let mut stream = response.bytes_stream();
        let mut parser = SseParser::default();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| HttpError::Other(format!("MCP event stream failed: {err}")))?;
            for data in parser.push(&chunk) {
                for message in messages_from(&data) {
                    if is_response_to(&message, id) {
                        return Ok(message);
                    }
                    let is_request = message.get("method").is_some() && message.get("id").is_some();
                    if is_request {
                        self.reply_to_server_request(config, &message).await;
                    }
                }
            }
        }
        Err(HttpError::Other("MCP event stream ended without a response.".to_string()))
    }

    async fn ensure_initialized(&mut self, config: &McpServerConfig) -> Result<(), HttpError> {
        if self.initialized {
            return Ok(());
        }
        let id = self.take_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": HTTP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": MCP_CLIENT_NAME, "version": MCP_CLIENT_VERSION }
            }
        });
        let response = self.post(config, &body).await?;
        let reply = self.read_reply(config, response, id).await?;
        let result = unwrap_rpc_response(reply)?;
        if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
            self.protocol_version = version.to_string();
        }
        self.initialized = true;
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        self.post(config, &notification).await?;
        Ok(())
    }

    async fn attempt(&mut self, config: &McpServerConfig, method: &str, params: &Value) -> Result<Value, HttpError> {
        self.ensure_initialized(config).await?;
        let id = self.take_id();
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.post(config, &body).await?;
        let reply = self.read_reply(config, response, id).await?;
        Ok(unwrap_rpc_response(reply)?)
    }

    /// One JSON-RPC call, re-initializing once on an expired session and
    /// refreshing the OAuth token once on a 401.
    pub(super) async fn request(&mut self, config: &McpServerConfig, method: &str, params: &Value) -> Result<Value, String> {
        let (mut reinitialized, mut refreshed) = (false, false);
        loop {
            match self.attempt(config, method, params).await {
                Ok(value) => return Ok(value),
                Err(HttpError::SessionExpired) if !reinitialized => {
                    reinitialized = true;
                    self.reset();
                }
                Err(HttpError::Unauthorized) if !refreshed => {
                    refreshed = true;
                    let renewed = oauth::access_token(&self.store, config, true).await.ok().flatten();
                    if renewed.is_none() {
                        return Err(unauthorized_message(config, &self.store));
                    }
                }
                Err(HttpError::Unauthorized) => return Err(unauthorized_message(config, &self.store)),
                Err(HttpError::SessionExpired) => {
                    return Err(format!("MCP server '{}' keeps dropping the session.", config.name))
                }
                Err(HttpError::Other(message)) => return Err(message),
            }
        }
    }

    /// Ends the server-side session (best effort; servers may answer 405).
    pub(super) async fn terminate(&mut self, config: Option<&McpServerConfig>) {
        let Some(id) = self.session_id.take() else { return };
        let mut headers = match config {
            Some(config) => self.headers(config).await.unwrap_or_default(),
            None => HeaderMap::new(),
        };
        let _ = insert_header(&mut headers, "Mcp-Session-Id", &id, false);
        let _ = tokio::time::timeout(DELETE_TIMEOUT, self.client.delete(&self.url).headers(headers).send()).await;
        self.initialized = false;
    }
}

fn unauthorized_message(config: &McpServerConfig, store: &TokenStore) -> String {
    if oauth::is_signed_in(store, config) {
        format!(
            "MCP server '{0}' rejected the stored credentials; sign in again with `{LOGIN_HINT} {0}`.",
            config.name
        )
    } else {
        format!("MCP server '{0}' requires authentication; run `{LOGIN_HINT} {0}`.", config.name)
    }
}

// ----------------------------------------------------------------------
// Session registry
// ----------------------------------------------------------------------

type SharedSession = Arc<tokio::sync::Mutex<HttpSession>>;

/// Live sessions with the config used to open them (for the DELETE auth).
fn registry() -> &'static Mutex<HashMap<String, (SharedSession, McpServerConfig)>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, (SharedSession, McpServerConfig)>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_for(config: &McpServerConfig, url: &str) -> SharedSession {
    let key = format!("{}|{url}", config.name);
    let mut map = registry().lock().expect("mcp session registry poisoned");
    let entry = map.entry(key).or_insert_with(|| {
        let session = HttpSession::new(url, TokenStore::default_location());
        (Arc::new(tokio::sync::Mutex::new(session)), config.clone())
    });
    entry.1 = config.clone();
    entry.0.clone()
}

/// One JSON-RPC call over the server's shared streamable HTTP session.
pub(super) async fn rpc_http(config: &McpServerConfig, method: &str, params: &Value) -> Result<Value, String> {
    let url = config
        .url
        .clone()
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| format!("MCP server '{}' has no url.", config.name))?;
    let session = session_for(config, &url);
    let mut session = session.lock().await;
    tokio::time::timeout(HTTP_TIMEOUT, session.request(config, method, params))
        .await
        .map_err(|_| format!("MCP server '{}' timed out after {}s.", config.name, HTTP_TIMEOUT.as_secs()))?
}

fn take_sessions(only_name: Option<&str>) -> Vec<(SharedSession, McpServerConfig)> {
    let mut map = registry().lock().expect("mcp session registry poisoned");
    let keys: Vec<String> = map
        .iter()
        .filter(|(_, (_, config))| only_name.is_none_or(|name| config.name == name))
        .map(|(key, _)| key.clone())
        .collect();
    keys.into_iter().filter_map(|key| map.remove(&key)).collect()
}

/// DELETEs every open HTTP session (call before the process exits).
pub async fn shutdown_sessions() {
    for (session, config) in take_sessions(None) {
        session.lock().await.terminate(Some(&config)).await;
    }
}

/// Drops the sessions of one server (config changed); the DELETE runs in the
/// background when a runtime is available.
pub(super) fn forget_server(name: &str) {
    let sessions = take_sessions(Some(name));
    if sessions.is_empty() {
        return;
    }
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            for (session, config) in sessions {
                session.lock().await.terminate(Some(&config)).await;
            }
        });
    }
}
