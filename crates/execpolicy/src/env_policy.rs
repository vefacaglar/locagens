//! Codex `[shell_environment_policy]`: decides which environment variables
//! spawned commands inherit. Installed process-wide by the host via
//! [`install`]; with no policy installed the legacy behaviour is unchanged.

use std::collections::HashMap;
use std::sync::RwLock;

/// How much of the parent environment is inherited before filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Inherit {
    #[default]
    All,
    Core,
    None,
}

/// Codex-style environment policy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShellEnvironmentPolicy {
    pub inherit: Inherit,
    pub ignore_default_excludes: bool,
    pub exclude: Vec<String>,
    pub set: HashMap<String, String>,
    pub include_only: Vec<String>,
}

const CORE_VARS: [&str; 14] = [
    "PATH", "SHELL", "TMPDIR", "TEMP", "TMP", "HOME", "LOGNAME", "USER", "USERNAME", "USERPROFILE",
    "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT",
];

/// Variables callers always set for non-interactive output.
const CALLER_OVERRIDES: [&str; 4] = ["PAGER", "GIT_PAGER", "GH_PAGER", "NO_COLOR"];

static POLICY: RwLock<Option<ShellEnvironmentPolicy>> = RwLock::new(None);

/// Installs (or clears) the process-wide policy.
pub fn install(policy: Option<ShellEnvironmentPolicy>) {
    if let Ok(mut slot) = POLICY.write() {
        *slot = policy;
    }
}

/// Case-insensitive glob match supporting `*` and `?`.
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some(('?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    go(&p, &t)
}

impl ShellEnvironmentPolicy {
    /// Builds the child environment from `vars` (normally the parent's).
    pub fn build_env<I>(&self, vars: I) -> HashMap<String, String>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut env: HashMap<String, String> = vars
            .into_iter()
            .filter(|(name, _)| match self.inherit {
                Inherit::All => true,
                Inherit::Core => CORE_VARS.iter().any(|core| core.eq_ignore_ascii_case(name)),
                Inherit::None => false,
            })
            .collect();
        if !self.ignore_default_excludes {
            env.retain(|name, _| {
                !["*KEY*", "*SECRET*", "*TOKEN*"].iter().any(|pat| glob_matches(pat, name))
            });
        }
        env.retain(|name, _| !self.exclude.iter().any(|pat| glob_matches(pat, name)));
        for (name, value) in &self.set {
            env.insert(name.clone(), value.clone());
        }
        if !self.include_only.is_empty() {
            env.retain(|name, _| self.include_only.iter().any(|pat| glob_matches(pat, name)));
        }
        env
    }
}

/// The `(clear, vars)` environment for a child. Without an installed policy
/// this is exactly what the caller asked for; with one, the policy replaces
/// the inherited environment and caller values only override variables the
/// policy kept (plus the pager / color overrides).
pub fn child_env(
    env_clean: bool,
    requested: Option<&HashMap<String, String>>,
) -> (bool, HashMap<String, String>) {
    let policy = POLICY.read().ok().and_then(|slot| slot.clone());
    resolve_child_env(policy.as_ref(), std::env::vars(), env_clean, requested)
}

/// [`child_env`] with the policy and parent environment passed in.
pub fn resolve_child_env<I>(
    policy: Option<&ShellEnvironmentPolicy>,
    parent: I,
    env_clean: bool,
    requested: Option<&HashMap<String, String>>,
) -> (bool, HashMap<String, String>)
where
    I: IntoIterator<Item = (String, String)>,
{
    let requested = requested.cloned().unwrap_or_default();
    let Some(policy) = policy else { return (env_clean, requested) };
    let mut env = policy.build_env(parent);
    for (name, value) in requested {
        if policy.set.contains_key(&name) {
            continue;
        }
        if env.contains_key(&name) || CALLER_OVERRIDES.contains(&name.as_str()) || !env_clean {
            env.insert(name, value);
        }
    }
    (true, env)
}

/// The complete environment of a command that needs an explicit map (the
/// Windows sandbox spawn, `run_command`): [`child_env`] applied to the
/// parent environment, so `[shell_environment_policy]` holds on every path.
/// A non-clean spawn without a policy inherits the whole parent environment.
pub fn full_child_env(
    env_clean: bool,
    requested: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let (clear, mut vars) = child_env(env_clean, requested);
    if clear {
        return vars;
    }
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.extend(vars.drain());
    env
}

/// Environment variable names that must never leak into child processes.
pub fn is_secret_env(name: &str) -> bool {
    let upper = name.to_uppercase();
    upper.contains("API_KEY")
        || upper.contains("APIKEY")
        || upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("PASSWD")
        || upper == "LOCAGENS_API_TOKEN"
        || upper.contains("PRIVATE_KEY")
}

/// Variables a stripped (default) command environment keeps.
const SAFE_VARS: [&str; 18] = [
    "PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "TEMP", "TMP", "HOME", "USERPROFILE",
    "APPDATA", "LOCALAPPDATA", "PROGRAMFILES", "NUMBER_OF_PROCESSORS", "OS", "PROCESSOR_ARCHITECTURE",
    "LANG", "TERM", "SHELL",
];

