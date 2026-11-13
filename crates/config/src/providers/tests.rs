use super::*;
use super::model_catalog::{provider_type, BUILTINS};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;

/// QA-055: credentials written through the manager are owner-only.
#[cfg(unix)]
#[test]
fn auth_json_is_written_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (agent, support) = temp_dirs("mode");
    let manager = PiProviderManager::with_dirs(agent.clone(), support);
    manager
        .set_pi_auth(&SetPiAuthInput {
            provider_id: "acme".into(),
            action: "set".into(),
            api_key: Some("sk-owner-only-1234".into()),
        })
        .unwrap();
    let mode = std::fs::metadata(agent.join("auth.json")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

fn temp_dirs(label: &str) -> (PathBuf, PathBuf) {
    let unique = uuid::Uuid::new_v4().to_string();
    let agent = std::env::temp_dir().join(format!("locagens-pi-{label}-{unique}-agent"));
    let support = std::env::temp_dir().join(format!("locagens-pi-{label}-{unique}-support"));
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&support).unwrap();
    (agent, support)
}

/// Serializes tests that touch `LOCAGENS_TEST_KEY_VAR` (env vars are
/// process-global, so parallel `set_var`/`remove_var` would race).
/// A tokio mutex, so async tests can hold it across awaits.
static ENV_VAR_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn auth_json_roundtrip_and_key_resolution() {
    let _guard = ENV_VAR_TEST_LOCK.lock().await;
    let (agent, support) = temp_dirs("auth");
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

    // No credential, no env → None.
    std::env::remove_var("LOCAGENS_TEST_KEY_VAR");
    assert!(manager.resolve_api_key("acme", None).await.is_none());

    // Set credential and read it back masked.
    manager
        .set_pi_auth(&SetPiAuthInput {
            provider_id: "acme".into(),
            action: "set".into(),
            api_key: Some("sk-abcdef123456".into()),
        })
        .unwrap();
    let status = manager.pi_auth_status();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0]["providerId"], "acme");
    assert_eq!(status[0]["keyPreview"], "***");
    assert_eq!(
        manager.resolve_api_key("acme", None).await.as_deref(),
        Some("sk-abcdef123456")
    );

    // Delete credential.
    manager
        .set_pi_auth(&SetPiAuthInput {
            provider_id: "acme".into(),
            action: "delete".into(),
            api_key: None,
        })
        .unwrap();
    assert!(!manager.has_api_key("acme"));
    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[test]
fn oauth_source_matches_auth_merge_precedence() {
    let (primary, support) = temp_dirs("oauth-src");
    let (first, second) = temp_dirs("oauth-src-fallbacks");
    for (dir, access) in [(&first, "from-first"), (&second, "from-second")] {
        let entry = json!({ "acme": { "type": "oauth", "access": access, "refresh": "r", "expires": 0 } });
        std::fs::write(dir.join("auth.json"), entry.to_string()).unwrap();
    }
    let manager = PiProviderManager::with_chain(
        primary.clone(),
        support.clone(),
        vec![first.clone(), second.clone()],
    );

    // The merged credential and the file a refresh would rewrite must be the same one.
    assert_eq!(manager.read_auth_map()["acme"]["access"], "from-first");
    assert_eq!(manager.oauth_source_path("acme"), Some(first.join("auth.json")));

    // The primary file still wins over every fallback.
    let primary_entry = json!({ "acme": { "type": "oauth", "access": "from-primary", "refresh": "r", "expires": 0 } });
    std::fs::write(primary.join("auth.json"), primary_entry.to_string()).unwrap();
    assert_eq!(manager.read_auth_map()["acme"]["access"], "from-primary");
    assert_eq!(manager.oauth_source_path("acme"), Some(primary.join("auth.json")));

    for dir in [primary, support, first, second] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tokio::test]
