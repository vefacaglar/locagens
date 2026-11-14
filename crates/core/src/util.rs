//! Small shared helpers: timestamps and canonical paths.

use std::path::{Path, PathBuf};

/// Current UTC time in the same shape as JS `new Date().toISOString()`.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Opens an http(s) URL in the default browser. The URL goes to the OS
/// opener as one argument, never through a shell.
pub fn open_browser(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|err| format!("Invalid URL: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http(s) URLs can be opened.".to_string());
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", parsed.as_str()]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(parsed.as_str());
        command
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(parsed.as_str());
        command
    };
    command.spawn().map(|_| ()).map_err(|err| format!("Could not open the browser: {err}"))
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

/// Canonical project path: validates the directory exists, is not the filesystem root and is not the
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
    // Both sides normalized: on Windows `canonicalize` adds a `\\?\` prefix,
    // and comparing a prefixed path to an unprefixed base always failed.
    let canonical = std::fs::canonicalize(&joined).map(normalize_canonical).ok()?;
    let base_canonical = std::fs::canonicalize(base).map(normalize_canonical).ok()?;
    canonical.starts_with(&base_canonical).then_some(canonical)
}

/// Like `join_inside`, but for a path that may not exist yet (a file about to
/// be written): the deepest existing ancestor must resolve inside `base`.
pub fn writable_inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_) | std::path::Component::CurDir))
    {
        return None;
    }
    let base_canonical = std::fs::canonicalize(base).map(normalize_canonical).ok()?;
    let joined = base_canonical.join(relative_path);
    let existing = joined.ancestors().find(|ancestor| ancestor.exists())?;
    let existing_canonical = std::fs::canonicalize(existing).map(normalize_canonical).ok()?;
    existing_canonical.starts_with(&base_canonical).then_some(joined)
}

/// Keeps a console child (git, hooks, MCP servers) from opening its own
/// console window when the parent has none: the Windows desktop release build
/// is a GUI-subsystem program, so every poll would flash a window otherwise.
/// No effect on other platforms.
pub fn hide_console_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}
