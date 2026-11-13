//! OAuth login support: provider registry, token shape and the pieces the
//! provider manager builds on (device-code poller, loopback callback server,
//! PKCE, one module per provider, `auth.json` lock).
//!
//! Stored credentials are pi-compatible:
//! `{ "type": "oauth", "access": "...", "refresh": "...", "expires": <epoch ms> }`
//! where `expires` already has the refresh skew subtracted, plus provider
//! fields next to them (`clientId`, `enterpriseUrl`, ...).

pub mod auth_lock;
pub mod callback_server;
pub mod catalog;
pub mod device_code;
pub mod github_copilot;
pub mod kimi;
pub mod openai;
pub mod openrouter;
pub mod pkce;
pub mod xai;

use device_code::DeviceCodeSession;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// How the user completes a sign-in, so UIs know what to show.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlow {
    /// Show a user code and a verification URI (RFC 8628).
    DeviceCode,
    /// Open an authorization URL; the redirect arrives on a loopback server
    /// or is pasted back by the user.
    AuthUrl,
}

/// Display metadata of a provider that supports OAuth login.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OAuthProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub login_label: &'static str,
    pub flow: OAuthFlow,
}

const OAUTH_PROVIDERS: [OAuthProviderInfo; 5] = [
    OAuthProviderInfo {
        id: "xai",
        name: "xAI (Grok/X subscription)",
        login_label: "Sign in with SuperGrok or X Premium",
        flow: OAuthFlow::DeviceCode,
    },
    OAuthProviderInfo {
        id: "openai",
        name: "OpenAI (ChatGPT subscription)",
        login_label: "Sign in with ChatGPT",
        flow: OAuthFlow::AuthUrl,
    },
    OAuthProviderInfo {
        id: "github-copilot",
        name: "GitHub Copilot",
        login_label: "Sign in with GitHub",
        flow: OAuthFlow::DeviceCode,
    },
    OAuthProviderInfo {
        id: "openrouter",
        name: "OpenRouter",
        login_label: "Sign in with OpenRouter",
        flow: OAuthFlow::AuthUrl,
    },
    OAuthProviderInfo {
        id: "kimi-coding",
        name: "Kimi Code (subscription)",
        login_label: "Sign in with Kimi Code",
        flow: OAuthFlow::DeviceCode,
    },
];

/// Providers that can be signed into with OAuth.
pub fn oauth_providers() -> &'static [OAuthProviderInfo] {
    &OAUTH_PROVIDERS
}

pub fn oauth_provider_info(provider_id: &str) -> Option<OAuthProviderInfo> {
    OAUTH_PROVIDERS.iter().find(|info| info.id == provider_id).copied()
}

/// One client per OAuth provider; tests swap in clients pointing at mock servers.
#[derive(Default)]
pub struct OAuthClients {
    pub xai: xai::XaiOAuth,
    pub chatgpt: openai::ChatgptOAuth,
    pub copilot: github_copilot::CopilotOAuth,
    pub openrouter: openrouter::OpenRouterOAuth,
    pub kimi: kimi::KimiOAuth,
}

/// A started authorization-URL login. Open `auth_url`; the redirect arrives
/// on the loopback server (see `PiProviderManager::finish_oauth_login`) or is
/// pasted back (`submit_oauth_redirect`). The PKCE verifier stays private.
#[derive(Clone, Serialize, Deserialize)]
pub struct AuthUrlSession {
    pub auth_url: String,
    pub redirect_uri: String,
    pub(crate) state: Option<String>,
    pub(crate) verifier: String,
}

impl std::fmt::Debug for AuthUrlSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthUrlSession").field("redirect_uri", &self.redirect_uri).finish_non_exhaustive()
    }
}

/// A started login of either flow kind.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LoginSession {
    DeviceCode(DeviceCodeSession),
    AuthUrl(AuthUrlSession),
}

/// Fresh OAuth tokens. `expires_ms` is epoch milliseconds with the refresh
/// skew already subtracted. `extra` holds provider fields stored next to the
/// tokens (`clientId`, `enterpriseUrl`, ...). Never logged or `Debug`-printed
/// with values.
#[derive(Clone)]
pub struct OAuthTokens {
    pub access: String,
    pub refresh: String,
    pub expires_ms: i64,
    pub extra: Map<String, Value>,
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens").field("expires_ms", &self.expires_ms).finish_non_exhaustive()
    }
}

impl OAuthTokens {
    pub fn new(access: String, refresh: String, expires_ms: i64) -> Self {
        Self { access, refresh, expires_ms, extra: Map::new() }
    }

    pub fn with_extra(mut self, name: &str, value: Value) -> Self {
        self.extra.insert(name.to_string(), value);
        self
    }

    /// Writes the tokens into an existing credential object, keeping any
    /// extra fields pi (or the user) stored next to them.
    pub fn apply_to(&self, entry: &mut Map<String, Value>) {
        entry.insert("type".to_string(), json!("oauth"));
        entry.insert("access".to_string(), json!(self.access));
        entry.insert("refresh".to_string(), json!(self.refresh));
        entry.insert("expires".to_string(), json!(self.expires_ms));
        for (name, value) in &self.extra {
            entry.insert(name.clone(), value.clone());
        }
    }

    pub fn to_credential(&self) -> Value {
        let mut entry = Map::new();
        self.apply_to(&mut entry);
        Value::Object(entry)
    }
}

/// Accepts only `http`/`https` URLs (pi's `trustedHttpUrl`) and returns the
/// normalized form, which percent-encodes control characters.
pub(crate) fn trusted_http_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

/// Server error text for messages: the body, trimmed and bounded.
pub(crate) fn error_detail(body: &str) -> String {
    body.trim().chars().take(300).collect()
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_providers_with_their_flow() {
        let flows: Vec<_> = oauth_providers().iter().map(|info| (info.id, info.flow)).collect();
        assert_eq!(
            flows,
            [
                ("xai", OAuthFlow::DeviceCode),
                ("openai", OAuthFlow::AuthUrl),
                ("github-copilot", OAuthFlow::DeviceCode),
                ("openrouter", OAuthFlow::AuthUrl),
                ("kimi-coding", OAuthFlow::DeviceCode),
            ]
        );
        assert!(oauth_provider_info("openai-codex").is_none());
        assert_eq!(serde_json::to_value(OAuthFlow::AuthUrl).unwrap(), "auth_url");
    }

    #[test]
    fn extra_fields_are_stored_next_to_the_tokens() {
        let tokens = OAuthTokens::new("acc-secret".into(), "ref-secret".into(), 5).with_extra("clientId", json!("cid"));
        let mut entry = Map::new();
        entry.insert("accountId".into(), json!("keep-me"));
        tokens.apply_to(&mut entry);
        assert_eq!(entry["clientId"], "cid");
        assert_eq!(entry["accountId"], "keep-me");
        assert_eq!(entry["expires"], 5);
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("acc-secret") && !debug.contains("ref-secret"));
    }

    #[test]
    fn trusted_urls_are_http_or_https_only() {
        assert_eq!(trusted_http_url("https://github.com/login/device").unwrap(), "https://github.com/login/device");
        assert!(trusted_http_url("http://localhost/x").is_some());
        assert!(trusted_http_url("javascript:alert(1)").is_none());
        assert!(trusted_http_url("file:///etc/passwd").is_none());
        assert!(trusted_http_url("nope").is_none());
    }
}
