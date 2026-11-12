//! `TurnDiffTracker` — Rust port of the Codex turn-diff concept.
//!
//! Before a file is modified or deleted during a turn (`apply_patch`,
//! `write_file`, `edit_file`, `delete_file`, `move_file`), its baseline
//! content is snapshotted (or marked as newly added when it did not exist).
//! `unified_diff()` then computes the aggregate unified diff across all files
//! touched in the turn by comparing each baseline against the current disk
//! state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use locagens_apply_patch::unified_diff;

#[derive(Default)]
struct Inner {
    /// `None` baseline = file did not exist at the start of the turn (added).
    baselines: HashMap<PathBuf, Option<String>>,
}

/// Tracks the net workspace diff for the current agent turn.
#[derive(Default)]
pub struct TurnDiffTracker {
    inner: Mutex<Inner>,
}

impl TurnDiffTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshots a file's baseline content before it is modified or deleted.
    /// Calling this twice for the same path is a no-op (the first baseline
    /// of the turn wins).
    pub fn snapshot(&self, path: &Path) {
        let mut inner = self.inner.lock().expect("turn diff tracker poisoned");
        if inner.baselines.contains_key(path) {
            return;
        }
        let content = std::fs::read_to_string(path).ok();
        inner.baselines.insert(path.to_path_buf(), content);
    }

    /// Number of files tracked in this turn.
    pub fn files_tracked(&self) -> usize {
        self.inner.lock().expect("turn diff tracker poisoned").baselines.len()
    }

    /// Number of tracked files whose current content differs from the baseline.
    pub fn changed_files(&self, _project: &Path) -> usize {
        let inner = self.inner.lock().expect("turn diff tracker poisoned");
        inner
            .baselines
            .iter()
            .filter(|(path, baseline)| {
                let current = std::fs::read_to_string(path).unwrap_or_default();
                current != baseline.as_ref().map(String::as_str).unwrap_or("")
            })
            .count()
    }

    /// Relative path for display, using forward slashes.
    fn rel(project: &Path, path: &Path) -> String {
        path.strip_prefix(project)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Computes the aggregate unified diff for every tracked file (baseline
    /// vs. current disk content). Files deleted mid-turn diff down to empty;
    /// files added mid-turn diff up from empty.
    pub fn unified_diff(&self, project: &Path) -> String {
        let inner = self.inner.lock().expect("turn diff tracker poisoned");
        let mut paths: Vec<&PathBuf> = inner.baselines.keys().collect();
        paths.sort();
        let mut output = String::new();
        for path in paths {
            let baseline = inner.baselines.get(path).cloned().flatten().unwrap_or_default();
            let current = std::fs::read_to_string(path).unwrap_or_default();
            if baseline == current {
                continue;
            }
            let label = Self::rel(project, path);
            if baseline.is_empty() {
                output.push_str(&unified_diff("/dev/null", &format!("b/{label}"), "", &current));
            } else if current.is_empty() {
                output.push_str(&unified_diff(&format!("a/{label}"), "/dev/null", &baseline, ""));
            } else {
                output.push_str(&unified_diff(&format!("a/{label}"), &format!("b/{label}"), &baseline, &current));
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("locagens-turn-diff-{label}-"))
            .tempdir()
            .unwrap()
    }

    #[test]
    fn tracks_added_modified_and_deleted_files() {
        let ws = temp_ws("diff");
        std::fs::write(ws.path().join("modified.txt"), "one\ntwo\n").unwrap();
        std::fs::write(ws.path().join("deleted.txt"), "bye\n").unwrap();

        let tracker = TurnDiffTracker::new();
        tracker.snapshot(&ws.path().join("modified.txt"));
        tracker.snapshot(&ws.path().join("deleted.txt"));
        tracker.snapshot(&ws.path().join("added.txt")); // did not exist → added
        assert_eq!(tracker.files_tracked(), 3);

        std::fs::write(ws.path().join("modified.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(ws.path().join("added.txt"), "new\n").unwrap();
        std::fs::remove_file(ws.path().join("deleted.txt")).unwrap();

        let diff = tracker.unified_diff(&ws.path());
        assert!(diff.contains("+++ b/added.txt"));
        assert!(diff.contains("+new"));
        assert!(diff.contains("--- a/modified.txt"));
        assert!(diff.contains("-two"));
        assert!(diff.contains("+TWO"));
        assert!(diff.contains("+three"));
        assert!(diff.contains("--- a/deleted.txt"));
        assert!(diff.contains("-bye"));
        // Unchanged files produce no diff (snapshot without mutation).
        std::fs::write(ws.path().join("untouched.txt"), "same\n").unwrap();
        tracker.snapshot(&ws.path().join("untouched.txt"));
        assert!(!tracker.unified_diff(&ws.path()).contains("untouched"));
    }

    #[test]
    fn first_snapshot_wins_and_empty_diff_when_untouched() {
        let ws = temp_ws("first");
        std::fs::write(ws.path().join("a.txt"), "v1\n").unwrap();
        let tracker = TurnDiffTracker::new();
        tracker.snapshot(&ws.path().join("a.txt"));
        std::fs::write(ws.path().join("a.txt"), "v2\n").unwrap();
        tracker.snapshot(&ws.path().join("a.txt")); // no-op
        std::fs::write(ws.path().join("a.txt"), "v3\n").unwrap();
        let diff = tracker.unified_diff(&ws.path());
        assert!(diff.contains("-v1"));
        assert!(diff.contains("+v3"));
        assert!(!diff.contains("v2"));
    }
}
