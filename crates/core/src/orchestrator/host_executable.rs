//! `host_executable` tool: locate an executable on the host `PATH`.
//!
//! Codex has no model-facing tool of this name — `host_executable(name=…,
//! paths=[…])` is an execpolicy `.rules` directive that pins which absolute
//! paths a bare program name (`git`) may resolve to, so a prefix rule written
//! for `git` also matches `/usr/bin/git`. This tool is the lookup half of
//! that idea: it answers "where would `name` resolve?" with the same
//! name-key normalisation Codex uses (case-folded, `.exe/.cmd/.bat/.com`
//! stripped on Windows) and `PATH`/`PATHEXT` search order, so the model (and
//! a user writing rules) can see the absolute paths involved.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Windows fallback when `PATHEXT` is unset (cmd.exe's own default).
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";
/// Suffixes Codex strips when building a lookup key on Windows.
const WINDOWS_KEY_SUFFIXES: [&str; 4] = [".exe", ".cmd", ".bat", ".com"];

pub fn tool_def() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "host_executable",
            "description": "Locate an executable on the host PATH and return its absolute path(s) in search order. On Windows PATHEXT is honoured (`git` finds `git.exe`). A name containing a path separator is checked directly. Use this before running a tool to learn whether it is installed and which binary would run.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Bare executable name (e.g. `git`, `python3`) or a path." },
                    "all": { "type": "boolean", "description": "Return every match on PATH instead of only the first (default false)." }
                },
                "required": ["name"]
            }
        }
    })
}

/// Executes the tool against the real environment.
pub fn execute(args: &Value) -> Result<String, String> {
    let name = args.get("name").and_then(Value::as_str).unwrap_or_default().trim();
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").ok();
    let env = LookupEnv {
        windows: cfg!(windows),
        path_dirs: std::env::split_paths(&path_var).collect(),
        pathext: pathext.as_deref(),
    };
    let matches = locate(name, &env, &is_executable_file)?;
    Ok(render(name, &matches, all, env.windows))
}

/// The environment a lookup runs in (injectable for tests).
pub struct LookupEnv<'a> {
    pub windows: bool,
    pub path_dirs: Vec<PathBuf>,
    pub pathext: Option<&'a str>,
}

/// Finds every executable `name` resolves to, in `PATH` order.
pub fn locate(
    name: &str,
    env: &LookupEnv,
    is_executable: &dyn Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, String> {
    if name.is_empty() {
        return Err("host_executable: name is empty.".into());
    }
    if name.contains('\0') {
        return Err("host_executable: name contains a null byte.".into());
    }
    let candidates = candidate_names(name, env);
    let has_separator = name.contains('/') || (env.windows && name.contains('\\'));
    if has_separator {
        return Ok(first_existing_path(&candidates, is_executable));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in env.path_dirs.iter().filter(|dir| !dir.as_os_str().is_empty()) {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if is_executable(&path) && !already_found(&found, &path, env.windows) {
                found.push(path);
            }
        }
    }
    Ok(found)
}

/// File names to try for `name`: on Windows the name itself when it already
/// ends in a `PATHEXT` extension, then `name` + each extension.
fn candidate_names(name: &str, env: &LookupEnv) -> Vec<String> {
    if !env.windows {
        return vec![name.to_string()];
    }
    let extensions = pathext_list(env.pathext);
    let lower = name.to_ascii_lowercase();
    let mut names = Vec::new();
    if extensions.iter().any(|ext| lower.ends_with(ext.as_str())) {
        names.push(name.to_string());
    }
    names.extend(extensions.iter().map(|ext| format!("{name}{ext}")));
    names
}

fn pathext_list(pathext: Option<&str>) -> Vec<String> {
    pathext
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_PATHEXT)
        .split(';')
        .map(|ext| ext.trim().to_ascii_lowercase())
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .collect()
}

/// A name with a separator is a path: `candidates` already hold the full
/// path with each `PATHEXT` extension appended where needed.
fn first_existing_path(candidates: &[String], is_executable: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| is_executable(path))
        .map(|path| vec![absolute(&path)])
        .unwrap_or_default()
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    }
}

fn already_found(found: &[PathBuf], path: &Path, windows: bool) -> bool {
    found.iter().any(|existing| {
        if windows {
            existing.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy())
        } else {
            existing == path
        }
    })
}

