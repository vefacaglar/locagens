//! Kimi Code subscription login (port of pi's `kimi-coding.js`): RFC 8628
//! device flow against `auth.kimi.com`. The access token is used as a bearer
//! for `https://api.kimi.com/coding/v1`.

use super::device_code::{poll_device_code, DeviceCodeSession, PollStep, PollTiming};
use super::{error_detail, trusted_http_url, OAuthTokens};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::time::Duration;

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEFAULT_OAUTH_HOST: &str = "https://auth.kimi.com";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_POLL_INTERVAL_SECS: f64 = 5.0;
const DEFAULT_DEVICE_CODE_TIMEOUT_SECS: f64 = 900.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REFRESH_MAX_RETRIES: u32 = 3;
/// The access token is refreshed five minutes before it expires.
const EXPIRY_MARGIN_MS: i64 = 5 * 60 * 1000;

/// Kimi Code OAuth client; the host and pacing are injectable for tests.
pub struct KimiOAuth {
    http: reqwest::Client,
    host: String,
    timing: PollTiming,
    /// First refresh retry delay; doubles for each further retry.
    refresh_backoff: Duration,
}

impl Default for KimiOAuth {
    fn default() -> Self {
        let host = ["KIMI_CODE_OAUTH_HOST", "KIMI_OAUTH_HOST"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()))
            .filter(|value| host_override_allowed(value))
            .unwrap_or_else(|| DEFAULT_OAUTH_HOST.to_string());
        Self::with_host(&host, PollTiming::default(), Duration::from_secs(1))
    }
}

impl KimiOAuth {
    pub fn with_host(host: &str, timing: PollTiming, refresh_backoff: Duration) -> Self {
        Self {
            http: reqwest::Client::new(),
            host: host.trim_end_matches('/').to_string(),
            timing,
            refresh_backoff,
        }
    }

    /// Requests a device code.
    pub async fn start(&self) -> Result<DeviceCodeSession> {
        let (status, body) = self.post_form("/api/oauth/device_authorization", &[("client_id", CLIENT_ID)]).await?;
        if !(200..300).contains(&status) {
            bail!("Kimi Code device authorization failed with status {status}{}", detail_suffix(&body));
        }
        parse_device_code(&body)
    }

    /// Polls the token endpoint until the user approves or the code expires.
    pub async fn poll(&self, session: &DeviceCodeSession) -> Result<OAuthTokens> {
        poll_device_code(session.interval_secs, session.expires_in_secs, self.timing, || async {
            let fields = [
                ("client_id", CLIENT_ID),
                ("device_code", session.device_code.as_str()),
                ("grant_type", DEVICE_GRANT),
            ];
            let (status, body) = self.post_form("/api/oauth/token", &fields).await?;
            Ok(poll_step(status, &body))
        })
        .await
    }

    /// Refreshes the tokens, retrying transient failures with backoff.
    /// A 401/403 or `invalid_grant` means the credential is dead: sign in again.
    pub async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokens> {
        let fields = [
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ];
        let mut attempt = 0;
        loop {
            if attempt > 0 {
                tokio::time::sleep(self.refresh_backoff * (1 << (attempt - 1))).await;
            }
            let retries_left = attempt < REFRESH_MAX_RETRIES;
            attempt += 1;
            let (status, body) = match self.post_form("/api/oauth/token", &fields).await {
                Ok(answer) => answer,
                Err(_) if retries_left => continue,
                Err(err) => return Err(err),
            };
            if (200..300).contains(&status) {
                return parse_tokens(&body).map_err(|err| anyhow!("Kimi Code token refresh failed: {err}"));
            }
            let unauthorized = matches!(status, 401 | 403) || body.get("error").and_then(Value::as_str) == Some("invalid_grant");
            if unauthorized {
                bail!("Kimi Code token refresh unauthorized (status {status}){}", description_suffix(&body));
            }
            if (status == 429 || status >= 500) && retries_left {
                continue;
            }
            bail!("Kimi Code token refresh failed with status {status}{}", detail_suffix(&body));
        }
    }

    /// Form POST; returns `(http_status, json_body)` (`Null` when not JSON).
    async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<(u16, Value)> {
        let response = self
            .http
            .post(format!("{}{path}", self.host))
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .form(fields)
            .send()
            .await
            .map_err(|err| anyhow!("Kimi Code OAuth request failed: {}", err.without_url()))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::String(text))))
    }
}

fn detail_suffix(body: &Value) -> String {
    let text = match body {
        Value::String(text) => error_detail(text),
        Value::Null => String::new(),
        other => error_detail(&other.to_string()),
    };
    if text.is_empty() { String::new() } else { format!(": {text}") }
}

fn description_suffix(body: &Value) -> String {
    match body.get("error_description").and_then(Value::as_str) {
        Some(description) => format!(": {description}"),
        None => String::new(),
    }
}