async fn env_var_credential_resolution() {
    let _guard = ENV_VAR_TEST_LOCK.lock().await;
    let (agent, support) = temp_dirs("env");
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
    std::env::set_var("LOCAGENS_TEST_KEY_VAR", "from-env");
    assert_eq!(
        manager
            .resolve_api_key("acme", Some("$LOCAGENS_TEST_KEY_VAR"))
            .await
            .as_deref(),
        Some("from-env")
    );
    std::env::remove_var("LOCAGENS_TEST_KEY_VAR");
    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[tokio::test]
async fn codex_chatgpt_tokens_are_ignored_but_openai_api_key_works() {
    let (agent, support) = temp_dirs("codex-tokens");
    let codex = std::env::temp_dir().join(format!(
        "locagens-pi-codex-tokens-fallback-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&codex).unwrap();
    let manager = PiProviderManager::with_chain(
        agent.clone(),
        support.clone(),
        vec![codex.clone()],
    );

    // ChatGPT-login `tokens` alone do NOT configure openai.
    let value = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": { "access_token": "ey-oauth-token", "refresh_token": "r", "account_id": "acc" }
    });
    PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
    assert!(manager.resolve_api_key("openai", None).await.is_none());
    assert!(!manager.has_api_key("openai"));
    assert!(manager.list_providers().await.is_empty());

    // A pi provider next to a ChatGPT-mode codex file lists only the pi one.
    write_auth(&agent, json!({ "opencode-go": { "type": "api_key", "key": "k-12345678" } }));
    let listed: Vec<String> = manager.list_providers().await.into_iter().map(|p| p.id).collect();
    assert_eq!(listed, vec!["opencode-go".to_string()]);

    // A non-empty OPENAI_API_KEY still maps to openai.
    let value = json!({
        "OPENAI_API_KEY": "sk-explicit-key",
        "tokens": { "access_token": "ey-oauth-token" }
    });
    PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
    assert_eq!(
        manager.resolve_api_key("openai", None).await.as_deref(),
        Some("sk-explicit-key")
    );

    // Null key and empty tokens configure nothing.
    let value = json!({ "OPENAI_API_KEY": null, "tokens": { "access_token": "" } });
    PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
    assert!(manager.read_auth_map().get("openai").is_none());

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
    let _ = std::fs::remove_dir_all(codex);
}

#[test]
fn builtin_catalog_and_configs_mask_keys() {
    let (agent, support) = temp_dirs("catalog");
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

    let mut entries = HashMap::new();
    entries.insert(
        "anthropic".to_string(),
        SaveProviderConfigInput {
            provider_type: Some("anthropic".into()),
            api_key: Some("sk-ant-live-key".into()),
            display_name: Some("Anthropic (work)".into()),
            model_settings: Some(json!({ "claude-sonnet-4-5": { "contextLimit": 200000 } })),
            ..Default::default()
        },
    );
    manager.save_configs(&entries).unwrap();

    let configs = manager.get_configs();
    let anthropic = configs.get("anthropic").unwrap();
    assert_eq!(anthropic.kind, "anthropic");
    assert_eq!(anthropic.api_key, PRESERVE_API_KEY);
    assert!(anthropic.has_api_key);
    assert_eq!(anthropic.display_name.as_deref(), Some("Anthropic (work)"));
    assert_eq!(anthropic.base_url, "https://api.anthropic.com/v1");
    assert!(anthropic.models.contains(&"claude-sonnet-4-5".to_string()));

    let providers = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(manager.list_providers());
    let anthropic_meta = providers.iter().find(|p| p.id == "anthropic").unwrap();
    assert_eq!(anthropic_meta.display_name, "Anthropic (work)");
    assert_eq!(anthropic_meta.kind, "anthropic");
    assert!(anthropic_meta.models.contains(&"claude-sonnet-4-5".to_string()));
    assert!(anthropic_meta.models.iter().all(|model| !model.is_empty()));
    assert_eq!(
        anthropic_meta.model_settings.as_ref().unwrap()["claude-sonnet-4-5"]["contextLimit"],
        200000
    );

    // No API keys are ever included in the provider metadata.
    let raw = serde_json::to_string(&providers).unwrap();
    assert!(!raw.contains("sk-"));
    assert!(!raw.contains("apiKey"));

    // Saved auth.json is a real credential file, not exposing metadata.
    let auth = PiProviderManager::read_json(&agent.join("auth.json"));
    assert_eq!(auth["anthropic"]["key"], "sk-ant-live-key");
    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[test]
fn provider_metadata_contract_shape() {
    let (agent, support) = temp_dirs("meta");
    std::fs::write(
        agent.join("models-store.json"),
        json!({
            "openai": { "models": [
                { "id": "gpt-x", "name": "GPT X", "contextWindow": 128000, "cost": { "input": 1.5, "output": 6.0, "cacheRead": 0.15 } }
            ]}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        agent.join("models.json"),
        json!({
            "providers": {
                "acme": { "baseUrl": "https://acme.example/v1", "api": "anthropic-messages", "models": ["fast-model"] }
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        support.join("locagens-providers-meta.json"),
        json!({
            "providers": {
                "anthropic": { "displayName": "Anthropic", "modelSettings": { "claude-opus-4-5": { "contextLimit": 200000 } } }
            }
        })
        .to_string(),
    )
    .unwrap();
    write_auth(&agent, json!({
        "openai": { "type": "api_key", "key": "sk-openai-12345678" },
        "anthropic": { "type": "api_key", "key": "sk-ant-12345678" },
    }));
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

    let providers = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(manager.list_providers());

    // models-store details derive contextLimit / pricing settings.
    let openai = providers.iter().find(|p| p.id == "openai").unwrap();
    assert_eq!(openai.kind, "openai-compatible");
    assert!(openai.models.contains(&"gpt-x".to_string()));
    let settings = openai.model_settings.as_ref().unwrap();
    assert_eq!(settings["gpt-x"]["contextLimit"], 128000);
    assert_eq!(settings["gpt-x"]["pricing"]["tiers"][0]["inputRate"], 1.5);
    assert_eq!(settings["gpt-x"]["pricing"]["tiers"][0]["outputRate"], 6.0);
    assert_eq!(settings["gpt-x"]["pricing"]["tiers"][0]["cacheReadRate"], 0.15);

    // UI-entered meta settings win, and `type` is "anthropic".
    let anthropic = providers.iter().find(|p| p.id == "anthropic").unwrap();
    assert_eq!(anthropic.kind, "anthropic");
    assert_eq!(anthropic.display_name, "Anthropic");
    assert_eq!(
        anthropic.model_settings.as_ref().unwrap()["claude-opus-4-5"]["contextLimit"],
        200000
    );

    // `anthropic-messages` api maps to the "anthropic" client type.
    let acme = providers.iter().find(|p| p.id == "acme").unwrap();
    assert_eq!(acme.kind, "anthropic");
    assert_eq!(acme.models, vec!["fast-model".to_string()]);

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[test]
fn config_map_save_deletes_missing_providers() {
    let (agent, support) = temp_dirs("configs");
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

    let mut entries = HashMap::new();
    entries.insert(
        "anthropic".to_string(),
        SaveProviderConfigInput {
            api_key: Some("sk-ant".into()),
            ..Default::default()
        },
    );
    entries.insert(
        "acme".to_string(),
        SaveProviderConfigInput {
            provider_type: Some("openai-compatible".into()),
            base_url: Some("https://acme.example/v1".into()),
            api_key: Some("sk-acme".into()),
            models: Some(vec!["m1".into(), "m2".into()]),
            display_name: Some("Acme".into()),
            model_settings: Some(json!({ "m1": { "contextLimit": 4096 } })),
        },
    );
    entries.insert(
        "grox".to_string(),
        SaveProviderConfigInput {
            provider_type: Some("openai-compatible".into()),
            base_url: Some("https://grox.example/v1".into()),
            api_key: Some("sk-grox".into()),
            display_name: Some("Grox".into()),
            ..Default::default()
        },
    );
    manager.save_configs(&entries).unwrap();

    // Grox exists in the editable config after the first save.
    assert!(manager.get_configs().contains_key("grox"));

    let auth = PiProviderManager::read_json(&agent.join("auth.json"));
    assert_eq!(auth["acme"]["key"], "sk-acme");
    assert_eq!(auth["anthropic"]["key"], "sk-ant");
    let models_doc = PiProviderManager::read_json(&agent.join("models.json"));
    assert_eq!(models_doc["providers"]["acme"]["baseUrl"], "https://acme.example/v1");
    assert_eq!(models_doc["providers"]["acme"]["api"], "openai-compatible");
    let meta = PiProviderManager::read_json(&support.join("locagens-providers-meta.json"));
    assert_eq!(meta["providers"]["acme"]["displayName"], "Acme");
    assert_eq!(meta["providers"]["acme"]["modelSettings"]["m1"]["contextLimit"], 4096);

    // The second map keeps only acme (with the preserve sentinel);
    // anthropic and grox are missing, so they are deleted entirely.
    let mut entries = HashMap::new();
    entries.insert(
        "acme".to_string(),
        SaveProviderConfigInput {
            api_key: Some(PRESERVE_API_KEY.into()),
            ..Default::default()
        },
    );
    manager.save_configs(&entries).unwrap();

    let auth = PiProviderManager::read_json(&agent.join("auth.json"));
    assert_eq!(auth["acme"]["key"], "sk-acme");
    assert!(auth.get("anthropic").is_none());
    assert!(auth.get("grox").is_none());
    let models_doc = PiProviderManager::read_json(&agent.join("models.json"));
    assert!(models_doc["providers"].get("grox").is_none());
    assert!(models_doc["providers"].get("acme").is_some());
    let meta = PiProviderManager::read_json(&support.join("locagens-providers-meta.json"));
    assert!(meta["providers"].get("grox").is_none());
    assert!(meta["providers"].get("acme").is_some());
    assert!(!manager.get_configs().contains_key("grox"));
    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

/// Tiny local HTTP server: serves queued JSON responses for sequential
/// requests and logs each raw request for assertions.
fn spawn_model_server(responses: Vec<Value>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let queue = Arc::new(Mutex::new(responses));
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log_clone = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = stream.read(&mut chunk).unwrap_or(0);
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            log_clone
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buffer).to_string());
            let body = queue
                .lock()
                .unwrap()
                .pop()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "{}".to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), log)
}

#[tokio::test]
async fn fetch_models_openai_compatible_without_provider_id() {
    let (agent, support) = temp_dirs("fetch-openai");
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
    let (base_url, log) = spawn_model_server(vec![json!({
        "data": [ { "id": "gpt-remote-1" }, { "id": "gpt-remote-2" } ]
    })]);

    let models = manager
        .fetch_models(&FetchModelsInput {
            provider_type: Some("openai-compatible".into()),
            base_url: Some(base_url),
            api_key: Some("sk-openai-test".into()),
            provider_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        models,
        vec!["gpt-remote-1".to_string(), "gpt-remote-2".to_string()]
    );

    let requests = log.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request.starts_with("GET /models "), "unexpected request: {request}");
    assert!(request.to_lowercase().contains("authorization: bearer sk-openai-test"));

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[tokio::test]
async fn fetch_models_anthropic_uses_v1_models() {
    let (agent, support) = temp_dirs("fetch-anthropic");
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
    let (base_url, log) = spawn_model_server(vec![json!({
        "data": [ { "id": "claude-remote" } ]
    })]);

    let models = manager
        .fetch_models(&FetchModelsInput {
            provider_type: Some("anthropic".into()),
            base_url: Some(base_url),
            api_key: Some("sk-ant-test".into()),
            provider_id: None,
        })
        .await
        .unwrap();
    assert_eq!(models, vec!["claude-remote".to_string()]);

    let requests = log.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request.starts_with("GET /v1/models "), "unexpected request: {request}");
    assert!(request.to_lowercase().contains("x-api-key: sk-ant-test"));
    assert!(request.to_lowercase().contains("anthropic-version: 2023-06-01"));

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[tokio::test]
async fn fetch_models_with_provider_id_merges_store() {
    let (agent, support) = temp_dirs("fetch-merge");
    std::fs::write(
        agent.join("models-store.json"),
        json!({ "mystore": { "models": [ { "id": "stored-model" } ] } }).to_string(),
    )
    .unwrap();
    let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
    let (base_url, log) = spawn_model_server(vec![json!({
        "data": [ { "id": "stored-model" }, { "id": "remote-model" } ]
    })]);

    let models = manager
        .fetch_models(&FetchModelsInput {
            provider_type: Some("openai-compatible".into()),
            base_url: Some(base_url),
            api_key: None,
            provider_id: Some("mystore".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        models,
        vec!["stored-model".to_string(), "remote-model".to_string()]
    );

    let requests = log.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /models "));

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
}

#[tokio::test]
async fn codex_wire_api_responses_resolves_to_openai_responses() {
    let (agent, support) = temp_dirs("wireapi");
    let codex = std::env::temp_dir().join(format!("locagens-pi-wireapi-codex-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(
        codex.join("config.toml"),
        r#"
[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"
env_key = "CUSTOM_API_KEY"
wire_api = "responses"

[model_providers.chatonly]
name = "ChatOnly"
base_url = "https://chat.example.com/v1"
wire_api = "chat"
"#,
    )
    .unwrap();
    let manager = PiProviderManager::with_chain(agent.clone(), support.clone(), vec![codex.clone()]);

    // `wire_api = "responses"` keeps the Codex Responses WireAPI.
    assert_eq!(manager.resolve_api("custom"), "openai-responses");
    assert_eq!(provider_type(&manager.resolve_api("custom")), "openai-responses");
    // Any other wire_api (or none) stays chat-completions.
    assert_eq!(manager.resolve_api("chatonly"), "openai-compatible");
    assert_eq!(provider_type("anthropic-messages"), "anthropic");
    assert_eq!(provider_type("responses"), "openai-responses");
    assert_eq!(provider_type("openai-compatible"), "openai-compatible");

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
    let _ = std::fs::remove_dir_all(codex);
}

#[test]
fn models_store_thinking_level_map_populates_reasoning_efforts() {
    let (agent, support) = temp_dirs("efforts");
    let fallback =
        std::env::temp_dir().join(format!("locagens-pi-efforts-fallback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&fallback).unwrap();

    // Fallback catalog: `thinkingLevelMap` metadata plus an empty map.
    PiProviderManager::write_json(
        &fallback.join("models-store.json"),
        &json!({
            "acme": { "models": [
                { "id": "gemini-4-argon", "name": "Gemini 4 Argon", "thinkingLevelMap": { "low": "low", "medium": "medium", "high": "high" } },
                { "id": "no-effort-model", "name": "No Effort", "thinkingLevelMap": {} }
            ]}
        }),
    )
    .unwrap();

    // Primary catalog: `thinkingLevelMap` and explicit `reasoningEfforts`.
    PiProviderManager::write_json(
        &agent.join("models-store.json"),
        &json!({
            "acme": { "models": [
                { "id": "gpt-5.6-luna", "name": "GPT 5.6 Luna", "thinkingLevelMap": { "low": "low", "medium": "medium", "high": "high", "xhigh": "xhigh", "max": "max" } },
                { "id": "glm-5.3-flash", "name": "GLM Flash", "reasoningEfforts": ["low", "high", "max"] }
            ]}
        }),
    )
    .unwrap();

    let manager =
        PiProviderManager::with_chain(agent.clone(), support.clone(), vec![fallback.clone()]);
    let models = manager.models_store_models("acme");
    let efforts = |id: &str| {
        models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.reasoning_efforts.clone().unwrap_or_default())
            .unwrap_or_else(|| panic!("model {id} missing"))
    };

    // Read from the fallback models-store.json.
    assert_eq!(
        efforts("gemini-4-argon"),
        vec!["low".to_string(), "medium".to_string(), "high".to_string()]
    );
    assert_eq!(efforts("gpt-5.6-luna"), vec![
        "low".to_string(),
        "medium".to_string(),
        "high".to_string(),
        "xhigh".to_string(),
        "max".to_string()
    ]);
    assert_eq!(
        efforts("glm-5.3-flash"),
        vec!["low".to_string(), "high".to_string(), "max".to_string()]
    );
    // An empty thinkingLevelMap means no effort levels.
    assert!(efforts("no-effort-model").is_empty());

    let _ = std::fs::remove_dir_all(agent);
    let _ = std::fs::remove_dir_all(support);
    let _ = std::fs::remove_dir_all(fallback);
}

// ------------------------------------------------------------------
// OAuth credentials
// ------------------------------------------------------------------

use crate::oauth::device_code::PollTiming;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Mock xAI endpoints. `/token` answers refresh and device-token posts
/// with `token_body`; `hits` counts token requests.
async fn spawn_xai_mock(token_status: u16, token_body: Value) -> (XaiOAuth, Arc<AtomicUsize>) {
    use axum::{routing::post, Json, Router};
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new()
        .route(
            "/device",
            post(|| async {
                Json(json!({
                    "device_code": "dev-secret",
                    "user_code": "WXYZ-0001",
                    "verification_uri": "https://auth.x.ai/device",
                    "interval": 0.01,
                    "expires_in": 5
                }))
            }),
        )
        .route(
            "/token",
            post(move || {
                let counter = counter.clone();
                let body = token_body.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (axum::http::StatusCode::from_u16(token_status).unwrap(), Json(body))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let timing = PollTiming {
        default_interval: Duration::from_millis(10),
        min_interval: Duration::from_millis(5),
        slow_down_step: Duration::from_millis(10),
    };
    (XaiOAuth::with_endpoints(&format!("{base}/device"), &format!("{base}/token"), timing), hits)
}

use std::time::Duration;

fn write_auth(dir: &Path, value: Value) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("auth.json"), serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn read_auth(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("auth.json")).unwrap()).unwrap()
}

fn oauth_entry(access: &str, refresh: &str, expires: i64) -> Value {
    json!({ "type": "oauth", "access": access, "refresh": refresh, "expires": expires, "accountId": "acct-1" })
}

#[tokio::test]
async fn valid_oauth_credential_resolves_without_network() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    write_auth(&agent, json!({ "xai": oauth_entry("acc-live", "ref-live", oauth::now_ms() + 600_000) }));
    let (xai, hits) = spawn_xai_mock(500, json!({})).await;
    let manager = PiProviderManager::with_dirs(agent.clone(), support).with_xai_oauth(xai);
    assert_eq!(manager.resolve_api_key("xai", None).await.as_deref(), Some("acc-live"));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn expired_oauth_credential_refreshes_into_its_source_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support, pi_dir) =
        (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"));
    // Credential lives only in the fallback dir; the refresh must go back there.
    write_auth(&pi_dir, json!({ "xai": oauth_entry("acc-old", "ref-old", 1), "other": { "type": "api_key", "key": "k" } }));
    // Token response omits refresh_token: the previous one is kept.
    let (xai, hits) = spawn_xai_mock(200, json!({ "access_token": "acc-new", "expires_in": 3600 })).await;
    let manager =
        PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir.clone()]).with_xai_oauth(xai);

    assert_eq!(manager.resolve_api_key("xai", None).await.as_deref(), Some("acc-new"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let stored = read_auth(&pi_dir);
    assert_eq!(stored["xai"]["access"], "acc-new");
    assert_eq!(stored["xai"]["refresh"], "ref-old");
    assert_eq!(stored["xai"]["accountId"], "acct-1");
    assert!(stored["xai"]["expires"].as_i64().unwrap() > oauth::now_ms());
    assert_eq!(stored["other"]["key"], "k");
    assert!(!agent.join("auth.json").exists(), "primary must stay untouched");
    assert!(!pi_dir.join("auth.json.lock").exists(), "lock dir must be released");

    // Second resolve uses the refreshed token without another request.
    assert_eq!(manager.resolve_api_key("xai", None).await.as_deref(), Some("acc-new"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_oauth_refresh_resolves_to_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    write_auth(&agent, json!({ "xai": oauth_entry("acc-old", "ref-old", 1) }));
    let (xai, _) = spawn_xai_mock(400, json!({ "error": "invalid_grant" })).await;
    let manager = PiProviderManager::with_dirs(agent.clone(), support).with_xai_oauth(xai);
    assert!(manager.resolve_api_key("xai", None).await.is_none());
    assert_eq!(read_auth(&agent)["xai"]["access"], "acc-old");
    assert!(!agent.join("auth.json.lock").exists());
}

#[tokio::test]
async fn oauth_login_stores_credential_in_primary_only() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support, pi_dir) =
        (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"));
    write_auth(&pi_dir, json!({ "other": { "type": "api_key", "key": "fallback-secret" } }));
    let (xai, _) = spawn_xai_mock(
        200,
        json!({ "access_token": "acc-login", "refresh_token": "ref-login", "expires_in": 3600 }),
    )
    .await;
    let manager =
        PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir.clone()]).with_xai_oauth(xai);

    let session = manager.start_oauth_login("xai", None).await.unwrap();
    match &session {
        LoginSession::DeviceCode(device) => assert_eq!(device.user_code, "WXYZ-0001"),
        LoginSession::AuthUrl(_) => panic!("xai is a device-code login"),
    }
    manager.finish_oauth_login("xai", &session).await.unwrap();

    let primary = read_auth(&agent);
    assert_eq!(primary["xai"]["type"], "oauth");
    assert_eq!(primary["xai"]["access"], "acc-login");
    assert_eq!(primary["xai"]["refresh"], "ref-login");
    assert!(primary.get("other").is_none());
    assert!(manager.start_oauth_login("acme", None).await.is_err());
}

#[test]
fn set_pi_auth_does_not_copy_fallback_credentials_into_primary() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support, pi_dir) =
        (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"));
    write_auth(&pi_dir, json!({ "groq": { "type": "api_key", "key": "fallback-secret" } }));
    let manager = PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir.clone()]);

    manager
        .set_pi_auth(&SetPiAuthInput {
            provider_id: "acme".into(),
            action: "set".into(),
            api_key: Some("sk-acme-12345678".into()),
        })
        .unwrap();
    let primary = read_auth(&agent);
    assert_eq!(primary["acme"]["key"], "sk-acme-12345678");
    assert!(primary.get("groq").is_none(), "fallback credential leaked into primary");
    assert!(!agent.join("auth.json.lock").exists());

    // The fallback credential still resolves, and its file is unchanged.
    assert!(manager.has_api_key("groq"));
    assert_eq!(read_auth(&pi_dir)["groq"]["key"], "fallback-secret");
}

#[test]
fn set_pi_auth_refuses_to_overwrite_a_corrupt_auth_file() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(agent.join("auth.json"), "{ not json").unwrap();
    let manager = PiProviderManager::with_dirs(agent.clone(), tmp.path().join("support"));
    let result = manager.set_pi_auth(&SetPiAuthInput {
        provider_id: "acme".into(),
        action: "set".into(),
        api_key: Some("sk-acme-12345678".into()),
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(agent.join("auth.json")).unwrap(), "{ not json");
}

#[test]
fn auth_status_for_oauth_never_contains_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    let expires = oauth::now_ms() + 600_000;
    write_auth(&agent, json!({
        "xai": oauth_entry("acc-secret-value", "ref-secret-value", expires),
        "gone": oauth_entry("acc-secret-2", "ref-secret-2", 1),
    }));
    let manager = PiProviderManager::with_dirs(agent, tmp.path().join("support"));
    let status = manager.pi_auth_status();
    let raw = serde_json::to_string(&status).unwrap();
    assert!(!raw.contains("secret"), "{raw}");
    let xai = status.iter().find(|s| s["providerId"] == "xai").unwrap();
    assert_eq!(xai["type"], "oauth");
    assert_eq!(xai["keyPreview"], "oauth");
    assert_eq!(xai["expiresAt"], expires);
    assert_eq!(xai["expired"], false);
    let gone = status.iter().find(|s| s["providerId"] == "gone").unwrap();
    assert_eq!(gone["expired"], true);
}

#[test]
fn xai_builtin_uses_responses_api_with_grok_models() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = PiProviderManager::with_dirs(tmp.path().join("agent"), tmp.path().join("support"));
    assert_eq!(manager.resolve_endpoint("xai"), ("https://api.x.ai/v1".to_string(), "openai-responses".to_string()));
    assert!(manager.model_info("xai", "grok-4.7").is_some());
    assert!(manager.model_info("xai", "grok-4.3").is_some());
}

#[tokio::test]
async fn list_providers_only_returns_configured_providers() {
    let _guard = ENV_VAR_TEST_LOCK.lock().await;
    for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY", "GROQ_API_KEY", "XAI_API_KEY"] {
        std::env::remove_var(var);
    }
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    let manager = PiProviderManager::with_dirs(agent.clone(), tmp.path().join("support"));
    let ids = |providers: Vec<ProviderMetadata>| providers.into_iter().map(|p| p.id).collect::<Vec<_>>();

    // Nothing configured: no built-ins.
    assert!(ids(manager.list_providers().await).is_empty());

    // auth.json entry (api key or oauth) lists the built-in, and only it.
    write_auth(&agent, json!({
        "anthropic": { "type": "api_key", "key": "sk-ant-12345678" },
        "xai": oauth_entry("a", "r", 1),
    }));
    let listed = ids(manager.list_providers().await);
    assert!(listed.contains(&"anthropic".to_string()) && listed.contains(&"xai".to_string()));
    assert!(!listed.contains(&"openai".to_string()) && !listed.contains(&"deepseek".to_string()));

    // Only its own env var set.
    std::env::set_var("GROQ_API_KEY", "gsk-test");
    let listed = ids(manager.list_providers().await);
    std::env::remove_var("GROQ_API_KEY");
    assert!(listed.contains(&"groq".to_string()));
    assert!(!listed.contains(&"openai".to_string()));

    // A shared-chain env var of another provider does not list this one.
    std::env::set_var("OPENAI_API_KEY", "sk-test");
    let listed = ids(manager.list_providers().await);
    std::env::remove_var("OPENAI_API_KEY");
    assert!(listed.contains(&"openai".to_string()));
    assert!(!listed.contains(&"deepseek".to_string()));

    // models.json custom provider.
    std::fs::write(
        agent.join("models.json"),
        json!({ "providers": { "my-local": { "baseUrl": "http://localhost:1/v1" } } }).to_string(),
    )
    .unwrap();
    assert!(ids(manager.list_providers().await).contains(&"my-local".to_string()));
}

#[test]
fn login_providers_report_status_without_secrets() {
    let _guard = ENV_VAR_TEST_LOCK.blocking_lock();
    for provider in BUILTINS {
        if !provider.env_var.is_empty() {
            std::env::remove_var(provider.env_var);
        }
    }
    std::env::set_var("GROQ_API_KEY", "gsk-secret-env");
    std::env::set_var("ACME_TEST_KEY", "acme-secret-env");
    let tmp = tempfile::tempdir().unwrap();
    let primary = tmp.path().join("primary");
    let fallback = tmp.path().join("fallback");
    write_auth(&primary, json!({
        "anthropic": { "type": "api_key", "key": "sk-secret-key" },
        "xai": oauth_entry("acc-secret", "ref-secret", oauth::now_ms() + 600_000),
    }));
    std::fs::write(
        primary.join("models.json"),
        json!({ "providers": { "acme": { "baseUrl": "http://localhost:1/v1", "envKey": "ACME_TEST_KEY" } } }).to_string(),
    )
    .unwrap();
    let manager = PiProviderManager::with_chain(primary, tmp.path().join("support"), vec![fallback]);
    let rows = manager.login_providers();
    std::env::remove_var("GROQ_API_KEY");
    std::env::remove_var("ACME_TEST_KEY");

    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        &ids[..11],
        ["opencode-go", "anthropic", "openai", "google", "openrouter", "deepseek", "groq", "xai", "github-copilot", "kimi-coding", "ollama"]
    );
    assert_eq!(ids[11], "acme");
    let row = |id: &str| rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row("anthropic").status, LoginStatus::ApiKey);
    assert_eq!(row("xai").status, LoginStatus::OAuth);
    assert_eq!(row("groq").status, LoginStatus::Env("GROQ_API_KEY".into()));
    assert_eq!(row("acme").status, LoginStatus::Env("ACME_TEST_KEY".into()));
    assert_eq!(row("openai").status, LoginStatus::SignedOut);
    assert!(row("xai").oauth_label.is_some() && row("groq").oauth_label.is_none());
    assert_eq!(row("openai").oauth_flow, Some(oauth::OAuthFlow::AuthUrl));
    assert_eq!(row("github-copilot").oauth_flow, Some(oauth::OAuthFlow::DeviceCode));
    assert_eq!(row("ollama").api_key_env, None);

    let raw = serde_json::to_string(&rows).unwrap();
    assert!(!raw.contains("secret"), "{raw}");
}

// ------------------------------------------------------------------
// ChatGPT / Copilot / OpenRouter / Kimi / OpenCode through the manager
// ------------------------------------------------------------------

use crate::oauth::openai::ChatgptOAuth;
use crate::oauth::openrouter::OpenRouterOAuth;

/// Mock answering `/token` (form) and `/keys` (JSON) with `(status, body)`.
async fn spawn_token_mock(status: u16, body: Value) -> String {
    use axum::{routing::post, Router};
    let answer = move || {
        let body = body.clone();
        async move { (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(body)) }
    };
    let app = Router::new().route("/token", post(answer.clone())).route("/keys", post(answer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn chatgpt_grant() -> Value {
    json!({ "id_token": "idt", "access_token": "acc-gpt", "refresh_token": "ref-gpt", "scope": "openid chatgpt.tokens.use.direct", "expires_in": 3600 })
}

fn chatgpt_clients(base: &str) -> OAuthClients {
    OAuthClients {
        chatgpt: ChatgptOAuth::with_endpoints("https://auth.example.test/authorize", &format!("{base}/token"), 0),
        ..OAuthClients::default()
    }
}

#[tokio::test]
async fn chatgpt_login_over_loopback_unlocks_the_responses_models() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    let base = spawn_token_mock(200, chatgpt_grant()).await;
    let manager = PiProviderManager::with_dirs(agent.clone(), support).with_oauth_clients(chatgpt_clients(&base));
    assert_eq!(manager.resolve_api("openai"), "openai-compatible", "API-key behavior before sign-in");

    let LoginSession::AuthUrl(session) = manager.start_oauth_login("openai", None).await.unwrap() else {
        panic!("ChatGPT sign-in is an authorization-URL login");
    };
    assert!(session.auth_url.contains("agent_name_hint=Locagens"));
    let redirect = format!("{}?code=c1&state={}&client_id=issued", session.redirect_uri, session.state.clone().unwrap());
    let login = LoginSession::AuthUrl(session);
    let finish = manager.finish_oauth_login("openai", &login);
    let browser = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        reqwest::get(redirect).await.unwrap().status()
    };
    let (result, browser_status) = tokio::join!(finish, browser);
    result.unwrap();
    assert_eq!(browser_status, 200);

    let stored = read_auth(&agent);
    assert_eq!(stored["openai"]["type"], "oauth");
    assert_eq!(stored["openai"]["clientId"], "issued");
    assert_eq!(manager.resolve_api_key("openai", None).await.as_deref(), Some("acc-gpt"));
    assert!(manager.pending_logins.lock().unwrap().is_empty(), "receiver released");
    assert_eq!(manager.device_id().unwrap().len(), 36);

    assert_eq!(manager.resolve_api("openai"), "openai-responses");
    let model = manager.model_info("openai", "gpt-5.5").unwrap();
    assert_eq!(model.api.as_deref(), Some("openai-responses"));
    assert_eq!(model.context_window, Some(272_000));
    assert!(manager.model_info("openai", "o4-mini").is_some());
    assert_eq!(manager.merged_models("openai").len(), 44);
}

#[tokio::test]
async fn pasted_redirect_completes_openrouter_login_and_stops_the_receiver() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    let base = spawn_token_mock(200, json!({ "key": "sk-or-secret" })).await;
    let clients = OAuthClients {
        openrouter: OpenRouterOAuth::with_endpoints("https://openrouter.example.test/auth", &format!("{base}/keys")),
        ..OAuthClients::default()
    };
    let manager = PiProviderManager::with_dirs(agent.clone(), support).with_oauth_clients(clients);
    let LoginSession::AuthUrl(session) = manager.start_oauth_login("openrouter", None).await.unwrap() else {
        panic!("OpenRouter sign-in is an authorization-URL login");
    };
    manager.submit_oauth_redirect("openrouter", &session, "paste-code").await.unwrap();

    let stored = read_auth(&agent);
    assert_eq!(stored["openrouter"], json!({ "type": "oauth", "access": "sk-or-secret", "refresh": "", "expires": 9_007_199_254_740_991_i64 }));
    assert_eq!(manager.resolve_api_key("openrouter", None).await.as_deref(), Some("sk-or-secret"));
    assert!(manager.pending_logins.lock().unwrap().is_empty());
    assert!(manager.submit_oauth_redirect("groq", &session, "x").await.is_err());
}

#[tokio::test]
async fn chatgpt_refresh_dispatches_with_the_stored_client_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    let mut entry = oauth_entry("acc-old", "ref-old", 1);
    entry["clientId"] = json!("issued");
    write_auth(&agent, json!({ "openai": entry }));
    let base = spawn_token_mock(200, chatgpt_grant()).await;
    let manager = PiProviderManager::with_dirs(agent.clone(), support).with_oauth_clients(chatgpt_clients(&base));
    assert_eq!(manager.resolve_api_key("openai", None).await.as_deref(), Some("acc-gpt"));
    let stored = read_auth(&agent);
    assert_eq!(stored["openai"]["refresh"], "ref-gpt");
    assert_eq!(stored["openai"]["clientId"], "issued");
    assert_eq!(stored["openai"]["accountId"], "acct-1", "unrelated fields survive a refresh");

    // Without the issued client id the credential cannot refresh.
    write_auth(&agent, json!({ "openai": oauth_entry("acc-old", "ref-old", 1) }));
    assert_eq!(manager.resolve_api_key("openai", None).await, None);
}

#[tokio::test]
async fn openrouter_keys_never_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    let permanent = json!({ "type": "oauth", "access": "sk-or-1", "refresh": "", "expires": 9_007_199_254_740_991_i64 });
    write_auth(&agent, json!({ "openrouter": permanent }));
    let manager = PiProviderManager::with_dirs(agent.clone(), support);
    assert_eq!(manager.resolve_api_key("openrouter", None).await.as_deref(), Some("sk-or-1"));
    assert!(manager.refresh_tokens("openrouter", "", &Map::new()).await.is_err());
}

