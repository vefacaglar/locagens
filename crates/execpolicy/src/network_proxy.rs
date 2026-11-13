//! Per-domain network proxy for sandboxed commands — the core of Codex's
//! `network-proxy` crate.
//!
//! A [`NetworkProxy`] listens on `127.0.0.1:<ephemeral>` and speaks plain
//! HTTP proxying (absolute-form `GET http://host/…`) plus `CONNECT host:port`
//! tunnels. Every request is checked against the command's
//! [`DomainAllowlist`]; hosts outside it are either sent to an optional
//! [`NetworkDecider`] (the run's permission prompt) or rejected with
//! `403 Forbidden`. Sandboxed commands find the proxy through the usual
//! `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` variables ([`proxy_env_vars`]).
//!
//! Like Codex, targets that resolve to loopback / private / link-local
//! addresses are refused unless the allowlist names that exact host
//! (`localhost` or an IP literal), which blocks DNS-rebinding tricks.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Upper bound for a request head (request line + headers).
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Environment variable set for proxied commands so tools can tell the
/// proxy is active.
pub const PROXY_ACTIVE_ENV_KEY: &str = "LOCAGENS_NETWORK_PROXY_ACTIVE";

/// Loopback stays direct: local dev servers and test fixtures keep working.
pub const DEFAULT_NO_PROXY_VALUE: &str = "localhost,127.0.0.1,::1";

/// Proxy-URL variables understood by curl, git, npm, yarn, pip, bundler,
/// docker and websocket clients (Codex `apply_proxy_env_overrides`).
const PROXY_URL_ENV_KEYS: [&str; 25] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "FTP_PROXY",
    "ftp_proxy",
    "WS_PROXY",
    "WSS_PROXY",
    "ws_proxy",
    "wss_proxy",
    "YARN_HTTP_PROXY",
    "YARN_HTTPS_PROXY",
    "npm_config_http_proxy",
    "npm_config_https_proxy",
    "npm_config_proxy",
    "NPM_CONFIG_HTTP_PROXY",
    "NPM_CONFIG_HTTPS_PROXY",
    "NPM_CONFIG_PROXY",
    "BUNDLE_HTTP_PROXY",
    "BUNDLE_HTTPS_PROXY",
    "PIP_PROXY",
    "DOCKER_HTTP_PROXY",
    "DOCKER_HTTPS_PROXY",
];

const NO_PROXY_ENV_KEYS: [&str; 5] = [
    "NO_PROXY",
    "no_proxy",
    "npm_config_noproxy",
    "NPM_CONFIG_NOPROXY",
    "BUNDLE_NO_PROXY",
];

/// The environment a proxied command needs: every proxy-URL variable points
/// at `addr`, `NO_PROXY` is limited to loopback, and Node/Electron are told
/// to honor the proxy variables.
pub fn proxy_env_vars(addr: SocketAddr) -> Vec<(String, String)> {
    let url = format!("http://{addr}");
    let mut vars: Vec<(String, String)> = PROXY_URL_ENV_KEYS
        .iter()
        .map(|key| (key.to_string(), url.clone()))
        .collect();
    vars.extend(
        NO_PROXY_ENV_KEYS
            .iter()
            .map(|key| (key.to_string(), DEFAULT_NO_PROXY_VALUE.to_string())),
    );
    vars.push(("NODE_USE_ENV_PROXY".to_string(), "1".to_string()));
    vars.push(("ELECTRON_GET_USE_PROXY".to_string(), "true".to_string()));
    vars.push((PROXY_ACTIVE_ENV_KEY.to_string(), "1".to_string()));
    vars
}

// ----------------------------------------------------------------------
// Allowlist
// ----------------------------------------------------------------------

/// Domain patterns with Codex semantics:
/// - `example.com` matches exactly that host,
/// - `*.example.com` matches any subdomain (not the apex),
/// - `**.example.com` matches the apex and any subdomain.
///
/// The global wildcard `*` is ignored on purpose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DomainAllowlist {
    patterns: Vec<String>,
}

