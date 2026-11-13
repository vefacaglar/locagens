//! xAI (Grok / SuperGrok / X Premium) OAuth device-code flow (port of pi's
//! `xai.js`).

use super::device_code::{poll_device_code, PollStep, PollTiming};
pub use super::device_code::DeviceCodeSession;
use super::OAuthTokens;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::time::Duration;

const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Refresh slightly before the reported expiry so a token never dies mid-request.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
const DEFAULT_TOKEN_LIFETIME_SECS: f64 = 3600.0;
const LOGIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Refresh runs while the auth.json lock is held, so keep it short.
const REFRESH_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// xAI OAuth client; endpoints are injectable so tests can use a mock server.
pub struct XaiOAuth {
    http: reqwest::Client,
    device_code_url: String,
    token_url: String,
    timing: PollTiming,
}

impl Default for XaiOAuth {
    fn default() -> Self {
        Self::with_endpoints(DEVICE_CODE_URL, TOKEN_URL, PollTiming::default())
    }
}

impl XaiOAuth {
    pub fn with_endpoints(device_code_url: &str, token_url: &str, timing: PollTiming) -> Self {
        Self {
            http: reqwest::Client::new(),
            device_code_url: device_code_url.to_string(),
            token_url: token_url.to_string(),
            timing,
        }
    }

    /// Requests a device code.
    pub async fn start(&self) -> Result<DeviceCodeSession> {
        let fields = [("client_id", CLIENT_ID), ("scope", SCOPE), ("referrer", "locagens")];
        let (ok, status, body) = self.post_form(&self.device_code_url, &fields, LOGIN_REQUEST_TIMEOUT).await?;
        if !ok {
            return Err(request_failure("device authorization", status, &body));
        }
        parse_device_code(&body)
    }

    /// Polls the token endpoint until the user approves or the code expires.
    pub async fn poll(&self, session: &DeviceCodeSession) -> Result<OAuthTokens> {
        poll_device_code(session.interval_secs, session.expires_in_secs, self.timing, || async {
            let fields = [
                ("grant_type", DEVICE_GRANT),
                ("client_id", CLIENT_ID),
                ("device_code", session.device_code.as_str()),
            ];
            let (ok, status, body) = self.post_form(&self.token_url, &fields, LOGIN_REQUEST_TIMEOUT).await?;
            if ok {
                return Ok(PollStep::Complete(tokens_from_response(&body, None)?));
            }
            Ok(match body.get("error").and_then(Value::as_str) {
                Some("authorization_pending") => PollStep::Pending,
                Some("slow_down") => PollStep::SlowDown {
                    interval_secs: body.get("interval").and_then(Value::as_f64),
                },
                Some("access_denied" | "authorization_denied") => {
                    PollStep::Failed("xAI device authorization was denied".to_string())
                }
                Some("expired_token") => PollStep::Failed("xAI device code expired".to_string()),
                _ => PollStep::Failed(request_failure("device token polling", status, &body).to_string()),
            })
        })
        .await
    }

    /// Exchanges a refresh token for new tokens; keeps the previous refresh
    /// token when the server does not rotate it.
    pub async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokens> {
        let fields = [
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
        ];
        let (ok, status, body) = self.post_form(&self.token_url, &fields, REFRESH_REQUEST_TIMEOUT).await?;
        if !ok {
            return Err(request_failure("token refresh", status, &body));
        }
        tokens_from_response(&body, Some(refresh_token))
    }

    /// Form-encoded POST; returns `(is_success, http_status, json_body)`.
    async fn post_form(
        &self,
        url: &str,
        fields: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<(bool, u16, Value)> {
        let response = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .timeout(timeout)
            .form(fields)
            .send()
            .await
            .map_err(|err| anyhow!("xAI OAuth request failed: {}", err.without_url()))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|_| anyhow!("xAI OAuth returned invalid JSON (HTTP {})", status.as_u16()))?;
        Ok((status.is_success(), status.as_u16(), body))
    }
}

fn request_failure(action: &str, status: u16, body: &Value) -> anyhow::Error {
    let detail = ["error", "error_description"]
        .iter()
        .filter_map(|field| body.get(*field).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(": ");
    if detail.is_empty() {
        anyhow!("xAI OAuth {action} failed (HTTP {status})")
    } else {
        anyhow!("xAI OAuth {action} failed (HTTP {status}): {detail}")
    }
}

fn required_string(body: &Value, field: &str) -> Result<String> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Invalid xAI OAuth response field: {field}"))
}

