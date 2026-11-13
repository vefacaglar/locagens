//! GitHub Copilot login (port of pi's `github-copilot.js`): GitHub device
//! flow, then exchange of the GitHub token for a short-lived Copilot token.
//!
//! Stored credential: `access` = Copilot token, `refresh` = GitHub token,
//! `expires` = Copilot expiry minus 5 minutes, plus `enterpriseUrl` and
//! `availableModelIds`. Refreshing re-runs the Copilot token exchange.

use super::catalog::COPILOT_MODELS;
use super::device_code::{poll_device_code, DeviceCodeSession, PollStep, PollTiming};
use super::{trusted_http_url, OAuthTokens};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// Public GitHub OAuth app id of the Copilot editor plugins.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const USER_AGENT: &str = "GitHubCopilotChat/0.35.0";
const COPILOT_API_VERSION: &str = "2026-06-01";
const INDIVIDUAL_BASE_URL: &str = "https://api.individual.githubcopilot.com";
/// The Copilot token is refreshed five minutes before it expires.
const EXPIRY_MARGIN_MS: i64 = 5 * 60 * 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MODELS_TIMEOUT: Duration = Duration::from_secs(5);

/// Headers Copilot requires on every request (token exchange, models, chat).
pub const COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", USER_AGENT),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];

/// Per-request headers: who initiated the call, and whether it carries images.
pub fn dynamic_headers(last_message_is_user: bool, has_images: bool) -> Vec<(&'static str, &'static str)> {
    let mut headers = vec![
        ("X-Initiator", if last_message_is_user { "user" } else { "agent" }),
        ("Openai-Intent", "conversation-edits"),
    ];
    if has_images {
        headers.push(("Copilot-Vision-Request", "true"));
    }
    headers
}

/// Host part of a GitHub Enterprise URL or bare domain; `None` when invalid.
pub fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    reqwest::Url::parse(&candidate).ok()?.host_str().map(str::to_string)
}

/// Chat API base URL for a Copilot token: its `proxy-ep` host with
/// `proxy.` swapped for `api.`, else the enterprise or individual endpoint.
pub fn base_url_from_token(token: &str, enterprise_domain: Option<&str>) -> String {
    let proxy_host = token
        .split_once("proxy-ep=")
        .map(|(_, rest)| rest.split(';').next().unwrap_or_default())
        .filter(|host| !host.is_empty());
    match (proxy_host, enterprise_domain) {
        (Some(host), _) => format!("https://{}", host.strip_prefix("proxy.").map_or_else(|| host.to_string(), |rest| format!("api.{rest}"))),
        (None, Some(domain)) => format!("https://copilot-api.{domain}"),
        (None, None) => INDIVIDUAL_BASE_URL.to_string(),
    }
}

/// The three GitHub URLs of the flow (plus a models base override for tests).
#[derive(Clone, Debug)]
pub struct CopilotEndpoints {
    pub device_code_url: String,
    pub access_token_url: String,
    pub copilot_token_url: String,
    pub models_base_url: Option<String>,
}

impl CopilotEndpoints {
    fn for_domain(domain: Option<&str>) -> Self {
        let domain = domain.unwrap_or("github.com");
        Self {
            device_code_url: format!("https://{domain}/login/device/code"),
            access_token_url: format!("https://{domain}/login/oauth/access_token"),
            copilot_token_url: format!("https://api.{domain}/copilot_internal/v2/token"),
            models_base_url: None,
        }
    }
}

/// Copilot OAuth client; endpoints are injectable so tests can use a mock server.
pub struct CopilotOAuth {
    http: reqwest::Client,
    endpoints: Option<CopilotEndpoints>,
    timing: PollTiming,
}

impl Default for CopilotOAuth {
    fn default() -> Self {
        Self { http: reqwest::Client::new(), endpoints: None, timing: PollTiming::default() }
    }
}

/// Models the account may use, and unconfigured ones worth enabling.
#[derive(Debug, Default, PartialEq, Eq)]
struct ModelAvailability {
    available: Vec<String>,
    to_enable: Vec<String>,
}

