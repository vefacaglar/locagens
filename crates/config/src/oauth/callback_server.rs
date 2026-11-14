//! Loopback OAuth redirect receiver (port of pi's `callback-server.js`) and
//! the manual-paste fallback for headless machines.
//!
//! The server binds `127.0.0.1` only, answers browser requests with small
//! HTML pages and hands the validated redirect parameters to [`CallbackServer::wait`].
//! Cancel by dropping the server (or calling [`CallbackServer::shutdown`]).

use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Query parameters of a validated redirect (`code` is always present).
#[derive(Clone, Default)]
pub struct CallbackParams(HashMap<String, String>);

impl CallbackParams {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn code(&self) -> &str {
        self.get("code").unwrap_or_default()
    }
}

/// Authorization codes must not leak through `Debug`.
impl std::fmt::Debug for CallbackParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackParams").finish_non_exhaustive()
    }
}

/// What the server accepts and how it words its pages.
#[derive(Clone, Debug)]
pub struct CallbackConfig {
    /// Shown in the pages ("Signed in to {provider_name}").
    pub provider_name: String,
    /// `0` picks an ephemeral port.
    pub port: u16,
    /// Absolute request path, e.g. `/auth/callback`.
    pub path: String,
    /// When set, requests with another `state` are rejected and ignored.
    pub expected_state: Option<String>,
    /// Gives up after this long; `None` waits forever.
    pub timeout: Option<Duration>,
}

type CallbackResult = Result<CallbackParams, String>;

/// A running loopback receiver.
pub struct CallbackServer {
    port: u16,
    receiver: tokio::sync::Mutex<Option<oneshot::Receiver<CallbackResult>>>,
    task: JoinHandle<()>,
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl CallbackServer {
    /// Binds `127.0.0.1:{config.port}` and starts serving.
    pub async fn start(config: CallbackConfig) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", config.port))
            .await
            .map_err(|err| anyhow!("Could not listen on 127.0.0.1:{}: {err}", config.port))?;
        let port = listener.local_addr()?.port();
        let (sender, receiver) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));
        let task = tokio::spawn(serve(listener, Arc::new(config), sender));
        Ok(Self { port, receiver: tokio::sync::Mutex::new(Some(receiver)), task })
    }

    /// The bound port (the real one when an ephemeral port was requested).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Waits for a valid redirect. Fails on a provider `error` redirect, on
    /// timeout, or after [`CallbackServer::shutdown`].
    pub async fn wait(&self) -> Result<CallbackParams> {
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow!("This sign-in is already being waited on"))?;
        match receiver.await {
            Ok(Ok(params)) => Ok(params),
            Ok(Err(message)) => bail!(message),
            Err(_) => bail!("OAuth callback server closed"),
        }
    }

    /// Stops listening; a pending [`CallbackServer::wait`] fails.
    pub fn shutdown(&self) {
        self.task.abort();
    }
}

type SharedSender = Arc<Mutex<Option<oneshot::Sender<CallbackResult>>>>;

async fn serve(listener: TcpListener, config: Arc<CallbackConfig>, sender: SharedSender) {
    let accept_loop = async {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_connection(stream, config.clone(), sender.clone()));
                }
                // A persistent accept error (e.g. out of file descriptors)
                // must not spin a core.
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
    };
    match config.timeout {
        Some(limit) => {
            if tokio::time::timeout(limit, accept_loop).await.is_err() {
                let message = format!("{} sign-in timed out", config.provider_name);
                finish(&sender, Err(message));
            }
        }
        None => accept_loop.await,
    }
}

/// Delivers the result once; returns false when it was already delivered.
fn finish(sender: &SharedSender, result: CallbackResult) -> bool {
    match sender.lock().expect("callback sender poisoned").take() {
        Some(sender) => sender.send(result).is_ok(),
        None => false,
    }
}

async fn handle_connection(mut stream: TcpStream, config: Arc<CallbackConfig>, sender: SharedSender) {
    let Some((method, target)) = read_request_line(&mut stream).await else { return };
    let already_handled = sender.lock().expect("callback sender poisoned").is_none();
    let reply = if already_handled {
        Reply::error(409, "This sign-in has already been handled.", None)
    } else {
        let (reply, outcome) = decide(&config, &method, &target);
        if let Some(result) = outcome {
            finish(&sender, result);
        }
        reply
    };
    let _ = stream.write_all(reply.to_http().as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Reads the request head and returns `(method, request target)`.
async fn read_request_line(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    let read_head = async {
        while head.len() < MAX_REQUEST_BYTES && !head.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(count) => head.extend_from_slice(&chunk[..count]),
            }
        }
    };
    tokio::time::timeout(REQUEST_READ_TIMEOUT, read_head).await.ok()?;
    let text = String::from_utf8_lossy(&head);
    let mut parts = text.lines().next()?.split_whitespace();
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

/// An HTTP answer for the browser.
#[derive(Debug, PartialEq, Eq)]
struct Reply {
    status: u16,
    html: String,
}

impl Reply {
    fn success(message: &str) -> Self {
        Self { status: 200, html: page("Sign-in complete", message, None) }
    }

    fn error(status: u16, message: &str, detail: Option<&str>) -> Self {
        Self { status, html: page("Sign-in failed", message, detail) }
    }

    fn to_http(&self) -> String {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            409 => "Conflict",
            _ => "Error",
        };
        format!(
            "HTTP/1.1 {} {reason}\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            self.status,
            self.html.len(),
            self.html
        )
    }
}