#[test]
fn device_id_is_created_once_and_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = PiProviderManager::with_dirs(tmp.path().join("agent"), tmp.path().join("support"));
    let first = manager.device_id().unwrap();
    assert_eq!(manager.device_id().unwrap(), first);
    assert!(uuid::Uuid::parse_str(&first).is_ok());
    std::fs::write(tmp.path().join("agent").join("device-id"), "garbage").unwrap();
    assert_ne!(manager.device_id().unwrap(), "garbage");
}

#[tokio::test]
async fn opencode_api_keys_are_the_lowest_precedence_read_only_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support, pi_dir, opencode) =
        (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"), tmp.path().join("opencode"));
    write_auth(&opencode, json!({
        "opencode-go": { "type": "api", "key": "oc-go-key" },
        "huggingface": { "type": "api", "key": "oc-hf-key" },
        "blank": { "type": "api", "key": "  " },
        "mixed-oauth": { "type": "oauth", "access": "oc-acc", "refresh": "oc-ref", "expires": 1 },
        "wellknown-co": { "type": "wellknown", "key": "X", "token": "t" },
    }));
    write_auth(&pi_dir, json!({ "opencode-go": { "type": "api_key", "key": "pi-key" } }));
    write_auth(&agent, json!({ "groq": { "type": "api_key", "key": "primary-key" } }));
    let manager = PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir]).with_opencode_dir(opencode.clone());

    assert_eq!(manager.resolve_api_key("opencode-go", None).await.as_deref(), Some("pi-key"), "pi overrides OpenCode");
    assert_eq!(manager.resolve_api_key("huggingface", None).await.as_deref(), Some("oc-hf-key"));
    assert_eq!(manager.read_auth_map()["huggingface"], json!({ "type": "api_key", "key": "oc-hf-key" }));
    for ignored in ["blank", "mixed-oauth", "wellknown-co"] {
        assert!(!manager.has_api_key(ignored), "{ignored}");
    }
    assert_eq!(manager.resolve_api_key("groq", None).await.as_deref(), Some("primary-key"));

    // Saving a key writes the primary file only; OpenCode's file is untouched.
    let before = std::fs::read_to_string(opencode.join("auth.json")).unwrap();
    manager
        .set_pi_auth(&SetPiAuthInput { provider_id: "huggingface".into(), action: "set".into(), api_key: Some("new-key-12345".into()) })
        .unwrap();
    assert_eq!(manager.resolve_api_key("huggingface", None).await.as_deref(), Some("new-key-12345"));
    assert_eq!(std::fs::read_to_string(opencode.join("auth.json")).unwrap(), before);
    assert!(read_auth(&agent).get("mixed-oauth").is_none());

    // A key for a provider Locagens cannot call (no base URL anywhere) is not
    // offered in the model picker; opencode-go (built-in) and groq are.
    let listed: Vec<String> = manager.list_providers().await.into_iter().map(|p| p.id).collect();
    assert!(!listed.contains(&"huggingface".to_string()), "{listed:?}");
    assert!(listed.contains(&"opencode-go".to_string()) && listed.contains(&"groq".to_string()), "{listed:?}");
    let logins: Vec<String> = manager.login_providers().into_iter().map(|p| p.id).collect();
    assert!(!logins.contains(&"huggingface".to_string()), "{logins:?}");
}