impl CopilotOAuth {
    /// Uses fixed endpoints instead of the ones derived from the domain.
    pub fn with_endpoints(endpoints: CopilotEndpoints, timing: PollTiming) -> Self {
        Self { http: reqwest::Client::new(), endpoints: Some(endpoints), timing }
    }

    fn endpoints_for(&self, domain: Option<&str>) -> CopilotEndpoints {
        self.endpoints.clone().unwrap_or_else(|| CopilotEndpoints::for_domain(domain))
    }

    /// Requests a GitHub device code. `enterprise_input` is the optional
    /// GitHub Enterprise URL or domain (blank for github.com).
    pub async fn start(&self, enterprise_input: Option<&str>) -> Result<DeviceCodeSession> {
        let domain = match enterprise_input.map(str::trim).filter(|input| !input.is_empty()) {
            Some(input) => Some(normalize_domain(input).ok_or_else(|| anyhow!("Invalid GitHub Enterprise URL/domain"))?),
            None => None,
        };
        let endpoints = self.endpoints_for(domain.as_deref());
        let (ok, body) = self
            .post_form(&endpoints.device_code_url, &[("client_id", CLIENT_ID), ("scope", "read:user")])
            .await?;
        if !ok {
            bail!("GitHub device code request failed");
        }
        parse_device_code(&body, domain)
    }

    /// Polls until the user authorizes, then exchanges the GitHub token for a
    /// Copilot token and looks up the models the account may use.
    pub async fn poll(&self, session: &DeviceCodeSession) -> Result<OAuthTokens> {
        let endpoints = self.endpoints_for(session.enterprise_domain.as_deref());
        let github_token = poll_device_code(session.interval_secs, session.expires_in_secs, self.timing, || async {
            let fields = [
                ("client_id", CLIENT_ID),
                ("device_code", session.device_code.as_str()),
                ("grant_type", DEVICE_GRANT),
            ];
            let (_, body) = self.post_form(&endpoints.access_token_url, &fields).await?;
            Ok(poll_step(&body))
        })
        .await?;
        let mut tokens = self.exchange_copilot_token(&github_token, session.enterprise_domain.as_deref()).await?;
        let availability = self.fetch_availability(&tokens.access, session.enterprise_domain.as_deref()).await;
        if let Some(availability) = availability {
            let enabled = self.enable_policies(&tokens.access, session.enterprise_domain.as_deref(), &availability.to_enable).await;
            tokens = tokens.with_extra("availableModelIds", json!(unique(availability.available, enabled)));
        }
        Ok(tokens)
    }

