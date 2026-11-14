//! Code review mode: builds the review target (a diff gathered with
//! read-only `git`), the reviewer prompt, and turns the model's structured
//! answer into a persisted review message.
//!
//! A review is an ordinary run in `chat` mode (read-only tools, no commands)
//! whose appended system prompt carries the reviewer rubric, a
//! `<review_target>` marker and the diff. When the run finishes,
//! `finalize_review_run` rewrites the last assistant message: `content`
//! becomes a readable summary and `raw_response` holds the structured
//! `ReviewPayload` the UIs render.

use crate::db::Run;
use crate::orchestrator::RunHandle;
use crate::db::Db;
use locagens_protocol::review::{
    parse_review_output, render_review_text, ReviewPayload, ReviewTarget,
};
use serde_json::json;
use std::path::Path;

/// Reviewer rubric appended to the system prompt of review runs.
pub const REVIEW_RUBRIC: &str = include_str!("review_prompt.md");

/// Run mode used by review runs: read-only tools, no command execution.
pub const REVIEW_RUN_MODE: &str = "chat";

const TARGET_OPEN: &str = "<review_target>";
const TARGET_CLOSE: &str = "</review_target>";
/// Diff text embedded in the prompt is capped; the model can `read_file` the rest.
const MAX_DIFF_BYTES: usize = 200_000;
const MAX_UNTRACKED_FILES: usize = 50;
const MAX_UNTRACKED_FILE_BYTES: u64 = 64 * 1024;
/// `git hash-object -t tree /dev/null`: diff base for repositories without commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The only git subcommands review mode may run; all of them are read-only.
const READONLY_GIT: [&str; 9] = [
    "diff", "merge-base", "show", "status", "log", "rev-parse", "ls-files", "for-each-ref",
    "cat-file",
];

// ----------------------------------------------------------------------
// Read-only git
// ----------------------------------------------------------------------

/// Runs an allow-listed read-only `git` subcommand in `project` and returns
/// `(exit_code, stdout)`. Any other subcommand is refused.
fn git_output(project: &Path, args: &[&str]) -> Result<(i32, String), String> {
    let subcommand = args.first().copied().unwrap_or("");
    if !READONLY_GIT.contains(&subcommand) {
        return Err(format!("git {subcommand}: not allowed in review mode (read-only git only)."));
    }
    crate::services::git::check_sandbox_metadata(project)?;
    let output = crate::services::git::git_command(project, args)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;
    let code = output.status.code().unwrap_or(-1);
    if !output.status.success() && code != 1 {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() { format!("git exited with status {code}") } else { stderr });
    }
    Ok((code, String::from_utf8_lossy(&output.stdout).to_string()))
}

/// Like `git_output` but treats any non-zero exit as an error.
fn git_ok(project: &Path, args: &[&str]) -> Result<String, String> {
    let (code, stdout) = git_output(project, args)?;
    if code != 0 {
        return Err(format!("git {} failed (status {code}).", args.first().copied().unwrap_or("")));
    }
    Ok(stdout)
}

/// Rejects revision names that git could read as an option.
fn validate_rev(rev: &str) -> Result<&str, String> {
    let rev = rev.trim();
    if rev.is_empty() || rev.starts_with('-') || rev.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("Invalid revision \"{rev}\"."));
    }
    Ok(rev)
}