impl DomainAllowlist {
    pub fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut list = Self::default();
        for pattern in patterns {
            list.insert(pattern.as_ref());
        }
        list
    }

    /// Adds one pattern (normalized; invalid or global patterns are skipped).
    pub fn insert(&mut self, pattern: &str) {
        let Some(pattern) = normalize_pattern(pattern) else {
            return;
        };
        if !self.patterns.contains(&pattern) {
            self.patterns.push(pattern);
        }
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `host` matches any pattern.
    pub fn allows(&self, host: &str) -> bool {
        let host = normalize_host(host);
        self.patterns.iter().any(|pattern| pattern_matches(pattern, &host))
    }

    /// Whether `host` is `localhost` or an IP literal AND listed as an exact
    /// entry. Only such entries may reach local/private addresses: an
    /// approved ordinary hostname that resolves to `127.0.0.1` or
    /// `169.254.169.254` (DNS rebinding, a hostile record) stays refused.
    fn names_local_literal(&self, host: &str) -> bool {
        let host = normalize_host(host);
        let literal = host == "localhost" || host.parse::<IpAddr>().is_ok();
        literal && self.patterns.contains(&host)
    }
}

/// Whether a request host is a plain hostname (`[A-Za-z0-9.-]`, no empty
/// labels, no wildcard) or an IP literal. Checked before the host can reach
/// the approval prompt or be stored as an allowlist entry.
pub fn is_valid_request_host(host: &str) -> bool {
    let host = normalize_host(host);
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// Lowercases a pattern and strips a scheme, path, port and trailing dot so
/// `https://Registry.npmjs.org/` and `registry.npmjs.org` mean the same.
fn normalize_pattern(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_ascii_lowercase();
    if let Some((_, rest)) = value.split_once("://") {
        value = rest.to_string();
    }
    if let Some((authority, _)) = value.split_once('/') {
        value = authority.to_string();
    }
    let (wildcard, host) = if let Some(rest) = value.strip_prefix("**.") {
        ("**.", rest.to_string())
    } else if let Some(rest) = value.strip_prefix("*.") {
        ("*.", rest.to_string())
    } else {
        ("", value)
    };
    let host = normalize_host(&strip_port(&host));
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    valid.then(|| format!("{wildcard}{host}"))
}

/// Removes a `:port` suffix (and IPv6 brackets) from an authority.
fn strip_port(authority: &str) -> String {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default().to_string();
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            host.to_string()
        }
        _ => authority.to_string(),
    }
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn pattern_matches(pattern: &str, host: &str) -> bool {
    if let Some(apex) = pattern.strip_prefix("**.") {
        return host == apex || host.ends_with(&format!(".{apex}"));
    }
    if let Some(apex) = pattern.strip_prefix("*.") {
        return host.ends_with(&format!(".{apex}"));
    }
    pattern == host
}

// ----------------------------------------------------------------------
// Policy + decider
// ----------------------------------------------------------------------

/// A host the proxy was asked to reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkRequest {
    pub host: String,
    pub port: u16,
}

/// Async approval hook for hosts outside the allowlist (resolves to `true`
/// when the user allows the host).
pub type NetworkDecider =
    Arc<dyn Fn(NetworkRequest) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// The network policy of one sandboxed command.
#[derive(Clone, Default)]
pub struct NetworkPolicy {
    pub allowlist: DomainAllowlist,
    pub decider: Option<NetworkDecider>,
}

impl NetworkPolicy {
    pub fn allowlist_only(allowlist: DomainAllowlist) -> Self {
        Self {
            allowlist,
            decider: None,
        }
    }
}

impl fmt::Debug for NetworkPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkPolicy")
            .field("allowlist", &self.allowlist)
            .field("decider", &self.decider.is_some())
            .finish()
    }
}