    /// Re-exchanges the stored GitHub token for a fresh Copilot token and
    /// refreshes the model list (kept as is when the lookup fails).
    pub async fn refresh(&self, entry: &Map<String, Value>) -> Result<OAuthTokens> {
        let github_token = entry
            .get("refresh")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| anyhow!("stored OAuth credential has no refresh token"))?;
        let domain = entry.get("enterpriseUrl").and_then(Value::as_str).and_then(normalize_domain);
        let mut tokens = self.exchange_copilot_token(github_token, domain.as_deref()).await?;
        if let Some(availability) = self.fetch_availability(&tokens.access, domain.as_deref()).await {
            tokens = tokens.with_extra("availableModelIds", json!(availability.available));
        }
        Ok(tokens)
    }

    async fn exchange_copilot_token(&self, github_token: &str, domain: Option<&str>) -> Result<OAuthTokens> {
        let url = self.endpoints_for(domain).copilot_token_url;
        let mut request = self.http.get(&url).header("Accept", "application/json").bearer_auth(github_token).timeout(REQUEST_TIMEOUT);
        for (name, value) in COPILOT_HEADERS {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|err| anyhow!("GitHub Copilot token request failed: {}", err.without_url()))?;
        let status = response.status();
        if !status.is_success() {
            bail!("GitHub Copilot token request failed (HTTP {})", status.as_u16());
        }
        let body: Value = response.json().await.map_err(|_| anyhow!("Invalid Copilot token response fields"))?;
        let token = body.get("token").and_then(Value::as_str).filter(|token| !token.is_empty());
        let expires_at = body.get("expires_at").and_then(Value::as_f64);
        let (Some(token), Some(expires_at)) = (token, expires_at) else {
            bail!("Invalid Copilot token response fields");
        };
        let expires_ms = (expires_at * 1000.0) as i64 - EXPIRY_MARGIN_MS;
        let mut tokens = OAuthTokens::new(token.to_string(), github_token.to_string(), expires_ms);
        if let Some(domain) = domain {
            tokens = tokens.with_extra("enterpriseUrl", json!(domain));
        }
        Ok(tokens)
    }

    fn models_base(&self, copilot_token: &str, domain: Option<&str>) -> String {
        self.endpoints_for(domain)
            .models_base_url
            .unwrap_or_else(|| base_url_from_token(copilot_token, domain))
    }

    /// `GET <base>/models`; `None` when the lookup fails (best effort).
    async fn fetch_availability(&self, copilot_token: &str, domain: Option<&str>) -> Option<ModelAvailability> {
        let base = self.models_base(copilot_token, domain);
        let mut request = self
            .http
            .get(format!("{base}/models"))
            .header("Accept", "application/json")
            .header("X-GitHub-Api-Version", COPILOT_API_VERSION)
            .bearer_auth(copilot_token)
            .timeout(MODELS_TIMEOUT);
        for (name, value) in COPILOT_HEADERS {
            request = request.header(name, value);
        }
        let response = request.send().await.ok().filter(|response| response.status().is_success())?;
        let body: Value = response.json().await.ok()?;
        Some(parse_models(&body, base == INDIVIDUAL_BASE_URL))
    }

    /// Best-effort: enables unconfigured models; stops at the first rate limit
    /// or failure. Returns the ids that were enabled.
    async fn enable_policies(&self, copilot_token: &str, domain: Option<&str>, model_ids: &[String]) -> Vec<String> {
        let base = self.models_base(copilot_token, domain);
        let mut enabled = Vec::new();
        for id in model_ids {
            let mut request = self
                .http
                .post(format!("{base}/models/{id}/policy"))
                .bearer_auth(copilot_token)
                .header("openai-intent", "chat-policy")
                .header("x-interaction-type", "chat-policy")
                .timeout(MODELS_TIMEOUT)
                .json(&json!({ "state": "enabled" }));
            for (name, value) in COPILOT_HEADERS {
                request = request.header(name, value);
            }
            match request.send().await {
                Ok(response) if response.status().as_u16() == 429 => break,
                Ok(response) if response.status().is_success() => enabled.push(id.clone()),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        enabled
    }

    /// Form POST with JSON answer; returns `(is_success, body)`.
    async fn post_form(&self, url: &str, fields: &[(&str, &str)]) -> Result<(bool, Value)> {
        let response = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .form(fields)
            .send()
            .await
            .map_err(|err| anyhow!("GitHub OAuth request failed: {}", err.without_url()))?;
        let ok = response.status().is_success();
        let body: Value = response.json().await.map_err(|_| anyhow!("Invalid device code response"))?;
        Ok((ok, body))
    }
}

fn parse_device_code(body: &Value, enterprise_domain: Option<String>) -> Result<DeviceCodeSession> {
    let text = |field: &str| body.get(field).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_string);
    let (Some(device_code), Some(user_code), Some(uri)) = (text("device_code"), text("user_code"), text("verification_uri")) else {
        bail!("Invalid device code response fields");
    };
    let verification_uri = trusted_http_url(&uri).ok_or_else(|| anyhow!("Untrusted verification_uri in device code response"))?;
    let expires_in_secs = body.get("expires_in").and_then(Value::as_f64).ok_or_else(|| anyhow!("Invalid device code response fields"))?;
    Ok(DeviceCodeSession {
        user_code,
        verification_uri,
        interval_secs: body.get("interval").and_then(Value::as_f64),
        expires_in_secs,
        device_code,
        enterprise_domain,
    })
}

