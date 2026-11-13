use super::*;
use super::model_catalog::{builtin};

/// Maps a fallback-chain `auth.json` entry onto a Locagens provider
/// credential. Codex stores `{ "OPENAI_API_KEY": "sk-…", "tokens": … }` while
/// Locagens/`pi` store `{ "<provider>": { "type": "api_key", "key": … } }`.
fn codex_credential(key: &str, value: &Value) -> Vec<(String, Value)> {
    if key == "OPENAI_API_KEY" {
        if let Some(api_key) = value.as_str().filter(|k| !k.trim().is_empty()) {
            return vec![(
                "openai".to_string(),
                json!({ "type": "api_key", "key": api_key.trim() }),
            )];
        }
        return Vec::new();
    }
    // Codex ChatGPT-login `tokens` are intentionally ignored: that OAuth
    // access token does not work against api.openai.com/v1 (Phase 2 adds
    // ChatGPT login as its own provider).
    if key == "tokens" {
        return Vec::new();
    }
    // pi-style credential entries pass through keyed by provider id.
    match value {
        Value::Object(_) if value.get("type").is_some() || value.get("key").is_some() => {
            vec![(key.to_string(), value.clone())]
        }
        _ => Vec::new(),
    }
}

/// Maps an OpenCode `auth.json` entry (`{ "type": "api", "key": "..." }`)
/// onto an api-key credential. OpenCode's other entry types (`oauth`,
/// `wellknown`) are not shared.
fn opencode_credential(value: &Value) -> Option<Value> {
    if value.get("type").and_then(Value::as_str) != Some("api") {
        return None;
    }
    let key = value.get("key").and_then(Value::as_str).map(str::trim).filter(|key| !key.is_empty())?;
    Some(json!({ "type": "api_key", "key": key }))
}

/// Drops the loopback receiver of a login when its wait ends (success,
/// failure or cancellation), unless a newer login replaced it already.
struct PendingLoginGuard<'a> {
    manager: &'a PiProviderManager,
    provider_id: &'a str,
    server: Arc<CallbackServer>,
}

impl Drop for PendingLoginGuard<'_> {
    fn drop(&mut self) {
        let mut pending = self.manager.pending_logins.lock().expect("pending logins poisoned");
        if pending.get(self.provider_id).is_some_and(|current| Arc::ptr_eq(current, &self.server)) {
            pending.remove(self.provider_id);
        }
        self.server.shutdown();
    }
}

/// A key supplied by the UI form: trimmed, used verbatim. The preserve marker
/// and blanks mean "none supplied".
pub(super) fn literal_ui_key(key: Option<&str>) -> Option<String> {
    key.map(str::trim).filter(|k| !k.is_empty() && *k != PRESERVE_API_KEY).map(str::to_string)
}

