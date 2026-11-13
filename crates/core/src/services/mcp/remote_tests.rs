//! Streamable HTTP + MCP OAuth tests against local axum mock servers.

use super::http::{static_headers, HttpSession, SseParser};
use super::oauth::{self, OAuthEntry, TokenStore};
use super::{probe_server, McpServerConfig};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    sse: bool,
    require_bearer: Option<String>,
    sessions: HashSet<String>,
    initializes: u32,
    deletes: u32,
    ping_replies: u32,
    protocol_headers: Vec<String>,
    custom_headers: Vec<String>,
    token_forms: Vec<HashMap<String, String>>,
    base: String,
}

type Shared = Arc<Mutex<Mock>>;

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn reply(sse: bool, messages: Vec<Value>, session: Option<&str>) -> Response {
    let mut headers = HeaderMap::new();
    if let Some(id) = session {
        headers.insert("mcp-session-id", HeaderValue::from_str(id).unwrap());
    }
    if sse {
        let body: String = messages.iter().map(|m| format!("event: message\ndata: {m}\n\n")).collect();
        headers.insert("content-type", HeaderValue::from_static("text/event-stream"));
        (StatusCode::OK, headers, body).into_response()
    } else {
        (StatusCode::OK, headers, Json(messages.into_iter().last().unwrap())).into_response()
    }
}

async fn mcp_post(State(mock): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let message: Value = serde_json::from_str(&body).unwrap();
    let mut mock = mock.lock().unwrap();
    if let Some(expected) = &mock.require_bearer {
        let got = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
        if got != format!("Bearer {expected}") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    if let Some(v) = headers.get("x-test").and_then(|v| v.to_str().ok()) {
        mock.custom_headers.push(v.to_string());
    }
    let method = message.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let sse = mock.sse;
    // Replies to server requests (our ping answer) carry an id and no method.
    if method.is_empty() {
        if message.get("result").is_some() && id == json!("srv-1") {
            mock.ping_replies += 1;
        }
        return StatusCode::ACCEPTED.into_response();
    }
    if method == "initialize" {
        mock.initializes += 1;
        let session = format!("sess-{}", mock.initializes);
        mock.sessions.insert(session.clone());
        let result = json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "mock", "version": "1" } });
        return reply(sse, vec![rpc_result(&id, result)], Some(&session));
    }
    let session = headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !mock.sessions.contains(session) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }
    if let Some(v) = headers.get("mcp-protocol-version").and_then(|v| v.to_str().ok()) {
        mock.protocol_headers.push(v.to_string());
    }
    let result = match method.as_str() {
        "tools/list" => json!({ "tools": [{ "name": "echo", "description": "Echo", "inputSchema": { "type": "object" } }] }),
        "tools/call" => json!({ "content": [{ "type": "text", "text": "hello over http" }] }),
        _ => json!({}),
    };
    let mut messages = Vec::new();
    if sse {
        messages.push(json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "ping" }));
        messages.push(json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }));
    }
    messages.push(rpc_result(&id, result));
    reply(sse, messages, None)
}

async fn mcp_delete(State(mock): State<Shared>, headers: HeaderMap) -> StatusCode {
    let mut mock = mock.lock().unwrap();
    let session = headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).unwrap_or_default();
    if mock.sessions.remove(session) {
        mock.deletes += 1;
    }
    StatusCode::OK
}

async fn protected_resource(State(mock): State<Shared>) -> Json<Value> {
    let base = mock.lock().unwrap().base.clone();
    Json(json!({ "resource": format!("{base}/mcp"), "authorization_servers": [base] }))
}

async fn auth_metadata(State(mock): State<Shared>) -> Json<Value> {
    let base = mock.lock().unwrap().base.clone();
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "registration_endpoint": format!("{base}/register"),
    }))
}

async fn register(Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert!(body["redirect_uris"][0].as_str().unwrap().starts_with("http://127.0.0.1:"));
    Json(json!({ "client_id": "cid-1" }))
}

async fn token(State(mock): State<Shared>, Form(form): Form<HashMap<String, String>>) -> Response {
    let mut mock = mock.lock().unwrap();
    let grant = form.get("grant_type").cloned().unwrap_or_default();
    let n = mock.token_forms.len() + 1;
    mock.token_forms.push(form);
    let (access, refresh) = if grant == "authorization_code" {
        ("at-1".to_string(), "rt-1".to_string())
    } else {
        (format!("at-{}", n + 1), format!("rt-{}", n + 1))
    };
    Json(json!({ "access_token": access, "refresh_token": refresh, "expires_in": 3600, "token_type": "Bearer" })).into_response()
}