fn poll_step(body: &Value) -> PollStep<String> {
    if let Some(token) = body.get("access_token").and_then(Value::as_str).filter(|token| !token.is_empty()) {
        return PollStep::Complete(token.to_string());
    }
    match body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => PollStep::Pending,
        Some("slow_down") => PollStep::SlowDown { interval_secs: body.get("interval").and_then(Value::as_f64) },
        Some(error) => {
            let description = body.get("error_description").and_then(Value::as_str).map(|text| format!(": {text}")).unwrap_or_default();
            PollStep::Failed(format!("Device flow failed: {error}{description}"))
        }
        None => PollStep::Failed("Invalid device token response".to_string()),
    }
}

fn parse_models(body: &Value, allow_policy_fallback: bool) -> ModelAvailability {
    struct Entry<'a> {
        id: &'a str,
        picker_enabled: bool,
        policy_state: &'a str,
    }
    let listed: &[Value] = body.get("data").and_then(Value::as_array).map_or(&[], Vec::as_slice);
    let entries: Vec<Entry> = listed
        .iter()
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?;
            if model.pointer("/capabilities/supports/tool_calls") == Some(&Value::Bool(false)) {
                return None;
            }
            Some(Entry {
                id,
                picker_enabled: model.get("model_picker_enabled") == Some(&Value::Bool(true)),
                policy_state: model.pointer("/policy/state").and_then(Value::as_str).unwrap_or_default(),
            })
        })
        .collect();
    let mut available: Vec<String> = entries
        .iter()
        .filter(|entry| entry.picker_enabled && entry.policy_state != "disabled")
        .map(|entry| entry.id.to_string())
        .collect();
    let used_fallback = available.is_empty() && allow_policy_fallback;
    if used_fallback {
        available = entries.iter().filter(|entry| entry.policy_state == "enabled").map(|entry| entry.id.to_string()).collect();
    }
    let to_enable = entries
        .iter()
        .filter(|entry| entry.policy_state == "unconfigured")
        .filter(|entry| COPILOT_MODELS.iter().any(|model| model.id == entry.id))
        .filter(|entry| entry.picker_enabled || used_fallback)
        .map(|entry| entry.id.to_string())
        .collect();
    ModelAvailability { available, to_enable }
}

