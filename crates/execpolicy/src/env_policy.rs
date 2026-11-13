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
    let requested = requested.cloned().unwrap_or_default();
    let Some(policy) = policy else { return (env_clean, requested) };
    let mut env = policy.build_env(std::env::vars());
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
        install(None);
        let requested: HashMap<String, String> = [("X".to_string(), "1".to_string())].into();
        let (clear, env) = child_env(true, Some(&requested));
        assert!(clear);
        assert_eq!(env, requested);
    }
}