async fn spawn_mock(sse: bool, require_bearer: Option<&str>) -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mock: Shared = Arc::new(Mutex::new(Mock {
        sse,
        require_bearer: require_bearer.map(str::to_string),
        base: base.clone(),
        ..Mock::default()
    }));
    let app = Router::new()
        .route("/mcp", post(mcp_post).delete(mcp_delete))
        .route("/.well-known/oauth-protected-resource/mcp", get(protected_resource))
        .route("/.well-known/oauth-authorization-server", get(auth_metadata))
        .route("/register", post(register))
        .route("/token", post(token))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

fn remote_config(name: &str, url: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        scope: "user".to_string(),
        transport: "streamable-http".to_string(),
        command: None,
        args: None,
        env: None,
        url: Some(url.to_string()),
        enabled: true,
        project_path: None,
        bearer_token_env_var: None,
        http_headers: None,
        env_http_headers: None,
        oauth_client_id: None,
        scopes: None,
    }
}

fn temp_store() -> (tempfile::TempDir, TokenStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::at(dir.path().join("mcp-oauth.json"));
    (dir, store)
}

#[tokio::test]
async fn json_replies_share_one_session_and_shutdown_deletes_it() {
    let (base, mock) = spawn_mock(false, None).await;
    let config = remote_config("json-srv", &format!("{base}/mcp"));
    let (_dir, store) = temp_store();
    let mut session = HttpSession::new(config.url.as_deref().unwrap(), store);

    let tools = session.request(&config, "tools/list", &json!({})).await.unwrap();
    assert_eq!(tools["tools"][0]["name"], "echo");
    let call = session.request(&config, "tools/call", &json!({ "name": "echo" })).await.unwrap();
    assert_eq!(call["content"][0]["text"], "hello over http");

    {
        let mock = mock.lock().unwrap();
        assert_eq!(mock.initializes, 1, "handshake happens once per session");
        assert_eq!(mock.protocol_headers, vec!["2025-06-18", "2025-06-18"]);
        assert_eq!(mock.sessions.len(), 1);
    }
    session.terminate(Some(&config)).await;
    let mock = mock.lock().unwrap();
    assert_eq!(mock.deletes, 1);
    assert!(mock.sessions.is_empty());
}

#[tokio::test]
async fn sse_replies_are_parsed_and_server_pings_are_answered() {
    let (base, mock) = spawn_mock(true, None).await;
    let config = remote_config("sse-srv", &format!("{base}/mcp"));
    let (_dir, store) = temp_store();
    let mut session = HttpSession::new(config.url.as_deref().unwrap(), store);

    let tools = session.request(&config, "tools/list", &json!({})).await.unwrap();
    assert_eq!(tools["tools"][0]["name"], "echo");
    assert_eq!(mock.lock().unwrap().ping_replies, 1);
}

#[tokio::test]
async fn expired_session_triggers_a_fresh_initialize() {
    let (base, mock) = spawn_mock(false, None).await;
    let config = remote_config("expiring", &format!("{base}/mcp"));
    let (_dir, store) = temp_store();
    let mut session = HttpSession::new(config.url.as_deref().unwrap(), store);

    session.request(&config, "tools/list", &json!({})).await.unwrap();
    mock.lock().unwrap().sessions.clear(); // server forgets the session -> 404
    let tools = session.request(&config, "tools/list", &json!({})).await.unwrap();
    assert_eq!(tools["tools"][0]["name"], "echo");
    assert_eq!(mock.lock().unwrap().initializes, 2);
}

#[test]
fn static_headers_resolve_env_and_bearer() {
    let mut config = remote_config("h", "http://x/mcp");
    config.http_headers = Some(BTreeMap::from([("X-Test".to_string(), "abc".to_string())]));
    config.env_http_headers = Some(BTreeMap::from([
        ("X-Env".to_string(), "SET_VAR".to_string()),
        ("X-Missing".to_string(), "UNSET_VAR".to_string()),
    ]));
    config.bearer_token_env_var = Some("TOKEN_VAR".to_string());
    let env = |name: &str| match name {
        "SET_VAR" => Some("from-env".to_string()),
        "TOKEN_VAR" => Some("s3cret".to_string()),
        _ => None,
    };
    let headers = static_headers(&config, &env);
    assert!(headers.contains(&("X-Test".to_string(), "abc".to_string())));
    assert!(headers.contains(&("X-Env".to_string(), "from-env".to_string())));
    assert!(headers.contains(&("Authorization".to_string(), "Bearer s3cret".to_string())));
    assert!(!headers.iter().any(|(name, _)| name == "X-Missing"));
}