/// Whether `name` is on the safe allowlist. Windows variable names are
/// case-insensitive (`SystemRoot`, `Path`, `ComSpec`); without `SystemRoot`
/// a child process cannot even initialize Winsock.
fn is_safe_env_name(name: &str) -> bool {
    let name = if cfg!(windows) { name.to_uppercase() } else { name.to_string() };
    SAFE_VARS.contains(&name.as_str()) || name.starts_with("SYSTEMROOT")
}

/// The secret-free base environment of agent commands (the caller-requested
/// side of [`child_env`]): the safe allowlist of `parent`, minus secrets,
/// plus pager / color overrides so long output never blocks on a pager.
pub fn safe_base_env_from<I>(parent: I) -> HashMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut env: HashMap<String, String> = parent
        .into_iter()
        .filter(|(name, _)| !is_secret_env(name) && is_safe_env_name(name))
        .collect();
    env.insert("PAGER".to_string(), "cat".to_string());
    env.insert("GIT_PAGER".to_string(), "cat".to_string());
    env.insert("GH_PAGER".to_string(), "cat".to_string());
    env.insert("NO_COLOR".to_string(), "1".to_string());
    env
}

/// [`safe_base_env_from`] for this process's environment.
pub fn safe_base_env() -> HashMap<String, String> {
    safe_base_env_from(std::env::vars())
}

/// The complete environment `run_command` (and every other clean spawn)
/// uses: the safe base refined by `[shell_environment_policy]`.
pub fn command_env() -> HashMap<String, String> {
    full_child_env(true, Some(&safe_base_env()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vec<(String, String)> {
        [("PATH", "/bin"), ("HOME", "/h"), ("MY_API_KEY", "k"), ("GITHUB_TOKEN", "t"), ("EDITOR", "vi")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn default_excludes_drop_secrets_and_core_inherit_keeps_basics() {
        let all = ShellEnvironmentPolicy::default().build_env(vars());
        assert!(all.contains_key("PATH") && all.contains_key("EDITOR"));
        assert!(!all.contains_key("MY_API_KEY") && !all.contains_key("GITHUB_TOKEN"));

        let keep_secrets = ShellEnvironmentPolicy { ignore_default_excludes: true, ..Default::default() };
        assert!(keep_secrets.build_env(vars()).contains_key("MY_API_KEY"));

        let core = ShellEnvironmentPolicy { inherit: Inherit::Core, ..Default::default() };
        assert_eq!(core.build_env(vars()).len(), 2);
        let none = ShellEnvironmentPolicy { inherit: Inherit::None, ..Default::default() };
        assert!(none.build_env(vars()).is_empty());
    }

    #[test]
    fn exclude_set_and_include_only_apply_in_codex_order() {
        let policy = ShellEnvironmentPolicy {
            exclude: vec!["editor".into()],
            set: [("FOO".to_string(), "bar".to_string())].into(),
            include_only: vec!["PATH".into(), "FOO".into()],
            ..Default::default()
        };
        let env = policy.build_env(vars());
        let mut keys: Vec<_> = env.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["FOO", "PATH"]);
    }

    #[test]
    fn glob_matching_is_case_insensitive() {
        assert!(glob_matches("*token*", "GITHUB_TOKEN"));
        assert!(glob_matches("A?C", "abc"));
        assert!(!glob_matches("A?C", "abbc"));
    }

    #[test]
    fn child_env_without_policy_is_passthrough() {
        let requested: HashMap<String, String> = [("X".to_string(), "1".to_string())].into();
        let (clear, env) = resolve_child_env(None, vars(), true, Some(&requested));
        assert!(clear);
        assert_eq!(env, requested);
    }

    #[test]
    fn safe_base_env_strips_secrets_and_unlisted_vars() {
        let env = safe_base_env_from(vars());
        assert_eq!(env.get("PATH").map(String::as_str), Some("/bin"));
        assert!(env.contains_key("HOME") && env.contains_key("NO_COLOR"));
        assert!(!env.contains_key("MY_API_KEY") && !env.contains_key("GITHUB_TOKEN"));
        assert!(!env.contains_key("EDITOR"));
    }

    /// QA-016: the policy refines the safe base the same way for every spawn.
    #[test]
    fn policy_applies_to_the_requested_safe_base() {
        let base = safe_base_env_from(vars());
        let policy = ShellEnvironmentPolicy {
            inherit: Inherit::None,
            exclude: vec!["AWS_*".into()],
            set: [("FOO".to_string(), "bar".to_string())].into(),
            ..Default::default()
        };
        let (clear, env) = resolve_child_env(Some(&policy), vars(), true, Some(&base));
        assert!(clear);
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert!(!env.contains_key("PATH") && !env.contains_key("HOME"));
        assert_eq!(env.get("PAGER").map(String::as_str), Some("cat"));

        let core = ShellEnvironmentPolicy { inherit: Inherit::Core, ..Default::default() };
        let (_, env) = resolve_child_env(Some(&core), vars(), true, Some(&base));
        assert!(env.contains_key("PATH") && !env.contains_key("EDITOR"));
    }
}