/// Validates one request. Returns the browser answer and, when the sign-in is
/// settled by this request, its result.
fn decide(config: &CallbackConfig, method: &str, target: &str) -> (Reply, Option<CallbackResult>) {
    let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
        return (Reply::error(404, "Callback route not found.", None), None);
    };
    if method != "GET" || url.path() != config.path {
        return (Reply::error(404, "Callback route not found.", None), None);
    }
    let params: HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    if let Some(expected) = &config.expected_state {
        if params.get("state") != Some(expected) {
            return (Reply::error(400, "State mismatch.", None), None);
        }
    }
    if let Some(error) = params.get("error") {
        let description = params.get("error_description").unwrap_or(error);
        let title = format!("{} authorization failed.", config.provider_name);
        let failure = format!("{} authorization failed: {description}", config.provider_name);
        return (Reply::error(400, &title, Some(description)), Some(Err(failure)));
    }
    if params.get("code").is_none_or(|code| code.is_empty()) {
        return (Reply::error(400, "Missing authorization code.", None), None);
    }
    let message = format!("Signed in to {}. You may now close this page.", config.provider_name);
    (Reply::success(&message), Some(Ok(CallbackParams(params))))
}

fn page(title: &str, message: &str, detail: Option<&str>) -> String {
    let detail = detail.map(|text| format!("<p>{}</p>", escape_html(text))).unwrap_or_default();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body style=\"font-family:system-ui,sans-serif;margin:3rem auto;max-width:32rem\"><h1>{}</h1><p>{}</p>{detail}</body></html>",
        escape_html(title),
        escape_html(title),
        escape_html(message)
    )
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ----------------------------------------------------------------------
// Manual paste fallback
// ----------------------------------------------------------------------

/// Parses a pasted redirect URL that must start with `redirect_uri`
/// (origin and path) and carry a `code` (and the expected `state`, if any).
pub fn parse_redirect_url(input: &str, redirect_uri: &str, expected_state: Option<&str>) -> Result<CallbackParams> {
    let wrong_url = || anyhow!("The pasted callback URL must start with {redirect_uri}");
    let pasted = reqwest::Url::parse(input.trim()).map_err(|_| wrong_url())?;
    let expected = reqwest::Url::parse(redirect_uri)?;
    if pasted.origin() != expected.origin() || pasted.path() != expected.path() {
        return Err(wrong_url());
    }
    let params: HashMap<String, String> = pasted.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    if let Some(error) = params.get("error") {
        bail!("Authorization failed: {}", params.get("error_description").unwrap_or(error));
    }
    if params.get("code").is_none_or(|code| code.is_empty()) {
        bail!("Missing authorization code");
    }
    if let Some(expected_state) = expected_state {
        match params.get("state") {
            None => bail!("Missing OAuth state"),
            Some(state) if state != expected_state => bail!("OAuth state mismatch"),
            Some(_) => {}
        }
    }
    Ok(CallbackParams(params))
}

/// Lenient variant for providers without a `state`: a redirect URL, a
/// `code=...` query string, or the bare code.
pub fn parse_authorization_code(input: &str) -> Result<String> {
    let input = input.trim();
    let code = if let Ok(url) = reqwest::Url::parse(input) {
        url.query_pairs().find(|(name, _)| name == "code").map(|(_, value)| value.into_owned())
    } else if input.contains("code=") {
        reqwest::Url::parse(&format!("http://localhost/?{}", input.trim_start_matches('?')))
            .ok()
            .and_then(|url| url.query_pairs().find(|(name, _)| name == "code").map(|(_, value)| value.into_owned()))
    } else {
        Some(input.to_string())
    };
    code.filter(|code| !code.is_empty()).ok_or_else(|| anyhow!("Missing authorization code"))
}

