//! Last-used model / effort / mode facade (`services::ui_state`,
//! `~/.locagens/ui_state.json`), shared by the desktop app and the TUI.

use crate::api::error::{ApiError, ApiResult};
use crate::services::ui_state::{self, UiState};

/// What the user picked last (empty when nothing is remembered).
pub fn load_ui_state() -> UiState {
    ui_state::load(&ui_state::default_path())
}

/// Remembers a model pick; `effort` is kept as well when given.
pub fn remember_model(provider_id: &str, model: &str, effort: Option<&str>) -> ApiResult<()> {
    ui_state::remember_model(&ui_state::default_path(), provider_id, model, effort).map_err(write_error)
}

/// Remembers a reasoning-effort pick.
pub fn remember_effort(effort: &str) -> ApiResult<()> {
    ui_state::remember_effort(&ui_state::default_path(), effort).map_err(write_error)
}

/// Remembers a mode pick (`chat`, `plan`, `accept_edits`, `ask_permissions`,
/// `auto`); anything else is a bad request.
pub fn remember_mode(mode: &str) -> ApiResult<()> {
    ui_state::remember_mode(&ui_state::default_path(), mode).map_err(write_error)
}

fn write_error(err: std::io::Error) -> ApiError {
    if err.kind() == std::io::ErrorKind::InvalidInput {
        ApiError::bad_request(err.to_string())
    } else {
        ApiError::internal(format!("Could not save ui_state.json: {err}"))
    }
}