fn positive_number(body: &Value, field: &str) -> Result<f64> {
    body.get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| anyhow!("Invalid xAI OAuth response field: {field}"))
}

/// The verification URI is opened in the user's browser, so it must be https.
pub fn validate_verification_uri(raw: &str) -> Result<String> {
    match reqwest::Url::parse(raw) {
        Ok(url) if url.scheme() == "https" => Ok(url.to_string()),
        _ => bail!("Untrusted verification URI in xAI OAuth response"),
    }
}

fn parse_device_code(body: &Value) -> Result<DeviceCodeSession> {
    let uri = validate_verification_uri(&required_string(body, "verification_uri")?)?;
    let complete = match body.get("verification_uri_complete").and_then(Value::as_str) {
        Some(raw) if !raw.is_empty() => Some(validate_verification_uri(raw)?),
        _ => None,
    };
    Ok(DeviceCodeSession {
        user_code: required_string(body, "user_code")?,
        verification_uri: complete.unwrap_or(uri),
        // RFC 8628 allows interval 0; fall back to the poller default.
        interval_secs: body
            .get("interval")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0),
        expires_in_secs: positive_number(body, "expires_in")?,
        device_code: required_string(body, "device_code")?,
        enterprise_domain: None,
    })
}

fn tokens_from_response(body: &Value, previous_refresh: Option<&str>) -> Result<OAuthTokens> {
    let access = required_string(body, "access_token")?;
    // xAI may omit refresh_token on refresh when the token is not rotated.
    let refresh = match (body.get("refresh_token"), previous_refresh) {
        (None, Some(previous)) => previous.to_string(),
        _ => required_string(body, "refresh_token")?,
    };
    let lifetime_secs = match body.get("expires_in") {
        None => DEFAULT_TOKEN_LIFETIME_SECS,
        Some(_) => positive_number(body, "expires_in")?,
    };
    let expires_ms = super::now_ms() + (lifetime_secs * 1000.0) as i64 - REFRESH_SKEW_MS;
    Ok(OAuthTokens::new(access, refresh, expires_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Form, Json, Router};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Mock xAI server: `/device` returns the device code, `/token` replays
    /// scripted responses in order (status, body) and records the form posts.
    struct Mock {
        base: String,
        token_script: Arc<Mutex<Vec<(u16, Value)>>>,
        forms: Arc<Mutex<Vec<HashMap<String, String>>>>,
    }

    type MockState = (Arc<Mutex<Vec<(u16, Value)>>>, Arc<Mutex<Vec<HashMap<String, String>>>>);

    async fn start_mock(token_script: Vec<(u16, Value)>) -> Mock {
        let script = Arc::new(Mutex::new(token_script));
        let forms = Arc::new(Mutex::new(Vec::new()));
        async fn device(
            State((_, forms)): State<MockState>,
            Form(form): Form<HashMap<String, String>>,
        ) -> Json<Value> {
            forms.lock().unwrap().push(form);
            Json(serde_json::json!({
                "device_code": "dev-secret",
                "user_code": "ABCD-1234",
                "verification_uri": "https://auth.x.ai/device",
                "verification_uri_complete": "https://auth.x.ai/device?user_code=ABCD-1234",
                "interval": 0.01,
                "expires_in": 5
            }))
        }
        async fn token(
            State((script, forms)): State<MockState>,
            Form(form): Form<HashMap<String, String>>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            forms.lock().unwrap().push(form);
            let mut script = script.lock().unwrap();
            let (status, body) = if script.len() > 1 { script.remove(0) } else { script[0].clone() };
            (axum::http::StatusCode::from_u16(status).unwrap(), Json(body))
        }
        let app = Router::new()
            .route("/device", post(device))
            .route("/token", post(token))
            .with_state((script.clone(), forms.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Mock { base, token_script: script, forms }
    }

    fn fast_timing() -> PollTiming {
        PollTiming {
            default_interval: Duration::from_millis(10),
            min_interval: Duration::from_millis(5),
            slow_down_step: Duration::from_millis(10),
        }
    }

    fn client(base: &str) -> XaiOAuth {
        XaiOAuth::with_endpoints(&format!("{base}/device"), &format!("{base}/token"), fast_timing())
    }

    fn pending() -> (u16, Value) {
        (400, serde_json::json!({ "error": "authorization_pending" }))
    }

    fn granted() -> (u16, Value) {
        (200, serde_json::json!({ "access_token": "acc-1", "refresh_token": "ref-1", "expires_in": 3600 }))
    }

    #[tokio::test]
    async fn device_flow_pending_slow_down_then_complete() {
        let mock = start_mock(vec![
            pending(),
            (400, serde_json::json!({ "error": "slow_down" })),
            (400, serde_json::json!({ "error": "slow_down", "interval": 0.02 })),
            granted(),
        ])
        .await;
        let xai = client(&mock.base);
        let session = xai.start().await.unwrap();
        assert_eq!(session.user_code, "ABCD-1234");
        assert_eq!(session.verification_uri, "https://auth.x.ai/device?user_code=ABCD-1234");
        assert!(!format!("{session:?}").contains("dev-secret"));

        let before = super::super::now_ms();
        let tokens = xai.poll(&session).await.unwrap();
        assert_eq!(tokens.access, "acc-1");
        assert_eq!(tokens.refresh, "ref-1");
        assert!(tokens.expires_ms > before + 50 * 60 * 1000 && tokens.expires_ms < before + 56 * 60 * 1000);

        let forms = mock.forms.lock().unwrap();
        assert_eq!(forms[0]["referrer"], "locagens");
        assert_eq!(forms[0]["client_id"], CLIENT_ID);
        assert_eq!(forms.len(), 1 + 4);
        assert_eq!(forms[1]["device_code"], "dev-secret");
        assert_eq!(forms[1]["grant_type"], DEVICE_GRANT);
        assert_eq!(mock.token_script.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn device_flow_denied_and_expired_are_failures() {
        for (error, expected) in [
            ("access_denied", "denied"),
            ("authorization_denied", "denied"),
            ("expired_token", "expired"),
        ] {
            let mock = start_mock(vec![(400, serde_json::json!({ "error": error }))]).await;
            let xai = client(&mock.base);
            let session = xai.start().await.unwrap();
            let err = xai.poll(&session).await.unwrap_err().to_string();
            assert!(err.contains(expected), "{error}: {err}");
        }
    }

    #[tokio::test]
    async fn device_flow_times_out_at_deadline() {
        let mock = start_mock(vec![pending()]).await;
        let xai = client(&mock.base);
        let mut session = xai.start().await.unwrap();
        session.expires_in_secs = 0.1;
        let err = xai.poll(&session).await.unwrap_err().to_string();
        assert_eq!(err, "Device flow timed out");

        let mock = start_mock(vec![(400, serde_json::json!({ "error": "slow_down" }))]).await;
        let xai = client(&mock.base);
        let mut session = xai.start().await.unwrap();
        session.expires_in_secs = 0.1;
        let err = xai.poll(&session).await.unwrap_err().to_string();
        assert!(err.contains("slow_down") && err.contains("clock drift"));
    }

    #[tokio::test]
    async fn refresh_keeps_previous_refresh_token_when_omitted() {
        let mock = start_mock(vec![(200, serde_json::json!({ "access_token": "acc-2" }))]).await;
        let tokens = client(&mock.base).refresh("ref-old").await.unwrap();
        assert_eq!(tokens.access, "acc-2");
        assert_eq!(tokens.refresh, "ref-old");
        assert_eq!(mock.forms.lock().unwrap()[0]["grant_type"], "refresh_token");
    }

    #[tokio::test]
    async fn refresh_failure_reports_server_error_without_tokens() {
        let mock = start_mock(vec![(400, serde_json::json!({ "error": "invalid_grant", "error_description": "revoked" }))]).await;
        let err = client(&mock.base).refresh("ref-secret").await.unwrap_err().to_string();
        assert_eq!(err, "xAI OAuth token refresh failed (HTTP 400): invalid_grant: revoked");
    }

    #[test]
    fn verification_uri_must_be_https() {
        assert!(validate_verification_uri("https://auth.x.ai/device").is_ok());
        assert!(validate_verification_uri("http://auth.x.ai/device").is_err());
        assert!(validate_verification_uri("file:///c:/windows/system32/calc.exe").is_err());
        assert!(validate_verification_uri("not a url").is_err());
    }
}
