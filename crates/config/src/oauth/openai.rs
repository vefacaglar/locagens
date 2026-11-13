//! OpenAI "Sign in with ChatGPT" (port of pi's `openai-chatgpt.js`): a
//! dynamically registered agent client, PKCE, loopback redirect on
//! `127.0.0.1:1455` with a manual-paste fallback. The resulting access token
//! is used as a bearer for `https://api.openai.com/v1` (Responses API).
//!
//! This is not the Codex CLI flow (`openai-codex`); that one is not implemented.

use super::callback_server::{parse_redirect_url, CallbackConfig, CallbackParams, CallbackServer};
use super::pkce::{generate_pkce, random_base64url};
use super::{error_detail, AuthUrlSession, OAuthTokens};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::time::Duration;

const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
const AGENT_NAME_HINT: &str = "Locagens";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const CALLBACK_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const DIRECT_TOKEN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The access token is refreshed three minutes before it expires.
const EXPIRY_MARGIN_MS: i64 = 3 * 60 * 1000;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// "Sign in with ChatGPT" client; endpoints and port are injectable for tests.
pub struct ChatgptOAuth {
    http: reqwest::Client,
    authorize_url: String,
    token_url: String,
    callback_port: u16,
}

impl Default for ChatgptOAuth {
    fn default() -> Self {
        Self::with_endpoints(AUTHORIZE_URL, TOKEN_URL, CALLBACK_PORT)
    }
}

impl ChatgptOAuth {
    /// `callback_port` 0 picks an ephemeral port (tests).
    pub fn with_endpoints(authorize_url: &str, token_url: &str, callback_port: u16) -> Self {
        Self {
            http: reqwest::Client::new(),
            authorize_url: authorize_url.to_string(),
            token_url: token_url.to_string(),
            callback_port,
        }
    }

    /// Starts the loopback receiver and builds the authorization URL.
    /// `device_id` is the per-installation UUID.
    pub async fn start(&self, device_id: &str) -> Result<(AuthUrlSession, CallbackServer)> {
        let host_id = host_id_for(device_id)?;
        let pkce = generate_pkce();
        let state = random_base64url(32);
        let nonce = random_base64url(32);
        let server = CallbackServer::start(CallbackConfig {
            provider_name: "ChatGPT".to_string(),
            port: self.callback_port,
            path: CALLBACK_PATH.to_string(),
            expected_state: Some(state.clone()),
            timeout: Some(LOGIN_TIMEOUT),
        })
        .await
        .map_err(|err| {
            if self.callback_port == CALLBACK_PORT {
                anyhow!("Port {CALLBACK_PORT} is in use, probably by an unfinished login in another session or by the Codex CLI. Cancel that login and try again.")
            } else {
                err
            }
        })?;
        let redirect_uri = format!("http://127.0.0.1:{}{CALLBACK_PATH}", server.port());
        let mut url = reqwest::Url::parse(&self.authorize_url)?;
        url.query_pairs_mut()
            .append_pair("client_id", DYNAMIC_CLIENT_ID)
            .append_pair("agent_name_hint", AGENT_NAME_HINT)
            .append_pair("ext_agent_host_id", &host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("resource", RESOURCE)
            .append_pair("scope", SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("nonce", &nonce);
        let session = AuthUrlSession {
            auth_url: url.to_string(),
            redirect_uri,
            state: Some(state),
            verifier: pkce.verifier,
        };
        Ok((session, server))
    }

    /// Validates a pasted redirect URL (manual fallback).
    pub fn parse_pasted_redirect(&self, session: &AuthUrlSession, input: &str) -> Result<CallbackParams> {
        parse_redirect_url(input, &session.redirect_uri, session.state.as_deref())
    }

    /// Exchanges the redirect parameters for tokens. The issued client id
    /// comes back in the redirect and is stored for refreshes.
    pub async fn exchange(&self, session: &AuthUrlSession, params: &CallbackParams) -> Result<OAuthTokens> {
        let client_id = params
            .get("client_id")
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("OpenAI OAuth registration callback did not contain an issued client ID"))?;
        let fields = [
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", params.code()),
            ("code_verifier", session.verifier.as_str()),
            ("redirect_uri", session.redirect_uri.as_str()),
            ("resource", RESOURCE),
        ];
        self.token_request(&fields, client_id).await
    }

    /// Refreshes with the stored client id (the refresh token rotates).
    pub async fn refresh(&self, entry: &Map<String, Value>) -> Result<OAuthTokens> {
        let client_id = entry
            .get("clientId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("Stored OpenAI OAuth credential does not contain an issued client ID; reconnect ChatGPT"))?;
        let refresh = entry
            .get("refresh")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("stored OAuth credential has no refresh token"))?;
        let fields = [
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh),
            ("resource", RESOURCE),
        ];
        self.token_request(&fields, client_id).await
    }

