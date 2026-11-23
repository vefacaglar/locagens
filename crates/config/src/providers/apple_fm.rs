//! Native Apple Foundation Models (`fm`) discovery, URL normalization,
//! and on-demand `fm serve` lifecycle management.

use super::model_refresh::CachedModelMetadata;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub const APPLE_FM_PROVIDER_ID: &str = "apple-fm";
pub const APPLE_FM_DEFAULT_HOST: &str = "127.0.0.1";
pub const APPLE_FM_DEFAULT_PORT: u16 = 1976;
pub const APPLE_FM_DEFAULT_BASE_URL: &str = "http://127.0.0.1:1976/v1";
pub const APPLE_FM_MODEL_LIST_MAX_AGE: Duration = Duration::from_secs(60);

const APPLE_FM_HEALTH_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(800);
const APPLE_FM_HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
const APPLE_FM_MODELS_CMD_TIMEOUT: Duration = Duration::from_secs(5);
const APPLE_FM_SERVER_START_TIMEOUT: Duration = Duration::from_secs(5);
const APPLE_FM_SERVER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_STDERR_CAPTURE_BYTES: usize = 8 * 1024;

static APPLE_FM_HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

static SERVER_START_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Discovered Apple Foundation Models entry with resolved metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiscoveredAppleFmModel {
    pub id: String,
    pub metadata: CachedModelMetadata,
}

/// Normalizes `APPLE_FM_BASE_URL` into an OpenAI-compatible `.../v1` URL,
/// adding `http://` for bare `host:port` and stripping trailing slashes.
pub(super) fn normalize_apple_fm_base_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if raw.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("http://[{raw}]")
    } else if raw.starts_with(':') {
        format!("http://{APPLE_FM_DEFAULT_HOST}{raw}")
    } else if !raw.contains("://") {
        if raw.matches(':').count() > 1 && !raw.starts_with('[') {
            format!("http://[{raw}]")
        } else {
            format!("http://{raw}")
        }
    } else {
        raw.to_string()
    };

    let mut url = reqwest::Url::parse(&candidate).ok()?;
    let trimmed_path = url.path().trim_end_matches('/');
    let next_path = if trimmed_path.is_empty() {
        "/v1".to_string()
    } else if trimmed_path == "/v1" || trimmed_path.ends_with("/v1") {
        trimmed_path.to_string()
    } else {
        format!("{trimmed_path}/v1")
    };
    url.set_path(&next_path);
    Some(url.to_string().trim_end_matches('/').to_string())
}

/// Reads and normalizes `APPLE_FM_BASE_URL` if set and non-empty.
pub(super) fn apple_fm_env_base_url() -> Option<String> {
    std::env::var("APPLE_FM_BASE_URL")
        .ok()
        .and_then(|raw| normalize_apple_fm_base_url(&raw))
}

/// True when `APPLE_FM_BASE_URL` is configured in the environment.
pub(super) fn apple_fm_env_configured() -> bool {
    apple_fm_env_base_url().is_some()
}

/// Locates the `fm` binary on macOS (`/usr/bin/fm` first, then `PATH`).
/// Always returns `None` on non-macOS platforms.
pub(super) fn find_fm_binary() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let system_fm = std::path::Path::new("/usr/bin/fm");
        if system_fm.is_file() {
            return Some(system_fm.to_path_buf());
        }
        let path_env = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join("fm");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// True when `base_url` points at a local host (`localhost`, `*.localhost`,
