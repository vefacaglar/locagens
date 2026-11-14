//! Reactive UI state of the desktop app: the `UiStore` bundle of Dioxus
//! signals plus the helpers that load data from `locagens_core::api` and
//! process live run-event streams.

use dioxus::prelude::*;
use crate::i18n::{self, Lang};
use crate::prefs::{self, Appearance, ThemeChoice};
use locagens_core::api::{self, EventRx};
use locagens_core::db::{ImageAttachment, Plan, Project, Run, RunMessage};
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
    /// Models & accounts: API keys, sign-ins and per-model switches (UX-001).
    Providers,
    Permissions,
    Memory,
    Skills,
    Mcp,
    Plugins,
    Security,
    Appearance,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 8] = [
        SettingsTab::Providers,
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

/// `catalog` minus hidden `provider/model` ids; providers with no model left
/// are dropped.
pub fn visible_catalog(
    catalog: &[ProviderMetadata],
    hidden: &std::collections::HashSet<String>,
) -> Vec<ProviderMetadata> {
    catalog
        .iter()
        .filter_map(|provider| {
            let mut provider = provider.clone();
            provider.models.retain(|model| !hidden.contains(&format!("{}/{model}", provider.id)));
            (!provider.models.is_empty()).then_some(provider)
        })
        .collect()
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
/// touch the sidebar list (status dot, title) or raise a toast that the run
/// waits for the user (FR-054).
pub fn applies_in_background(kind: &str) -> bool {
    matches!(
        kind,
        "status_changed"
            | "run_title_changed"
            | "run_completed"
            | "run_failed"
            | "permission_requested"
            | "question_requested"
            | "queued_messages_undelivered"
    )
}

/// Whether a permission card still shows the request `answered` (same prompt
/// id when the core sends one, else the same payload). Answering clears the
/// card only then, so the next queued prompt that already arrived survives
/// (FR-048).
pub fn is_same_request(shown: Option<&Value>, answered: &Value) -> bool {
    let Some(shown) = shown else { return false };
    match (shown.get("promptId"), answered.get("promptId")) {
        (Some(a), Some(b)) => a == b,
        _ => shown == answered,
    }
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

/// One toast line; each has its own timer, so a later toast never cuts an
/// earlier one short (FR-062).
#[derive(Clone, Debug, PartialEq)]
pub struct ToastItem {
    pub id: u64,
    pub text: String,
}

/// Toasts shown at once; the oldest goes first when a new one arrives.
pub const MAX_TOASTS: usize = 3;

/// Appends `item`, dropping the oldest entries beyond `MAX_TOASTS`.
pub fn push_toast(list: &mut Vec<ToastItem>, item: ToastItem) {
    list.push(item);
    let overflow = list.len().saturating_sub(MAX_TOASTS);
    list.drain(..overflow);
}

/// How long a toast stays: 4 s, plus reading time for long text (max 12 s).
pub fn toast_duration(text: &str) -> std::time::Duration {
    let extra = text.chars().count().saturating_sub(60) as u64 * 40;
    std::time::Duration::from_millis((4000 + extra).min(12_000))
}

/// A message queued on a generating run, shown above the composer until the
/// model picks it up (UX-011).
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedMessage {
    pub run_id: String,
    pub text: String,
}

/// Drops the first queued entry of `run_id` that the model just received as
/// user message `content`; `true` when one matched.
pub fn take_delivered(queue: &mut Vec<QueuedMessage>, run_id: &str, content: &str) -> bool {
    let content = content.trim();
    match queue.iter().position(|queued| queued.run_id == run_id && queued.text.trim() == content) {
        Some(index) => {
            queue.remove(index);
            true
        }
        None => false,
    }
}

/// The run's undelivered queued texts, removed from `queue` and joined by
/// blank lines (put back into the composer when the run stops).
pub fn take_undelivered(queue: &mut Vec<QueuedMessage>, run_id: &str) -> Option<String> {
    let (mine, rest): (Vec<_>, Vec<_>) = queue.drain(..).partition(|queued| queued.run_id == run_id);
    *queue = rest;
    let texts: Vec<String> = mine.into_iter().map(|queued| queued.text).collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

/// The texts of a `queued_messages_undelivered` event (FR-010).
pub fn undelivered_texts(event: &Value) -> Vec<String> {
    event
        .get("messages")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).filter(|text| !text.trim().is_empty()).collect())
        .unwrap_or_default()
}

/// Full-window overlays opened from the keyboard layer (UX-019).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// Command palette; `chats_only` opens it on the chat list (`/sessions`).
    Palette { chats_only: bool },
    /// Shortcuts and commands sheet (`Ctrl+/`, `/help`).
    Shortcuts,
}

/// Modes the desktop offers; others (`ask_permissions`) are CLI-only.
pub const DESKTOP_MODES: [&str; 4] = ["accept_edits", "plan", "auto", "chat"];

/// `Ctrl+.`: Build and Plan alternate; Full Access and Chat step back to
/// Build, so the keyboard never switches prompts off by accident (UX-012).
pub fn next_mode(mode: &str) -> &'static str {
    match mode {
        "accept_edits" => "plan",
        _ => "accept_edits",
    }
}