/// Wraps parsed manual input as redirect parameters.
pub fn params_from_code(code: &str) -> CallbackParams {
    CallbackParams(HashMap::from([("code".to_string(), code.to_string())]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(state: Option<&str>) -> CallbackConfig {
        CallbackConfig {
            provider_name: "Acme".to_string(),
            port: 0,
            path: "/auth/callback".to_string(),
            expected_state: state.map(str::to_string),
            timeout: None,
        }
    }

    async fn get(server: &CallbackServer, path_and_query: &str) -> (u16, String) {
        let url = format!("http://127.0.0.1:{}{path_and_query}", server.port());
        let response = reqwest::Client::new().get(url).send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.text().await.unwrap())
    }

    #[tokio::test]
    async fn success_delivers_params_and_shows_success_page() {
        let server = CallbackServer::start(config(Some("st-1"))).await.unwrap();
        let (status, body) = get(&server, "/auth/callback?code=abc&state=st-1&client_id=cid").await;
        assert_eq!(status, 200);
        assert!(body.contains("Signed in to Acme"));
        let params = server.wait().await.unwrap();
        assert_eq!(params.code(), "abc");
        assert_eq!(params.get("client_id"), Some("cid"));
        // A second callback is refused.
        assert_eq!(get(&server, "/auth/callback?code=again&state=st-1").await.0, 409);
    }

    #[tokio::test]
    async fn state_mismatch_wrong_path_and_missing_code_keep_waiting() {
        let server = CallbackServer::start(config(Some("st-1"))).await.unwrap();
        assert_eq!(get(&server, "/auth/callback?code=abc&state=other").await.0, 400);
        assert_eq!(get(&server, "/auth/callback?code=abc").await.0, 400);
        assert_eq!(get(&server, "/elsewhere?code=abc&state=st-1").await.0, 404);
        assert_eq!(get(&server, "/auth/callback?state=st-1").await.0, 400);
        let waiting = tokio::time::timeout(Duration::from_millis(100), server.wait()).await;
        assert!(waiting.is_err(), "no valid callback has arrived yet");
    }

    #[tokio::test]
    async fn provider_error_param_fails_the_wait() {
        let server = CallbackServer::start(config(Some("st-1"))).await.unwrap();
        let (status, body) = get(&server, "/auth/callback?state=st-1&error=access_denied&error_description=No%20thanks").await;
        assert_eq!(status, 400);
        assert!(body.contains("No thanks"));
        let err = server.wait().await.unwrap_err().to_string();
        assert_eq!(err, "Acme authorization failed: No thanks");
    }

    #[tokio::test]
    async fn timeout_and_shutdown_fail_the_wait() {
        let mut quick = config(None);
        quick.timeout = Some(Duration::from_millis(50));
        let server = CallbackServer::start(quick).await.unwrap();
        assert_eq!(server.wait().await.unwrap_err().to_string(), "Acme sign-in timed out");

        let server = CallbackServer::start(config(None)).await.unwrap();
        server.shutdown();
        assert_eq!(server.wait().await.unwrap_err().to_string(), "OAuth callback server closed");
    }

    #[tokio::test]
    async fn binds_loopback_only_and_dropping_releases_the_port() {
        let server = CallbackServer::start(config(None)).await.unwrap();
        let port = server.port();
        assert_ne!(port, 0);
        drop(server);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut fixed = config(None);
        fixed.port = port;
        assert!(CallbackServer::start(fixed).await.is_ok());
    }

    #[test]
    fn pages_escape_html() {
        let (reply, _) = decide(&config(None), "GET", "/auth/callback?error=x&error_description=%3Cb%3E");
        assert!(reply.html.contains("&lt;b&gt;") && !reply.html.contains("<b>"));
    }

    #[test]
    fn pasted_redirect_url_is_validated() {
        let redirect = "http://127.0.0.1:1455/auth/callback";
        let ok = parse_redirect_url("http://127.0.0.1:1455/auth/callback?code=c1&state=s1&client_id=x", redirect, Some("s1")).unwrap();
        assert_eq!((ok.code(), ok.get("client_id")), ("c1", Some("x")));
        for (input, expected) in [
            ("http://127.0.0.1:1455/other?code=c1&state=s1", "must start with"),
            ("https://evil.example/auth/callback?code=c1&state=s1", "must start with"),
            ("just-a-code", "must start with"),
            ("http://127.0.0.1:1455/auth/callback?state=s1", "Missing authorization code"),
            ("http://127.0.0.1:1455/auth/callback?code=c1", "Missing OAuth state"),
            ("http://127.0.0.1:1455/auth/callback?code=c1&state=zzz", "state mismatch"),
            ("http://127.0.0.1:1455/auth/callback?error=access_denied&state=s1", "access_denied"),
        ] {
            let err = parse_redirect_url(input, redirect, Some("s1")).unwrap_err().to_string();
            assert!(err.contains(expected), "{input}: {err}");
        }
    }

    #[test]
    fn lenient_code_parsing_accepts_url_query_or_bare_code() {
        assert_eq!(parse_authorization_code("http://127.0.0.1:9/oauth/callback/x?code=abc").unwrap(), "abc");
        assert_eq!(parse_authorization_code(" code=abc&x=1 ").unwrap(), "abc");
        assert_eq!(parse_authorization_code("abc").unwrap(), "abc");
        assert!(parse_authorization_code("   ").is_err());
        assert!(parse_authorization_code("http://127.0.0.1:9/cb?nope=1").is_err());
    }
}
