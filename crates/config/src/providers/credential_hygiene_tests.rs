use super::*;
use super::auth_store::{key_preview_for, literal_ui_key, run_credential_command};

#[test]
fn key_preview_shows_only_four_characters_of_long_keys() {
    assert_eq!(key_preview_for("sk-ant-api03-abcdefghijklmnopabcd"), "…abcd");
    assert_eq!(key_preview_for("short-key"), "***");
    // Multi-byte characters never panic the slicing.
    assert_eq!(key_preview_for("ééééééééééééééééééééé"), "…éééé");
}

#[tokio::test]
async fn credential_command_is_killed_after_the_timeout() {
    let command = if cfg!(windows) { "powershell -NoProfile -Command Start-Sleep 30" } else { "sleep 30" };
    let started = std::time::Instant::now();
    let err = run_credential_command(command, std::time::Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn ui_keys_are_literal_and_stored_keys_stay_on_their_endpoint() {
    assert_eq!(literal_ui_key(Some(" !rm -rf ")), Some("!rm -rf".to_string()));
    assert_eq!(literal_ui_key(Some(PRESERVE_API_KEY)), None);
    assert_eq!(literal_ui_key(Some("  ")), None);
    let dir = tempfile::tempdir().unwrap();
    let manager = PiProviderManager::with_dirs(dir.path().join("agent"), dir.path().join("support"));
    let own = manager.resolve_base_url("openai");
    assert!(manager.is_configured_base_url("openai", &format!("{own}/")));
    assert!(!manager.is_configured_base_url("openai", "https://evil.example.com/v1"));
}

#[test]
fn debug_output_of_credential_types_hides_keys() {
    let credential = AuthCredential::api_key("sk-super-secret-key");
    let input = TestProviderInput { api_key: Some("sk-super-secret-key".into()), ..Default::default() };
    let text = format!("{credential:?} {input:?}");
    assert!(!text.contains("super-secret"), "{text}");
}

#[tokio::test]
async fn credential_command_keeps_embedded_quotes() {
    let out = run_credential_command("echo \"quoted value\"", std::time::Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.contains("quoted value"), "{out}");
}

#[test]
fn device_id_and_auth_writes_leave_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("agent");
    let manager = PiProviderManager::with_dirs(agent.clone(), dir.path().join("support"));
    manager.device_id().unwrap();
    PiProviderManager::write_json(&agent.join("auth.json"), &json!({"a": 1})).unwrap();
    let names: Vec<String> = std::fs::read_dir(&agent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
}