fn positive(body: &Value, field: &str) -> Option<f64> {
    body.get(field).and_then(Value::as_f64).filter(|value| value.is_finite() && *value > 0.0)
}

fn parse_device_code(body: &Value) -> Result<DeviceCodeSession> {
    let text = |field: &str| body.get(field).and_then(Value::as_str).filter(|value| !value.is_empty());
    let uri = text("verification_uri_complete").or_else(|| text("verification_uri")).and_then(trusted_http_url);
    let (Some(device_code), Some(user_code), Some(verification_uri)) = (text("device_code"), text("user_code"), uri) else {
        bail!("Invalid Kimi Code device authorization response: {}", error_detail(&body.to_string()));
    };
    Ok(DeviceCodeSession {
        user_code: user_code.to_string(),
        verification_uri,
        interval_secs: Some(positive(body, "interval").unwrap_or(DEFAULT_POLL_INTERVAL_SECS)),
        expires_in_secs: positive(body, "expires_in").unwrap_or(DEFAULT_DEVICE_CODE_TIMEOUT_SECS),
        device_code: device_code.to_string(),
        enterprise_domain: None,
    })
}

fn parse_tokens(body: &Value) -> Result<OAuthTokens> {
    let text = |field: &str| body.get(field).and_then(Value::as_str).filter(|value| !value.is_empty());
    let (Some(access), Some(refresh), Some(expires_in)) = (text("access_token"), text("refresh_token"), positive(body, "expires_in")) else {
        bail!("Kimi Code token response is missing access_token, refresh_token or expires_in");
    };
    // Like the other providers, refresh a little before the real expiry
    // (at most half the lifetime for very short-lived tokens).
    let lifetime_ms = (expires_in * 1000.0) as i64;
    let expires_ms = super::now_ms() + lifetime_ms - EXPIRY_MARGIN_MS.min(lifetime_ms / 2);
    Ok(OAuthTokens::new(access.to_string(), refresh.to_string(), expires_ms))
}

fn poll_step(status: u16, body: &Value) -> PollStep<OAuthTokens> {
    if status >= 500 {
        return PollStep::Failed(format!("Kimi Code device token request failed with status {status}{}", detail_suffix(body)));
    }
    if body.get("access_token").and_then(Value::as_str).is_some() {
        return match parse_tokens(body) {
            Ok(tokens) => PollStep::Complete(tokens),
            Err(err) => PollStep::Failed(err.to_string()),
        };
    }
    match body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => PollStep::Pending,
        Some("slow_down") => PollStep::SlowDown { interval_secs: positive(body, "interval") },
        Some("expired_token") => PollStep::Failed("Kimi Code device authorization expired. Please restart login.".to_string()),
        Some("access_denied") => PollStep::Failed("Kimi Code login was denied.".to_string()),
        other => {
            let error = other.map(|error| format!(": {error}")).unwrap_or_default();
            let description = description_suffix(body);
            let description = if other.is_some() { description } else { String::new() };
            PollStep::Failed(format!("Kimi Code device token request failed (status {status}){error}{description}"))
        }
    }
}