/// Shared state of a running proxy.
struct ProxyState {
    allowlist: Mutex<DomainAllowlist>,
    decider: Option<NetworkDecider>,
    /// Hosts the decider already answered (avoids prompting twice).
    decided: Mutex<HashMap<String, bool>>,
    /// Serializes decider calls so parallel connections prompt once.
    decide_lock: tokio::sync::Mutex<()>,
    blocked: Mutex<Vec<String>>,
}

impl ProxyState {
    fn allowlisted(&self, host: &str) -> bool {
        self.allowlist.lock().expect("allowlist poisoned").allows(host)
    }

    async fn authorize(&self, request: &NetworkRequest) -> bool {
        if self.allowlisted(&request.host) {
            return true;
        }
        let allowed = match &self.decider {
            Some(decider) => {
                let _guard = self.decide_lock.lock().await;
                let host = normalize_host(&request.host);
                let cached = self.decided.lock().expect("decided poisoned").get(&host).copied();
                match cached {
                    Some(answer) => answer,
                    None if self.allowlisted(&host) => true,
                    None => {
                        let answer = decider(request.clone()).await;
                        self.decided.lock().expect("decided poisoned").insert(host.clone(), answer);
                        if answer {
                            self.allowlist.lock().expect("allowlist poisoned").insert(&host);
                        }
                        answer
                    }
                }
            }
            None => false,
        };
        if !allowed {
            self.record_blocked(&request.host);
        }
        allowed
    }

    fn record_blocked(&self, host: &str) {
        let host = normalize_host(host);
        let mut blocked = self.blocked.lock().expect("blocked poisoned");
        if !blocked.contains(&host) {
            blocked.push(host);
        }
    }
}

// ----------------------------------------------------------------------
// Proxy server
// ----------------------------------------------------------------------

/// A running loopback proxy. Dropping it stops accepting connections.
pub struct NetworkProxy {
    addr: SocketAddr,
    state: Arc<ProxyState>,
    accept_task: tokio::task::JoinHandle<()>,
}

