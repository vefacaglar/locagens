//! Reactive UI state of the desktop app: the `UiStore` bundle of Dioxus
//! signals plus the helpers that load data from `locagens_core::api` and
//! process live run-event streams.

use dioxus::prelude::*;
use crate::i18n::{self, Lang};
use crate::prefs::{self, Appearance, ThemeChoice};
use locagens_core::api::{self, EventRx};
use locagens_core::db::{Plan, Project, Run, RunMessage};
use locagens_core::pi_providers::ProviderMetadata;
use serde_json::Value;
use std::sync::Arc;

/// The shared backend core, initialized in `main` before launch.
pub fn core() -> Arc<locagens_core::AppState> {
    CORE_STATE
        .get()
        .cloned()
        .expect("Locagens core is not initialized")
}

static CORE_STATE: std::sync::OnceLock<Arc<locagens_core::AppState>> = std::sync::OnceLock::new();

/// Registers the shared backend state (called by `main` before launch).
pub fn set_core(state: Arc<locagens_core::AppState>) {
    let _ = CORE_STATE.set(state);
}

/// Top-level pages of the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppPage {
    Chat,
    Settings(SettingsTab),
    UsageLogs,
}

/// Tabs inside the settings page (Phase 7a: providers + auth are live, the
/// rest list what the core exposes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Providers,
    Auth,
    Permissions,
    Memory,
    Skills,
    Mcp,
    Plugins,
    Security,
    Appearance,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 9] = [
        SettingsTab::Providers,
        SettingsTab::Auth,
        SettingsTab::Permissions,
        SettingsTab::Memory,
        SettingsTab::Skills,
        SettingsTab::Mcp,
        SettingsTab::Plugins,
        SettingsTab::Security,
        SettingsTab::Appearance,
    ];

    /// i18n key of the tab's label (see `crate::i18n`).
    pub fn label_key(self) -> &'static str {
        match self {
            SettingsTab::Providers => "tab.providers",
            SettingsTab::Auth => "tab.auth",
            SettingsTab::Permissions => "tab.permissions",
            SettingsTab::Memory => "tab.memory",
            SettingsTab::Skills => "tab.skills",
            SettingsTab::Mcp => "tab.mcp",
            SettingsTab::Plugins => "tab.plugins",
            SettingsTab::Security => "tab.security",
            SettingsTab::Appearance => "tab.appearance",
        }
    }
}

/// Tabs of the right-hand inspector panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightPanelTab {
    Plan,
    Agents,
    Git,
    Terminal,
}

impl RightPanelTab {
    pub const ALL: [RightPanelTab; 4] = [
        RightPanelTab::Plan,
        RightPanelTab::Agents,
        RightPanelTab::Git,
        RightPanelTab::Terminal,
    ];

    /// i18n key of the tab's label.
    pub fn label_key(self) -> &'static str {
        match self {
            RightPanelTab::Plan => "inspector.plan",
            RightPanelTab::Agents => "inspector.agents",
            RightPanelTab::Git => "inspector.git",
            RightPanelTab::Terminal => "inspector.terminal",
        }
    }
}

/// Where a run event lands: the visible conversation or only the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventScope {
    /// The event's run is the one shown in the chat view.
    Foreground,
    /// Another run's event: it may only update that run's sidebar entry.
    Background,
}

/// Scope of an event from `stream_run` while `active` is the open run.
pub fn event_scope(active: Option<&str>, stream_run: &str) -> EventScope {
    if active == Some(stream_run) {
        EventScope::Foreground
    } else {
        EventScope::Background
    }
}

/// Event types that still matter for a run that is not in view: they only
/// touch the sidebar list (status dot, title).
pub fn applies_in_background(kind: &str) -> bool {
    matches!(kind, "status_changed" | "run_title_changed" | "run_completed" | "run_failed")
}

/// Latest event-stream generation per run, so re-attaching a run (or opening
/// it again) retires the previous listener task instead of doubling events.
#[derive(Default)]
pub struct StreamRegistry {
    latest: std::collections::BTreeMap<String, u64>,
    counter: u64,
}