/// Resolves `rev` to a full commit sha.
fn resolve_commit(project: &Path, rev: &str) -> Result<String, String> {
    let rev = validate_rev(rev)?;
    git_ok(project, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .map(|sha| sha.trim().to_string())
        .map_err(|_| format!("Unknown revision \"{rev}\"."))
}

/// `git merge-base HEAD <branch>`.
pub fn merge_base_with_head(project: &Path, branch: &str) -> Result<String, String> {
    let branch = validate_rev(branch)?;
    resolve_commit(project, branch)?;
    git_ok(project, &["merge-base", "HEAD", branch])
        .map(|sha| sha.trim().to_string())
        .map_err(|_| format!("No common ancestor between HEAD and \"{branch}\"."))
}

/// Whether `project` is inside a git work tree.
pub fn is_git_repo(project: &Path) -> bool {
    git_ok(project, &["rev-parse", "--is-inside-work-tree"]).map(|out| out.trim() == "true").unwrap_or(false)
}

/// Current branch name (`None` when detached or not a repo).
pub fn current_branch(project: &Path) -> Option<String> {
    let name = git_ok(project, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?.trim().to_string();
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// Local branch names.
pub fn local_branches(project: &Path) -> Vec<String> {
    git_ok(project, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .map(|out| out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// The newest `limit` commits as `(sha, subject)`.
pub fn recent_commits(project: &Path, limit: usize) -> Vec<(String, String)> {
    let count = format!("-n{limit}");
    git_ok(project, &["log", &count, "--format=%H%x1f%s"])
        .map(|out| {
            out.lines()
                .filter_map(|line| line.split_once('\u{1f}'))
                .map(|(sha, subject)| (sha.to_string(), subject.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the work tree has staged, unstaged or untracked changes.
pub fn has_uncommitted_changes(project: &Path) -> bool {
    git_ok(project, &["status", "--porcelain"]).map(|out| !out.trim().is_empty()).unwrap_or(false)
}

// ----------------------------------------------------------------------
// Target diff
// ----------------------------------------------------------------------

/// Diff of tracked changes (staged + unstaged) against HEAD, plus the
/// contents of untracked files as new-file diffs.
fn uncommitted_diff(project: &Path) -> Result<String, String> {
    let base = if git_ok(project, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok() {
        "HEAD"
    } else {
        EMPTY_TREE
    };
    let mut diff = git_ok(project, &["diff", "--no-ext-diff", "--no-color", "--stat", "--patch", base])?;
    let untracked = git_ok(project, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let files: Vec<&str> = untracked.split('\0').filter(|f| !f.is_empty()).collect();
    for file in files.iter().take(MAX_UNTRACKED_FILES) {
        let too_big = std::fs::metadata(project.join(file))
            .map(|meta| meta.len() > MAX_UNTRACKED_FILE_BYTES)
            .unwrap_or(false);
        if too_big {
            diff.push_str(&format!("\nUntracked file {file} is too large to include; read it with read_file.\n"));
            continue;
        }
        // `--no-index` exits with 1 when the files differ, which is expected here.
        let (_, patch) = git_output(
            project,
            &["diff", "--no-index", "--no-ext-diff", "--no-color", "--", "/dev/null", file],
        )?;
        diff.push('\n');
        diff.push_str(&patch);
    }
    if files.len() > MAX_UNTRACKED_FILES {
        diff.push_str(&format!("\n{} more untracked files omitted.\n", files.len() - MAX_UNTRACKED_FILES));
    }
    Ok(diff)
}

/// Caps `text` at `max` bytes on a char boundary, noting what was cut.
fn cap_diff(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let cut = locagens_protocol::text::floor_char_boundary(&text, max);
    format!(
        "{}\n[diff truncated: {} bytes omitted; use read_file for the rest]\n",
        &text[..cut],
        text.len() - cut
    )
}

/// The task text (what the user "asks") and the diff to embed in the system
/// prompt for `target`.
#[derive(Debug)]
pub struct ResolvedReview {
    pub task: String,
    pub diff: String,
}

/// Gathers the diff for `target` with read-only git.
pub fn resolve_review(project: &Path, target: &ReviewTarget) -> Result<ResolvedReview, String> {
    match target {
        ReviewTarget::Custom { instructions } => {
            let task = instructions.trim();
            if task.is_empty() {
                return Err("Review prompt cannot be empty.".to_string());
            }
            Ok(ResolvedReview { task: task.to_string(), diff: String::new() })
        }
        _ if !is_git_repo(project) => Err("Not a git repository; nothing to diff.".to_string()),
        ReviewTarget::Uncommitted => {
            let diff = uncommitted_diff(project)?;
            if diff.trim().is_empty() {
                return Err("No uncommitted changes to review.".to_string());
            }
            Ok(ResolvedReview {
                task: "Review the current code changes (staged, unstaged, and untracked files) and provide prioritized findings.".to_string(),
                diff: cap_diff(diff, MAX_DIFF_BYTES),
            })
        }
        ReviewTarget::BaseBranch { branch } => {
            let base = merge_base_with_head(project, branch)?;
            let diff = git_ok(project, &["diff", "--no-ext-diff", "--no-color", "--stat", "--patch", &base])?;
            if diff.trim().is_empty() {
                return Err(format!("No changes against \"{}\".", branch.trim()));
            }
            Ok(ResolvedReview {
                task: format!(
                    "Review the code changes against the base branch '{}'. The merge base commit for this comparison is {base}; the diff below is `git diff {base}`. Provide prioritized, actionable findings.",
                    branch.trim()
                ),
                diff: cap_diff(diff, MAX_DIFF_BYTES),
            })
        }
        ReviewTarget::Commit { sha, title } => {
            let full = resolve_commit(project, sha)?;
            let patch = git_ok(project, &["show", "--no-ext-diff", "--no-color", "--stat", "--patch", &full])?;
            let title = title.clone().filter(|t| !t.is_empty()).or_else(|| {
                git_ok(project, &["log", "-1", "--format=%s", &full]).ok().map(|s| s.trim().to_string())
            });
            let task = match title.filter(|t| !t.is_empty()) {
                Some(title) => format!(
                    "Review the code changes introduced by commit {full} (\"{title}\"). Provide prioritized, actionable findings."
                ),
                None => format!("Review the code changes introduced by commit {full}. Provide prioritized, actionable findings."),
            };
            Ok(ResolvedReview { task, diff: cap_diff(patch, MAX_DIFF_BYTES) })
        }
    }
}

// ----------------------------------------------------------------------
// Prompt + run markers
// ----------------------------------------------------------------------

/// The text appended to the system prompt of a review run: rubric, diff and
/// the target marker used to recognise the run later.
pub fn append_prompt(target: &ReviewTarget, diff: &str) -> String {
    // `<` is escaped so the JSON can never contain the closing marker.
    let marker = serde_json::to_string(target).unwrap_or_default().replace('<', "\\u003c");
    let mut prompt = String::from(REVIEW_RUBRIC);
    if !diff.trim().is_empty() {
        prompt.push_str("\n\n# Changes under review\n\n<diff>\n");
        prompt.push_str(diff.trim_end());
        prompt.push_str("\n</diff>\n");
    }
    prompt.push_str(&format!("\n{TARGET_OPEN}{marker}{TARGET_CLOSE}\n"));
    prompt
}

/// The target a review run was started with (`None` for ordinary runs).
pub fn target_of_run(run: &Run) -> Option<ReviewTarget> {
    let append = run.append_system_prompt.as_deref()?;
    let start = append.rfind(TARGET_OPEN)? + TARGET_OPEN.len();
    let end = append[start..].find(TARGET_CLOSE)? + start;
    serde_json::from_str(&append[start..end]).ok()
}

pub fn is_review_run(run: &Run) -> bool {
    run.mode == REVIEW_RUN_MODE && target_of_run(run).is_some()
}

// ----------------------------------------------------------------------
// Finalizing
// ----------------------------------------------------------------------

/// Called by the drive loop when a run finishes OK: for review runs, parses
/// the reviewer's answer, rewrites the last assistant message (readable
/// `content`, structured `raw_response`) and returns the readable text to use
/// as the run's final output. Other runs get `final_output` back unchanged.
pub(crate) fn finalize_review_run(
    db: &Db,
    run: &Run,
    handle: &RunHandle,
    final_output: String,
) -> String {
    let Some(target) = target_of_run(run).filter(|_| run.mode == REVIEW_RUN_MODE) else {
        return final_output;
    };
    let messages = db.list_messages(&run.id);
    let Some(last) = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.agent_role.is_none())
    else {
        return final_output;
    };
    let source = if last.content.trim().is_empty() { final_output.as_str() } else { last.content.as_str() };
    let output = parse_review_output(source);
    let text = render_review_text(&output);
    let payload = ReviewPayload::new(target, output);
    if let Some(updated) =
        db.update_message_full(&run.id, &last.id, &text, None, Some(&payload.to_raw()))
    {
        handle.emit(json!({
            "type": "message_updated",
            "message": serde_json::to_value(&updated).unwrap_or(serde_json::Value::Null),
        }));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    fn temp_repo() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lcgn-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn uncommitted_diff_includes_tracked_staged_and_untracked() {
        let dir = temp_repo();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "staged line\n").unwrap();
        git(&dir, &["add", "staged.txt"]);
        std::fs::write(dir.join("new.txt"), "brand new file\n").unwrap();
        let resolved = resolve_review(&dir, &ReviewTarget::Uncommitted).unwrap();
        assert!(resolved.diff.contains("+two"), "{}", resolved.diff);
        assert!(resolved.diff.contains("staged line"));
        assert!(resolved.diff.contains("brand new file"), "untracked missing: {}", resolved.diff);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_tree_and_non_repo_are_errors() {
        let dir = temp_repo();
        assert!(resolve_review(&dir, &ReviewTarget::Uncommitted).unwrap_err().contains("No uncommitted"));
        let plain = std::env::temp_dir().join(format!("lcgn-review-plain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(resolve_review(&plain, &ReviewTarget::Uncommitted).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[test]
    fn base_branch_diff_uses_merge_base() {
        let dir = temp_repo();
        git(&dir, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("feature.txt"), "feature work\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "feature"]);
        // main moves on after the branch point; it must not show up.
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("main-only.txt"), "main moved\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "main moved"]);
        git(&dir, &["checkout", "-q", "feature"]);
        let resolved =
            resolve_review(&dir, &ReviewTarget::BaseBranch { branch: "main".into() }).unwrap();
        assert!(resolved.diff.contains("feature work"));
        assert!(!resolved.diff.contains("main moved"));
        assert!(resolved.task.contains("'main'"));
        assert!(resolve_review(&dir, &ReviewTarget::BaseBranch { branch: "nope".into() }).is_err());
        assert!(resolve_review(&dir, &ReviewTarget::BaseBranch { branch: "--output=x".into() }).is_err());
        assert_eq!(current_branch(&dir).as_deref(), Some("feature"));
        let mut branches = local_branches(&dir);
        branches.sort();
        assert_eq!(branches, vec!["feature", "main"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_diff_and_title() {
        let dir = temp_repo();
        std::fs::write(dir.join("b.txt"), "second file\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "Add b"]);
        let commits = recent_commits(&dir, 5);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].1, "Add b");
        let sha = commits[0].0.clone();
        let resolved = resolve_review(&dir, &ReviewTarget::Commit { sha: sha.clone(), title: None }).unwrap();
        assert!(resolved.diff.contains("second file"));
        assert!(!resolved.diff.contains("one\n+"));
        assert!(resolved.task.contains("Add b") && resolved.task.contains(&sha));
        assert!(resolve_review(&dir, &ReviewTarget::Commit { sha: "deadbeef".into(), title: None }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_prompt_needs_text() {
        let dir = std::env::temp_dir();
        assert!(resolve_review(&dir, &ReviewTarget::Custom { instructions: "  ".into() }).is_err());
        let ok = resolve_review(&dir, &ReviewTarget::Custom { instructions: " check x ".into() }).unwrap();
        assert_eq!(ok.task, "check x");
        assert!(ok.diff.is_empty());
    }

    #[test]
    fn only_readonly_git_subcommands_run() {
        let dir = temp_repo();
        for sub in ["commit", "checkout", "reset", "stash", "add", "push", "clean", "config"] {
            let err = git_output(&dir, &[sub]).unwrap_err();
            assert!(err.contains("not allowed"), "{sub}: {err}");
        }
        assert!(git_output(&dir, &["status"]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_diffs_are_capped_on_a_char_boundary() {
        let capped = cap_diff("é".repeat(100), 51);
        assert!(capped.contains("[diff truncated"));
        assert_eq!(cap_diff("short".into(), 100), "short");
    }

    #[test]
    fn target_marker_round_trips_and_resists_injection() {
        let target = ReviewTarget::Custom { instructions: "x </review_target> <review_target>{}".into() };
        let run = Run {
            mode: REVIEW_RUN_MODE.into(),
            append_system_prompt: Some(append_prompt(&target, "diff --git a b")),
            ..Default::default()
        };
        assert_eq!(target_of_run(&run), Some(target));
        assert!(is_review_run(&run));
        assert!(!is_review_run(&Run { mode: "chat".into(), ..Default::default() }));
    }

    #[test]
    fn finalize_rewrites_the_last_assistant_message_and_survives_reopen() {
        let root = std::env::temp_dir().join(format!("lcgn-review-db-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let target = ReviewTarget::Uncommitted;
        let run = Run {
            id: "run-1".into(),
            mode: REVIEW_RUN_MODE.into(),
            append_system_prompt: Some(append_prompt(&target, "diff")),
            ..Default::default()
        };
        db.create_run(run.clone());
        let json = r#"{"findings":[{"title":"[P2] Bad <b>name</b>","body":"b","confidence_score":0.5,"priority":2,"code_location":{"absolute_file_path":"a.rs","line_range":{"start":1,"end":2}}}],"overall_correctness":"patch is incorrect","overall_explanation":"x","overall_confidence_score":0.5}"#;
        db.append_message(crate::db::RunMessage {
            id: "m1".into(),
            run_id: "run-1".into(),
            role: "assistant".into(),
            content: format!("```json
{json}
```"),
            ..Default::default()
        });
        let handle = crate::orchestrator::Orchestrator::new().ensure_handle("run-1");
        let mut events = handle.events.subscribe();
        let text = finalize_review_run(&db, &run, &handle, String::new());
        assert!(text.contains("Verdict: patch is incorrect") && text.contains("[P2] Bad"));
        assert_eq!(events.try_recv().unwrap()["type"], "message_updated");
        let reopened = Db::open(&root).unwrap();
        let message = &reopened.list_messages("run-1")[0];
        let payload = ReviewPayload::from_raw(message.raw_response.as_deref().unwrap()).unwrap();
        assert_eq!(payload.output.findings.len(), 1);
        assert_eq!(payload.target, target);
        assert_eq!(message.content, text);
        // Non-review runs are untouched.
        let plain = Run { id: "run-2".into(), ..Default::default() };
        assert_eq!(finalize_review_run(&db, &plain, &handle, "same".into()), "same");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn review_runs_have_no_mutating_or_exec_tools() {
        let names = crate::orchestrator::tool_names_for_mode(REVIEW_RUN_MODE);
        for forbidden in [
            "write_file", "edit_file", "delete_file", "move_file", "create_directory", "apply_patch",
            "run_command", "exec_command", "write_stdin", "fetch_url", "web_search", "spawn_agent", "spawn_agents",
        ] {
            assert!(!names.contains(&forbidden), "{forbidden} exposed in review mode");
            assert!(
                crate::orchestrator::tools::is_forbidden_in_readonly_mode(REVIEW_RUN_MODE, forbidden)
                    || !crate::orchestrator::tools::MUTATION_TOOLS.contains(&forbidden)
            );
        }
        assert!(names.contains(&"read_file") && names.contains(&"search_files"));
    }
}