/// A `KIMI_*_OAUTH_HOST` override must be https (plain http only to loopback);
/// anything else would send the device grant and refresh tokens in clear.
fn host_override_allowed(host: &str) -> bool {
    let host = host.trim();
    host.starts_with("https://")
        || ["http://127.0.0.1", "http://localhost", "http://[::1]"]
            .iter()
            .any(|prefix| host.starts_with(prefix) && matches!(host[prefix.len()..].chars().next(), None | Some(':' | '/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_override_requires_https_or_loopback() {
        assert!(host_override_allowed("https://auth.example.com"));
        assert!(host_override_allowed("http://127.0.0.1:8080"));
        assert!(host_override_allowed("http://localhost"));
        assert!(!host_override_allowed("http://auth.example.com"));
        assert!(!host_override_allowed("http://localhost.evil.com"));
        assert!(!host_override_allowed("auth.example.com"));
    }

    use axum::{extract::State, routing::post, Form, Json, Router};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    type Script = Arc<Mutex<Vec<(u16, Value)>>>;
    type Forms = Arc<Mutex<Vec<HashMap<String, String>>>>;

    /// `/api/oauth/device_authorization` always answers a device code;
    /// `/api/oauth/token` replays `script` (the last entry repeats).
    async fn start_mock(script: Vec<(u16, Value)>) -> (KimiOAuth, Forms) {
        let script: Script = Arc::new(Mutex::new(script));
        let forms: Forms = Arc::new(Mutex::new(Vec::new()));
        async fn device(State((_, forms)): State<(Script, Forms)>, Form(form): Form<HashMap<String, String>>) -> Json<Value> {
            forms.lock().unwrap().push(form);
            Json(json!({
                "device_code": "dev-secret", "user_code": "KIMI-1234",
                "verification_uri": "https://www.kimi.com/code/authorize_device",
                "verification_uri_complete": "https://www.kimi.com/code/authorize_device?user_code=KIMI-1234",
                "interval": 0.01, "expires_in": 5
            }))
        }
        async fn token(
            State((script, forms)): State<(Script, Forms)>,
            Form(form): Form<HashMap<String, String>>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            forms.lock().unwrap().push(form);
            let mut script = script.lock().unwrap();
            let (status, body) = if script.len() > 1 { script.remove(0) } else { script[0].clone() };
            (axum::http::StatusCode::from_u16(status).unwrap(), Json(body))
        }
        let app = Router::new()
            .route("/api/oauth/device_authorization", post(device))
            .route("/api/oauth/token", post(token))
            .with_state((script, forms.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let timing = PollTiming {
            default_interval: Duration::from_millis(10),
            min_interval: Duration::from_millis(5),
            slow_down_step: Duration::from_millis(10),
        };
        (KimiOAuth::with_host(&host, timing, Duration::from_millis(5)), forms)
    }

    fn granted() -> (u16, Value) {
        (200, json!({ "access_token": "acc-1", "refresh_token": "ref-1", "expires_in": 3600 }))
    }

    #[tokio::test]
    async fn device_login_stores_tokens_without_extra_fields_and_with_a_margin() {
        let (kimi, forms) = start_mock(vec![
            (400, json!({ "error": "authorization_pending" })),
            (400, json!({ "error": "slow_down", "interval": 0.02 })),
            granted(),
        ])
        .await;
        let session = kimi.start().await.unwrap();
        assert_eq!(session.user_code, "KIMI-1234");
        assert_eq!(session.verification_uri, "https://www.kimi.com/code/authorize_device?user_code=KIMI-1234");
        assert!(!format!("{session:?}").contains("dev-secret"));

        let before = super::super::now_ms();
        let credential = kimi.poll(&session).await.unwrap().to_credential();
        assert_eq!(credential["access"], "acc-1");
        assert_eq!(credential["refresh"], "ref-1");
        let expires = credential["expires"].as_i64().unwrap();
        assert!(expires >= before + 3_300_000 && expires < before + 3_320_000, "5 minute refresh margin");
        assert_eq!(credential.as_object().unwrap().len(), 4);

        let forms = forms.lock().unwrap();
        assert_eq!(forms[0]["client_id"], CLIENT_ID);
        assert_eq!(forms[1]["grant_type"], DEVICE_GRANT);
        assert_eq!(forms[1]["device_code"], "dev-secret");
    }

    #[tokio::test]
    async fn device_login_failures_have_clear_messages() {
        for (status, body, expected) in [
            (400, json!({ "error": "expired_token" }), "authorization expired"),
            (400, json!({ "error": "access_denied" }), "was denied"),
            (503, json!({ "oops": true }), "failed with status 503"),
            (400, json!({ "error": "weird", "error_description": "why" }), "(status 400): weird: why"),
        ] {
            let (kimi, _) = start_mock(vec![(status, body)]).await;
            let session = kimi.start().await.unwrap();
            let err = kimi.poll(&session).await.unwrap_err().to_string();
            assert!(err.contains(expected), "{err}");
        }
    }

    #[tokio::test]
    async fn refresh_rotates_tokens() {
        let (kimi, forms) = start_mock(vec![(200, json!({ "access_token": "acc-2", "refresh_token": "ref-2", "expires_in": 60 }))]).await;
        let tokens = kimi.refresh("ref-old").await.unwrap();
        assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("acc-2", "ref-2"));
        let form = forms.lock().unwrap()[0].clone();
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["refresh_token"], "ref-old");
        assert_eq!(form["client_id"], CLIENT_ID);
    }

    #[tokio::test]
    async fn refresh_retries_transient_failures() {
        let (kimi, forms) = start_mock(vec![(503, json!({})), (429, json!({})), (200, granted().1)]).await;
        assert_eq!(kimi.refresh("ref").await.unwrap().access, "acc-1");
        assert_eq!(forms.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn refresh_gives_up_after_four_attempts() {
        let (kimi, forms) = start_mock(vec![(500, json!({ "x": 1 }))]).await;
        let err = kimi.refresh("ref").await.unwrap_err().to_string();
        assert!(err.starts_with("Kimi Code token refresh failed with status 500"), "{err}");
        assert_eq!(forms.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn refresh_unauthorized_is_final() {
        for (status, body) in [(401, json!({})), (403, json!({})), (400, json!({ "error": "invalid_grant", "error_description": "revoked" }))] {
            let (kimi, forms) = start_mock(vec![(status, body)]).await;
            let err = kimi.refresh("ref-secret").await.unwrap_err().to_string();
            assert!(err.starts_with(&format!("Kimi Code token refresh unauthorized (status {status})")), "{err}");
            assert!(!err.contains("ref-secret"));
            assert_eq!(forms.lock().unwrap().len(), 1, "no retries");
        }
    }
}
