//! Typed in-process API of the Locagens core; no HTTP involved; used by the
//! desktop UI (`apps/desktop-rs`) and the `lcgn` CLI.

pub mod error;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod plugins;
pub mod processes;
pub mod projects;
pub mod providers;
pub mod runs;
pub mod security;
pub mod skills;
pub mod symbols;

pub use error::{ApiError, ApiErrorKind, ApiResult};

/// Run events are JSON objects with a `"type"` field — the same payloads the
/// SSE endpoint streams.
pub type RunEvent = serde_json::Value;
/// Receiving half of a run event broadcast channel.
pub type EventRx = tokio::sync::broadcast::Receiver<RunEvent>;

/// Opens the data store and builds an `AppState` (what `main.rs` does before
/// serving HTTP); the desktop UI calls this once at startup.
pub fn bootstrap() -> anyhow::Result<crate::AppState> {
    let core = crate::build_core()?;
    core.db.run_startup_tasks(&core.default_project_path.clone(), "Locagens");
    Ok(core.into_app_state(std::sync::Arc::new(crate::services::processes::ProcessManager::new())))
}