    async fn token_request(&self, fields: &[(&str, &str)], client_id: &str) -> Result<OAuthTokens> {
        let response = self
            .http
            .post(&self.token_url)
            .header("accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .form(fields)
            .send()
            .await
            .map_err(|err| anyhow!("OpenAI OAuth token request failed: {}", err.without_url()))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let detail = if body.trim().is_empty() { status.to_string() } else { error_detail(&body) };
            bail!("OpenAI OAuth token request failed ({}): {detail}", status.as_u16());
        }
        tokens_from_response(&body, client_id)
    }
}

/// `urn:uuid:<lowercase uuid>` of the installation's device id.
fn host_id_for(device_id: &str) -> Result<String> {
    match uuid::Uuid::parse_str(device_id.trim()) {
        Ok(id) if device_id.trim().len() == 36 => Ok(format!("urn:uuid:{}", id.hyphenated())),
        _ => bail!("Sign in with ChatGPT requires a device ID (UUID) for this installation"),
    }
}

fn non_blank<'a>(body: &'a Value, field: &str) -> Result<&'a str> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("OpenAI OAuth token response is missing {field}"))
}

fn tokens_from_response(raw_body: &str, client_id: &str) -> Result<OAuthTokens> {
    let body: Value = serde_json::from_str(raw_body).map_err(|_| anyhow!("OpenAI OAuth token response was not JSON"))?;
    non_blank(&body, "id_token")?;
    let access = non_blank(&body, "access_token")?;
    let refresh = non_blank(&body, "refresh_token")?;
    let scopes: Vec<&str> = non_blank(&body, "scope")?.split_whitespace().collect();
    if !scopes.contains(&DIRECT_TOKEN_SCOPE) {
        bail!("OpenAI OAuth grant did not include {DIRECT_TOKEN_SCOPE}");
    }
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| anyhow!("OpenAI OAuth token response is missing expires_in"))?;
    let expires_ms = super::now_ms() + (expires_in * 1000.0) as i64 - EXPIRY_MARGIN_MS;
    Ok(OAuthTokens::new(access.to_string(), refresh.to_string(), expires_ms)
        .with_extra("clientId", json!(client_id))
        .with_extra("scopes", json!(scopes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Form, Json, Router};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const DEVICE_ID: &str = "0F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9";

    type Forms = Arc<Mutex<Vec<HashMap<String, String>>>>;

    /// Token endpoint answering every request with `(status, body)`.
    async fn mock_token_server(status: u16, body: Value) -> (String, Forms) {
        let forms: Forms = Arc::new(Mutex::new(Vec::new()));
        async fn token(
            State((forms, status, body)): State<(Forms, u16, Value)>,
            Form(form): Form<HashMap<String, String>>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            forms.lock().unwrap().push(form);
            (axum::http::StatusCode::from_u16(status).unwrap(), Json(body))
        }
        let app = Router::new().route("/token", post(token)).with_state((forms.clone(), status, body));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, forms)
    }

    fn granted(scope: &str) -> Value {
        json!({ "id_token": "idt", "access_token": "acc-1", "refresh_token": "ref-1", "scope": scope, "expires_in": 3600 })
    }

    fn client(base: &str) -> ChatgptOAuth {
        ChatgptOAuth::with_endpoints("https://auth.example.test/authorize", &format!("{base}/token"), 0)
    }

    #[tokio::test]
    async fn start_builds_the_spec_authorize_url() {
        let (session, server) = ChatgptOAuth::with_endpoints("https://auth.example.test/authorize", "http://unused", 0)
            .start(DEVICE_ID)
            .await
            .unwrap();
        let url = reqwest::Url::parse(&session.auth_url).unwrap();
        let query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let names: Vec<&str> = query.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            ["client_id", "agent_name_hint", "ext_agent_host_id", "response_type", "redirect_uri", "resource", "scope", "state", "code_challenge", "code_challenge_method", "nonce"]
        );
        let get = |name: &str| query.iter().find(|(k, _)| k == name).unwrap().1.clone();
        assert_eq!(get("client_id"), "dynamic_agent_client");
        assert_eq!(get("agent_name_hint"), "Locagens");
        assert_eq!(get("ext_agent_host_id"), "urn:uuid:0f1e2d3c-4b5a-4978-8695-a4b3c2d1e0f9");
        assert_eq!(get("resource"), "https://api.openai.com/v1");
        assert!(get("scope").split(' ').any(|scope| scope == "chatgpt.tokens.use.direct"));
        assert_eq!(get("redirect_uri"), format!("http://127.0.0.1:{}/auth/callback", server.port()));
        assert_eq!(get("code_challenge"), super::super::pkce::challenge_for(&session.verifier));
        assert_eq!(Some(get("state")), session.state);
    }

    #[tokio::test]
    async fn start_rejects_a_bad_device_id() {
        let err = ChatgptOAuth::default().start("not-a-uuid").await.err().unwrap().to_string();
        assert!(err.contains("device ID"), "{err}");
    }

    #[tokio::test]
    async fn login_exchange_stores_client_id_scopes_and_margin() {
        let (base, forms) = mock_token_server(200, granted("openid chatgpt.tokens.use.direct")).await;
        let oauth = client(&base);
        let (session, _server) = oauth.start(DEVICE_ID).await.unwrap();
        let pasted = format!("{}?code=the-code&state={}&client_id=issued-client", session.redirect_uri, session.state.clone().unwrap());
        let params = oauth.parse_pasted_redirect(&session, &pasted).unwrap();

        let before = super::super::now_ms();
        let tokens = oauth.exchange(&session, &params).await.unwrap();
        let credential = tokens.to_credential();
        assert_eq!(credential["type"], "oauth");
        assert_eq!(credential["access"], "acc-1");
        assert_eq!(credential["refresh"], "ref-1");
        assert_eq!(credential["clientId"], "issued-client");
        assert_eq!(credential["scopes"], json!(["openid", "chatgpt.tokens.use.direct"]));
        let expires = credential["expires"].as_i64().unwrap();
        assert!(expires > before + 56 * 60 * 1000 && expires < before + 58 * 60 * 1000, "3 minute margin");

        let form = forms.lock().unwrap()[0].clone();
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["client_id"], "issued-client");
        assert_eq!(form["code"], "the-code");
        assert_eq!(form["code_verifier"], session.verifier);
        assert_eq!(form["redirect_uri"], session.redirect_uri);
        assert_eq!(form["resource"], "https://api.openai.com/v1");
    }

    #[tokio::test]
    async fn exchange_requires_issued_client_id_and_direct_scope() {
        let (base, _) = mock_token_server(200, granted("openid profile")).await;
        let oauth = client(&base);
        let (session, _server) = oauth.start(DEVICE_ID).await.unwrap();
        let state = session.state.clone().unwrap();

        let without_client = oauth.parse_pasted_redirect(&session, &format!("{}?code=c&state={state}", session.redirect_uri)).unwrap();
        let err = oauth.exchange(&session, &without_client).await.unwrap_err().to_string();
        assert!(err.contains("issued client ID"), "{err}");

        let with_client = oauth.parse_pasted_redirect(&session, &format!("{}?code=c&state={state}&client_id=x", session.redirect_uri)).unwrap();
        let err = oauth.exchange(&session, &with_client).await.unwrap_err().to_string();
        assert_eq!(err, "OpenAI OAuth grant did not include chatgpt.tokens.use.direct");
    }

    #[tokio::test]
    async fn refresh_uses_stored_client_id_and_keeps_it() {
        let (base, forms) = mock_token_server(200, granted("openid chatgpt.tokens.use.direct")).await;
        let entry = json!({ "type": "oauth", "refresh": "old-ref", "clientId": "issued-client" });
        let tokens = client(&base).refresh(entry.as_object().unwrap()).await.unwrap();
        assert_eq!(tokens.to_credential()["clientId"], "issued-client");
        let form = forms.lock().unwrap()[0].clone();
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["client_id"], "issued-client");
        assert_eq!(form["refresh_token"], "old-ref");
        assert_eq!(form["resource"], "https://api.openai.com/v1");
    }

    #[tokio::test]
    async fn refresh_without_client_id_asks_to_reconnect() {
        let entry = json!({ "type": "oauth", "refresh": "old-ref" });
        let err = ChatgptOAuth::default().refresh(entry.as_object().unwrap()).await.unwrap_err().to_string();
        assert!(err.contains("reconnect ChatGPT"), "{err}");
    }

    #[tokio::test]
    async fn token_errors_name_the_status_without_secrets() {
        let (base, _) = mock_token_server(400, json!({ "error": "invalid_grant" })).await;
        let entry = json!({ "refresh": "ref-secret", "clientId": "cid" });
        let err = client(&base).refresh(entry.as_object().unwrap()).await.unwrap_err().to_string();
        assert!(err.starts_with("OpenAI OAuth token request failed (400)"), "{err}");
        assert!(!err.contains("ref-secret"));
    }

    #[test]
    fn incomplete_token_responses_are_rejected() {
        for missing in ["id_token", "access_token", "refresh_token", "scope", "expires_in"] {
            let mut body = granted("chatgpt.tokens.use.direct");
            body.as_object_mut().unwrap().remove(missing);
            assert!(tokens_from_response(&body.to_string(), "cid").is_err(), "{missing}");
        }
    }
}