#[tokio::test]
async fn copilot_credential_narrows_models_and_picks_the_base_url() {
    let tmp = tempfile::tempdir().unwrap();
    let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
    let manager = PiProviderManager::with_dirs(agent.clone(), support);
    assert_eq!(manager.merged_models("github-copilot").len(), 27, "everything before sign-in");

    let mut entry = oauth_entry("tid=1;proxy-ep=proxy.business.githubcopilot.com", "gho-ref", oauth::now_ms() + 600_000);
    entry["availableModelIds"] = json!(["claude-sonnet-5", "gpt-6-sol", "gemini-4-argon"]);
    write_auth(&agent, json!({ "github-copilot": entry }));
    let ids: Vec<String> = manager.merged_models("github-copilot").into_iter().map(|model| model.id).collect();
    assert_eq!(ids, ["claude-sonnet-5", "gemini-4-argon", "gpt-6-sol"]);
    assert_eq!(manager.model_info("github-copilot", "claude-sonnet-5").unwrap().api.as_deref(), Some("anthropic-messages"));
    assert_eq!(manager.model_info("github-copilot", "gemini-4-argon").unwrap().api.as_deref(), Some("openai-completions"));
    assert_eq!(manager.model_info("github-copilot", "gpt-6-sol").unwrap().api.as_deref(), Some("openai-responses"));
    assert_eq!(
        manager.copilot_base_url("tid=1;proxy-ep=proxy.business.githubcopilot.com"),
        "https://api.business.githubcopilot.com"
    );

    let mut enterprise = oauth_entry("tid=1", "gho-ref", oauth::now_ms() + 600_000);
    enterprise["enterpriseUrl"] = json!("co.ghe.com");
    write_auth(&agent, json!({ "github-copilot": enterprise }));
    assert_eq!(manager.copilot_base_url("tid=1"), "https://copilot-api.co.ghe.com");
}

