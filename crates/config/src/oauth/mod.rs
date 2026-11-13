//! OAuth login support: provider registry, token shape and the pieces the
//! provider manager builds on (device-code poller, xAI flow, `auth.json` lock).
//!
//! Stored credentials are pi-compatible:
//! `{ "type": "oauth", "access": "...", "refresh": "...", "expires": <epoch ms> }`
//! where `expires` already has the refresh skew subtracted.

pub mod auth_lock;
pub mod device_code;
pub mod xai;

use serde::Serialize;
use serde_json::{json, Map, Value};

/// Display metadata of a provider that supports OAuth login.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OAuthProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub login_label: &'static str,
}

const OAUTH_PROVIDERS: [OAuthProviderInfo; 1] = [OAuthProviderInfo {
    id: "xai",
    name: "xAI (Grok/X subscription)",
    login_label: "Sign in with SuperGrok or X Premium",
}];

/// Providers that can be signed into with OAuth.
pub fn oauth_providers() -> &'static [OAuthProviderInfo] {
    &OAUTH_PROVIDERS
}

pub fn oauth_provider_info(provider_id: &str) -> Option<OAuthProviderInfo> {
    OAUTH_PROVIDERS.iter().find(|info| info.id == provider_id).copied()
}

/// Fresh OAuth tokens. `expires_ms` is epoch milliseconds with the refresh
/// skew already subtracted. Never logged or `Debug`-printed with values.
#[derive(Clone)]
pub struct OAuthTokens {
    pub access: String,
    pub refresh: String,
    pub expires_ms: i64,
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens").field("expires_ms", &self.expires_ms).finish_non_exhaustive()
    }
}

impl OAuthTokens {
    /// Writes the tokens into an existing credential object, keeping any
    /// extra fields pi (or the user) stored next to them.
    pub fn apply_to(&self, entry: &mut Map<String, Value>) {
        entry.insert("type".to_string(), json!("oauth"));
        entry.insert("access".to_string(), json!(self.access));
        entry.insert("refresh".to_string(), json!(self.refresh));
        entry.insert("expires".to_string(), json!(self.expires_ms));
    }

    pub fn to_credential(&self) -> Value {
        let mut entry = Map::new();
        self.apply_to(&mut entry);
        Value::Object(entry)
    }
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
    fn registry_knows_xai_only() {
        assert_eq!(oauth_providers().len(), 1);
        assert_eq!(oauth_provider_info("xai").unwrap().id, "xai");
        assert!(oauth_provider_info("openai").is_none());
    }
}