fn unique(first: Vec<String>, second: Vec<String>) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in first.into_iter().chain(second) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use axum::{Form, Json, Router};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    const GITHUB_TOKEN: &str = "gho_github_secret";

    #[derive(Default)]
    struct Seen {
        access_polls: AtomicUsize,
        token_headers: Mutex<Vec<HeaderMap>>,
        policy_calls: Mutex<Vec<String>>,
        forms: Mutex<Vec<HashMap<String, String>>>,
        copilot_expires_at: Mutex<i64>,
    }

    async fn start_mock(models: Value) -> (CopilotOAuth, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        *seen.copilot_expires_at.lock().unwrap() = 4_000_000_000;
        let models = Arc::new(models);
        let app = Router::new()
            .route(
                "/login/device/code",
                post(|State(seen): State<Arc<Seen>>, Form(form): Form<HashMap<String, String>>| async move {
                    seen.forms.lock().unwrap().push(form);
                    Json(json!({
                        "device_code": "dev-secret", "user_code": "WXYZ-1234",
                        "verification_uri": "https://github.com/login/device", "interval": 0.01, "expires_in": 5
                    }))
                }),
            )
            .route(
                "/login/oauth/access_token",
                post(|State(seen): State<Arc<Seen>>, Form(form): Form<HashMap<String, String>>| async move {
                    seen.forms.lock().unwrap().push(form);
                    if seen.access_polls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Json(json!({ "error": "authorization_pending" }))
                    } else {
                        Json(json!({ "access_token": GITHUB_TOKEN }))
                    }
                }),
            )
            .route(
                "/copilot_internal/v2/token",
                get(|State(seen): State<Arc<Seen>>, headers: HeaderMap| async move {
                    seen.token_headers.lock().unwrap().push(headers);
                    let expires_at = *seen.copilot_expires_at.lock().unwrap();
                    Json(json!({ "token": "tid=1;proxy-ep=proxy.individual.githubcopilot.com;exp=1", "expires_at": expires_at }))
                }),
            )
            .route("/models", get({
                let models = models.clone();
                move || async move { Json((*models).clone()) }
            }))
            .route(
                "/models/:id/policy",
                post(|State(seen): State<Arc<Seen>>, axum::extract::Path(id): axum::extract::Path<String>| async move {
                    seen.policy_calls.lock().unwrap().push(id);
                    Json(json!({}))
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let endpoints = CopilotEndpoints {
            device_code_url: format!("{base}/login/device/code"),
            access_token_url: format!("{base}/login/oauth/access_token"),
            copilot_token_url: format!("{base}/copilot_internal/v2/token"),
            models_base_url: Some(base),
        };
        let timing = PollTiming {
            default_interval: Duration::from_millis(10),
            min_interval: Duration::from_millis(5),
            slow_down_step: Duration::from_millis(10),
        };
        (CopilotOAuth::with_endpoints(endpoints, timing), seen)
    }

    fn model_list() -> Value {
        json!({ "data": [
            { "id": "claude-sonnet-5", "model_picker_enabled": true, "policy": { "state": "enabled" } },
            { "id": "gpt-6-sol", "model_picker_enabled": true, "policy": { "state": "unconfigured" } },
            { "id": "grok-4.7", "model_picker_enabled": true, "policy": { "state": "disabled" } },
            { "id": "no-tools", "model_picker_enabled": true, "capabilities": { "supports": { "tool_calls": false } } },
            { "id": "not-in-catalog", "model_picker_enabled": true, "policy": { "state": "unconfigured" } }
        ] })
    }

    #[tokio::test]
    async fn login_stores_the_spec_credential_shape() {
        let (copilot, seen) = start_mock(model_list()).await;
        let session = copilot.start(None).await.unwrap();
        assert_eq!(session.user_code, "WXYZ-1234");
        assert!(!format!("{session:?}").contains("dev-secret"));
        let tokens = copilot.poll(&session).await.unwrap();

        let credential = tokens.to_credential();
        assert_eq!(credential["type"], "oauth");
        assert_eq!(credential["access"], "tid=1;proxy-ep=proxy.individual.githubcopilot.com;exp=1");
        assert_eq!(credential["refresh"], GITHUB_TOKEN);
        assert_eq!(credential["expires"], 4_000_000_000_i64 * 1000 - 300_000);
        assert!(credential.get("enterpriseUrl").is_none());
        assert_eq!(credential["availableModelIds"], json!(["claude-sonnet-5", "gpt-6-sol", "not-in-catalog"]));

        let forms = seen.forms.lock().unwrap();
        assert_eq!(forms[0]["client_id"], CLIENT_ID);
        assert_eq!(forms[0]["scope"], "read:user");
        assert_eq!(forms[1]["grant_type"], DEVICE_GRANT);
        assert_eq!(forms[1]["device_code"], "dev-secret");
        let headers = seen.token_headers.lock().unwrap();
        assert_eq!(headers[0]["authorization"], format!("Bearer {GITHUB_TOKEN}").as_str());
        assert_eq!(headers[0]["user-agent"], "GitHubCopilotChat/0.35.0");
        assert_eq!(headers[0]["editor-version"], "vscode/1.107.0");
        assert_eq!(headers[0]["editor-plugin-version"], "copilot-chat/0.35.0");
        assert_eq!(headers[0]["copilot-integration-id"], "vscode-chat");
        assert_eq!(*seen.policy_calls.lock().unwrap(), vec!["gpt-6-sol".to_string()]);
    }

    #[tokio::test]
    async fn refresh_re_exchanges_with_the_github_token_and_keeps_enterprise_url() {
        let (copilot, seen) = start_mock(model_list()).await;
        *seen.copilot_expires_at.lock().unwrap() = 4_100_000_000;
        let entry = json!({
            "type": "oauth", "access": "old", "refresh": GITHUB_TOKEN, "expires": 1, "enterpriseUrl": "company.ghe.com"
        });
        let tokens = copilot.refresh(entry.as_object().unwrap()).await.unwrap();
        let credential = tokens.to_credential();
        assert_eq!(credential["refresh"], GITHUB_TOKEN);
        assert_eq!(credential["expires"], 4_100_000_000_i64 * 1000 - 300_000);
        assert_eq!(credential["enterpriseUrl"], "company.ghe.com");
        assert_eq!(credential["availableModelIds"], json!(["claude-sonnet-5", "gpt-6-sol", "not-in-catalog"]));
        assert!(seen.policy_calls.lock().unwrap().is_empty(), "refresh never changes policies");
        assert_eq!(seen.token_headers.lock().unwrap()[0]["authorization"], format!("Bearer {GITHUB_TOKEN}").as_str());
    }

    #[tokio::test]
    async fn start_validates_the_enterprise_domain() {
        let (copilot, _) = start_mock(json!({})).await;
        let err = copilot.start(Some("http://")).await.unwrap_err().to_string();
        assert_eq!(err, "Invalid GitHub Enterprise URL/domain");
        assert_eq!(normalize_domain("https://Company.GHE.com/path").as_deref(), Some("company.ghe.com"));
        assert_eq!(normalize_domain("company.ghe.com").as_deref(), Some("company.ghe.com"));
        assert_eq!(normalize_domain("  "), None);
    }

    #[test]
    fn device_response_must_carry_a_trusted_verification_uri() {
        let body = json!({ "device_code": "d", "user_code": "u", "verification_uri": "javascript:alert(1)", "expires_in": 9 });
        assert!(parse_device_code(&body, None).unwrap_err().to_string().contains("Untrusted"));
        let body = json!({ "device_code": "d", "user_code": "u", "verification_uri": "https://github.com/login/device" });
        assert!(parse_device_code(&body, None).is_err(), "expires_in is required");
    }

    #[test]
    fn poll_steps_follow_the_device_flow() {
        assert!(matches!(poll_step(&json!({ "error": "authorization_pending" })), PollStep::Pending));
        assert!(matches!(poll_step(&json!({ "error": "slow_down", "interval": 9 })), PollStep::SlowDown { interval_secs: Some(_) }));
        match poll_step(&json!({ "error": "access_denied", "error_description": "no" })) {
            PollStep::Failed(message) => assert_eq!(message, "Device flow failed: access_denied: no"),
            _ => panic!("expected failure"),
        }
        assert!(matches!(poll_step(&json!({})), PollStep::Failed(_)));
    }

    #[test]
    fn base_url_comes_from_proxy_ep() {
        assert_eq!(
            base_url_from_token("tid=1;proxy-ep=proxy.business.githubcopilot.com;exp=9", None),
            "https://api.business.githubcopilot.com"
        );
        assert_eq!(base_url_from_token("tid=1;exp=9", Some("co.ghe.com")), "https://copilot-api.co.ghe.com");
        assert_eq!(base_url_from_token("tid=1;exp=9", None), "https://api.individual.githubcopilot.com");
    }

    #[test]
    fn dynamic_headers_mark_initiator_and_vision() {
        assert_eq!(dynamic_headers(true, false), [("X-Initiator", "user"), ("Openai-Intent", "conversation-edits")]);
        let agent = dynamic_headers(false, true);
        assert!(agent.contains(&("X-Initiator", "agent")) && agent.contains(&("Copilot-Vision-Request", "true")));
    }

    #[test]
    fn model_policy_fallback_only_applies_to_individual_accounts() {
        let body = json!({ "data": [
            { "id": "gpt-6-sol", "policy": { "state": "enabled" } },
            { "id": "grok-4.7", "policy": { "state": "unconfigured" } }
        ] });
        let individual = parse_models(&body, true);
        assert_eq!(individual.available, ["gpt-6-sol"]);
        assert_eq!(individual.to_enable, ["grok-4.7"]);
        let other = parse_models(&body, false);
        assert!(other.available.is_empty() && other.to_enable.is_empty());
    }
}
