//! "What changed" card at the end of a turn that touched files (UX-009):
//! per-file `+added −removed`, `Review changes` (the inspector's Changes
//! section) and `Undo this turn` with an inline confirm.

use crate::state_ui::{core, RightPanelTab, UiStore};
use dioxus::prelude::*;
use locagens_core::services::snapshots::GitSnapshot;

/// Line counts of one file in a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// Per-file stats of a unified diff (`--- a/x` / `+++ b/x` headers; added or
/// deleted files name `/dev/null` on one side).
pub fn diff_file_stats(diff: &str) -> Vec<FileStat> {
    let strip = |path: &str| -> String {
        let path = path.trim();
        path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path).to_string()
    };
    let mut stats: Vec<FileStat> = Vec::new();
    let mut old_path: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            old_path = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let path = if rest.trim() == "/dev/null" { strip(old_path.as_deref().unwrap_or_default()) } else { strip(rest) };
            stats.push(FileStat { path, added: 0, removed: 0 });
            old_path = None;
        } else if let Some(current) = stats.last_mut() {
            if line.starts_with('+') {
                current.added += 1;
            } else if line.starts_with('-') {
                current.removed += 1;
            }
        }
    }
    stats
}

/// Where the undo flow stands.
#[derive(Clone, Debug, PartialEq)]
enum UndoStep {
    Idle,
    Confirming,
    Working,
    Done { restored: usize, skipped: Vec<String> },
}

/// The card; rendered by the thread after a finished turn with a diff.
#[component]
pub fn ChangesCard(diff: String, run_id: String) -> Element {
    let store: UiStore = use_context();
    let stats = diff_file_stats(&diff);
    let mut step = use_signal(|| UndoStep::Idle);
    if stats.is_empty() {
        return rsx! {};
    }
    let added: usize = stats.iter().map(|stat| stat.added).sum();
    let removed: usize = stats.iter().map(|stat| stat.removed).sum();
    let count = stats.len().to_string();
    let current = step.read().clone();

    rsx! {
        div {
            class: "changes-card card-enter",
            role: "group",
            "aria-label": "{store.tf(\"changes.title\", &[&count])}",
            div {
                class: "changes-head",
                span { class: "changes-title", "{store.tf(\"changes.title\", &[&count])}" }
                span { class: "diff-stat add", "+{added}" }
                span { class: "diff-stat del", "−{removed}" }
            }
            div {
                class: "changes-files",
                for stat in stats.iter() {
                    div {
                        key: "{stat.path}",
                        class: "changes-file",
                        span { class: "mono grow", title: "{stat.path}", "{stat.path}" }
                        span { class: "diff-stat add", "+{stat.added}" }
                        span { class: "diff-stat del", "−{stat.removed}" }
                    }
                }
            }
            match current {
                UndoStep::Idle => rsx! {
                    div {
                        class: "changes-actions",
                        button {
                            onclick: move |_| open_changes(store),
                            "{store.t(\"changes.review\")}"
                        }
                        button {
                            onclick: move |_| step.set(UndoStep::Confirming),
                            "{store.t(\"changes.undo\")}"
                        }
                    }
                },
                UndoStep::Confirming | UndoStep::Working => rsx! {
                    div { class: "changes-confirm", "{store.tf(\"changes.confirm\", &[&count])}" }
                    div {
                        class: "changes-actions",
                        button {
                            class: "danger",
                            disabled: current == UndoStep::Working,
                            onmounted: move |event| async move {
                                let _ = event.set_focus(true).await;
                            },
                            onclick: move |_| {
                                step.set(UndoStep::Working);
                                let run_id = run_id.clone();
                                spawn(async move {
                                    match undo_turn(store, &run_id).await {
                                        Ok(snapshot) => step.set(UndoStep::Done { restored: snapshot.restored.len(), skipped: snapshot.skipped }),
                                        Err(err) => {
                                            store.toast(store.tf("toast.undo_failed", &[&err]));
                                            step.set(UndoStep::Idle);
                                        }
                                    }
                                });
                            },
                            "{store.t(\"changes.undo_confirm\")}"
                        }
                        button {
                            disabled: current == UndoStep::Working,
                            onclick: move |_| step.set(UndoStep::Idle),
                            "{store.t(\"common.cancel\")}"
                        }
                    }
                },
                UndoStep::Done { restored, skipped } => rsx! {
                    div {
                        class: "changes-confirm",
                        "{store.tf(\"changes.reverted\", &[&restored.to_string()])}"
                        if !skipped.is_empty() {
                            " · {store.tf(\"changes.left_alone\", &[&skipped.len().to_string(), &skipped.join(\", \")])}"
                        }
                    }
                },
            }
        }
    }
}

/// Opens the inspector on its Changes section.
pub fn open_changes(store: UiStore) {
    let mut store = store;
    store.right_panel_open.set(true);
    store.right_panel_tab.set(RightPanelTab::Git);
    dioxus::document::eval(
        "requestAnimationFrame(() => requestAnimationFrame(() => { const s = document.getElementById('inspector-git'); if (s) s.scrollIntoView({ block: 'start' }); }));",
    );
}

/// Reverts the last turn of `run_id` in the current project.
pub async fn undo_turn(store: UiStore, run_id: &str) -> Result<GitSnapshot, String> {
    let Some(project) = store.current_project() else {
        return Err(store.t("toast.select_project").to_string());
    };
    locagens_core::api::projects::undo_last_turn(&core(), &project, Some(run_id))
        .await
        .map_err(|err| err.to_string())
}

/// `/undo` result text: "Reverted 3 files…" plus the files left alone.
pub fn undo_summary(store: &UiStore, snapshot: &GitSnapshot) -> String {
    let mut text = store.tf("undo.reverted", &[&snapshot.restored.len().to_string()]);
    if !snapshot.skipped.is_empty() {
        text.push(' ');
        text.push_str(&store.tf("undo.left_alone", &[&snapshot.skipped.join(", ")]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_count_lines_per_file_including_added_and_deleted() {
        let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n ctx\n-old\n+new\n+more\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n--- a/gone.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        assert_eq!(
            diff_file_stats(diff),
            vec![
                FileStat { path: "src/a.rs".into(), added: 2, removed: 1 },
                FileStat { path: "new.txt".into(), added: 1, removed: 0 },
                FileStat { path: "gone.txt".into(), added: 0, removed: 2 },
            ]
        );
        assert!(diff_file_stats("").is_empty());
    }
}