#[tokio::test]
async fn configured_bearer_and_custom_headers_are_sent() {
    let (base, mock) = spawn_mock(false, Some("env-token-123")).await;
    std::env::set_var("LOCAGENS_TEST_MCP_BEARER_A", "env-token-123");
    let mut config = remote_config("authed", &format!("{base}/mcp"));
    config.bearer_token_env_var = Some("LOCAGENS_TEST_MCP_BEARER_A".to_string());
    config.http_headers = Some(BTreeMap::from([("X-Test".to_string(), "abc".to_string())]));
    let (_dir, store) = temp_store();
    let mut session = HttpSession::new(config.url.as_deref().unwrap(), store);

    session.request(&config, "tools/list", &json!({})).await.unwrap();
    assert!(mock.lock().unwrap().custom_headers.iter().all(|v| v == "abc"));
    assert!(!mock.lock().unwrap().custom_headers.is_empty());
}

#[tokio::test]
async fn unauthorized_without_credentials_asks_for_login() {
    let (base, _mock) = spawn_mock(false, Some("needed")).await;
    let config = remote_config("locked", &format!("{base}/mcp"));
    let info = probe_server(&config).await;
    assert_eq!(info.status, "error");
    assert!(info.needs_login, "error: {:?}", info.error);
    assert!(info.error.unwrap().contains("lcgn mcp login locked"));
}

#[tokio::test]
async fn unauthorized_with_expired_token_refreshes_and_retries() {
    let (base, mock) = spawn_mock(false, Some("at-2")).await;
    let config = remote_config("oauth-srv", &format!("{base}/mcp"));
    let (_dir, store) = temp_store();
    store
        .put(
            &oauth::store_key(&config),
            OAuthEntry {
                server_url: format!("{base}/mcp"),
                client_id: "cid-1".to_string(),
                client_secret: None,
                token_endpoint: format!("{base}/token"),
                access_token: "stale".to_string(),
                refresh_token: Some("rt-0".to_string()),
                expires_at: None, // looks valid, the server disagrees
            },
        )
        .unwrap();
    let mut session = HttpSession::new(config.url.as_deref().unwrap(), store.clone());

    let tools = session.request(&config, "tools/list", &json!({})).await.unwrap();
    assert_eq!(tools["tools"][0]["name"], "echo");
    let forms = mock.lock().unwrap().token_forms.clone();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0]["grant_type"], "refresh_token");
    assert_eq!(forms[0]["refresh_token"], "rt-0");
    let saved = store.get(&oauth::store_key(&config)).unwrap();
    assert_eq!(saved.access_token, "at-2");
    assert_eq!(saved.refresh_token.as_deref(), Some("rt-2"));
}

#[tokio::test]
async fn oauth_login_discovers_registers_exchanges_and_refreshes() {
    let (base, mock) = spawn_mock(false, None).await;
    let config = remote_config("login-srv", &format!("{base}/mcp"));
    let (dir, store) = temp_store();

    let challenge = Arc::new(Mutex::new(String::new()));
    let seen_challenge = challenge.clone();
    let on_url = move |auth_url: &str| {
        let url = url::Url::parse(auth_url).unwrap();
        let query: HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        assert_eq!(url.path(), "/authorize");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "cid-1");
        assert_eq!(query["code_challenge_method"], "S256");
        assert!(query["resource"].ends_with("/mcp"));
        *seen_challenge.lock().unwrap() = query["code_challenge"].clone();
        // Play the browser: hit the loopback redirect with a code.
        let redirect = format!("{}?code=auth-code&state={}", query["redirect_uri"], query["state"]);
        tokio::spawn(async move {
            reqwest::get(redirect).await.unwrap();
        });
    };
    oauth::login(&store, &config, &on_url).await.unwrap();

    let forms = mock.lock().unwrap().token_forms.clone();
    assert_eq!(forms[0]["grant_type"], "authorization_code");
    assert_eq!(forms[0]["code"], "auth-code");
    assert_eq!(forms[0]["client_id"], "cid-1");
    assert_eq!(
        locagens_config::oauth::pkce::challenge_for(&forms[0]["code_verifier"]),
        *challenge.lock().unwrap(),
        "verifier must match the challenge sent to the authorization endpoint"
    );
    assert!(oauth::is_signed_in(&store, &config));
    assert_eq!(oauth::access_token(&store, &config, false).await.unwrap().as_deref(), Some("at-1"));

    // Expired -> refreshed with the rotated refresh token stored.
    let key = oauth::store_key(&config);
    let mut entry = store.get(&key).unwrap();
    entry.expires_at = Some(1);
    store.put(&key, entry).unwrap();
    let refreshed = oauth::access_token(&store, &config, false).await.unwrap();
    assert_eq!(refreshed.as_deref(), Some("at-3"));
    assert_eq!(store.get(&key).unwrap().refresh_token.as_deref(), Some("rt-3"));

    // Tokens are on disk only in the injected store and never in Debug output.
    assert!(dir.path().join("mcp-oauth.json").exists());
    assert!(!format!("{:?}", store.get(&key).unwrap()).contains("at-3"));

    assert!(oauth::logout(&store, &config).unwrap());
    assert!(!oauth::is_signed_in(&store, &config));
    assert_eq!(oauth::access_token(&store, &config, false).await.unwrap(), None);
}