#[test]
fn kimi_coding_builtin_serves_the_subscription_models() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = PiProviderManager::with_dirs(tmp.path().join("agent"), tmp.path().join("support"));
    assert_eq!(
        manager.resolve_endpoint("kimi-coding"),
        ("https://api.kimi.com/coding/v1".to_string(), "openai-compatible".to_string())
    );
    let model = manager.model_info("kimi-coding", "kimi-for-coding").unwrap();
    assert_eq!(model.context_window, Some(1_048_576));
    assert_eq!(manager.merged_models("kimi-coding").len(), 6);
}

#[test]
fn auth_status_for_the_new_oauth_providers_never_contains_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    let expires = oauth::now_ms() + 600_000;
    let mut chatgpt = oauth_entry("gpt-access-secret", "gpt-refresh-secret", expires);
    chatgpt["clientId"] = json!("client-secret-id");
    let mut copilot = oauth_entry("tid=copilot-secret", "gho-github-secret", expires);
    copilot["enterpriseUrl"] = json!("co.ghe.com");
    write_auth(&agent, json!({
        "openai": chatgpt,
        "github-copilot": copilot,
        "openrouter": { "type": "oauth", "access": "sk-or-secret", "refresh": "", "expires": 9_007_199_254_740_991_i64 },
        "kimi-coding": oauth_entry("kimi-access-secret", "kimi-refresh-secret", expires),
    }));
    let manager = PiProviderManager::with_dirs(agent, tmp.path().join("support"));
    let raw = serde_json::to_string(&manager.pi_auth_status()).unwrap() + &serde_json::to_string(&manager.login_providers()).unwrap();
    assert!(!raw.contains("secret"), "{raw}");
    assert!(raw.contains("github-copilot") && raw.contains("kimi-coding"));
}