/// i18n key of a mode's label.
pub fn mode_key(mode: &str) -> &'static str {
    match mode {
        "plan" => "mode.plan",
        "auto" => "mode.auto",
        "chat" => "mode.chat",
        _ => "mode.build",
    }
}

/// The model to select at start: the remembered `provider/model` when the
/// catalog still offers it, else the first visible model.
pub fn default_model(catalog: &[ProviderMetadata], remembered: Option<(&str, &str)>) -> Option<(String, String)> {
    if let Some((provider, model)) = remembered {
        let offered = catalog.iter().any(|p| p.id == provider && p.models.iter().any(|m| m == model));
        if offered {
            return Some((provider.to_string(), model.to_string()));
        }
    }
    let first = catalog.first()?;
    Some((first.id.clone(), first.models.first()?.clone()))
}

/// Bundle of every reactive signal, handed to components via context.
/// `Copy` so event-handler closures can capture it freely.
#[derive(Clone, Copy)]
pub struct UiStore {
    /// Persisted appearance preferences (language, theme, fonts, size).
    pub appearance: Signal<Appearance>,
    /// Live OS dark-mode state (only consulted for `ThemeChoice::System`).
    pub system_dark: Signal<bool>,
    /// Persisted widths of the sidebar and the right panel.
    pub layout: Signal<prefs::Layout>,
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
    /// Context-window fill of the active run (inspector "Context").
    pub context_usage: Signal<Option<api::runs::ContextUsage>>,
    pub pending_permission: Signal<Option<Value>>,
    pub pending_question: Signal<Option<Value>>,
    pub providers_catalog: Signal<Vec<ProviderMetadata>>,
    /// Whether the catalog has loaded once (the first-run panel waits for it).
    pub providers_loaded: Signal<bool>,
    /// `provider/model` ids switched off in Settings → Providers.
    pub hidden_models: Signal<std::collections::HashSet<String>>,
    /// Bumped when usage lands or a run ends; the usage page reloads on it.
    pub usage_epoch: Signal<u64>,
    pub selected_provider: Signal<String>,
    pub selected_model: Signal<String>,
    pub selected_effort: Signal<String>,
    pub selected_mode: Signal<String>,
    pub right_panel_open: Signal<bool>,
    pub right_panel_tab: Signal<RightPanelTab>,
    /// Visible toasts, oldest first.
    pub toasts: Signal<Vec<ToastItem>>,
    pub prompt: Signal<String>,
    /// Images pasted into the composer, sent with the next message.
    pub pending_images: Signal<Vec<ImageAttachment>>,
    /// Whether the left sidebar is shown (`Ctrl+B`, the toolbar toggle).
    pub sidebar_open: Signal<bool>,
    /// Messages queued on generating runs, not yet picked up.
    pub queued: Signal<Vec<QueuedMessage>>,
    /// Queued messages a background run could not deliver (FR-010); they go
    /// back into the composer when that run is opened.
    pub undelivered: Signal<Vec<QueuedMessage>>,
    /// Palette / shortcuts overlay.
    pub overlay: Signal<Overlay>,
    /// Whether the composer's model picker is open (`Ctrl+M`, `/model`).
    pub model_picker_open: Signal<bool>,
    /// Prompts sent from this window, newest last (`Up` recalls them).
    pub prompt_history: Signal<Vec<String>>,
    /// Text of the assertive live region (a card that blocks the run).
    pub announcement: Signal<String>,
    /// Aggregate workspace diff of the current turn: `(diff, files_changed)`.
    pub turn_diff: Signal<Option<(String, usize)>>,
    /// Live `spawn_agent` / `spawn_agents` registrations not yet visible as
    /// messages (Codex-style background agents).
    pub spawned_agents: Signal<Vec<Value>>,
    /// Latest `patch_progress` event while the model streams an `apply_patch`
    /// call (files + line counts); cleared when the patch is complete.
    pub patch_progress: Signal<Option<Value>>,
}