/// `127.0.0.1`, `::1`, or `0.0.0.0`).
pub(super) fn is_local_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    let Some(raw_host) = url.host_str() else {
        return false;
    };
    let host = raw_host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    if host == "0.0.0.0" {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

/// Strips trailing `/` and `/v1` from `base_url` to obtain the `fm serve` origin.
pub(super) fn apple_fm_origin(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .map(|prefix| prefix.trim_end_matches('/'))
        .unwrap_or(trimmed)
        .to_string()
}

fn friendly_model_name(id: &str, label: Option<&str>) -> String {
    let clean_label = label.map(str::trim).filter(|l| !l.is_empty());
    match id {
        "pcc" => match clean_label {
            None | Some("PCC") | Some("Private Cloud Compute") => {
                "Apple Intelligence (PCC)".to_string()
            }
            Some(other) => format!("Apple Intelligence ({other})"),
        },
        "system" => match clean_label {
            Some(other) => format!("Apple Intelligence ({other})"),
            None => "Apple Intelligence (AFM 3 Core)".to_string(),
        },
        _ => match clean_label {
            Some(other) => format!("Apple Intelligence ({other})"),
            None => id.to_string(),
        },
    }
}

fn make_model(id: String, label: Option<&str>) -> DiscoveredAppleFmModel {
    let name = friendly_model_name(&id, label);
    let (context_window, max_tokens) = if id == "system" {
        (Some(4096), Some(1024))
    } else {
        (None, None)
    };
    DiscoveredAppleFmModel {
        id,
        metadata: CachedModelMetadata {
            name: Some(name),
            context_window,
            max_tokens,
            reasoning: Some(false),
        },
    }
}

/// Parses `GET {origin}/health` JSON (`{ models: [{ name, available }] }`).
pub(super) fn parse_health_models(body: &Value) -> Vec<DiscoveredAppleFmModel> {
    let Some(models) = body.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut discovered = Vec::new();
    for entry in models {
        if entry.get("available").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(id) = entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if seen.insert(id.to_string()) {
            discovered.push(make_model(id.to_string(), None));
        }
    }
    discovered
}

fn parse_openai_models(body: &Value) -> Vec<DiscoveredAppleFmModel> {
    let entries = match body {
        Value::Array(items) => items.as_slice(),
        Value::Object(_) => body
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        _ => &[],
    };
    let mut seen = HashSet::new();
    let mut discovered = Vec::new();
    for entry in entries {
        let id = match entry {
            Value::String(s) => s.trim(),
            _ => entry
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default(),
        };
        if !id.is_empty() && seen.insert(id.to_string()) {
            discovered.push(make_model(id.to_string(), None));
        }
    }
    discovered
}

/// Parses `fm models` stdout lines starting with `✓` (e.g. `✓ system (AFM 3 Core)`).
pub(super) fn parse_fm_models_output(output: &str) -> Vec<DiscoveredAppleFmModel> {
    let mut seen = HashSet::new();
    let mut discovered = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix('✓') else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        let (id_part, label) = match rest.find('(') {
            Some(open_idx) => {
                let id_part = rest[..open_idx].trim();
                let after = &rest[open_idx + 1..];
                let label = after.strip_suffix(')').unwrap_or(after).trim();
                (id_part, (!label.is_empty()).then_some(label))
            }
            None => (rest, None),
        };
        let Some(id) = id_part.split_whitespace().next().filter(|id| !id.is_empty()) else {
            continue;
        };
        if seen.insert(id.to_string()) {
            discovered.push(make_model(id.to_string(), label));
        }
    }
    discovered
}

fn format_fm_error(prefix: &str, details: &str) -> anyhow::Error {
    let clean = details.trim();
    if clean.to_ascii_lowercase().contains("license") {
        anyhow::anyhow!(
            "{prefix}: {clean} (Accept the Apple Foundation Models license by running `sudo fm license` in Terminal.)"
        )
    } else if clean.is_empty() {
        anyhow::anyhow!("{prefix}")
    } else {
        anyhow::anyhow!("{prefix}: {clean}")
    }
}

/// Discovers available Apple Foundation Models without starting `fm serve`.
///
/// 1. Probes `GET {origin}/health` (falling back to `GET {base_url}/models` if
///    `/health` returns 404 on a custom proxy).
/// 2. If unreachable locally and `fm` exists, runs `fm models` (read-only).
/// 3. If neither server nor `fm` is available locally, returns `Ok(Vec::new())`.
pub(super) async fn discover_apple_fm_models(base_url: &str) -> Result<Vec<DiscoveredAppleFmModel>> {
    let base_url = base_url.trim().trim_end_matches('/');
    let origin = apple_fm_origin(base_url);
    let health_url = format!("{origin}/health");

    match APPLE_FM_HTTP
        .get(&health_url)
        .timeout(APPLE_FM_HEALTH_DISCOVERY_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                return Ok(parse_health_models(&body));
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                let models_res = APPLE_FM_HTTP
                    .get(format!("{base_url}/models"))
                    .timeout(APPLE_FM_HEALTH_DISCOVERY_TIMEOUT)
                    .send()
                    .await?;
                let models_status = models_res.status();
                let body: Value = models_res.json().await.unwrap_or(Value::Null);
                if !models_status.is_success() {
                    anyhow::bail!("Apple FM model fetch failed with status {models_status}");
                }
                return Ok(parse_openai_models(&body));
            }
            anyhow::bail!("Apple FM /health failed with status {status}");
        }
        Err(err) => {
            if !is_local_base_url(base_url) {
                return Err(err.into());
            }
        }
    }

    let Some(fm_bin) = find_fm_binary() else {
        return Ok(Vec::new());
    };

    let output = tokio::time::timeout(
        APPLE_FM_MODELS_CMD_TIMEOUT,
        tokio::process::Command::new(&fm_bin)
            .arg("models")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("`fm models` timed out"))??;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed = parse_fm_models_output(&stdout);
    // `fm models` exits non-zero when `pcc` is unavailable in non-Terminal
    // contexts even though `system` is listed with `✓`. Return available models
    // whenever any `✓` line parsed cleanly.
    if !parsed.is_empty() || output.status.success() {
        return Ok(parsed);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = if stderr.trim().is_empty() {
        stdout.as_ref()
    } else {
        stderr.as_ref()
    };
    Err(format_fm_error("`fm models` failed", details))
}