impl StreamRegistry {
    pub const fn new() -> Self {
        Self { latest: std::collections::BTreeMap::new(), counter: 0 }
    }

    /// Registers a new stream for `run_id`, superseding any earlier one.
    pub fn claim(&mut self, run_id: &str) -> u64 {
        self.counter += 1;
        self.latest.insert(run_id.to_string(), self.counter);
        self.counter
    }

    /// Whether `generation` is still the newest stream of `run_id`.
    pub fn is_current(&self, run_id: &str, generation: u64) -> bool {
        self.latest.get(run_id) == Some(&generation)
    }

    /// Forgets the stream when it ended (only if it is still the newest).
    pub fn release(&mut self, run_id: &str, generation: u64) {
        if self.is_current(run_id, generation) {
            self.latest.remove(run_id);
        }
    }
}

static STREAMS: std::sync::Mutex<StreamRegistry> = std::sync::Mutex::new(StreamRegistry::new());

fn streams() -> std::sync::MutexGuard<'static, StreamRegistry> {
    STREAMS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Bundle of every reactive signal, handed to components via context.
/// `Copy` so event-handler closures can capture it freely.
#[derive(Clone, Copy)]
pub struct UiStore {
    /// Persisted appearance preferences (language, theme, fonts, size).
    pub appearance: Signal<Appearance>,
    /// Live OS dark-mode state (only consulted for `ThemeChoice::System`).
    pub system_dark: Signal<bool>,
    pub page: Signal<AppPage>,
    pub projects: Signal<Vec<Project>>,
    pub selected_project: Signal<Option<String>>,
    pub runs: Signal<Vec<Run>>,
    pub search_query: Signal<String>,
    pub active_run_id: Signal<Option<String>>,
    pub active_run: Signal<Option<Run>>,
    pub messages: Signal<Vec<RunMessage>>,
    pub plan: Signal<Option<Plan>>,
    pub run_usage: Signal<Option<api::runs::RunUsageSummary>>,
    pub pending_permission: Signal<Option<Value>>,
    pub pending_question: Signal<Option<Value>>,
    pub providers_catalog: Signal<Vec<ProviderMetadata>>,
    pub selected_provider: Signal<String>,
    pub selected_model: Signal<String>,
    pub selected_effort: Signal<String>,
    pub selected_mode: Signal<String>,
    pub right_panel_open: Signal<bool>,
    pub right_panel_tab: Signal<RightPanelTab>,
    pub status_toast: Signal<Option<String>>,
    pub prompt: Signal<String>,
    /// Aggregate workspace diff of the current turn: `(diff, files_changed)`.
    pub turn_diff: Signal<Option<(String, usize)>>,
    /// Live `spawn_agent` / `spawn_agents` registrations not yet visible as
    /// messages (Codex-style background agents).
    pub spawned_agents: Signal<Vec<Value>>,
    /// Latest `patch_progress` event while the model streams an `apply_patch`
    /// call (files + line counts); cleared when the patch is complete.
    pub patch_progress: Signal<Option<Value>>,
}

impl UiStore {
    /// The UI language (subscribes the calling component to changes).
    pub fn lang(&self) -> Lang {
        self.appearance.read().language
    }