/// Creates every `UiStore` signal (a hook: call it once, from the root
/// component; the end-to-end test builds its store the same way).
pub fn use_ui_store() -> UiStore {
    // UX-003 / FR-057: reopen on the last effort and mode (the model is
    // matched against the catalog once it loads).
    let remembered = use_hook(api::ui_state::load_ui_state);
    let effort = remembered.effort.clone().unwrap_or_else(|| "medium".to_string());
    let mode = remembered
        .mode
        .clone()
        .filter(|mode| DESKTOP_MODES.contains(&mode.as_str()))
        .unwrap_or_else(|| "accept_edits".to_string());
    UiStore {
        appearance: use_signal(prefs::load),
        layout: use_signal(prefs::load_layout),
        system_dark: use_signal(|| true),
        page: use_signal(|| AppPage::Chat),
        projects: use_signal(Vec::new),
        selected_project: use_signal(|| None),
        runs: use_signal(Vec::new),
        search_query: use_signal(String::new),
        active_run_id: use_signal(|| None),
        active_run: use_signal(|| None),
        messages: use_signal(Vec::new),
        plan: use_signal(|| None),
        run_usage: use_signal(|| None),
        context_usage: use_signal(|| None),
        pending_permission: use_signal(|| None),
        pending_question: use_signal(|| None),
        providers_catalog: use_signal(Vec::new),
        providers_loaded: use_signal(|| false),
        hidden_models: use_signal(Default::default),
        usage_epoch: use_signal(|| 0),
        selected_provider: use_signal(String::new),
        selected_model: use_signal(String::new),
        selected_effort: use_signal(|| effort),
        selected_mode: use_signal(|| mode),
        right_panel_open: use_signal(|| false),
        right_panel_tab: use_signal(|| RightPanelTab::Plan),
        toasts: use_signal(Vec::new),
        prompt: use_signal(String::new),
        pending_images: use_signal(Vec::new),
        sidebar_open: use_signal(|| true),
        queued: use_signal(Vec::new),
        undelivered: use_signal(Vec::new),
        overlay: use_signal(|| Overlay::None),
        model_picker_open: use_signal(|| false),
        prompt_history: use_signal(Vec::new),
        announcement: use_signal(String::new),
        turn_diff: use_signal(|| None),
        spawned_agents: use_signal(Vec::new),
        patch_progress: use_signal(|| None),
    }
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
        spawn(async move {
            store.load_providers().await;
            store.refresh_model_lists().await;
        });
    }

    /// Re-fetches stale provider `/models` lists in the background and
    /// reloads the catalog when one changed.
    async fn refresh_model_lists(&self) {
        if api::providers::refresh_model_lists(&core()).await > 0 {
            self.load_providers().await;
        }
    }

    /// Loads the provider catalog and, when nothing is selected yet or the
    /// selection is no longer offered (a refreshed list dropped it), picks
    /// the remembered model (UX-003) or else the first visible one.
    async fn load_providers(&self) {
        let mut store = *self;
        let Ok(providers) = api::providers::list_providers(&core()).await else { return };
        store.hidden_models.set(api::providers::hidden_models(&core()));
        store.providers_catalog.set(providers);
        store.providers_loaded.set(true);
        let catalog = store.visible_catalog();
        let still_offered = {
            let (provider, model) = (store.selected_provider.peek(), store.selected_model.peek());
            catalog.iter().any(|p| p.id == *provider && p.models.iter().any(|m| m == &*model))
        };
        if still_offered {
            return;
        }
        let remembered = api::ui_state::load_ui_state();
        if let Some((provider, model)) = default_model(&catalog, remembered.provider_and_model()) {
            store.selected_provider.set(provider);
            store.selected_model.set(model);
        }
    }

    /// The user picked a model (and maybe an effort): select and remember it.
    pub fn pick_model(&self, provider_id: &str, model: &str, effort: Option<&str>) {
        let mut store = *self;
        store.selected_provider.set(provider_id.to_string());
        store.selected_model.set(model.to_string());
        if let Some(effort) = effort {
            store.selected_effort.set(effort.to_string());
        }
        let _ = api::ui_state::remember_model(provider_id, model, effort);
    }

    /// The user picked a reasoning effort.
    pub fn pick_effort(&self, effort: &str) {
        let mut store = *self;
        store.selected_effort.set(effort.to_string());
        let _ = api::ui_state::remember_effort(effort);
    }

    /// The user picked a mode; the mode control itself shows the change.
    pub fn pick_mode(&self, mode: &str) {
        let mut store = *self;
        store.selected_mode.set(mode.to_string());
        let _ = api::ui_state::remember_mode(mode);
    }

    /// The folder the chat works in: the open run's, else the selected
    /// project (FR-050). Every project-scoped action uses this.
    pub fn current_project(&self) -> Option<String> {
        self.active_run
            .read()
            .as_ref()
            .map(|run| run.project_path.clone())
            .filter(|path| !path.is_empty())
            .or_else(|| self.selected_project.read().clone())
    }

    /// [`Self::current_project`] without subscribing (for background tasks).
    pub fn current_project_peek(&self) -> Option<String> {
        self.active_run
            .peek()
            .as_ref()
            .map(|run| run.project_path.clone())
            .filter(|path| !path.is_empty())
            .or_else(|| self.selected_project.peek().clone())
    }

    /// Clears the open conversation (new chat, `/new`, a deleted run).
    pub fn reset_conversation(&self) {
        let mut store = *self;
        store.active_run_id.set(None);
        store.active_run.set(None);
        store.messages.set(Vec::new());
        store.plan.set(None);
        store.run_usage.set(None);
        store.context_usage.set(None);
        store.pending_permission.set(None);
        store.pending_question.set(None);
        store.turn_diff.set(None);
        store.spawned_agents.set(Vec::new());
        store.patch_progress.set(None);
    }

    /// Writes `text` into the assertive live region (screen readers).
    pub fn announce(&self, text: String) {
        let mut store = *self;
        store.announcement.set(text);
    }

    /// The provider catalog without hidden models; providers left with no
    /// visible model are dropped (the model pickers use this).
    pub fn visible_catalog(&self) -> Vec<ProviderMetadata> {
        visible_catalog(&self.providers_catalog.read(), &self.hidden_models.read())
    }

    /// Switches one model on or off and persists it through the api facade.
    pub fn set_model_hidden(&self, provider_id: &str, model: &str, hidden: bool) {
        let mut store = *self;
        if api::providers::set_model_hidden(&core(), provider_id, model, hidden).is_err() {
            store.toast(store.t("toast.model_visibility_failed").to_string());
            return;
        }
        store.hidden_models.set(api::providers::hidden_models(&core()));
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
            store.refresh_model_lists().await;
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
    ///
    /// The stream is subscribed BEFORE the messages are read (FR-056): events
    /// that land while loading wait in the receiver and are applied after
    /// the load, so nothing between the two is lost.
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
            let run = api::runs::get_run(&state, &run_id).await.ok();
            let live = run.as_ref().is_some_and(|run| run_is_active(&run.status));
            let rx = if live { Some(api::runs::subscribe_run_events(&state, &run_id).await) } else { None };
            let messages = api::runs::list_messages(&state, &run_id).await.unwrap_or_default();
            let plan = api::runs::run_plan(&state, &run_id).await.unwrap_or(None);
            let usage = api::runs::run_usage(&state, &run_id).await.ok();
            let context = api::runs::context_usage(&state, &run_id).await.ok();
            let pending = api::runs::run_pending(&state, &run_id).await.ok();
            if event_scope(store.active_run_id.peek().as_deref(), &run_id) == EventScope::Background {
                return;
            }
            if let Some(run) = run {
                // FR-050: the project follows the opened conversation.
                if !run.project_path.is_empty() {
                    store.selected_project.set(Some(run.project_path.clone()));
                }
                store.adopt_run_selection(&run);
                store.active_run.set(Some(run));
            }
            if let Some(text) = store.undelivered.with_mut(|list| take_undelivered(list, &run_id)) {
                store.restore_to_composer(text);
            }
            store.messages.set(messages);
            store.plan.set(plan);
            store.run_usage.set(usage);
            store.context_usage.set(context);
            if let Some(pending) = pending {
                store.pending_permission.set(pending.permission_request);
                store.pending_question.set(pending.question_request);
            }
            scroll_chat_to_bottom();
            if let Some(rx) = rx {
                store.attach_event_stream(run_id, rx);
            }
        });
    }

    /// FR-057: continuing a conversation keeps its model, effort and mode
    /// unless the user picks others. Review runs keep the composer's mode.
    /// A model the loaded catalog no longer offers is not adopted, so the
    /// next message does not go to a model the provider dropped.
    fn adopt_run_selection(&self, run: &Run) {
        let mut store = *self;
        let offered = !*store.providers_loaded.peek()
            || store
                .visible_catalog()
                .iter()
                .any(|p| p.id == run.provider_id && p.models.iter().any(|m| m == &run.model));
        if !run.provider_id.is_empty() && !run.model.is_empty() && offered {
            store.selected_provider.set(run.provider_id.clone());
            store.selected_model.set(run.model.clone());
        }
        if let Some(effort) = run.reasoning_effort.clone().filter(|effort| !effort.is_empty()) {
            store.selected_effort.set(effort);
        }
        let review = locagens_core::orchestrator::review::is_review_run(run);
        if !review && DESKTOP_MODES.contains(&run.mode.as_str()) {
            store.selected_mode.set(run.mode.clone());
        }
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
        // New usage anywhere (any run, any agent) makes the usage page stale.
        if matches!(kind.as_str(), "usage_recorded" | "run_completed" | "run_failed") {
            store.usage_epoch.with_mut(|epoch| *epoch += 1);
        }
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
                        // A queued message reached the model: drop it from the strip.
                        if message.role == "user" && message.agent_role.is_none() {
                            store.queued.with_mut(|queue| take_delivered(queue, run_id, &message.content));
                        }
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
            "permission_requested" | "question_requested" if !foreground => {
                // FR-054: a run left in the background now waits for the user.
                let title = store
                    .runs
                    .peek()
                    .iter()
                    .find(|run| run.id == run_id)
                    .map(|run| run.title.clone())
                    .unwrap_or_default();
                store.toast(store.tf("toast.background_waiting", &[&title]));
            }
            "permission_requested" => {
                store.announce(store.t("a11y.permission_needed").to_string());
                store.pending_permission.set(Some(event.clone()));
            }
            "question_requested" => {
                store.announce(store.t("a11y.question_needed").to_string());
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
            // A model call was accounted: refresh cost/tokens and context fill.
            "usage_recorded" => {
                if foreground {
                    store.reload_usage(run_id);
                }
            }
            "patch_progress" => {
                let complete = event.get("complete").and_then(Value::as_bool).unwrap_or(true);
                store.patch_progress.set((!complete).then(|| event.clone()));
            }
            "run_completed" | "run_failed" => {
                let run_id = event.get("runId").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| run_id.to_string());
                if !foreground {
                    store.queued.with_mut(|queue| take_undelivered(queue, &run_id));
                    store.refresh_runs();
                    return true;
                }
                store.patch_progress.set(None);
                store.reload_usage(&run_id);
                let reload_id = run_id.clone();
                spawn(async move {
                    let state = core();
                    let run = api::runs::get_run(&state, &reload_id).await;
                    // FR-056: another run may have been opened meanwhile.
                    if let Ok(run) = run {
                        if store.active_run_id.peek().as_deref() == Some(run.id.as_str()) {
                            store.active_run.set(Some(run));
                        }
                    }
                    store.refresh_runs();
                });
                store.pending_permission.set(None);
                store.pending_question.set(None);
                // UX-011: undelivered queued text goes back into the composer.
                if let Some(text) = store.queued.with_mut(|queue| take_undelivered(queue, &run_id)) {
                    store.restore_to_composer(text);
                }
                // A failure is shown as a block at the end of the thread
                // (UX-008), read from the run's `error_message`.
            }
            "queued_messages_undelivered" => {
                // FR-010: the core's list of queued messages the turn never
                // delivered replaces the strip's guess.
                let texts = undelivered_texts(&event);
                store.queued.with_mut(|queue| queue.retain(|queued| queued.run_id != run_id));
                if texts.is_empty() {
                    return true;
                }
                if foreground {
                    store.restore_to_composer(texts.join("\n\n"));
                } else {
                    let entries = texts.into_iter().map(|text| QueuedMessage { run_id: run_id.to_string(), text });
                    store.undelivered.with_mut(|list| list.extend(entries));
                }
            }
            // `round_started`, `run_started`, unknown event types — ignored.
            _ => {}
        }
        true
    }

    /// Puts undelivered queued text in front of the composer's draft.
    fn restore_to_composer(&self, text: String) {
        let mut store = *self;
        store.prompt.with_mut(|prompt| {
            *prompt = if prompt.trim().is_empty() { text } else { format!("{text}\n\n{prompt}") };
        });
        store.toast(store.t("toast.queued_restored").to_string());
    }

    /// Reloads the usage summary and context fill of `run_id` (dropped when
    /// another run was selected meanwhile).
    pub fn reload_usage(&self, run_id: &str) {
        let mut store = *self;
        let run_id = run_id.to_string();
        spawn(async move {
            let state = core();
            let usage = api::runs::run_usage(&state, &run_id).await.ok();
            let context = api::runs::context_usage(&state, &run_id).await.ok();
            if store.active_run_id.peek().as_deref() != Some(run_id.as_str()) {
                return;
            }
            store.run_usage.set(usage);
            store.context_usage.set(context);
        });
    }

    /// Shows a transient toast. Each toast has its own timer (longer for
    /// long text) and only removes itself (FR-062).
    pub fn toast(&self, message: String) {
        static NEXT_TOAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut store = *self;
        let id = NEXT_TOAST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let duration = toast_duration(&message);
        store.toasts.with_mut(|list| push_toast(list, ToastItem { id, text: message }));
        spawn(async move {
            tokio::time::sleep(duration).await;
            store.toasts.with_mut(|list| list.retain(|toast| toast.id != id));
        });
    }

    /// Drops one toast now (its close button).
    pub fn dismiss_toast(&self, id: u64) {
        let mut store = *self;
        store.toasts.with_mut(|list| list.retain(|toast| toast.id != id));
    }
}

