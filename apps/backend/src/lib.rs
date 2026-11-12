//! Locagens core library — the shared Rust core behind both binaries
//! (`locagens-backend` HTTP server and `lcgn` CLI).
//!
//! `build_core()` resolves the file-based data dir, opens the `Db`, and
//! builds the `PiProviderManager` (`~/.pi/agent/auth.json`, `models.json`,
//! `models-store.json`) plus the in-process orchestrator. No `pi` process is
//! ever spawned.

pub mod api;
pub mod auth;
pub mod cli;
pub mod db;
pub mod orchestrator;
pub mod paths;
pub mod pi_providers;
pub mod routes;
pub mod services;
pub mod state;
pub mod util;

/// Re-exported so route modules can keep referring to `crate::AppState`.
pub use state::AppState;

use std::path::PathBuf;
use std::sync::Arc;

/// Default HTTP port of the backend (settings.json / PORT env override it).
pub const DEFAULT_PORT: u16 = 4321;

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
    /// Wraps the core into a full `AppState` (adds process manager + token).
    pub fn into_app_state(self, processes: Arc<services::processes::ProcessManager>) -> AppState {
        AppState {
            db: self.db,
            pi: self.pi,
            orchestrator: self.orchestrator,
            settings_path: self.settings_path,
            default_project_path: self.default_project_path,
            processes,
            auth_token: std::env::var("LOCAGENS_API_TOKEN")
                .ok()
                .filter(|token| !token.is_empty()),
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
    let default_project_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Ok(Core {
        db: Arc::new(database),
        pi: Arc::new(pi_providers::PiProviderManager::new()),
        orchestrator: Arc::new(orchestrator::Orchestrator::new()),
        settings_path,
        default_project_path,
    })
}