    /// Translated static string for `key`.
    pub fn t(&self, key: &'static str) -> &'static str {
        i18n::t(self.lang(), key)
    }

    /// Translated string with `{0}`, `{1}`… replaced by `args`.
    pub fn tf(&self, key: &'static str, args: &[&str]) -> String {
        i18n::tf(self.lang(), key, args)
    }

    /// Whether the effective theme is dark (resolves `System`).
    pub fn is_dark(&self) -> bool {
        match self.appearance.read().theme {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => *self.system_dark.read(),
        }
    }

    /// Applies and persists new appearance settings.
    pub fn set_appearance(&self, next: Appearance) {
        let mut store = *self;
        prefs::save(&next);
        store.appearance.set(next);
    }

    /// Header toggle: flips to the opposite of the effective theme, keeping
    /// the Appearance setting in sync (System becomes an explicit choice).
    pub fn toggle_theme(&self) {
        let mut next = self.appearance.read().clone();
        next.theme = if self.is_dark() { ThemeChoice::Light } else { ThemeChoice::Dark };
        self.set_appearance(next);
    }

    /// Initial load: projects, runs and the provider catalog (picking the
    /// first configured provider + its first model as defaults), then select
    /// the most recent project and its most recent run.
    /// Reloads the provider/model catalog, e.g. after a sign-in or a new
    /// API key in Settings, so the model picker shows it without a restart.
    pub fn refresh_providers(&self) {
        let store = *self;
        spawn(async move { store.load_providers().await });
    }

    /// Loads the provider catalog and picks a default model when none is set.
    async fn load_providers(&self) {
        let mut store = *self;
        let Ok(providers) = api::providers::list_providers(&core()).await else { return };
        let first = providers.iter().find(|provider| !provider.models.is_empty()).cloned();
        store.providers_catalog.set(providers);
        if let Some(provider) = first {
            if store.selected_provider.read().is_empty() {
                store.selected_provider.set(provider.id.clone());
            }
            if store.selected_model.read().is_empty() {
                store.selected_model.set(provider.models.first().cloned().unwrap_or_default());
            }
        }
    }

    pub fn refresh_initial(&self) {
        let mut store = *self;
        spawn(async move {
            let state = core();
            if let Ok(projects) = api::projects::list_projects(&state).await {
                let recent = projects.first().cloned();
                store.projects.set(projects);
                store.selected_project.set(recent.map(|project| project.path));
            }
            let runs = api::runs::list_runs(&state).await.unwrap_or_default();
            let latest_run = runs.first().cloned();
            store.runs.set(runs);

            store.load_providers().await;

            if let Some(run) = latest_run {
                store.select_run(run.id);
            }
        });
    }

    /// Reloads the runs list (used after create/delete/status changes).
    pub fn refresh_runs(&self) {
        let mut store = *self;
        spawn(async move {
            if let Ok(runs) = api::runs::list_runs(&core()).await {
                store.runs.set(runs);
            }
        });
    }

    /// Reloads the registered projects list.
    pub fn refresh_projects(&self) {
        let mut store = *self;
        spawn(async move {
            if let Ok(projects) = api::projects::list_projects(&core()).await {
                store.projects.set(projects);
            }
        });
    }

    /// Loads a run's full details into the store and subscribes to its event
    /// stream when it is still active.
    pub fn select_run(&self, run_id: String) {
        let mut store = *self;
        store.active_run_id.set(Some(run_id.clone()));
        store.turn_diff.set(None);
        store.spawned_agents.set(Vec::new());
        store.patch_progress.set(None);
        // The previous run's prompts must not linger over this one.
        store.pending_permission.set(None);
        store.pending_question.set(None);
        spawn(async move {
            let state = core();
            // Another selection may land while the loads below are awaited;
            // the guard after them drops this load if it did.
            // 1. Run + messages + plan + usage + pending requests.
            let run = api::runs::get_run(&state, &run_id).await.ok();
            let messages = api::runs::list_messages(&state, &run_id).await.unwrap_or_default();
            let plan = api::runs::run_plan(&state, &run_id).await.unwrap_or(None);
            let usage = api::runs::run_usage(&state, &run_id).await.ok();
            let pending = api::runs::run_pending(&state, &run_id).await.ok();
            if event_scope(store.active_run_id.peek().as_deref(), &run_id) == EventScope::Background {
                return;
            }
            if let Some(run) = run {
                store.active_run.set(Some(run));
            }
            store.messages.set(messages);
            store.plan.set(plan);
            store.run_usage.set(usage);
            if let Some(pending) = pending {
                store.pending_permission.set(pending.permission_request);
                store.pending_question.set(pending.question_request);
            }
            scroll_chat_to_bottom();

            // 2. Live event subscription for active runs.
            let active = store
                .active_run
                .read()
                .as_ref()
                .map(|run| matches!(run.status.as_str(), "created" | "generating" | "awaiting_permission" | "awaiting_input"))
                .unwrap_or(false);
            if active {
                let rx = api::runs::subscribe_run_events(&state, &run_id).await;
                store.attach_event_stream(run_id, rx);
            }
        });
    }

    /// Starts processing a run's event stream (used both for runs selected
    /// after the fact and for runs started from the composer, which pass the
    /// receiver subscribed BEFORE the drive task began).
    pub fn attach_event_stream(&self, run_id: String, mut rx: EventRx) {
        let store = *self;
        let generation = streams().claim(&run_id);
        spawn(async move {
            loop {
                let event = match rx.recv().await {
                    Ok(event) if event.get("type").and_then(Value::as_str) != Some("lagged") => Some(event),
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                // A newer stream for the same run took over: retire this one.
                if !streams().is_current(&run_id, generation) {
                    return;
                }
                let alive = match event {
                    Some(event) => store.process_event(&run_id, event),
                    // Events were dropped: rebuild from the store so a lost
                    // prompt or terminal event is not missed (QA-046).
                    None => store.resync(&run_id).await,
                };
                if !alive {
                    break;
                }
            }
            streams().release(&run_id, generation);
        });
    }

    /// Reloads a run after its event stream lagged and replays the pending
    /// prompts / terminal event; returns `false` when the run has ended.
    async fn resync(&self, run_id: &str) -> bool {
        let mut store = *self;
        let snapshot = api::runs::resync_run(&core(), run_id).await;
        if event_scope(store.active_run_id.peek().as_deref(), run_id) == EventScope::Foreground {
            if let Some(run) = snapshot.run {
                store.active_run.set(Some(run));
            }
            store.messages.set(snapshot.messages);
        }
        snapshot.events.into_iter().all(|event| store.process_event(run_id, event))
    }

    /// Processes one run event; returns `false` when the stream ended.
    fn process_event(&self, run_id: &str, event: Value) -> bool {
        let mut store = *self;
        let kind = event.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
        // Events of a run that is not in view may only touch its sidebar entry.
        let foreground = event_scope(store.active_run_id.peek().as_deref(), run_id) == EventScope::Foreground;
        if !foreground && !applies_in_background(&kind) {
            return true;
        }
        match kind.as_str() {
            "message_created" | "message_updated" => {
                if let Some(message) = event
                    .get("message")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<RunMessage>(value).ok())
                {
                    if message.run_id != run_id {
                        return true;
                    }
                    let mut updated = false;
                    store.messages.with_mut(|messages| {
                        if let Some(existing) = messages.iter_mut().find(|m| m.id == message.id) {
                            *existing = message.clone();
                            updated = true;
                        }
                    });
                    if !updated {
                        store.messages.with_mut(|messages| messages.push(message));
                    }
                    follow_chat();
                }
            }
            "status_changed" => {
                let status = event.get("status").and_then(Value::as_str).unwrap_or_default().to_string();
                let snapshot = store.active_run.read().clone();
                if let Some(mut run) = snapshot {
                    if run.id == run_id {
                        run.status = status.clone();
                        store.active_run.set(Some(run));
                    }
                }
                store.runs.with_mut(|runs| {
                    if let Some(run) = runs.iter_mut().find(|run| run.id == run_id) {
                        run.status = status.clone();
                    }
                });
                if foreground && status == "generating" {
                    // Permission/question resolved; drop stale cards.
                    store.pending_permission.set(None);
                    store.pending_question.set(None);
                }
            }
            "permission_requested" => {
                store.pending_permission.set(Some(event.clone()));
            }
            "question_requested" => {
                store.pending_question.set(Some(event.clone()));
            }
            "plan_updated" => {
                if let Some(plan) = event
                    .get("plan")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<Plan>(value).ok())
                {
                    store.plan.set(Some(plan));
                }
            }
            "turn_diff_updated" => {
                let diff = event.get("diff").and_then(Value::as_str).unwrap_or_default().to_string();
                if !diff.is_empty() {
                    let files_changed = event
                        .get("filesChanged")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    store.turn_diff.set(Some((diff, files_changed)));
                }
            }
            "context_compacted" => {
                let before = event.get("messagesBefore").and_then(Value::as_u64).unwrap_or(0);
                let after = event.get("messagesAfter").and_then(Value::as_u64).unwrap_or(0);
                store.toast(store.tf("toast.compacted", &[&before.to_string(), &after.to_string()]));
            }
            "agent_spawned" => {
                let agent_id = event.get("agentId").and_then(Value::as_str).unwrap_or_default().to_string();
                let name = event.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let role = event.get("role").and_then(Value::as_str).unwrap_or("explorer").to_string();
                if agent_id.is_empty() {
                    return true;
                }
                let entry = serde_json::json!({
                    "agentId": agent_id,
                    "name": name,
                    "role": role,
                    "status": "running",
                });
                store.spawned_agents.with_mut(|agents| {
                    if let Some(existing) = agents
                        .iter_mut()
                        .find(|agent| agent.get("agentId").and_then(Value::as_str) == Some(agent_id.as_str()))
                    {
                        *existing = entry;
                    } else {
                        agents.push(entry);
                    }
                });
            }
            "run_title_changed" => {
                let title = event.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
                store.runs.with_mut(|runs| {
                    if let Some(run) = runs.iter_mut().find(|run| run.id == run_id) {
                        run.title = title.clone();
                    }
                });
                let snapshot = store.active_run.read().clone();
                if let Some(mut run) = snapshot {
                    if run.id == run_id {
                        run.title = title;
                        store.active_run.set(Some(run));
                    }
                }
            }
            "patch_progress" => {
                let complete = event.get("complete").and_then(Value::as_bool).unwrap_or(true);
                store.patch_progress.set((!complete).then(|| event.clone()));
            }
            "run_completed" | "run_failed" => {
                let run_id = event.get("runId").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| run_id.to_string());
                if !foreground {
                    store.refresh_runs();
                    return true;
                }
                store.patch_progress.set(None);
                spawn(async move {
                    let state = core();
                    if let Ok(run) = api::runs::get_run(&state, &run_id).await {
                        store.active_run.set(Some(run));
                    }
                    store.refresh_runs();
                    store.run_usage.set(api::runs::run_usage(&state, &run_id).await.ok());
                });
                store.pending_permission.set(None);
                store.pending_question.set(None);
                if let Some(message) = event.get("errorMessage").and_then(Value::as_str) {
                    store.toast(store.tf("toast.run_failed", &[message]));
                }
            }
            // `round_started`, `run_started`, unknown event types — ignored.
            _ => {}
        }
        true
    }

    /// Shows a transient toast message that auto-clears after 4 s.
    pub fn toast(&self, message: String) {
        let mut store = *self;
        store.status_toast.set(Some(message));
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            store.status_toast.set(None);
        });
    }
}