/// Moves keyboard focus into the composer (after opening a chat, starting a
/// new one or answering a card).
pub fn focus_composer() {
    dioxus::document::eval("requestAnimationFrame(() => { const t = document.querySelector('.composer textarea'); if (t && !t.disabled) t.focus(); });");
}

/// Whether a run status counts as "active" (event stream worth attaching).
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
    // Height of the content from the pinned message down to the spacer;
    // the dock covers the bottom of the box.
    const below = s.getBoundingClientRect().top - pinned.getBoundingClientRect().top;
    s.style.height = Math.max(0, b.clientHeight - dockHeight() - below - 40) + 'px';
  }
  function dockHeight() {
    const d = document.getElementById('chat-dock');
    return d ? d.offsetHeight : 0;
  }
  // The thread's bottom padding follows the dock's height (composer growth,
  // permission cards), so the last message is never hidden under it.
  function watchDock() {
    const d = document.getElementById('chat-dock');
    if (!d || d.dataset.lcgnWatched) return;
    d.dataset.lcgnWatched = '1';
    const apply = () => {
      const area = d.parentElement;
      if (area) area.style.setProperty('--dock-h', d.offsetHeight + 'px');
      scheduleFit();
    };
    new ResizeObserver(apply).observe(d);
    apply();
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
    watchDock();
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
        for kind in ["status_changed", "run_title_changed", "run_completed", "run_failed", "permission_requested", "question_requested"] {
            assert!(applies_in_background(kind), "{kind}");
        }
        for kind in [
            "message_created",
            "message_updated",
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
    fn answering_clears_only_the_answered_card() {
        let first = serde_json::json!({ "runId": "r", "promptId": 1, "toolCall": { "id": "a" } });
        let next = serde_json::json!({ "runId": "r", "promptId": 2, "toolCall": { "id": "a" } });
        assert!(is_same_request(Some(&first), &first));
        assert!(!is_same_request(Some(&next), &first), "the next queued prompt stays");
        assert!(!is_same_request(None, &first));
        // Without ids (older cores) the payload decides.
        let legacy = serde_json::json!({ "runId": "r", "toolCall": { "id": "a" } });
        assert!(is_same_request(Some(&legacy), &legacy.clone()));
        assert!(!is_same_request(Some(&legacy), &serde_json::json!({ "runId": "r", "toolCall": { "id": "b" } })));
    }

    #[test]
    fn toasts_queue_and_cap() {
        let mut list = Vec::new();
        for id in 1..=5 {
            push_toast(&mut list, ToastItem { id, text: format!("t{id}") });
        }
        assert_eq!(list.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(toast_duration("short"), std::time::Duration::from_secs(4));
        assert!(toast_duration(&"x".repeat(200)) > std::time::Duration::from_secs(4));
        assert_eq!(toast_duration(&"x".repeat(5000)), std::time::Duration::from_secs(12));
    }

    #[test]
    fn undelivered_event_lists_its_texts() {
        let event = serde_json::json!({"type": "queued_messages_undelivered", "messages": ["one", " ", "two"]});
        assert_eq!(undelivered_texts(&event), vec!["one".to_string(), "two".to_string()]);
        assert!(undelivered_texts(&serde_json::json!({"type": "x"})).is_empty());
    }

    #[test]
    fn queued_messages_are_dropped_when_delivered_and_restored_when_not() {
        let mut queue = vec![
            QueuedMessage { run_id: "a".into(), text: "first".into() },
            QueuedMessage { run_id: "b".into(), text: "other".into() },
            QueuedMessage { run_id: "a".into(), text: "second".into() },
        ];
        assert!(take_delivered(&mut queue, "a", " first "));
        assert!(!take_delivered(&mut queue, "a", "other"), "another run's entry is not touched");
        assert_eq!(take_undelivered(&mut queue, "a").as_deref(), Some("second"));
        assert_eq!(queue, vec![QueuedMessage { run_id: "b".into(), text: "other".into() }]);
        assert_eq!(take_undelivered(&mut queue, "a"), None);
    }

    #[test]
    fn mode_cycle_never_enters_full_access() {
        assert_eq!(next_mode("accept_edits"), "plan");
        assert_eq!(next_mode("plan"), "accept_edits");
        assert_eq!(next_mode("auto"), "accept_edits");
        assert_eq!(next_mode("chat"), "accept_edits");
        assert_eq!(mode_key("chat"), "mode.chat");
    }

    #[test]
    fn default_model_prefers_the_remembered_one_when_offered() {
        let catalog = vec![
            ProviderMetadata { id: "a".into(), display_name: "A".into(), kind: "x".into(), models: vec!["a1".into()], model_settings: None },
            ProviderMetadata { id: "b".into(), display_name: "B".into(), kind: "x".into(), models: vec!["b1".into(), "b/2".into()], model_settings: None },
        ];
        assert_eq!(default_model(&catalog, Some(("b", "b/2"))), Some(("b".into(), "b/2".into())));
        assert_eq!(default_model(&catalog, Some(("b", "gone"))), Some(("a".into(), "a1".into())));
        assert_eq!(default_model(&catalog, None), Some(("a".into(), "a1".into())));
        assert_eq!(default_model(&[], None), None);
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
