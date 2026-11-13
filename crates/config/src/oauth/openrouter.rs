//! OpenRouter "Sign in with OpenRouter" (port of pi's `openrouter.js`): PKCE
//! with a loopback redirect on an ephemeral port (random path as the guard)
//! and a manual-paste fallback. The result is a permanent API key, stored as
//! an oauth credential with an empty refresh token and a far-future expiry.

use super::callback_server::{parse_authorization_code, params_from_code, CallbackConfig, CallbackParams, CallbackServer};
use super::pkce::generate_pkce;
use super::{error_detail, AuthUrlSession, OAuthTokens};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const TOKEN_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// `Number.MAX_SAFE_INTEGER`: the key never expires.
const NEVER_EXPIRES_MS: i64 = 9_007_199_254_740_991;

/// OpenRouter OAuth client; endpoints are injectable for tests.
pub struct OpenRouterOAuth {
    http: reqwest::Client,
    authorize_url: String,
    token_url: String,
}

impl Default for OpenRouterOAuth {
    fn default() -> Self {
        Self::with_endpoints(AUTHORIZE_URL, TOKEN_URL)
    }
}

impl OpenRouterOAuth {
    pub fn with_endpoints(authorize_url: &str, token_url: &str) -> Self {
        Self { http: reqwest::Client::new(), authorize_url: authorize_url.to_string(), token_url: token_url.to_string() }
    }

    /// Starts the loopback receiver and builds the authorization URL.
    pub async fn start(&self) -> Result<(AuthUrlSession, CallbackServer)> {
        let pkce = generate_pkce();
        let path = format!("/oauth/callback/{}", uuid::Uuid::new_v4());
        let server = CallbackServer::start(CallbackConfig {
            provider_name: "OpenRouter".to_string(),
            port: 0,
            path: path.clone(),
            expected_state: None,
            timeout: Some(LOGIN_TIMEOUT),
        })
        .await?;
        let redirect_uri = format!("http://127.0.0.1:{}{path}", server.port());
        let mut url = reqwest::Url::parse(&self.authorize_url)?;
        url.query_pairs_mut()
            .append_pair("callback_url", &redirect_uri)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256");
        let session = AuthUrlSession { auth_url: url.to_string(), redirect_uri, state: None, verifier: pkce.verifier };
        Ok((session, server))
    }

    /// Accepts a pasted redirect URL, `code=...` query or bare code.
    pub fn parse_pasted_redirect(&self, input: &str) -> Result<CallbackParams> {
        Ok(params_from_code(&parse_authorization_code(input)?))
    }

    /// Exchanges the authorization code for the API key.
    pub async fn exchange(&self, session: &AuthUrlSession, params: &CallbackParams) -> Result<OAuthTokens> {
        let response = self
            .http
            .post(&self.token_url)
            .header("accept", "application/json")
            .timeout(TOKEN_EXCHANGE_TIMEOUT)
            .json(&json!({
                "code": params.code(),
                "code_verifier": session.verifier,
                "code_challenge_method": "S256",
            }))
            .send()
            .await
            .map_err(|err| anyhow!("OpenRouter OAuth key exchange failed: {}", err.without_url()))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let detail = failure_detail(&text).unwrap_or_else(|| error_detail(&text));
            let suffix = if detail.is_empty() { String::new() } else { format!(": {detail}") };
            bail!("OpenRouter OAuth key exchange failed (HTTP {}){suffix}", status.as_u16());
        }
        let body: Value = serde_json::from_str(&text).map_err(|_| anyhow!("OpenRouter OAuth returned invalid JSON"))?;
        let key = body
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| anyhow!("OpenRouter OAuth response carries no \"key\""))?;
        Ok(OAuthTokens::new(key.to_string(), String::new(), NEVER_EXPIRES_MS))
    }
}