/// Whether a run status counts as "active" (event stream worth attaching).
#[allow(dead_code)]
pub fn run_is_active(status: &str) -> bool {
    matches!(
        status,
        "created" | "generating" | "awaiting_permission" | "awaiting_input"
    )
}

/// Keyboard and screen-reader basics, installed once at startup
/// (`window.lcgnA11y`):
/// - Enter / Space activate focused `[role=button]` elements (clickable rows
///   that are not native buttons).
/// - Escape closes the topmost modal or popover by clicking its backdrop.
/// - Icon-only buttons that carry a `title` get the same text as `aria-label`.
pub const A11Y_SCRIPT: &str = r#"(() => {
  if (window.lcgnA11y) return;
  window.lcgnA11y = true;
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      const all = document.querySelectorAll('.modal-backdrop, .popover-backdrop');
      if (all.length) { all[all.length - 1].click(); e.preventDefault(); }
      return;
    }
    if (e.key !== 'Enter' && e.key !== ' ') return;
    const t = e.target;
    if (t && t.getAttribute && t.getAttribute('role') === 'button' && t.tagName !== 'BUTTON') {
      e.preventDefault();
      t.click();
    }
  });
  const label = (root) => {
    const list = root.querySelectorAll ? root.querySelectorAll('button[title]:not([aria-label])') : [];
    list.forEach((b) => b.setAttribute('aria-label', b.getAttribute('title')));
  };
  label(document);
  new MutationObserver((muts) => {
    for (const m of muts) m.addedNodes.forEach((n) => { if (n.nodeType === 1) { label(n); if (n.matches && n.matches('button[title]:not([aria-label])')) n.setAttribute('aria-label', n.getAttribute('title')); } });
  }).observe(document.body, { childList: true, subtree: true });
})();"#;