async fn probe_health(health_url: &str) -> bool {
    APPLE_FM_HTTP
        .get(health_url)
        .timeout(APPLE_FM_HEALTH_PROBE_TIMEOUT)
        .send()
        .await
        .is_ok_and(|resp| resp.status() == reqwest::StatusCode::OK)
}

fn extract_host_port(base_url: &str) -> (String, u16) {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return (APPLE_FM_DEFAULT_HOST.to_string(), APPLE_FM_DEFAULT_PORT);
    };
    let host = url
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
        .filter(|h| !h.is_empty())
        .map(|h| {
            if h.eq_ignore_ascii_case("localhost") {
                APPLE_FM_DEFAULT_HOST.to_string()
            } else {
                h.to_string()
            }
        })
        .unwrap_or_else(|| APPLE_FM_DEFAULT_HOST.to_string());
    let port = url.port().unwrap_or(APPLE_FM_DEFAULT_PORT);
    (host, port)
}

/// Ensures `fm serve` is reachable at `base_url`, reusing an already running
/// instance or auto-starting local `/usr/bin/fm serve` on macOS when needed.
pub async fn ensure_apple_fm_server(base_url: &str) -> Result<()> {
    let base_url = base_url.trim().trim_end_matches('/');
    let origin = apple_fm_origin(base_url);
    let health_url = format!("{origin}/health");

    if probe_health(&health_url).await {
        return Ok(());
    }

    if !is_local_base_url(base_url) {
        anyhow::bail!(
            "Apple Foundation Models server (`fm serve`) is not reachable at {base_url} (on macOS 27+, `/usr/bin/fm` is used automatically for local endpoints)."
        );
    }

    let Some(fm_bin) = find_fm_binary() else {
        anyhow::bail!(
            "`fm serve` is not reachable at {base_url} and the `fm` binary was not found (on macOS 27+, `/usr/bin/fm` is used automatically)."
        );
    };

    let _guard = SERVER_START_LOCK.lock().await;
    if probe_health(&health_url).await {
        return Ok(());
    }

    let (host, port) = extract_host_port(base_url);
    let mut child = tokio::process::Command::new(&fm_bin)
        .arg("serve")
        .arg("--host")
        .arg(&host)
        .arg("--port")
        .arg(port.to_string())
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| anyhow::anyhow!("Failed to launch `{}`: {err}", fm_bin.display()))?;

    // Drain stderr continuously so `fm serve` can never block on a full pipe,
    // while keeping the captured tail available if startup fails.
    let stderr_buf = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let stderr_handle = child.stderr.take().map(|mut pipe| {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 1024];
            loop {
                match pipe.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut guard = buf.lock().await;
                        if guard.len() < MAX_STDERR_CAPTURE_BYTES {
                            let remaining = MAX_STDERR_CAPTURE_BYTES - guard.len();
                            guard.extend_from_slice(&chunk[..n.min(remaining)]);
                        }
                    }
                }
            }
        })
    });

    let deadline = tokio::time::Instant::now() + APPLE_FM_SERVER_START_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            if let Some(handle) = stderr_handle {
                let _ = tokio::time::timeout(Duration::from_millis(200), handle).await;
            }
            let raw = stderr_buf.lock().await;
            let stderr = String::from_utf8_lossy(&raw);
            return Err(format_fm_error(
                &format!("`fm serve` exited early with {status}"),
                &stderr,
            ));
        }

        if probe_health(&health_url).await {
            // Reap the child in the background if it ever exits so no zombie accumulates.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(handle) = stderr_handle {
                let _ = tokio::time::timeout(Duration::from_millis(200), handle).await;
            }
            let raw = stderr_buf.lock().await;
            let stderr = String::from_utf8_lossy(&raw);
            return Err(format_fm_error(
                &format!("`fm serve` did not become healthy at {health_url} within 5s"),
                &stderr,
            ));
        }

        tokio::time::sleep(APPLE_FM_SERVER_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_apple_fm_base_urls() {
        assert_eq!(
            normalize_apple_fm_base_url("127.0.0.1:1976").as_deref(),
            Some("http://127.0.0.1:1976/v1")
        );
        assert_eq!(
            normalize_apple_fm_base_url("http://127.0.0.1:1976").as_deref(),
            Some("http://127.0.0.1:1976/v1")
        );
        assert_eq!(
            normalize_apple_fm_base_url("http://127.0.0.1:1976/").as_deref(),
            Some("http://127.0.0.1:1976/v1")
        );
        assert_eq!(
            normalize_apple_fm_base_url("http://127.0.0.1:1976/v1/").as_deref(),
            Some("http://127.0.0.1:1976/v1")
        );
        assert_eq!(
            normalize_apple_fm_base_url("https://proxy.example.com/fm/").as_deref(),
            Some("https://proxy.example.com/fm/v1")
        );
        assert_eq!(
            normalize_apple_fm_base_url(":1976").as_deref(),
            Some("http://127.0.0.1:1976/v1")
        );
        assert_eq!(normalize_apple_fm_base_url("   "), None);
    }

    #[test]
    fn parses_fm_models_output_lines() {
        let output = "\
  Apple Foundation Models
  ✓ system (AFM 3 Core)
  ✗ pcc                  (Private Cloud Compute is not available in this context. Please use the Terminal app.)
";
        let models = parse_fm_models_output(output);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "system");
        assert_eq!(
            models[0].metadata.name.as_deref(),
            Some("Apple Intelligence (AFM 3 Core)")
        );
        assert_eq!(models[0].metadata.context_window, Some(4096));
        assert_eq!(models[0].metadata.max_tokens, Some(1024));
        assert_eq!(models[0].metadata.reasoning, Some(false));

        let both = "\
  ✓ system (AFM 3 Core)
  ✓ pcc (Private Cloud Compute)
";
        let both_models = parse_fm_models_output(both);
        assert_eq!(both_models.len(), 2);
        assert_eq!(both_models[1].id, "pcc");
        assert_eq!(
            both_models[1].metadata.name.as_deref(),
            Some("Apple Intelligence (PCC)")
        );
        assert_eq!(both_models[1].metadata.context_window, None);
        assert_eq!(both_models[1].metadata.max_tokens, None);
    }

    #[test]
    fn parses_health_models_filtering_unavailable() {
        let body = json!({
            "status": "fm serve is running",
            "models": [
                { "name": "system", "available": true },
                { "name": "pcc", "available": false, "reason": "Not in Terminal" }
            ]
        });
        let models = parse_health_models(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "system");
        assert_eq!(
            models[0].metadata.name.as_deref(),
            Some("Apple Intelligence (AFM 3 Core)")
        );
        assert_eq!(models[0].metadata.context_window, Some(4096));
        assert_eq!(models[0].metadata.max_tokens, Some(1024));
        assert_eq!(models[0].metadata.reasoning, Some(false));

        let both = json!({
            "models": [
                { "name": "system", "available": true },
                { "name": "pcc", "available": true }
            ]
        });
        let both_models = parse_health_models(&both);
        assert_eq!(both_models.len(), 2);
        assert_eq!(both_models[1].id, "pcc");
        assert_eq!(
            both_models[1].metadata.name.as_deref(),
            Some("Apple Intelligence (PCC)")
        );
        assert_eq!(both_models[1].metadata.context_window, None);
        assert_eq!(both_models[1].metadata.max_tokens, None);
    }

    #[test]
    fn origin_strips_v1_and_trailing_slashes() {
        assert_eq!(
            apple_fm_origin("http://127.0.0.1:1976/v1"),
            "http://127.0.0.1:1976"
        );
        assert_eq!(
            apple_fm_origin("http://127.0.0.1:1976/v1/"),
            "http://127.0.0.1:1976"
        );
        assert_eq!(
            apple_fm_origin("https://proxy.example/fm/v1"),
            "https://proxy.example/fm"
        );
    }
}