impl NetworkProxy {
    /// Binds `127.0.0.1:0` and starts serving. Must be called inside a
    /// Tokio runtime.
    pub fn start(policy: NetworkPolicy) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let listener = TcpListener::from_std(listener)?;
        let state = Arc::new(ProxyState {
            allowlist: Mutex::new(policy.allowlist),
            decider: policy.decider,
            decided: Mutex::new(HashMap::new()),
            decide_lock: tokio::sync::Mutex::new(()),
            blocked: Mutex::new(Vec::new()),
        });
        let accept_task = tokio::spawn(accept_loop(listener, Arc::clone(&state)));
        Ok(Self {
            addr,
            state,
            accept_task,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The environment variables that route a command through this proxy.
    pub fn env_vars(&self) -> Vec<(String, String)> {
        proxy_env_vars(self.addr)
    }

    /// Hosts that were refused so far (normalized, de-duplicated).
    pub fn blocked_hosts(&self) -> Vec<String> {
        self.state.blocked.lock().expect("blocked poisoned").clone()
    }
}

impl Drop for NetworkProxy {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn accept_loop(listener: TcpListener, state: Arc<ProxyState>) {
    loop {
        let Ok((client, _)) = listener.accept().await else {
            continue;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = handle_connection(client, state).await;
        });
    }
}

/// What the client asked the proxy to do.
#[derive(Debug, PartialEq, Eq)]
enum ProxyRequest {
    /// `CONNECT host:port` — opaque tunnel (HTTPS, websockets, git).
    Connect { host: String, port: u16 },
    /// Plain HTTP with an absolute-form target, forwarded in origin form.
    Http {
        host: String,
        port: u16,
        forward_head: Vec<u8>,
    },
}

impl ProxyRequest {
    fn target(&self) -> NetworkRequest {
        let (host, port) = match self {
            ProxyRequest::Connect { host, port } | ProxyRequest::Http { host, port, .. } => {
                (host.clone(), *port)
            }
        };
        NetworkRequest { host, port }
    }
}

async fn handle_connection(mut client: TcpStream, state: Arc<ProxyState>) -> std::io::Result<()> {
    let (head, leftover) = match read_request_head(&mut client).await? {
        Some(parts) => parts,
        None => return Ok(()),
    };
    let request = match parse_request(&head) {
        Ok(request) => request,
        Err(reason) => return respond(&mut client, "400 Bad Request", &reason).await,
    };
    let target = request.target();
    if !is_valid_request_host(&target.host) {
        return respond(&mut client, "400 Bad Request", &format!("invalid host {:?}", target.host)).await;
    }
    if !state.authorize(&target).await {
        return respond(&mut client, "403 Forbidden", &blocked_message(&target.host)).await;
    }
    let explicit_local = state.allowlist.lock().expect("allowlist poisoned").names_local_literal(&target.host);
    let mut upstream = match connect_upstream(&target, explicit_local).await {
        Ok(stream) => stream,
        Err(reason) => return respond(&mut client, "502 Bad Gateway", &reason).await,
    };
    match request {
        ProxyRequest::Connect { .. } => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
        }
        ProxyRequest::Http { forward_head, .. } => {
            upstream.write_all(&forward_head).await?;
        }
    }
    if !leftover.is_empty() {
        upstream.write_all(&leftover).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

/// The message a refused command sees (also the model, via its output).
pub fn blocked_message(host: &str) -> String {
    format!(
        "Locagens sandbox: network access to \"{host}\" is blocked (not on this command's allowlist). \
Request it by re-running the command with network_domains: [\"{host}\"] so the user can approve it."
    )
}

/// One-line summary of refused hosts for a command's result.
pub fn blocked_hosts_note(hosts: &[String]) -> String {
    let list = hosts.join(", ");
    let quoted = hosts
        .iter()
        .map(|host| format!("\"{host}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Sandbox blocked network access to: {list}. To request access, re-run the command with network_domains: [{quoted}]."
    )
}

/// Reads until the end of the request head; returns `(head, leftover)`.
async fn read_request_head(
    client: &mut TcpStream,
) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let read = client.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = find_head_end(&buffer) {
            let leftover = buffer.split_off(end);
            return Ok(Some((buffer, leftover)));
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return Ok(None);
        }
    }
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n").map(|i| i + 4)
}

/// Parses a proxy request head into a [`ProxyRequest`].
fn parse_request(head: &[u8]) -> Result<ProxyRequest, String> {
    let text = std::str::from_utf8(head).map_err(|_| "request head is not UTF-8".to_string())?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v)) if !m.is_empty() && !t.is_empty() => (m, t, v),
        _ => return Err("malformed request line".to_string()),
    };
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_authority(target, 443)?;
        return Ok(ProxyRequest::Connect { host, port });
    }
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))
        .ok_or_else(|| "only absolute http:// targets or CONNECT are supported".to_string())?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = split_authority(authority, 80)?;

    let mut forward = format!("{method} {path} {version}\r\n");
    let mut has_host = false;
    for line in lines.filter(|line| !line.is_empty()) {
        let name = line.split(':').next().unwrap_or_default().trim().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "proxy-connection" | "proxy-authorization" | "connection" | "keep-alive"
        ) {
            continue;
        }
        has_host |= name == "host";
        forward.push_str(line);
        forward.push_str("\r\n");
    }
    if !has_host {
        forward.push_str(&format!("Host: {authority}\r\n"));
    }
    forward.push_str("Connection: close\r\n\r\n");
    Ok(ProxyRequest::Http {
        host,
        port,
        forward_head: forward.into_bytes(),
    })
}

/// Splits `host[:port]` / `[v6][:port]` into host and port.
fn split_authority(authority: &str, default_port: u16) -> Result<(String, u16), String> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or_else(|| format!("invalid authority {authority:?}"))?;
        (host.to_string(), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), Some(port)),
            None => (authority.to_string(), None),
        }
    };
    let port = match port {
        Some(port) => port
            .parse::<u16>()
            .map_err(|_| format!("invalid port in {authority:?}"))?,
        None => default_port,
    };
    if host.is_empty() {
        return Err(format!("missing host in {authority:?}"));
    }
    Ok((normalize_host(&host), port))
}