/// Chat scrolling, installed once at startup (`window.lcgnChat`).
///
/// Dioxus patches the DOM after the store changes, so a scroll issued right
/// after `messages.set` runs before the new message exists. Both helpers
/// therefore wait for the DOM over a few frames instead of scrolling once.
///
/// - `pin(id)`: waits for the user message `[data-mid=id]` (falls back to the
///   last user message), scrolls it to the top of the thread, and sizes
///   `#chat-tail-spacer` so it can stay there while the reply is still short.
///   The spacer shrinks as the reply grows (Claude/ChatGPT-style).
/// - `follow()`: on streamed updates, sticks to the bottom only when nothing
///   is pinned and the reader is already near the bottom.
/// - `toBottom()`: drops any pin and keeps the thread at the bottom while the
///   freshly opened run lays out.
pub const CHAT_SCROLL_SCRIPT: &str = r#"(() => {
  if (window.lcgnChat) return;
  const box = () => document.getElementById('chat-scroll');
  const spacer = () => document.getElementById('chat-tail-spacer');
  let pinned = null;
  let token = 0;
  let queued = false;

  function lastUser() {
    const b = box();
    if (!b) return null;
    const all = b.querySelectorAll('.msg-user');
    return all.length ? all[all.length - 1] : null;
  }
  function fitSpacer() {
    const b = box(), s = spacer();
    if (!b || !s) return;
    if (!pinned || !pinned.isConnected) { pinned = null; s.style.height = '0px'; return; }
    // Height of the content from the pinned message down to the spacer.
    const below = s.getBoundingClientRect().top - pinned.getBoundingClientRect().top;
    s.style.height = Math.max(0, b.clientHeight - below - 40) + 'px';
  }
  function scheduleFit() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; fitSpacer(); });
  }
  // Whether the reader is at the bottom, tracked from scroll events so that
  // content growth (which fires none) does not flip it.
  let stuck = true;
  function watch() {
    const b = box();
    if (!b || b.dataset.lcgnWatched) return;
    b.dataset.lcgnWatched = '1';
    new MutationObserver(scheduleFit).observe(b, { childList: true, subtree: true, characterData: true });
    new ResizeObserver(scheduleFit).observe(b);
    b.addEventListener('scroll', () => {
      stuck = b.scrollHeight - b.scrollTop - b.clientHeight < 80;
    }, { passive: true });
  }
  function pinTo(el) {
    const b = box();
    if (!b || !el) return;
    pinned = el;
    watch();
    fitSpacer();
    const top = Math.max(0, b.scrollTop + el.getBoundingClientRect().top - b.getBoundingClientRect().top - 16);
    // Long smooth scrolls lag far behind; jump unless the move is short.
    const near = Math.abs(top - b.scrollTop) < b.clientHeight;
    b.scrollTo({ top, behavior: near ? 'smooth' : 'auto' });
  }
  window.lcgnChat = {
    pin(id) {
      const mine = ++token;
      let frames = 0;
      const tick = () => {
        if (mine !== token) return;
        const b = box();
        const el = b && id ? b.querySelector('.msg-user[data-mid="' + CSS.escape(id) + '"]') : null;
        if (el) return pinTo(el);
        if (++frames > 90) return pinTo(lastUser());
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    },
    follow() {
      // Streaming: leave a pinned message where it is, and never yank a
      // reader who scrolled up; otherwise stay glued to the bottom.
      if (pinned && pinned.isConnected) return;
      const b = box();
      if (!b) return;
      watch();
      if (!stuck) return;
      requestAnimationFrame(() => requestAnimationFrame(() => {
        if (stuck && !(pinned && pinned.isConnected)) b.scrollTop = b.scrollHeight;
      }));
    },
    toBottom() {
      const mine = ++token;
      pinned = null;
      stuck = true;
      let frames = 0;
      const tick = () => {
        // Stop if a newer request took over or the reader scrolled away.
        if (mine !== token || (frames > 0 && !stuck)) return;
        const b = box();
        if (b) {
          watch();
          fitSpacer();
          b.scrollTop = b.scrollHeight;
        }
        if (++frames < 20) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    },
  };
})();"#;

