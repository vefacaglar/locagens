//! MCP OAuth for remote servers (MCP authorization spec, as used by Codex's
//! `rmcp-client`): protected-resource + authorization-server discovery,
//! dynamic client registration, PKCE authorization-code flow over a
//! `127.0.0.1` loopback callback, token storage and refresh.
//!
//! Tokens live in `~/.locagens/mcp-oauth.json` (`$LOCAGENS_HOME`), written
//! atomically with mode 0600 on unix. They are never logged and never stored in
//! the project or app data.

use super::McpServerConfig;
use locagens_config::oauth::callback_server::{CallbackConfig, CallbackServer};
use locagens_config::oauth::pkce::{generate_pkce, random_base64url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use url::Url;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Access tokens are refreshed this long before they expire.
const EXPIRY_SKEW_MS: i64 = 60_000;
const CALLBACK_PATH: &str = "/callback";
const CLIENT_NAME: &str = "Locagens";

/// One stored sign-in. `Debug` is implemented by hand so tokens never print.
#[derive(Serialize, Deserialize, Clone)]
pub struct OAuthEntry {
    pub server_url: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Epoch milliseconds; `None` when the server did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

impl std::fmt::Debug for OAuthEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthEntry")
            .field("server_url", &self.server_url)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize, Default)]
struct StoreFile {
    #[serde(default)]
    servers: BTreeMap<String, OAuthEntry>,
}

static STORE_LOCK: Mutex<()> = Mutex::new(());

/// File-backed token store (`mcp-oauth.json`).
#[derive(Clone, Debug)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    pub fn default_location() -> Self {
        Self { path: locagens_config::paths::locagens_home().join("mcp-oauth.json") }
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// A missing file is an empty store; an unreadable or unparsable one is an
    /// error so a write never replaces other servers' tokens with a blank file.
    fn load(&self) -> Result<StoreFile, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|err| {
                format!("{} is not valid JSON ({err}); fix or delete it before signing in again.", self.path.display())
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
            Err(err) => Err(format!("Cannot read {}: {err}", self.path.display())),
        }
    }

    fn save(&self, file: &StoreFile) -> Result<(), String> {
        let body = serde_json::to_vec_pretty(file).map_err(|err| err.to_string())?;
        locagens_config::private_file::write_private_atomic(&self.path, &body).map_err(|err| err.to_string())
    }

    pub fn get(&self, key: &str) -> Option<OAuthEntry> {
        let _guard = STORE_LOCK.lock().ok()?;
        self.load().ok()?.servers.get(key).cloned()
    }

    /// Read-modify-write under the in-process mutex and the cross-process
    /// `mcp-oauth.json.lock` directory lock.
    fn update<T>(&self, change: impl FnOnce(&mut StoreFile) -> T) -> Result<T, String> {
        let _guard = STORE_LOCK.lock().map_err(|err| err.to_string())?;
        let _file_lock = locagens_config::oauth::auth_lock::AuthFileLock::acquire_blocking(&self.path)
            .map_err(|err| err.to_string())?;
        let mut file = self.load()?;
        let result = change(&mut file);
        self.save(&file)?;
        Ok(result)
    }

    pub fn put(&self, key: &str, entry: OAuthEntry) -> Result<(), String> {
        self.update(|file| {
            file.servers.insert(key.to_string(), entry);
        })
    }

    /// Removes a stored sign-in; returns whether one existed.
    pub fn remove(&self, key: &str) -> Result<bool, String> {
        if self.get(key).is_none() {
            return Ok(false);
        }
        self.update(|file| file.servers.remove(key).is_some())
    }

    /// Lock path for token refreshes. Separate from the store's own lock
    /// because `put` takes that one while a refresh is still in flight.
    fn refresh_lock_path(&self) -> PathBuf {
        self.path.with_file_name("mcp-oauth.refresh")
    }
}

/// One async mutex per store key: concurrent tool calls share a single refresh.
fn refresh_gate(key: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static GATES: Mutex<BTreeMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>> = Mutex::new(BTreeMap::new());
    let mut gates = GATES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    gates.entry(key.to_string()).or_default().clone()
}

/// Store key for a server: name plus URL, so editing the URL signs out.
pub fn store_key(config: &McpServerConfig) -> String {
    format!("{}|{}", config.name, config.url.as_deref().unwrap_or_default())
}

pub fn is_signed_in(store: &TokenStore, config: &McpServerConfig) -> bool {
    store.get(&store_key(config)).is_some()
}

pub fn logout(store: &TokenStore, config: &McpServerConfig) -> Result<bool, String> {
    store.remove(&store_key(config))
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| format!("Failed to build HTTP client: {err}"))
}

// ----------------------------------------------------------------------
// Discovery
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
}