/// Resolves and connects to the target, refusing non-public addresses unless
/// the host was allow-listed by its exact name.
async fn connect_upstream(target: &NetworkRequest, explicit_local: bool) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((target.host.as_str(), target.port))
        .await
        .map_err(|err| format!("cannot resolve {}: {err}", target.host))?
        .collect();
    let usable: Vec<SocketAddr> = addrs
        .into_iter()
        .filter(|addr| explicit_local || !is_non_public_ip(&addr.ip()))
        .collect();
    if usable.is_empty() {
        return Err(format!(
            "Locagens sandbox: {} resolves to a local/private address; add the exact host or IP to the allowlist to reach it.",
            target.host
        ));
    }
    let mut last_error = String::new();
    for addr in usable {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = format!("connect {addr}: {err}"),
        }
    }
    Err(last_error)
}

/// Loopback, private, link-local, CGNAT, unspecified and broadcast ranges.
pub fn is_non_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || a == 0
                || (a == 100 && (b & 0b1100_0000) == 64)
                // Multicast 224/4 and reserved 240/4.
                || a >= 224
                // IETF protocol assignments and benchmarking ranges.
                || (a == 192 && b == 0 && v4.octets()[2] == 0)
                || (a == 198 && (b & 0xfe) == 18)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_non_public_ip(&IpAddr::V4(v4));
            }
            let segments = v6.segments();
            // NAT64 (64:ff9b::/96) embeds an IPv4 target.
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [hi, lo] = [segments[6], segments[7]];
                let v4 = std::net::Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
                return is_non_public_ip(&IpAddr::V4(v4));
            }
            let first = segments[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                // Site-local (deprecated) and multicast.
                || (first & 0xffc0) == 0xfec0
                || (first & 0xff00) == 0xff00
                // IPv4-compatible (`::a.b.c.d`, deprecated).
                || segments[..6] == [0, 0, 0, 0, 0, 0]
        }
    }
}

