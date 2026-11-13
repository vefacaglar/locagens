//! Locagens core library — the shared Rust core behind both binaries
//! (the `lcgn` CLI and the native Dioxus desktop app).
//!
//! `build_core()` resolves the file-based data dir, opens the `Db`, and
//! builds the `PiProviderManager` (`~/.pi/agent/auth.json`, `models.json`,
//! `models-store.json`) plus the in-process orchestrator. No `pi` process is
//! ever spawned and there is no HTTP server — consumers call the typed
//! in-process facade in `api` directly.

pub mod api;
pub mod cli;
pub mod config_runtime;
pub mod db;
pub mod orchestrator;
pub mod paths;
pub mod pi_providers;
pub mod services;
pub mod state;
pub mod util;

/// Shared protocol types (review findings etc.) for the UIs.
pub use locagens_protocol as protocol;

/// Re-exported so consumers can keep referring to `crate::AppState`.
pub use state::AppState;

use std::path::PathBuf;
use std::sync::Arc;

/// Shared bootstrap built once per process and reused by both binaries.
pub struct Core {
    pub db: Arc<db::Db>,
    pub pi: Arc<pi_providers::PiProviderManager>,
    pub orchestrator: Arc<orchestrator::Orchestrator>,
    pub settings_path: PathBuf,
    /// Directory the process considers "current project" (cwd at startup).
    pub default_project_path: PathBuf,
}

impl Core {
    /// Wraps the core into a full `AppState` (adds process manager).
    pub fn into_app_state(self, processes: Arc<services::processes::ProcessManager>) -> AppState {
        AppState {
            db: self.db,
            pi: self.pi,
            orchestrator: self.orchestrator,
            settings_path: self.settings_path,
            default_project_path: self.default_project_path,
            processes,
        }
    }
}

/// Resolves the data dir, opens the store and builds the provider manager
/// and orchestrator. The caller decides what to do with the pieces.
pub fn build_core() -> anyhow::Result<Core> {
    let settings_path = paths::default_settings_path();
    let data_root = db::resolve_data_root();
    let database = db::Db::open(&data_root).map_err(|err| {
        anyhow::anyhow!(
            "Failed to open data store at {}: {err}",
            data_root.display()
        )
    })?;
    config_runtime::init_from_disk();
    let default_project_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Ok(Core {
        db: Arc::new(database),
        pi: Arc::new(pi_providers::PiProviderManager::new()),
        orchestrator: Arc::new(orchestrator::Orchestrator::new()),
        settings_path,
        default_project_path,
    })
}