#[test]
fn sse_parser_handles_split_chunks_crlf_and_multiline_data() {
    let mut parser = SseParser::default();
    assert!(parser.push(b"event: message\r\ndata: {\"a\"").is_empty());
    let events = parser.push(b":1}\r\n\r\n: comment\n\ndata: x\ndata: y\n\n");
    assert_eq!(events, vec!["{\"a\":1}".to_string(), "x\ny".to_string()]);
}

#[test]
fn saving_from_the_form_keeps_stored_auth_settings() {
    let project = tempfile::tempdir().unwrap();
    let project_path = std::fs::canonicalize(project.path()).unwrap().to_string_lossy().to_string();
    let mut stored = remote_config("remote-auth", "http://127.0.0.1:1/mcp");
    stored.scope = "project".to_string();
    stored.project_path = Some(project_path.clone());
    stored.bearer_token_env_var = Some("MY_TOKEN".to_string());
    stored.http_headers = Some(BTreeMap::from([("X-Team".to_string(), "core".to_string())]));
    stored.scopes = Some(vec!["read".to_string()]);
    super::save_config(&stored).unwrap();

    // The edit form only knows name/transport/url/enabled.
    let mut edited = remote_config("remote-auth", "http://127.0.0.1:2/mcp");
    edited.scope = "project".to_string();
    edited.project_path = Some(project_path);
    let merged = super::keep_stored_auth(edited);

    assert_eq!(merged.url.as_deref(), Some("http://127.0.0.1:2/mcp"));
    assert_eq!(merged.bearer_token_env_var.as_deref(), Some("MY_TOKEN"));
    assert_eq!(merged.http_headers.unwrap()["X-Team"], "core");
    assert_eq!(merged.scopes.unwrap(), vec!["read".to_string()]);
}

#[test]
fn saving_from_the_form_keeps_stored_args_and_env() {
    let project = tempfile::tempdir().unwrap();
    let project_path = std::fs::canonicalize(project.path()).unwrap().to_string_lossy().to_string();
    let mut stored = remote_config("local-tool", "http://127.0.0.1:1/mcp");
    stored.transport = "stdio".to_string();
    stored.url = None;
    stored.command = Some("node".to_string());
    stored.args = Some(vec!["server.js".to_string()]);
    stored.env = Some(BTreeMap::from([("API_TOKEN".to_string(), "secret".to_string())]));
    stored.scope = "project".to_string();
    stored.project_path = Some(project_path.clone());
    super::save_config(&stored).unwrap();

    let mut edited = stored.clone();
    edited.args = None;
    edited.env = None;
    let merged = super::keep_stored_auth(edited);
    assert_eq!(merged.args.as_deref(), Some(&["server.js".to_string()][..]));
    assert_eq!(merged.env.unwrap()["API_TOKEN"], "secret");
}

#[tokio::test]
async fn concurrent_expired_calls_share_one_refresh() {
    let (base, mock) = spawn_mock(false, None).await;
    let config = remote_config("single-flight", &format!("{base}/mcp"));
    let (_dir, store) = temp_store();
    store
        .put(
            &oauth::store_key(&config),
            OAuthEntry {
                server_url: format!("{base}/mcp"),
                client_id: "cid-1".to_string(),
                client_secret: None,
                token_endpoint: format!("{base}/token"),
                access_token: "old".to_string(),
                refresh_token: Some("rt-0".to_string()),
                expires_at: Some(1),
            },
        )
        .unwrap();
    let calls = (0..4).map(|_| oauth::access_token(&store, &config, false));
    let results = futures::future::join_all(calls).await;
    for result in results {
        assert_eq!(result.unwrap().as_deref(), Some("at-2"));
    }
    assert_eq!(mock.lock().unwrap().token_forms.len(), 1);
}

