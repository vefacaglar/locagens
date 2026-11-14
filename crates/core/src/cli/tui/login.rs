//! The `/login` overlay (UX-002): pick a provider, then paste an API key
//! (masked) or approve an OAuth sign-in with a code or URL.
//!
//! Pure state and key handling; the driver performs the network calls and
//! stores the credential. Secrets are never printed: the key input is drawn
//! as bullets and none of these types derive `Debug`.

use super::composer::printable_char;
use crate::pi_providers::{LoginProvider, LoginStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One provider row of the picker.
#[derive(Clone)]
pub struct LoginRow {
    pub id: String,
    pub name: String,
    /// `Sign in with ChatGPT`, `API key` or `no sign-in needed`.
    pub method: String,
    /// `✔ connected`, `✔ env $VAR`, or empty.
    pub status: String,
    pub has_oauth: bool,
    pub keyless: bool,
}

impl LoginRow {
    pub fn from_provider(provider: &LoginProvider) -> Self {
        let keyless = provider.id == "ollama";
        let has_oauth = provider.oauth_label.is_some();
        let method = match (&provider.oauth_label, keyless) {
            (_, true) => "no sign-in needed (local)".to_string(),
            // The label is the whole phrase ("Sign in with ChatGPT").
            (Some(label), _) => label.clone(),
            (None, _) if provider.web_search => "API key (web search)".to_string(),
            (None, _) => "API key".to_string(),
        };
        let status = match &provider.status {
            LoginStatus::OAuth => "✔ signed in".to_string(),
            LoginStatus::ApiKey => "✔ connected".to_string(),
            LoginStatus::Env(var) => format!("✔ env ${var}"),
            LoginStatus::SignedOut => String::new(),
        };
        Self { id: provider.id.clone(), name: provider.name.clone(), method, status, has_oauth, keyless }
    }

    fn matches(&self, query: &str) -> bool {
        let haystack = format!("{} {} {}", self.id, self.name, self.method).to_lowercase();
        query.split_whitespace().all(|word| haystack.contains(&word.to_lowercase()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputPurpose {
    ApiKey,
    /// GitHub Enterprise domain for Copilot (blank = github.com).
    Domain,
}

/// What the OAuth waiting screen shows.
pub struct WaitInfo {
    /// Device flow: the code to type at the URL.
    pub code: Option<String>,
    pub url: String,
    /// Authorization-URL flow: the redirect may be pasted back.
    pub accepts_paste: bool,
    pub paste: String,
    /// Why the last pasted redirect was refused.
    pub error: Option<String>,
}

pub enum Stage {
    Pick { query: String, selected: usize, error: Option<String> },
    Input { purpose: InputPurpose, provider: LoginRow, text: String },
    /// `info` is `None` until the provider answered the start request.
    Waiting { provider: LoginRow, info: Option<WaitInfo> },
}

pub struct LoginFlow {
    pub rows: Vec<LoginRow>,
    pub stage: Stage,
}

/// What a key asks the driver to do.
pub enum LoginAction {
    Close,
    BeginOauth { provider: String, domain: Option<String> },
    SaveKey { provider: String, key: String },
    OpenBrowser,
    CopyCode,
    SubmitRedirect(String),
    /// Abort a pending sign-in and close.
    Cancel,
    /// A provider that needs nothing: just tell the user.
    Note(String),
}

impl LoginFlow {
    pub fn new(rows: Vec<LoginRow>) -> Self {
        Self { rows, stage: Stage::Pick { query: String::new(), selected: 0, error: None } }
    }

    /// Jumps straight to one provider (`/login anthropic`); `None` when the
    /// id is unknown.
    pub fn for_provider(rows: Vec<LoginRow>, provider_id: &str) -> Option<Self> {
        let mut flow = Self::new(rows);
        let row = flow.rows.iter().find(|row| row.id == provider_id)?.clone();
        flow.stage = flow.default_stage_for(row);
        Some(flow)
    }

    pub fn visible(&self) -> Vec<&LoginRow> {
        match &self.stage {
            Stage::Pick { query, .. } => self.rows.iter().filter(|row| row.matches(query)).collect(),
            _ => Vec::new(),
        }
    }

    fn default_stage_for(&self, row: LoginRow) -> Stage {
        if row.has_oauth && row.id == "github-copilot" {
            Stage::Input { purpose: InputPurpose::Domain, provider: row, text: String::new() }
        } else if row.has_oauth {
            Stage::Waiting { provider: row, info: None }
        } else {
            Stage::Input { purpose: InputPurpose::ApiKey, provider: row, text: String::new() }
        }
    }

    /// The action that goes with entering the stage `default_stage_for` built.
    pub fn begin_action(&self) -> Option<LoginAction> {
        match &self.stage {
            Stage::Waiting { provider, info: None } => {
                Some(LoginAction::BeginOauth { provider: provider.id.clone(), domain: None })
            }
            _ => None,
        }
    }

    /// Back to the picker with a message under the list.
    pub fn fail(&mut self, message: String) {
        self.stage = Stage::Pick { query: String::new(), selected: 0, error: Some(message) };
    }

    pub fn set_started(&mut self, info: WaitInfo) {
        if let Stage::Waiting { info: slot, .. } = &mut self.stage {
            *slot = Some(info);
        }
    }

    /// A pasted redirect was refused: say why on the waiting screen.
    pub fn set_wait_error(&mut self, message: String) {
        if let Stage::Waiting { info: Some(info), .. } = &mut self.stage {
            info.paste.clear();
            info.error = Some(message);
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.stage {
            Stage::Input { text, .. } => text.push_str(clean.trim()),
            Stage::Waiting { info: Some(info), .. } if info.accepts_paste => info.paste.push_str(clean.trim()),
            Stage::Pick { query, selected, .. } => {
                query.push_str(&clean);
                *selected = 0;
            }
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<LoginAction> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return self.escape();
        }
        match &self.stage {
            Stage::Pick { .. } => self.on_pick_key(key),
            Stage::Input { .. } => self.on_input_key(key),
            Stage::Waiting { .. } => self.on_waiting_key(key),
        }
    }

    /// Esc: back out one step, or close from the picker.
    fn escape(&mut self) -> Option<LoginAction> {
        match &self.stage {
            Stage::Pick { .. } => Some(LoginAction::Close),
            Stage::Input { .. } => {
                self.stage = Stage::Pick { query: String::new(), selected: 0, error: None };
                None
            }
            Stage::Waiting { .. } => Some(LoginAction::Cancel),
        }
    }

    fn on_pick_key(&mut self, key: KeyEvent) -> Option<LoginAction> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.visible().len();
        let Stage::Pick { query, selected, error } = &mut self.stage else { return None };
        *error = None;
        if let Some(ch) = printable_char(&key) {
            query.push(ch);
            *selected = 0;
            return None;
        }
        match key.code {
            KeyCode::Esc => return Some(LoginAction::Close),
            KeyCode::Up => *selected = selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            KeyCode::Char('p') if ctrl => *selected = selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            KeyCode::Down => *selected = if *selected + 1 >= count { 0 } else { *selected + 1 },
            KeyCode::Char('n') if ctrl => *selected = if *selected + 1 >= count { 0 } else { *selected + 1 },
            KeyCode::Backspace => {
                query.pop();
                *selected = 0;
            }
            KeyCode::Enter | KeyCode::Tab => {
                let want_key = key.code == KeyCode::Tab;
                let index = *selected;
                let row = self.visible().get(index).map(|row| (*row).clone())?;
                return self.choose(row, want_key);
            }
            _ => {}
        }
        None
    }

    fn choose(&mut self, row: LoginRow, want_key: bool) -> Option<LoginAction> {
        if row.keyless {
            return Some(LoginAction::Note(format!("{} needs no sign-in.", row.name)));
        }
        self.stage = if want_key || !row.has_oauth {
            Stage::Input { purpose: InputPurpose::ApiKey, provider: row, text: String::new() }
        } else {
            self.default_stage_for(row)
        };
        self.begin_action()
    }

    fn on_input_key(&mut self, key: KeyEvent) -> Option<LoginAction> {
        if key.code == KeyCode::Esc {
            return self.escape();
        }
        let Stage::Input { purpose, provider, text } = &mut self.stage else { return None };
        if let Some(ch) = printable_char(&key) {
            text.push(ch);
            return None;
        }
        match key.code {
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Enter => {
                let value = text.trim().to_string();
                match purpose {
                    InputPurpose::ApiKey if !value.is_empty() => {
                        return Some(LoginAction::SaveKey { provider: provider.id.clone(), key: value });
                    }
                    InputPurpose::Domain => {
                        let id = provider.id.clone();
                        let provider = provider.clone();
                        self.stage = Stage::Waiting { provider, info: None };
                        return Some(LoginAction::BeginOauth {
                            provider: id,
                            domain: Some(value).filter(|d| !d.is_empty()),
                        });
                    }
                    InputPurpose::ApiKey => {}
                }
            }
            _ => {}
        }
        None
    }

    fn on_waiting_key(&mut self, key: KeyEvent) -> Option<LoginAction> {
        if key.code == KeyCode::Esc {
            return self.escape();
        }
        let Stage::Waiting { info: Some(info), .. } = &mut self.stage else { return None };
        if info.accepts_paste {
            if let Some(ch) = printable_char(&key) {
                info.paste.push(ch);
                return None;
            }
        }
        match key.code {
            KeyCode::Backspace if info.accepts_paste => {
                info.paste.pop();
            }
            KeyCode::Enter if info.accepts_paste && !info.paste.trim().is_empty() => {
                return Some(LoginAction::SubmitRedirect(info.paste.trim().to_string()));
            }
            KeyCode::Enter => return Some(LoginAction::OpenBrowser),
            KeyCode::Char('c') if info.code.is_some() => return Some(LoginAction::CopyCode),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(id: &str, oauth: bool) -> LoginRow {
        LoginRow {
            id: id.into(),
            name: id.to_uppercase(),
            method: if oauth { "Sign in with X".into() } else { "API key".into() },
            status: String::new(),
            has_oauth: oauth,
            keyless: id == "ollama",
        }
    }

    #[test]
    fn sign_in_label_is_not_doubled() {
        let provider = LoginProvider {
            id: "openai-codex".into(),
            name: "OpenAI".into(),
            oauth_label: Some("Sign in with ChatGPT".into()),
            oauth_flow: None,
            api_key_env: None,
            status: LoginStatus::SignedOut,
            web_search: false,
        };
        assert_eq!(LoginRow::from_provider(&provider).method, "Sign in with ChatGPT");
    }

    fn flow() -> LoginFlow {
        LoginFlow::new(vec![row("anthropic", false), row("openai-codex", true), row("ollama", false)])
    }

    #[test]
    fn api_key_provider_goes_to_masked_input_and_saves_the_key() {
        let mut flow = flow();
        assert!(flow.on_key(press(KeyCode::Enter)).is_none());
        assert!(matches!(&flow.stage, Stage::Input { purpose: InputPurpose::ApiKey, .. }));
        assert!(flow.on_key(press(KeyCode::Enter)).is_none(), "an empty key saves nothing");
        flow.on_paste(" sk-test \r\n");
        let Some(LoginAction::SaveKey { provider, key }) = flow.on_key(press(KeyCode::Enter)) else {
            panic!("expected SaveKey");
        };
        assert_eq!((provider.as_str(), key.as_str()), ("anthropic", "sk-test"));
    }

    #[test]
    fn escape_steps_back_then_closes() {
        let mut flow = flow();
        flow.on_key(press(KeyCode::Enter));
        assert!(flow.on_key(press(KeyCode::Esc)).is_none());
        assert!(matches!(flow.stage, Stage::Pick { .. }));
        assert!(matches!(flow.on_key(press(KeyCode::Esc)), Some(LoginAction::Close)));
    }

    #[test]
    fn oauth_provider_begins_a_sign_in_and_tab_picks_the_key_instead() {
        let mut flow = flow();
        flow.on_key(press(KeyCode::Down));
        let Some(LoginAction::BeginOauth { provider, domain }) = flow.on_key(press(KeyCode::Enter)) else {
            panic!("expected BeginOauth");
        };
        assert_eq!(provider, "openai-codex");
        assert!(domain.is_none());
        assert!(matches!(flow.stage, Stage::Waiting { info: None, .. }));

        let mut flow = self::flow();
        flow.on_key(press(KeyCode::Down));
        assert!(flow.on_key(press(KeyCode::Tab)).is_none());
        assert!(matches!(&flow.stage, Stage::Input { purpose: InputPurpose::ApiKey, .. }));
    }

    #[test]
    fn waiting_screen_offers_browser_code_copy_and_cancel() {
        let mut flow = flow();
        flow.on_key(press(KeyCode::Down));
        flow.on_key(press(KeyCode::Enter));
        flow.set_started(WaitInfo { code: Some("AB-12".into()), url: "https://x".into(), accepts_paste: false, paste: String::new(), error: None });
        assert!(matches!(flow.on_key(press(KeyCode::Enter)), Some(LoginAction::OpenBrowser)));
        assert!(matches!(flow.on_key(press(KeyCode::Char('c'))), Some(LoginAction::CopyCode)));
        assert!(matches!(flow.on_key(press(KeyCode::Esc)), Some(LoginAction::Cancel)));
    }

    #[test]
    fn redirect_flow_takes_a_pasted_url() {
        let mut flow = flow();
        flow.on_key(press(KeyCode::Down));
        flow.on_key(press(KeyCode::Enter));
        flow.set_started(WaitInfo { code: None, url: "https://x".into(), accepts_paste: true, paste: String::new(), error: None });
        flow.on_paste("http://localhost:1/cb?code=1");
        let Some(LoginAction::SubmitRedirect(url)) = flow.on_key(press(KeyCode::Enter)) else {
            panic!("expected SubmitRedirect");
        };
        assert!(url.ends_with("code=1"));
    }

    #[test]
    fn failure_returns_to_the_picker_with_a_message() {
        let mut flow = flow();
        flow.on_key(press(KeyCode::Down));
        flow.on_key(press(KeyCode::Enter));
        flow.fail("boom".into());
        assert!(matches!(&flow.stage, Stage::Pick { error: Some(e), .. } if e == "boom"));
    }

    #[test]
    fn keyless_provider_needs_no_key_and_search_filters() {
        let mut flow = flow();
        for ch in "olla".chars() {
            flow.on_key(press(KeyCode::Char(ch)));
        }
        assert_eq!(flow.visible().len(), 1);
        assert!(matches!(flow.on_key(press(KeyCode::Enter)), Some(LoginAction::Note(_))));
    }

    #[test]
    fn for_provider_skips_the_picker() {
        let flow = LoginFlow::for_provider(vec![row("anthropic", false)], "anthropic").unwrap();
        assert!(matches!(flow.stage, Stage::Input { purpose: InputPurpose::ApiKey, .. }));
        assert!(LoginFlow::for_provider(vec![row("anthropic", false)], "nope").is_none());
    }
}