/// Scrolls the chat thread to the bottom once the opened run has rendered.
pub fn scroll_chat_to_bottom() {
    dioxus::document::eval("window.lcgnChat && window.lcgnChat.toBottom();");
}

/// Keeps up with streamed output (see `follow()` above).
pub fn follow_chat() {
    dioxus::document::eval("window.lcgnChat && window.lcgnChat.follow();");
}

/// Pins the just-sent user message to the top of the thread.
pub fn pin_sent_message(messages: &[RunMessage]) {
    let id = messages
        .iter()
        .rev()
        .find(|m| m.role == "user" && m.agent_role.is_none())
        .map(|m| m.id.as_str())
        .unwrap_or_default();
    let literal = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".into());
    dioxus::document::eval(&format!("window.lcgnChat && window.lcgnChat.pin({literal});"));
}

/// Formats a cost in USD for display (`$1.2345`).
pub fn fmt_cost(cost: f64) -> String {
    format!("${:.4}", cost)
}

/// Formats a token total compactly (`1.2k`, `3.4M`).
pub fn fmt_tokens(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;

    #[test]
    fn events_of_the_open_run_are_foreground() {
        assert_eq!(event_scope(Some("run-a"), "run-a"), EventScope::Foreground);
    }

    #[test]
    fn events_of_other_runs_or_no_run_are_background() {
        assert_eq!(event_scope(Some("run-b"), "run-a"), EventScope::Background);
        assert_eq!(event_scope(None, "run-a"), EventScope::Background);
    }

    #[test]
    fn background_runs_only_update_the_sidebar() {
        for kind in ["status_changed", "run_title_changed", "run_completed", "run_failed"] {
            assert!(applies_in_background(kind), "{kind}");
        }
        for kind in [
            "message_created",
            "message_updated",
            "permission_requested",
            "question_requested",
            "plan_updated",
            "turn_diff_updated",
            "agent_spawned",
            "patch_progress",
            "context_compacted",
        ] {
            assert!(!applies_in_background(kind), "{kind} must not leak into the open run");
        }
    }

    #[test]
    fn newest_stream_of_a_run_supersedes_older_ones() {
        let mut registry = StreamRegistry::new();
        let first = registry.claim("run-a");
        let other = registry.claim("run-b");
        assert!(registry.is_current("run-a", first));
        let second = registry.claim("run-a");
        assert!(!registry.is_current("run-a", first), "old listener must retire");
        assert!(registry.is_current("run-a", second));
        assert!(registry.is_current("run-b", other), "other runs are untouched");
        registry.release("run-a", first);
        assert!(registry.is_current("run-a", second), "a retired stream cannot release the new one");
        registry.release("run-a", second);
        assert!(!registry.is_current("run-a", second));
    }
}