#[test]
fn ui_view_masks_env_and_header_values_and_saving_it_back_keeps_them() {
    let project = tempfile::tempdir().unwrap();
    let project_path = std::fs::canonicalize(project.path()).unwrap().to_string_lossy().to_string();
    let mut stored = remote_config("masked-srv", "http://127.0.0.1:1/mcp");
    stored.scope = "project".to_string();
    stored.project_path = Some(project_path.clone());
    stored.env = Some(BTreeMap::from([("API_KEY".to_string(), "env-secret-123".to_string())]));
    stored.http_headers = Some(BTreeMap::from([("Authorization".to_string(), "Bearer hdr-secret-456".to_string())]));
    super::save_config(&stored).unwrap();

    let masked = stored.masked();
    let json = serde_json::to_string(&masked).unwrap();
    assert!(!json.contains("env-secret") && !json.contains("hdr-secret"), "{json}");
    assert!(json.contains("API_KEY") && json.contains("Authorization"));
    let debug = format!("{stored:?}");
    assert!(!debug.contains("env-secret") && !debug.contains("hdr-secret"), "{debug}");

    // The form sends the masked view back, with an extra new variable.
    let mut edited = masked;
    edited.env.as_mut().unwrap().insert("NEW".to_string(), "fresh".to_string());
    let merged = super::keep_stored_auth(edited);
    let env = merged.env.unwrap();
    assert_eq!(env["API_KEY"], "env-secret-123");
    assert_eq!(env["NEW"], "fresh");
    assert_eq!(merged.http_headers.unwrap()["Authorization"], "Bearer hdr-secret-456");
}

#[test]
fn preserve_markers_without_a_stored_value_are_dropped() {
    let mut edited = remote_config("never-stored", "http://127.0.0.1:1/mcp");
    edited.env = Some(BTreeMap::from([("GHOST".to_string(), super::PRESERVE_SECRET.to_string())]));
    assert!(super::keep_stored_auth(edited).env.unwrap().is_empty());
}

#[test]
fn unparsable_token_store_is_never_overwritten() {
    let (dir, store) = temp_store();
    let path = dir.path().join("mcp-oauth.json");
    std::fs::write(&path, "{ not json").unwrap();
    let entry = OAuthEntry {
        server_url: "https://x.test/mcp".to_string(),
        client_id: "c".to_string(),
        client_secret: None,
        token_endpoint: "https://x.test/token".to_string(),
        access_token: "a".to_string(),
        refresh_token: None,
        expires_at: None,
    };
    assert!(store.put("k", entry).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn insecure_or_foreign_oauth_endpoints_are_refused() {
    assert!(oauth::require_secure_endpoint("https://auth.example.com/token").is_ok());
    assert!(oauth::require_secure_endpoint("http://127.0.0.1:9/token").is_ok());
    assert!(oauth::require_secure_endpoint("http://auth.example.com/token").is_err());
    assert!(oauth::require_secure_endpoint("ftp://127.0.0.1/token").is_err());
}

#[test]
fn expires_in_accepts_floats_and_strings() {
    let at = |v: Value| oauth::expires_at_from(Some(&v));
    let now = oauth::now_ms();
    assert!(at(json!(3600.0)).unwrap() - now > 3_500_000);
    assert!(at(json!("3600")).unwrap() - now > 3_500_000);
    assert!(at(json!(-5)).is_none());
    assert!(oauth::expires_at_from(None).is_none());
}

#[test]
fn config_toml_servers_map_onto_user_scope() {
    let toml = r#"
[mcp_servers.Docs]
url = "https://example.test/mcp"
bearer_token_env_var = "DOCS_TOKEN"

[mcp_servers.local]
command = "node"
args = ["server.js"]
env_vars = ["LOCAGENS_TEST_MCP_FORWARDED"]
enabled = false
"#;
    std::env::set_var("LOCAGENS_TEST_MCP_FORWARDED", "yes");
    let config: locagens_config::config_toml::Config = toml::from_str(toml).unwrap();
    let mapped = super::config_toml_servers(&config.mcp_servers);
    let docs = mapped.iter().find(|server| server.name == "docs").unwrap();
    assert_eq!(docs.transport, "streamable-http");
    assert_eq!(docs.scope, "user");
    assert_eq!(docs.bearer_token_env_var.as_deref(), Some("DOCS_TOKEN"));
    let local = mapped.iter().find(|server| server.name == "local").unwrap();
    assert_eq!(local.transport, "stdio");
    assert!(!local.enabled);
    assert_eq!(local.args.as_deref(), Some(&["server.js".to_string()][..]));
    assert_eq!(local.env.as_ref().unwrap()["LOCAGENS_TEST_MCP_FORWARDED"], "yes");
}
