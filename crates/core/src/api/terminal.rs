//! Interactive terminal facade: the user's own shell on a pty, one session
//! per desktop Terminal tab (`services::terminal`). Not an agent tool, so
//! nothing here is permission-gated.

use crate::api::error::{ApiError, ApiResult};
use crate::services::terminal::TerminalSessions;
use std::path::Path;

pub use crate::services::terminal::{TerminalColor, TerminalInfo, TerminalScreen, TerminalSpan};

/// Starts the default shell in `project_dir` on a `cols` x `rows` grid and
/// returns the session id.
pub fn open(project_dir: &str, cols: u16, rows: u16) -> ApiResult<String> {
    let dir = Path::new(project_dir.trim());
    if project_dir.trim().is_empty() || !dir.is_dir() {
        return Err(ApiError::bad_request(format!("Not a folder: {project_dir}")));
    }
    TerminalSessions::global()
        .open(dir, cols, rows)
        .map_err(|err| ApiError::internal(format!("Could not start the shell: {err}")))
}

/// Open sessions of `project_dir` (all sessions for `None`), oldest first.
pub fn list(project_dir: Option<&str>) -> Vec<TerminalInfo> {
    TerminalSessions::global().list(project_dir.map(|dir| Path::new(dir.trim())))
}

/// Sends input bytes (keys, pastes) to the shell.
pub fn write(id: &str, bytes: &[u8]) -> ApiResult<()> {
    if TerminalSessions::global().write(id, bytes) {
        Ok(())
    } else {
        Err(ApiError::not_found(format!("Unknown terminal: {id}")))
    }
}

/// Resizes the session's pty and screen.
pub fn resize(id: &str, cols: u16, rows: u16) -> ApiResult<()> {
    TerminalSessions::global()
        .resize(id, cols, rows)
        .map_err(|err| ApiError::not_found(err.to_string()))
}

/// Scrolls the view into history (`lines > 0`) or back toward the live screen.
pub fn scroll(id: &str, lines: i32) {
    TerminalSessions::global().scroll(id, lines);
}

/// The visible screen; `None` once the session is closed.
pub fn snapshot(id: &str) -> Option<TerminalScreen> {
    TerminalSessions::global().snapshot(id)
}

/// Screen version; it changes whenever a redraw is due.
pub fn version(id: &str) -> Option<u64> {
    TerminalSessions::global().version(id)
}

/// Wakes on every screen change (`changed().await`); errors once closed.
pub fn subscribe(id: &str) -> Option<tokio::sync::watch::Receiver<u64>> {
    TerminalSessions::global().subscribe(id)
}

/// Kills the shell and frees its pty.
pub fn close(id: &str) -> bool {
    TerminalSessions::global().close(id)
}

/// Kills every shell; call on app exit.
pub fn close_all() {
    TerminalSessions::global().close_all();
}