/// Codex's execpolicy lookup key: on Windows the name is lower-cased and a
/// trailing `.exe/.cmd/.bat/.com` removed; elsewhere it is unchanged.
pub fn executable_lookup_key(raw: &str, windows: bool) -> String {
    if !windows {
        return raw.to_string();
    }
    let lower = raw.to_ascii_lowercase();
    for suffix in WINDOWS_KEY_SUFFIXES {
        if let Some(stripped) = lower.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    lower
}

fn render(name: &str, matches: &[PathBuf], all: bool, windows: bool) -> String {
    let shown: Vec<String> = matches
        .iter()
        .take(if all { usize::MAX } else { 1 })
        .map(|path| path.display().to_string())
        .collect();
    serde_json::to_string_pretty(&json!({
        "name": name,
        "lookup_key": executable_lookup_key(name, windows),
        "found": !matches.is_empty(),
        "path": shown.first(),
        "paths": shown,
        "total_matches": matches.len(),
    }))
    .unwrap_or_else(|_| "{}".into())
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|meta| meta.is_file()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn fake_fs(files: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<String> = files.iter().map(|f| f.to_string()).collect();
        move |path: &Path| set.contains(&path.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
    }

    fn env(windows: bool, dirs: &[&str], pathext: Option<&'static str>) -> LookupEnv<'static> {
        LookupEnv { windows, path_dirs: dirs.iter().map(PathBuf::from).collect(), pathext }
    }

    #[test]
    fn unix_search_follows_path_order_and_exact_names() {
        let fs = fake_fs(&["/a/git", "/b/git", "/b/gitk"]);
        let found = locate("git", &env(false, &["/a", "/b"], None), &fs).unwrap();
        let shown: Vec<String> = found.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(shown, vec!["/a/git", "/b/git"]);
        assert!(locate("nope", &env(false, &["/a"], None), &fs).unwrap().is_empty());
    }

    #[test]
    fn windows_tries_pathext_extensions_in_order() {
        let fs = fake_fs(&["c:/bin/git.exe", "c:/bin/git.cmd", "c:/tools/git.bat"]);
        let e = env(true, &["C:/bin", "C:/tools"], Some(".COM;.EXE;.BAT;.CMD"));
        let found = locate("git", &e, &fs).unwrap();
        let shown: Vec<String> = found.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(shown, vec!["C:/bin/git.exe", "C:/bin/git.cmd", "C:/tools/git.bat"]);
    }

    #[test]
    fn windows_name_with_extension_and_missing_pathext() {
        let fs = fake_fs(&["c:/bin/node.exe", "c:/bin/node"]);
        // Explicit extension is used as-is; the extensionless file is not executable on Windows.
        let found = locate("node.exe", &env(true, &["C:/bin"], None), &fs).unwrap();
        assert_eq!(found.len(), 1);
        let bare = locate("node", &env(true, &["C:/bin"], None), &fs).unwrap();
        assert_eq!(bare.len(), 1);
        assert!(bare[0].to_string_lossy().ends_with("node.exe"));
    }

    #[test]
    fn windows_search_is_case_insensitive_and_dedupes_repeated_dirs() {
        let fs = fake_fs(&["c:/bin/git.exe"]);
        let e = env(true, &["C:/bin", "c:/BIN"], Some(".exe"));
        assert_eq!(locate("GIT", &e, &fs).unwrap().len(), 1);
    }

    #[test]
    fn empty_and_nul_names_are_rejected() {
        let fs = fake_fs(&[]);
        assert!(locate("", &env(false, &[], None), &fs).is_err());
        assert!(locate("a\0b", &env(false, &[], None), &fs).is_err());
    }

    #[test]
    fn lookup_key_matches_codex_normalisation() {
        assert_eq!(executable_lookup_key("Git.EXE", true), "git");
        assert_eq!(executable_lookup_key("tool.cmd", true), "tool");
        assert_eq!(executable_lookup_key("script.ps1", true), "script.ps1");
        assert_eq!(executable_lookup_key("Git.exe", false), "Git.exe");
    }

    #[test]
    fn pathext_defaults_when_unset_or_blank() {
        assert_eq!(pathext_list(None), vec![".com", ".exe", ".bat", ".cmd"]);
        assert_eq!(pathext_list(Some("  ")), vec![".com", ".exe", ".bat", ".cmd"]);
        assert_eq!(pathext_list(Some(".EXE;;bad;.Ps1")), vec![".exe", ".ps1"]);
    }

    #[test]
    fn real_filesystem_lookup_honours_platform_rules() {
        let dir = std::env::temp_dir().join(format!("locagens-hostexe-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_name = if cfg!(windows) { "locagens-probe.exe" } else { "locagens-probe" };
        let path = dir.join(file_name);
        std::fs::write(&path, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let e = LookupEnv { windows: cfg!(windows), path_dirs: vec![dir.clone()], pathext: None };
        let found = locate("locagens-probe", &e, &is_executable_file).unwrap();
        assert_eq!(found, vec![path.clone()]);
        // A direct path (with separator) resolves too.
        let direct = locate(&path.to_string_lossy(), &e, &is_executable_file).unwrap();
        assert_eq!(direct, vec![path]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_reports_first_match_unless_all() {
        let matches = vec![PathBuf::from("/a/git"), PathBuf::from("/b/git")];
        let one: Value = serde_json::from_str(&render("git", &matches, false, false)).unwrap();
        assert_eq!(one["found"], true);
        assert_eq!(one["paths"].as_array().unwrap().len(), 1);
        assert_eq!(one["total_matches"], 2);
        let many: Value = serde_json::from_str(&render("git", &matches, true, false)).unwrap();
        assert_eq!(many["paths"].as_array().unwrap().len(), 2);
        let none: Value = serde_json::from_str(&render("zzz", &[], false, false)).unwrap();
        assert_eq!(none["found"], false);
        assert!(none["path"].is_null());
    }
}