async fn respond(client: &mut TcpStream, status: &str, message: &str) -> std::io::Result<()> {
    let body = format!("{message}\n");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nX-Proxy-Error: locagens-sandbox\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes()).await?;
    client.shutdown().await
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_uses_codex_pattern_semantics() {
        let list = DomainAllowlist::new([
            "Registry.npmjs.org",
            "*.github.com",
            "**.crates.io",
            "https://pypi.org/simple",
            "*",
        ]);
        assert!(list.allows("registry.npmjs.org"));
        assert!(list.allows("REGISTRY.NPMJS.ORG."));
        assert!(!list.allows("npmjs.org"));
        assert!(!list.allows("evil-registry.npmjs.org"));
        assert!(list.allows("api.github.com"));
        assert!(!list.allows("github.com"));
        assert!(list.allows("crates.io"));
        assert!(list.allows("static.crates.io"));
        assert!(!list.allows("notcrates.io"));
        assert!(list.allows("pypi.org"));
        assert!(!list.allows("example.com"), "global wildcard must be ignored");
        assert_eq!(list.patterns().len(), 4);
    }

    #[test]
    fn proxy_env_points_every_client_at_the_proxy() {
        let addr: SocketAddr = "127.0.0.1:4567".parse().unwrap();
        let vars: HashMap<String, String> = proxy_env_vars(addr).into_iter().collect();
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
            assert_eq!(vars.get(key).map(String::as_str), Some("http://127.0.0.1:4567"), "{key}");
        }
        assert_eq!(vars.get("NO_PROXY").map(String::as_str), Some(DEFAULT_NO_PROXY_VALUE));
        assert_eq!(vars.get("no_proxy").map(String::as_str), Some(DEFAULT_NO_PROXY_VALUE));
        assert_eq!(vars.get(PROXY_ACTIVE_ENV_KEY).map(String::as_str), Some("1"));
    }

    #[test]
    fn parses_connect_and_absolute_http_requests() {
        let connect = parse_request(b"CONNECT example.com:8443 HTTP/1.1\r\nHost: example.com\r\n\r\n").unwrap();
        assert_eq!(connect, ProxyRequest::Connect { host: "example.com".into(), port: 8443 });

        let http = parse_request(
            b"GET http://user:pw@Example.com/a/b?c=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\nAccept: */*\r\n\r\n",
        )
        .unwrap();
        let ProxyRequest::Http { host, port, forward_head } = http else {
            panic!("expected plain http request");
        };
        assert_eq!((host.as_str(), port), ("example.com", 80));
        let head = String::from_utf8(forward_head).unwrap();
        assert!(head.starts_with("GET /a/b?c=1 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("Accept: */*\r\n"));
        assert!(!head.to_ascii_lowercase().contains("proxy-connection"));
        assert!(head.ends_with("Connection: close\r\n\r\n"));

        assert!(parse_request(b"GET /relative HTTP/1.1\r\n\r\n").is_err());
        assert_eq!(
            parse_request(b"CONNECT [::1]:443 HTTP/1.1\r\n\r\n").unwrap(),
            ProxyRequest::Connect { host: "::1".into(), port: 443 }
        );
    }

    #[test]
    fn non_public_ranges_are_detected() {
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1", "::ffff:127.0.0.1"] {
            assert!(is_non_public_ip(&ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "93.184.216.34", "2606:4700::1111"] {
            assert!(!is_non_public_ip(&ip.parse().unwrap()), "{ip}");
        }
    }

    /// A one-shot local HTTP server answering every request with `body`.
    async fn spawn_test_server(body: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        addr
    }

    async fn send_raw(proxy: SocketAddr, request: String) -> String {
        let mut stream = TcpStream::connect(proxy).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read_to_end(&mut response)).await;
        String::from_utf8_lossy(&response).to_string()
    }

    #[tokio::test]
    async fn proxy_forwards_allowed_hosts_and_blocks_others() {
        let server = spawn_test_server("hello-from-upstream").await;
        let proxy = NetworkProxy::start(NetworkPolicy::allowlist_only(DomainAllowlist::new(["127.0.0.1"]))).unwrap();

        let allowed = send_raw(
            proxy.addr(),
            format!("GET http://127.0.0.1:{}/x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", server.port()),
        )
        .await;
        assert!(allowed.starts_with("HTTP/1.1 200 OK"), "{allowed}");
        assert!(allowed.contains("hello-from-upstream"));

        // CONNECT tunnels to an allowed host relay raw bytes.
        let mut tunnel = TcpStream::connect(proxy.addr()).await.unwrap();
        tunnel
            .write_all(format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\n\r\n", server.port()).as_bytes())
            .await
            .unwrap();
        tunnel.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut tunneled = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), tunnel.read_to_end(&mut tunneled)).await;
        let tunneled = String::from_utf8_lossy(&tunneled);
        assert!(tunneled.starts_with("HTTP/1.1 200 Connection Established"), "{tunneled}");
        assert!(tunneled.contains("hello-from-upstream"));

        // A host outside the allowlist never reaches DNS or the network.
        let blocked = send_raw(proxy.addr(), "GET http://blocked.example/ HTTP/1.1\r\nHost: blocked.example\r\n\r\n".into()).await;
        assert!(blocked.starts_with("HTTP/1.1 403 Forbidden"), "{blocked}");
        assert!(blocked.contains("network_domains"));
        let blocked_connect = send_raw(proxy.addr(), "CONNECT blocked.example:443 HTTP/1.1\r\n\r\n".into()).await;
        assert!(blocked_connect.starts_with("HTTP/1.1 403 Forbidden"), "{blocked_connect}");
        assert_eq!(proxy.blocked_hosts(), vec!["blocked.example".to_string()]);
    }

    #[tokio::test]
    async fn local_targets_need_an_exact_allowlist_entry() {
        let server = spawn_test_server("secret").await;
        // `localhost` resolves to loopback; a wildcard entry is not enough.
        let proxy = NetworkProxy::start(NetworkPolicy::allowlist_only(DomainAllowlist::new(["**.localhost"]))).unwrap();
        let response = send_raw(
            proxy.addr(),
            format!("GET http://localhost:{}/ HTTP/1.1\r\n\r\n", server.port()),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 502"), "{response}");
        assert!(!response.contains("secret"));
    }

    /// QA-014: only `localhost` / IP literals may bypass the private-IP
    /// filter; an approved ordinary hostname (which could resolve to
    /// 127.0.0.1 or 169.254.169.254) may not.
    #[test]
    fn only_localhost_and_ip_literals_bypass_the_private_ip_filter() {
        let list = DomainAllowlist::new(["evil.example", "localhost", "127.0.0.1", "::1", "**.corp.test"]);
        assert!(!list.names_local_literal("evil.example"));
        assert!(!list.names_local_literal("a.corp.test"));
        assert!(list.names_local_literal("localhost"));
        assert!(list.names_local_literal("LOCALHOST."));
        assert!(list.names_local_literal("127.0.0.1"));
        assert!(list.names_local_literal("[::1]"));
        assert!(!list.names_local_literal("10.0.0.1"));
    }

    #[test]
    fn request_hosts_are_validated_before_prompting() {
        for ok in ["example.com", "a-b.example.com", "127.0.0.1", "::1", "xn--bcher-kva.example"] {
            assert!(is_valid_request_host(ok), "{ok}");
        }
        for bad in ["", "*.com", "**.example.com", "a..b", ".example.com", "exa mple.com", "ex_ample.com", "a/b"] {
            assert!(!is_valid_request_host(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn wildcard_request_hosts_never_reach_the_decider() {
        let asked = Arc::new(Mutex::new(0usize));
        let asked_in_decider = Arc::clone(&asked);
        let decider: NetworkDecider = Arc::new(move |_request: NetworkRequest| {
            let asked = Arc::clone(&asked_in_decider);
            Box::pin(async move {
                *asked.lock().unwrap() += 1;
                true
            })
        });
        let proxy = NetworkProxy::start(NetworkPolicy { allowlist: DomainAllowlist::default(), decider: Some(decider) }).unwrap();
        let response = send_raw(proxy.addr(), "CONNECT *.com:443 HTTP/1.1\r\n\r\n".into()).await;
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert_eq!(*asked.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn decider_is_asked_once_per_host_and_can_allow() {
        let server = spawn_test_server("approved").await;
        let asked = Arc::new(Mutex::new(Vec::<NetworkRequest>::new()));
        let asked_in_decider = Arc::clone(&asked);
        let decider: NetworkDecider = Arc::new(move |request: NetworkRequest| {
            let asked = Arc::clone(&asked_in_decider);
            Box::pin(async move {
                let allow = request.host == "127.0.0.1";
                asked.lock().unwrap().push(request);
                allow
            })
        });
        let proxy = NetworkProxy::start(NetworkPolicy {
            allowlist: DomainAllowlist::default(),
            decider: Some(decider),
        })
        .unwrap();
        for _ in 0..2 {
            let response = send_raw(
                proxy.addr(),
                format!("GET http://127.0.0.1:{}/ HTTP/1.1\r\n\r\n", server.port()),
            )
            .await;
            assert!(response.contains("approved"), "{response}");
        }
        let denied = send_raw(proxy.addr(), "CONNECT denied.example:443 HTTP/1.1\r\n\r\n".into()).await;
        assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 2, "one prompt per host: {asked:?}");
        assert_eq!(proxy.blocked_hosts(), vec!["denied.example".to_string()]);
    }
}