/// `error_description`, `message`, `error` (string) or `error.message`.
fn failure_detail(text: &str) -> Option<String> {
    let body: Value = serde_json::from_str(text).ok()?;
    ["error_description", "message", "error"]
        .iter()
        .find_map(|field| body.get(*field).and_then(Value::as_str))
        .or_else(|| body.pointer("/error/message").and_then(Value::as_str))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    type Bodies = Arc<Mutex<Vec<Value>>>;

    async fn mock_key_server(status: u16, response: Value) -> (String, Bodies) {
        let bodies: Bodies = Arc::new(Mutex::new(Vec::new()));
        async fn keys(
            State((bodies, status, response)): State<(Bodies, u16, Value)>,
            Json(body): Json<Value>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            bodies.lock().unwrap().push(body);
            (axum::http::StatusCode::from_u16(status).unwrap(), Json(response))
        }
        let app = Router::new().route("/keys", post(keys)).with_state((bodies.clone(), status, response));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, bodies)
    }

    #[tokio::test]
    async fn start_builds_pkce_url_with_random_callback_path() {
        let oauth = OpenRouterOAuth::with_endpoints("https://openrouter.example.test/auth", "http://unused");
        let (session, server) = oauth.start().await.unwrap();
        let url = reqwest::Url::parse(&session.auth_url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["code_challenge"], super::super::pkce::challenge_for(&session.verifier));
        assert_eq!(query["callback_url"], session.redirect_uri);
        assert!(session.redirect_uri.starts_with(&format!("http://127.0.0.1:{}/oauth/callback/", server.port())));
        assert!(session.state.is_none());
    }

    #[tokio::test]
    async fn loopback_redirect_then_key_exchange_stores_a_non_refreshing_credential() {
        let (base, bodies) = mock_key_server(200, json!({ "key": "sk-or-secret" })).await;
        let oauth = OpenRouterOAuth::with_endpoints("https://openrouter.example.test/auth", &format!("{base}/keys"));
        let (session, server) = oauth.start().await.unwrap();

        let redirect = format!("{}?code=auth-code", session.redirect_uri);
        assert_eq!(reqwest::get(&redirect).await.unwrap().status(), 200);
        let params = server.wait().await.unwrap();
        let credential = oauth.exchange(&session, &params).await.unwrap().to_credential();
        assert_eq!(credential["type"], "oauth");
        assert_eq!(credential["access"], "sk-or-secret");
        assert_eq!(credential["refresh"], "");
        assert_eq!(credential["expires"], 9_007_199_254_740_991_i64);

        let body = bodies.lock().unwrap()[0].clone();
        assert_eq!(body["code"], "auth-code");
        assert_eq!(body["code_verifier"], session.verifier);
        assert_eq!(body["code_challenge_method"], "S256");
    }

    #[tokio::test]
    async fn another_path_on_the_callback_server_is_refused() {
        let oauth = OpenRouterOAuth::default();
        let (session, _server) = oauth.start().await.unwrap();
        let wrong = session.redirect_uri.replace("/oauth/callback/", "/oauth/other/");
        assert_eq!(reqwest::get(format!("{wrong}?code=x")).await.unwrap().status(), 404);
    }

    #[tokio::test]
    async fn manual_paste_accepts_url_or_code() {
        let oauth = OpenRouterOAuth::default();
        assert_eq!(oauth.parse_pasted_redirect("http://127.0.0.1:5/oauth/callback/x?code=c1").unwrap().code(), "c1");
        assert_eq!(oauth.parse_pasted_redirect("c2").unwrap().code(), "c2");
        assert!(oauth.parse_pasted_redirect("").is_err());
    }

    #[tokio::test]
    async fn exchange_failures_report_the_server_detail() {
        let (base, _) = mock_key_server(403, json!({ "error": { "message": "bad verifier" } })).await;
        let oauth = OpenRouterOAuth::with_endpoints("https://x.test/auth", &format!("{base}/keys"));
        let (session, _server) = oauth.start().await.unwrap();
        let err = oauth.exchange(&session, &params_from_code("c")).await.unwrap_err().to_string();
        assert_eq!(err, "OpenRouter OAuth key exchange failed (HTTP 403): bad verifier");

        let (base, _) = mock_key_server(200, json!({ "no": "key" })).await;
        let oauth = OpenRouterOAuth::with_endpoints("https://x.test/auth", &format!("{base}/keys"));
        let err = oauth.exchange(&session, &params_from_code("c")).await.unwrap_err().to_string();
        assert!(err.contains("carries no \"key\""), "{err}");
    }
}
