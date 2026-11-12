//! Small shared helpers: timestamps, canonical paths and JSON error responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Current UTC time in the same shape as JS `new Date().toISOString()`.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Uniform error body: `{ "error": "..." }`.
pub fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// The filesystem root of a path via its last ancestor component.
pub fn filesystem_root(path: &Path) -> PathBuf {
    path.ancestors().last().unwrap_or(path).to_path_buf()
}

/// Strip the Windows extended-length prefix that `fs::canonicalize` adds.
pub fn normalize_canonical(canonical: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = canonical.to_string_lossy();
        if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", unc));
        }
        if let Some(plain) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(plain);
        }
    }
    canonical
}

/// Port of `canonicalProjectPath` from `apps/api/src/security/projectPaths.ts`:
/// validates the directory exists, is not the filesystem root and is not the
/// user's home directory.
pub fn canonical_project_path(input: &str) -> Result<PathBuf, String> {
    let candidate = input.trim();
    if candidate.is_empty() || candidate.contains('\0') {
        return Err("Invalid project path.".to_string());
    }
    let canonical = std::fs::canonicalize(candidate)
        .map_err(|_| "Project directory does not exist or cannot be accessed.".to_string())?;
    let canonical = normalize_canonical(canonical);
    if !canonical.is_dir() {
        return Err("Project path must be a directory.".to_string());
    }
    let filesystem_root = filesystem_root(&canonical);
    let home = std::fs::canonicalize(home_dir())
        .map(normalize_canonical)
        .unwrap_or_else(|_| home_dir());
    if canonical == filesystem_root || canonical == home {
        return Err(
            "Choose a project folder, not the filesystem root or your entire home directory."
                .to_string(),
        );
    }
    Ok(canonical)
}

/// Real home directory, canonicalized when possible.
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Best-effort containment check used before reading files referenced by git.
pub fn join_inside(base: &Path, relative: &str) -> Option<PathBuf> {
    if relative.contains("..") {
        return None;
    }
    let joined = base.join(relative);
    let canonical = std::fs::canonicalize(&joined).ok()?;
    let base_canonical = std::fs::canonicalize(base).map(normalize_canonical).ok()?;
    if canonical.starts_with(&base_canonical) {
        Some(normalize_canonical(canonical))
    } else {
        None
    }
}
