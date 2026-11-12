//! Shared application state handed to HTTP handlers and the in-process CLI.

use crate::db;
use crate::pi_providers;
use crate::services::processes::ProcessManager;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared application state passed to every handler.
pub struct AppState {
    pub db: Arc<db::Db>,
    pub pi: Arc<pi_providers::PiProviderManager>,
    pub orchestrator: Arc<crate::orchestrator::Orchestrator>,
    pub settings_path: PathBuf,
    pub default_project_path: PathBuf,
    pub processes: Arc<ProcessManager>,
}