/// RFC 8414 / RFC 9728 well-known URL: the suffix goes between host and path.
fn well_known(base: &Url, suffix: &str) -> String {
    let path = base.path().trim_end_matches('/');
    let origin = base.origin().ascii_serialization();
    format!("{origin}/.well-known/{suffix}{path}")
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http.get(url).header("Accept", "application/json").send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

fn str_field(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Finds the authorization server for an MCP server URL: the protected
/// resource document names it (falling back to the server origin), then its
/// RFC 8414 / OIDC metadata gives the endpoints (falling back to the default
/// `/authorize`, `/token`, `/register` paths of the 2025-03-26 spec).
pub async fn discover(http: &reqwest::Client, server_url: &str) -> Result<AuthServerMetadata, String> {
    let server = Url::parse(server_url).map_err(|err| format!("Invalid MCP server url: {err}"))?;
    let mut issuer = server.clone();
    issuer.set_path("/");
    issuer.set_query(None);

    let mut resource_urls = vec![well_known(&server, "oauth-protected-resource")];
    if server.path() != "/" {
        resource_urls.push(format!("{}/.well-known/oauth-protected-resource", server.origin().ascii_serialization()));
    }
    for url in resource_urls {
        if let Some(doc) = get_json(http, &url).await {
            let first = doc
                .get("authorization_servers")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(Value::as_str);
            if let Some(parsed) = first.and_then(|s| Url::parse(s).ok()) {
                issuer = parsed;
                break;
            }
        }
    }

    let issuer_path = issuer.path().trim_end_matches('/').to_string();
    let issuer_origin = issuer.origin().ascii_serialization();
    let candidates = [
        well_known(&issuer, "oauth-authorization-server"),
        well_known(&issuer, "openid-configuration"),
        format!("{issuer_origin}{issuer_path}/.well-known/openid-configuration"),
    ];
    let expected_issuer = issuer.as_str().trim_end_matches('/').to_string();
    for url in candidates {
        let Some(doc) = get_json(http, &url).await else { continue };
        // RFC 8414 section 3.3: the document must name the issuer it was fetched for.
        if str_field(&doc, "issuer").is_some_and(|found| found.trim_end_matches('/') != expected_issuer) {
            continue;
        }
        if let (Some(authorization_endpoint), Some(token_endpoint)) =
            (str_field(&doc, "authorization_endpoint"), str_field(&doc, "token_endpoint"))
        {
            let registration_endpoint = str_field(&doc, "registration_endpoint");
            for endpoint in [Some(&authorization_endpoint), Some(&token_endpoint), registration_endpoint.as_ref()]
                .into_iter()
                .flatten()
            {
                require_secure_endpoint(endpoint)?;
            }
            return Ok(AuthServerMetadata { authorization_endpoint, token_endpoint, registration_endpoint });
        }
    }
    let fallback = AuthServerMetadata {
        authorization_endpoint: format!("{issuer_origin}/authorize"),
        token_endpoint: format!("{issuer_origin}/token"),
        registration_endpoint: Some(format!("{issuer_origin}/register")),
    };
    require_secure_endpoint(&fallback.token_endpoint)?;
    Ok(fallback)
}

/// Endpoints that receive codes, tokens or client registrations must be https
/// (plain http only to a loopback host, for local servers and tests).
pub(super) fn require_secure_endpoint(endpoint: &str) -> Result<(), String> {
    let url = Url::parse(endpoint).map_err(|err| format!("Bad OAuth endpoint \"{endpoint}\": {err}"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(format!("Refusing OAuth endpoint \"{endpoint}\": it must use https."))
    }
}

/// RFC 7591 dynamic registration of a public loopback client.
async fn register_client(
    http: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let response = http
        .post(endpoint)
        .json(&json!({
            "client_name": CLIENT_NAME,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .map_err(|err| format!("Client registration failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("Client registration was rejected ({}).", response.status()));
    }
    let doc: Value = response.json().await.map_err(|err| format!("Bad registration response: {err}"))?;
    let client_id = str_field(&doc, "client_id").ok_or("Registration response has no client_id.")?;
    Ok((client_id, str_field(&doc, "client_secret")))
}

// ----------------------------------------------------------------------
// Token endpoint
// ----------------------------------------------------------------------

struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

async fn token_request(
    http: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, String> {
    let response = http
        .post(endpoint)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|err| format!("Token request failed: {err}"))?;
    let status = response.status();
    let doc: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let reason = str_field(&doc, "error_description")
            .or_else(|| str_field(&doc, "error"))
            .unwrap_or_else(|| "no detail".to_string());
        return Err(format!("Token endpoint returned {status}: {reason}"));
    }
    Ok(TokenResponse {
        access_token: str_field(&doc, "access_token").ok_or("Token response has no access_token.")?,
        refresh_token: str_field(&doc, "refresh_token"),
        expires_at: expires_at_from(doc.get("expires_in")),
    })
}

/// `expires_in` is seconds, sometimes a float (`3600.0`) or a numeric string.
pub(super) fn expires_at_from(expires_in: Option<&Value>) -> Option<i64> {
    let secs = match expires_in? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then(|| now_ms().saturating_add((secs.min(1e9) * 1000.0) as i64))
}

// ----------------------------------------------------------------------
// Login / refresh
// ----------------------------------------------------------------------

/// Runs the browser login. `on_url` receives the authorization URL to open or
/// print; the flow completes when the loopback redirect arrives.
pub async fn login(
    store: &TokenStore,
    config: &McpServerConfig,
    on_url: &(dyn Fn(&str) + Sync),
) -> Result<(), String> {
    let server_url = config
        .url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| format!("MCP server '{}' has no url.", config.name))?;
    let http = http_client()?;
    let meta = discover(&http, &server_url).await?;

    let state = random_base64url(32);
    let callback = CallbackServer::start(CallbackConfig {
        provider_name: format!("MCP server {}", config.name),
        port: 0,
        path: CALLBACK_PATH.to_string(),
        expected_state: Some(state.clone()),
        timeout: Some(LOGIN_TIMEOUT),
    })
    .await
    .map_err(|err| err.to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{}{CALLBACK_PATH}", callback.port());

    let (client_id, client_secret) = match config.oauth_client_id.clone().filter(|id| !id.is_empty()) {
        Some(id) => (id, None),
        None => {
            let endpoint = meta.registration_endpoint.as_deref().ok_or_else(|| {
                "The authorization server has no dynamic client registration; set oauth_client_id.".to_string()
            })?;
            register_client(&http, endpoint, &redirect_uri).await?
        }
    };

    let pkce = generate_pkce();
    let mut auth_url = Url::parse(&meta.authorization_endpoint).map_err(|err| format!("Bad authorization endpoint: {err}"))?;
    {
        let mut query = auth_url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", &server_url);
        if let Some(scopes) = config.scopes.as_ref().filter(|s| !s.is_empty()) {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    on_url(auth_url.as_str());

    let params = callback.wait().await.map_err(|err| err.to_string())?;
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", params.code()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", pkce.verifier.as_str()),
        ("resource", server_url.as_str()),
    ];
    if let Some(secret) = client_secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let tokens = token_request(&http, &meta.token_endpoint, &form).await?;
    store.put(
        &store_key(config),
        OAuthEntry {
            server_url,
            client_id,
            client_secret,
            token_endpoint: meta.token_endpoint,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
        },
    )
}

async fn refresh(store: &TokenStore, key: &str, entry: &OAuthEntry) -> Result<Option<OAuthEntry>, String> {
    let Some(refresh_token) = entry.refresh_token.as_deref() else { return Ok(None) };
    let http = http_client()?;
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", entry.client_id.as_str()),
        ("resource", entry.server_url.as_str()),
    ];
    if let Some(secret) = entry.client_secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let tokens = token_request(&http, &entry.token_endpoint, &form).await?;
    let updated = OAuthEntry {
        access_token: tokens.access_token,
        // Servers that do not rotate refresh tokens omit the field.
        refresh_token: tokens.refresh_token.or_else(|| entry.refresh_token.clone()),
        expires_at: tokens.expires_at,
        ..entry.clone()
    };
    store.put(key, updated.clone())?;
    Ok(Some(updated))
}

/// The bearer token to send, refreshing it when it is about to expire (or
/// unconditionally with `force`, after a 401). `Ok(None)` means not signed in
/// or no way to renew.
pub async fn access_token(
    store: &TokenStore,
    config: &McpServerConfig,
    force_refresh: bool,
) -> Result<Option<String>, String> {
    let key = store_key(config);
    let Some(entry) = store.get(&key) else { return Ok(None) };
    let expiring = entry.expires_at.is_some_and(|at| at - EXPIRY_SKEW_MS <= now_ms());
    if !force_refresh && !expiring {
        return Ok(Some(entry.access_token));
    }
    // A token that is merely close to expiry stays usable if renewing fails.
    let still_valid = !force_refresh && !entry.expires_at.is_some_and(|at| at <= now_ms());

    // Single flight: one refresh per server in this process (async mutex) and
    // across processes (lock directory), because refresh tokens may rotate.
    let gate = refresh_gate(&key);
    let _in_process = gate.lock().await;
    let _cross_process = locagens_config::oauth::auth_lock::AuthFileLock::acquire(&store.refresh_lock_path())
        .await
        .map_err(|err| err.to_string())?;
    // Someone else may have refreshed while we waited.
    let entry = match store.get(&key) {
        Some(current) if current.access_token != entry.access_token => {
            let expiring = current.expires_at.is_some_and(|at| at - EXPIRY_SKEW_MS <= now_ms());
            if !expiring {
                return Ok(Some(current.access_token));
            }
            current
        }
        Some(current) => current,
        None => return Ok(None),
    };
    match refresh(store, &key, &entry).await {
        Ok(Some(updated)) => Ok(Some(updated.access_token)),
        Ok(None) => Ok(still_valid.then_some(entry.access_token)),
        Err(err) if still_valid => {
            let _ = err;
            Ok(Some(entry.access_token))
        }
        Err(err) => Err(err),
    }
}

/// Opens an http(s) URL in the default browser (never through a shell).
pub fn open_browser(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|err| format!("Invalid URL: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http(s) URLs can be opened.".to_string());
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", parsed.as_str()]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(parsed.as_str());
        command
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(parsed.as_str());
        command
    };
    command.spawn().map(|_| ()).map_err(|err| format!("Could not open the browser: {err}"))
}