/// How long a `!command` credential helper may run before it is killed.
const KEY_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs a `!command` credential helper (no stdin, killed on timeout or drop)
/// and returns its trimmed stdout.
pub(super) async fn run_credential_command(command: &str, timeout: std::time::Duration) -> Result<String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        // `cmd` does not parse Rust's backslash-quote escaping: pass the line
        // verbatim; `/S` strips exactly the outer quotes.
        c.raw_arg(format!("/S /C \"{command}\""));
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => output?,
        Err(_) => anyhow::bail!("credential command timed out after {}s", timeout.as_secs()),
    };
    if !output.status.success() {
        anyhow::bail!("credential command exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Key preview for status screens: at most the last 4 characters, and only
/// for keys long enough that 4 characters reveal little.
pub(super) fn key_preview_for(key: &str) -> String {
    let count = key.chars().count();
    if count < 20 {
        return "***".to_string();
    }
    let tail: String = key.chars().skip(count - 4).collect();
    format!("…{tail}")
}

/// `access` of an OAuth credential whose `expires` (epoch ms) is still ahead.
fn valid_oauth_access(credential: &Value) -> Option<String> {
    let expires = credential.get("expires").and_then(Value::as_i64)?;
    if expires <= oauth::now_ms() {
        return None;
    }
    credential
        .get("access")
        .and_then(Value::as_str)
        .filter(|access| !access.is_empty())
        .map(str::to_string)
}

impl PiProviderManager {
    pub(super) fn read_json(path: &Path) -> Value {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(Value::Null)
    }

    pub(super) fn write_json(path: &Path, value: &Value) -> Result<()> {
        let json = serde_json::to_string_pretty(value)?;
        crate::private_file::write_private_atomic(path, json.as_bytes())?;
        Ok(())
    }

    pub(super) fn read_auth_map(&self) -> Map<String, Value> {
        // Fallback chain first (codex + pi), primary last so it wins.
        // Among the fallbacks the first listed directory wins a conflict.
        let mut merged = Map::new();
        for directory in self.fallback_dirs.iter() {
            let path = directory.join("auth.json");
            match Self::read_json(&path) {
                Value::Object(map) => {
                    for (key, value) in map {
                        for (provider_id, entry) in codex_credential(&key, &value) {
                            merged.entry(provider_id).or_insert(entry);
                        }
                    }
                }
                _ => continue,
            }
        }
        // OpenCode last: it only fills providers nothing else knows.
        if let Some(Value::Object(map)) = self.opencode_dir.as_ref().map(|dir| Self::read_json(&dir.join("auth.json"))) {
            for (provider_id, entry) in map {
                if let Some(credential) = opencode_credential(&entry) {
                    merged.entry(provider_id).or_insert(credential);
                }
            }
        }
        if let Value::Object(map) = Self::read_json(&self.auth_path()) {
            for (key, value) in map {
                merged.insert(key, value);
            }
        }
        merged
    }

    /// Strict read of one `auth.json`: a missing file is empty, a file that is
    /// not a JSON object is an error so it is never silently overwritten.
    fn read_auth_file(path: &Path) -> Result<Map<String, Value>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(err) => return Err(err.into()),
        };
        if raw.trim().is_empty() {
            return Ok(Map::new());
        }
        match serde_json::from_str(&raw) {
            Ok(Value::Object(map)) => Ok(map),
            _ => anyhow::bail!("{} is not a valid auth.json; fix or remove it first", path.display()),
        }
    }

    /// Read-modify-write of the PRIMARY `auth.json` only, under its lock, so
    /// fallback (`~/.codex`, `~/.pi`) credentials are never copied into it.
    pub(super) fn update_primary_auth(&self, change: impl FnOnce(&mut Map<String, Value>) -> Result<()>) -> Result<()> {
        let path = self.auth_path();
        let _lock = AuthFileLock::acquire_blocking(&path)?;
        let mut map = Self::read_auth_file(&path)?;
        change(&mut map)?;
        Self::write_json(&path, &Value::Object(map))
    }

    /// Resolves the API key for a provider: explicit override first, then
    /// `auth.json` (literal, `!command` stdout, or `$ENV_VAR`), then the
    /// environment-variable fallback chain.
    pub async fn resolve_api_key(
        &self,
        provider_id: &str,
        override_key: Option<&str>,
    ) -> Option<String> {
        if let Some(key) = override_key.map(str::trim).filter(|k| !k.is_empty()) {
            let resolved = self.expand_key_value(key, provider_id).await;
            if !resolved.is_empty() {
                return Some(resolved);
            }
        }

        let auth = self.read_auth_map();
        if let Some(credential) = auth.get(provider_id) {
            let kind = credential
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == "oauth" {
                // A failed refresh is final: do not silently fall back to env keys.
                return self.resolve_oauth_access(provider_id, credential).await;
            }
            if kind == "api_key" || kind.is_empty() {
                if let Some(key) = credential
                    .get("key")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                {
                    let resolved = self.expand_key_value(key, provider_id).await;
                    if !resolved.is_empty() {
                        return Some(resolved);
                    }
                }
            }
        }

        // Codex `config.toml` `env_key` fallback.
        if let Some(entry) = self.models_json_provider(provider_id) {
            if let Some(env_key) = entry.get("envKey").and_then(Value::as_str).filter(|k| !k.is_empty()) {
                if let Ok(value) = std::env::var(env_key) {
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        return Some(value);
                    }
                }
            }
        }

        // Environment fallback: provider's preferred variable first, then the
        // shared chain.
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(builtin) = builtin(provider_id) {
            if !builtin.env_var.is_empty() {
                candidates.push(builtin.env_var);
            }
        }
        for var in ENV_FALLBACKS {
            if !candidates.contains(&var) {
                candidates.push(var);
            }
        }
        candidates
            .into_iter()
            .find_map(|var| std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
            .map(|value| value.trim().to_string())
    }

    /// Starts an OAuth login. Device-code providers return a code to show;
    /// authorization-URL providers start a loopback receiver and return the
    /// URL to open. `enterprise_domain` is only used by `github-copilot`.
    pub async fn start_oauth_login(&self, provider_id: &str, enterprise_domain: Option<&str>) -> Result<LoginSession> {
        match provider_id {
            "xai" => Ok(LoginSession::DeviceCode(self.oauth.xai.start().await?)),
            "github-copilot" => Ok(LoginSession::DeviceCode(self.oauth.copilot.start(enterprise_domain).await?)),
            "kimi-coding" => Ok(LoginSession::DeviceCode(self.oauth.kimi.start().await?)),
            "openai" => {
                let (session, server) = self.oauth.chatgpt.start(&self.device_id()?).await?;
                self.remember_login(provider_id, server);
                Ok(LoginSession::AuthUrl(session))
            }
            "openrouter" => {
                let (session, server) = self.oauth.openrouter.start().await?;
                self.remember_login(provider_id, server);
                Ok(LoginSession::AuthUrl(session))
            }
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth login"),
        }
    }

    /// Waits for the user to complete the login (device approval or the
    /// loopback redirect), then stores the tokens in the primary
    /// `auth.json`. Cancel by dropping the future.
    pub async fn finish_oauth_login(&self, provider_id: &str, session: &LoginSession) -> Result<()> {
        let tokens = match session {
            LoginSession::DeviceCode(device) => self.poll_device_login(provider_id, device).await?,
            LoginSession::AuthUrl(auth) => self.await_redirect_login(provider_id, auth).await?,
        };
        self.store_oauth_tokens(provider_id, &tokens).await
    }

    /// Manual fallback of an authorization-URL login: the user pasted the
    /// redirect URL (or code) from the browser. Ends the loopback wait.
    pub async fn submit_oauth_redirect(&self, provider_id: &str, session: &AuthUrlSession, pasted: &str) -> Result<()> {
        let params = match provider_id {
            "openai" => self.oauth.chatgpt.parse_pasted_redirect(session, pasted)?,
            "openrouter" => self.oauth.openrouter.parse_pasted_redirect(pasted)?,
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support pasting a redirect"),
        };
        let tokens = self.exchange_redirect(provider_id, session, &params).await?;
        self.store_oauth_tokens(provider_id, &tokens).await?;
        if let Some(server) = self.pending_logins.lock().expect("pending logins poisoned").remove(provider_id) {
            server.shutdown();
        }
        Ok(())
    }

    async fn poll_device_login(&self, provider_id: &str, session: &oauth::device_code::DeviceCodeSession) -> Result<OAuthTokens> {
        match provider_id {
            "xai" => self.oauth.xai.poll(session).await,
            "github-copilot" => self.oauth.copilot.poll(session).await,
            "kimi-coding" => self.oauth.kimi.poll(session).await,
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth device login"),
        }
    }

    async fn await_redirect_login(&self, provider_id: &str, session: &AuthUrlSession) -> Result<OAuthTokens> {
        let server = self
            .pending_logins
            .lock()
            .expect("pending logins poisoned")
            .get(provider_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No sign-in is waiting for a redirect; start the login again"))?;
        let _forget_on_exit = PendingLoginGuard { manager: self, provider_id, server: server.clone() };
        let params = server.wait().await?;
        self.exchange_redirect(provider_id, session, &params).await
    }

    async fn exchange_redirect(&self, provider_id: &str, session: &AuthUrlSession, params: &CallbackParams) -> Result<OAuthTokens> {
        match provider_id {
            "openai" => self.oauth.chatgpt.exchange(session, params).await,
            "openrouter" => self.oauth.openrouter.exchange(session, params).await,
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth redirect login"),
        }
    }

    /// Keeps the loopback receiver of a started login; a receiver left over
    /// from an earlier attempt is stopped.
    fn remember_login(&self, provider_id: &str, server: CallbackServer) {
        let previous = self
            .pending_logins
            .lock()
            .expect("pending logins poisoned")
            .insert(provider_id.to_string(), Arc::new(server));
        if let Some(previous) = previous {
            previous.shutdown();
        }
    }

    async fn store_oauth_tokens(&self, provider_id: &str, tokens: &OAuthTokens) -> Result<()> {
        let path = self.auth_path();
        let _lock = AuthFileLock::acquire(&path).await?;
        let mut map = Self::read_auth_file(&path)?;
        map.insert(provider_id.to_string(), tokens.to_credential());
        Self::write_json(&path, &Value::Object(map))
    }

    /// Per-installation UUID sent as the host id of "Sign in with ChatGPT";
    /// created once next to `auth.json` (`<primary dir>/device-id`).
    pub fn device_id(&self) -> Result<String> {
        let path = self.primary_dir.join("device-id");
        if let Some(existing) = std::fs::read_to_string(&path)
            .ok()
            .map(|text| text.trim().to_lowercase())
            .filter(|id| id.len() == 36 && uuid::Uuid::parse_str(id).is_ok())
        {
            return Ok(existing);
        }
        let created = uuid::Uuid::new_v4().to_string();
        crate::private_file::write_private_atomic(&path, created.as_bytes())?;
        Ok(created)
    }

    /// Access token of an OAuth credential, refreshing it when expired.
    async fn resolve_oauth_access(&self, provider_id: &str, credential: &Value) -> Option<String> {
        if let Some(access) = valid_oauth_access(credential) {
            return Some(access);
        }
        match self.refresh_oauth_credential(provider_id).await {
            Ok(access) => Some(access),
            Err(err) => {
                tracing::error!("[Providers] OAuth token refresh for {provider_id} failed: {err}");
                None
            }
        }
    }

    /// Refreshes under the lock of the `auth.json` the credential came from
    /// (primary or a fallback dir), picking up a refresh another process
    /// already did, and writes the result back to that same file.
    async fn refresh_oauth_credential(&self, provider_id: &str) -> Result<String> {
        let path = self
            .oauth_source_path(provider_id)
            .ok_or_else(|| anyhow::anyhow!("no stored OAuth credential found"))?;
        let _lock = AuthFileLock::acquire(&path).await?;
        let mut map = Self::read_auth_file(&path)?;
        let entry = map
            .get(provider_id)
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no stored OAuth credential found"))?;
        if let Some(access) = valid_oauth_access(&Value::Object(entry.clone())) {
            return Ok(access);
        }
        let refresh_token = entry
            .get("refresh")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| anyhow::anyhow!("stored OAuth credential has no refresh token"))?;
        let tokens = self.refresh_tokens(provider_id, refresh_token, &entry).await?;
        let mut updated = entry;
        tokens.apply_to(&mut updated);
        map.insert(provider_id.to_string(), Value::Object(updated));
        Self::write_json(&path, &Value::Object(map))?;
        Ok(tokens.access)
    }

    pub(super) async fn refresh_tokens(&self, provider_id: &str, refresh_token: &str, entry: &Map<String, Value>) -> Result<OAuthTokens> {
        match provider_id {
            "xai" => self.oauth.xai.refresh(refresh_token).await,
            "openai" => self.oauth.chatgpt.refresh(entry).await,
            "github-copilot" => self.oauth.copilot.refresh(entry).await,
            "kimi-coding" => self.oauth.kimi.refresh(refresh_token).await,
            "openrouter" => anyhow::bail!("OpenRouter keys do not refresh; sign in again"),
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth refresh"),
        }
    }

    /// The winning OAuth credential of a provider, if it signed in with OAuth.
    pub(super) fn oauth_credential(&self, provider_id: &str) -> Option<Value> {
        self.read_auth_map()
            .remove(provider_id)
            .filter(|credential| credential.get("type").and_then(Value::as_str) == Some("oauth"))
    }

    /// Chat API base URL for a Copilot access token: its `proxy-ep` host, else
    /// the enterprise or individual endpoint of the stored credential.
    pub fn copilot_base_url(&self, access_token: &str) -> String {
        let domain = self
            .oauth_credential("github-copilot")
            .and_then(|credential| credential.get("enterpriseUrl").and_then(Value::as_str).and_then(oauth::github_copilot::normalize_domain));
        oauth::github_copilot::base_url_from_token(access_token, domain.as_deref())
    }

    /// Models an OAuth login unlocks, when the provider has a fixed list:
    /// the ChatGPT chat models (`openai` OAuth only), the Copilot models the
    /// account may use, and the Kimi Code models.
    pub(super) fn oauth_catalog(&self, provider_id: &str) -> Option<Vec<CatalogModel>> {
        match provider_id {
            "openai" => self.oauth_credential("openai").map(|_| catalog::CHATGPT_MODELS.to_vec()),
            "github-copilot" => {
                let allowed: Option<Vec<String>> = self
                    .oauth_credential("github-copilot")
                    .and_then(|credential| credential.get("availableModelIds").cloned())
                    .and_then(|ids| serde_json::from_value(ids).ok());
                let models = catalog::COPILOT_MODELS.iter().filter(|model| allowed.as_ref().is_none_or(|ids| ids.iter().any(|id| id == model.id)));
                Some(models.copied().collect())
            }
            "kimi-coding" => Some(catalog::KIMI_MODELS.to_vec()),
            _ => None,
        }
    }

    /// The `auth.json` holding the winning pi-style credential for a
    /// provider, in the same precedence as `read_auth_map`: primary first,
    /// then fallbacks in listed order (the first listed fallback wins the
    /// merge). OpenCode only holds api keys, so it is never an oauth source.
    pub(super) fn oauth_source_path(&self, provider_id: &str) -> Option<PathBuf> {
        std::iter::once(self.auth_path())
            .chain(self.fallback_dirs.iter().map(|dir| dir.join("auth.json")))
            .find(|path| {
                Self::read_json(path)
                    .get(provider_id)
                    .map(|entry| entry.get("type").and_then(Value::as_str) == Some("oauth"))
                    .unwrap_or(false)
            })
    }

    /// Expands `!command` (shell stdout, cached) and `$ENV_VAR` forms.
    async fn expand_key_value(&self, raw: &str, provider_id: &str) -> String {
        let raw = raw.trim();
        if let Some(command) = raw.strip_prefix('!') {
            let command = command.trim();
            if let Some(cached) = self.command_cache.lock().expect("cache poisoned").get(command) {
                return cached.clone();
            }
            match self.run_key_command(command).await {
                Ok(stdout) => {
                    self.command_cache
                        .lock()
                        .expect("cache poisoned")
                        .insert(command.to_string(), stdout.clone());
                    stdout
                }
                Err(err) => {
                    tracing::error!(
                        "[Providers] `!command` credential for {provider_id} failed: {err}"
                    );
                    String::new()
                }
            }
        } else if let Some(var) = raw.strip_prefix('$') {
            std::env::var(var.trim())
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        } else {
            raw.to_string()
        }
    }

    async fn run_key_command(&self, command: &str) -> Result<String> {
        run_credential_command(command, KEY_COMMAND_TIMEOUT).await
    }

    pub fn has_api_key(&self, provider_id: &str) -> bool {
        self.read_auth_map().contains_key(provider_id)
    }

    /// Masked credential status for each provider in
    /// `auth.json`.
    pub fn pi_auth_status(&self) -> Vec<Value> {
        self.read_auth_map()
            .iter()
            .map(|(provider_id, credential)| {
                let kind = credential
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let key_preview = credential
                    .get("key")
                    .and_then(Value::as_str)
                    .map(|key| {
                        let key = key.trim();
                        if key.starts_with('!') {
                            "command".to_string()
                        } else if key.starts_with('$') {
                            "environment".to_string()
                        } else {
                            key_preview_for(key)
                        }
                    });
                let mut status = json!({
                    "providerId": provider_id,
                    "type": kind,
                    "hasCredential": true,
                    "keyPreview": key_preview,
                });
                if kind == "oauth" {
                    // Expiry only; the access/refresh tokens never leave auth.json.
                    let expires = credential.get("expires").and_then(Value::as_i64).unwrap_or(0);
                    status["keyPreview"] = json!("oauth");
                    status["expiresAt"] = json!(expires);
                    status["expired"] = json!(expires <= oauth::now_ms());
                }
                status
            })
            .collect()
    }

    /// Sets or deletes a provider credential in
    /// `auth.json`.
    pub fn set_pi_auth(&self, input: &SetPiAuthInput) -> Result<()> {
        if input.provider_id.trim().is_empty() {
            anyhow::bail!("providerId is required");
        }
        let provider_id = input.provider_id.trim();
        match input.action.as_str() {
            "delete" | "remove" => self.update_primary_auth(|auth| {
                if auth.remove(provider_id).is_none() {
                    anyhow::bail!("No credential for provider \"{}\"", input.provider_id);
                }
                Ok(())
            }),
            "set" | "save" => {
                let key = input
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("apiKey is required to set a credential"))?;
                let credential = serde_json::to_value(AuthCredential::api_key(key))?;
                self.update_primary_auth(|auth| {
                    auth.insert(provider_id.to_string(), credential);
                    Ok(())
                })
            }
            _ => anyhow::bail!("Invalid action. Must be 'set' or 'delete'"),
        }
    }
}
