//! The Changes tab of the right tool panel: a PR-style review of the work
//! tree against `HEAD`. A compact `branch → working tree` bar (repo actions
//! in its "⋯" menu), every changed file as a collapsible section with a
//! sticky header and a two-gutter unified diff, and a commit footer while
//! there are changes. Diffs are parsed by `crate::unified_diff`.

use crate::components::icons::*;
use crate::i18n::Lang;
use crate::state_ui::{core, UiStore};
use crate::unified_diff::{group_digits, is_generated, parse_unified_diff, review_rows, FileDiff, FileStatus, LineKind, Row};
use dioxus::prelude::*;
use locagens_core::api::projects::{GitFileDiff, GitStatus, GitWorktree};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

/// Line rows rendered per file before "Show more".
const LINE_CAP: usize = 400;
/// Lines each "Show more" adds.
const LINE_STEP: usize = 1000;

/// One changed file of the review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewFile {
    pub diff: FileDiff,
    /// The file as it is now, used to expand unmodified gaps.
    pub content: Option<Rc<str>>,
    /// Some of its changes are in the index.
    pub staged: bool,
}

/// Review files from the work-tree diff (`git diff HEAD`), the staged diff
/// and the per-file details (which also list untracked files, shown as all
/// added). Files keep the details' (status) order.
pub fn review_files(worktree_diff: &str, staged_diff: &str, details: &[GitFileDiff]) -> Vec<ReviewFile> {
    let staged: HashSet<String> = parse_unified_diff(staged_diff).iter().map(|file| file.path().to_string()).collect();
    let mut parsed = parse_unified_diff(worktree_diff);
    let mut files = Vec::new();
    for detail in details {
        let found = parsed.iter().position(|file| file.path() == detail.path);
        let diff = match (found, detail.kind.as_str()) {
            (Some(index), _) => parsed.remove(index),
            (None, "created") => FileDiff::whole_file(&detail.path, &detail.new_text, false),
            (None, "deleted") => FileDiff::whole_file(&detail.path, &detail.old_text, true),
            (None, _) => continue,
        };
        let content = (diff.status != FileStatus::Deleted).then(|| Rc::from(detail.new_text.as_str()));
        files.push(ReviewFile { staged: staged.contains(diff.path()), diff, content });
    }
    files.extend(parsed.into_iter().map(|diff| ReviewFile { staged: staged.contains(diff.path()), diff, content: None }));
    files
}

/// Whether a file starts expanded: the first few and small ones, never
/// binaries, lockfiles or huge diffs.
fn opens_by_default(index: usize, diff: &FileDiff) -> bool {
    let changed = diff.changed_lines();
    !diff.binary && !is_generated(diff.path()) && changed <= 600 && (index < 6 || changed <= 60)
}

/// `src/app/main.rs` → (`src/app/`, `main.rs`).
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(index) => (&path[..=index], &path[index + 1..]),
        None => ("", path),
    }
}

fn thousands(store: &UiStore, value: u32) -> String {
    group_digits(value, if store.lang() == Lang::Tr { '.' } else { ',' })
}

/// The tab: loads status, the review files and worktrees, keeps them fresh
/// every few seconds and after each action.
#[component]
pub fn ChangesTab() -> Element {
    let store: UiStore = use_context();
    let status: Signal<Option<GitStatus>> = use_signal(|| None);
    let files: Signal<Vec<Rc<ReviewFile>>> = use_signal(Vec::new);
    let worktrees: Signal<Vec<GitWorktree>> = use_signal(Vec::new);
    let mut menu_open = use_signal(|| false);
    let mut show_worktrees = use_signal(|| false);
    let mut message = use_signal(String::new);
    let busy = use_signal(|| false);
    let mut version = use_signal(|| 0u32);

    // Bumping `version` restarts the poll loop at once (after an action); a
    // project switch re-runs the effect and the old loop ends (FR-049).
    use_effect(move || {
        let path = store.current_project();
        let mut status = status;
        let mut files = files;
        let mut worktrees = worktrees;
        let current_version = *version.read();
        spawn(async move {
            let state = core();
            loop {
                if *version.peek() != current_version || store.current_project_peek() != path {
                    break;
                }
                let Some(path) = path.as_deref() else {
                    status.set(None);
                    files.set(Vec::new());
                    worktrees.set(Vec::new());
                    break;
                };
                use locagens_core::api::projects as git;
                let next_status = git::git_status(&state, path).await.ok();
                let is_git = next_status.as_ref().is_some_and(|status| status.is_git);
                let (worktree_diff, staged_diff, details, next_worktrees) = if is_git {
                    (
                        git::git_diff_worktree(&state, path).await.unwrap_or_default(),
                        git::git_diff(&state, path, true).await.unwrap_or_default(),
                        git::git_diff_details(&state, path).await.unwrap_or_default(),
                        git::list_worktrees(&state, path).await.unwrap_or_default(),
                    )
                } else {
                    Default::default()
                };
                // A switch that landed during the calls above must not leave
                // the old project's data on screen.
                if store.current_project_peek().as_deref() != Some(path) || *version.peek() != current_version {
                    break;
                }
                let next_files: Vec<ReviewFile> = review_files(&worktree_diff, &staged_diff, &details);
                if files.peek().iter().map(|file| file.as_ref()).ne(next_files.iter()) {
                    files.set(next_files.into_iter().map(Rc::new).collect());
                }
                if status.peek().as_ref().map(|s| (s.is_git, s.branch.clone(), s.has_changes))
                    != next_status.as_ref().map(|s| (s.is_git, s.branch.clone(), s.has_changes))
                {
                    status.set(next_status);
                }
                if *worktrees.peek() != next_worktrees {
                    worktrees.set(next_worktrees);
                }
                tokio::time::sleep(Duration::from_millis(4000)).await;
            }
        });
    });

    let project = store.current_project();
    let current_status = status.read().clone();
    let file_list = files.read().clone();
    let mut bump = move || version.with_mut(|value| *value += 1);

    // Runs a git action on the project, toasts its error, then refreshes.
    let run_action = move |action: GitAction| {
        let Some(path) = store.current_project() else { return };
        let run_id = store.active_run_id.peek().clone();
        spawn(async move {
            let state = core();
            use locagens_core::api::projects as git;
            let result = match &action {
                GitAction::Stage(files) => git::git_stage(&state, &path, files).await.map_err(|err| store.tf("toast.stage_failed", &[&err.to_string()])),
                GitAction::Unstage(files) => git::git_unstage(&state, &path, files).await.map_err(|err| store.tf("toast.unstage_failed", &[&err.to_string()])),
                GitAction::Undo => match git::undo_last_turn(&state, &path, run_id.as_deref()).await {
                    Ok(snapshot) => {
                        store.toast(super::changes::undo_summary(&store, &snapshot));
                        Ok(())
                    }
                    Err(err) => Err(store.tf("toast.undo_failed", &[&err.to_string()])),
                },
            };
            if let Err(text) = result {
                store.toast(text);
            }
            bump();
        });
    };

    let Some(git_status) = current_status else {
        return rsx! {
            if project.is_some() {
                TurnDiffReview {}
            } else {
                super::right_panel::EmptyState { icon: icon_git_branch(), title: store.t("git.no_project"), hint: store.t("git.no_project_hint").to_string() }
            }
        };
    };
    if !git_status.is_git {
        return rsx! {
            super::right_panel::EmptyState { icon: icon_git_branch(), title: store.t("git.not_repo"), hint: store.t("git.not_repo_hint").to_string() }
            TurnDiffReview {}
        };
    }

    let added: usize = file_list.iter().map(|file| file.diff.added).sum();
    let removed: usize = file_list.iter().map(|file| file.diff.removed).sum();
    let has_files = !file_list.is_empty();
    let branch = git_status.branch.clone().filter(|branch| !branch.is_empty()).unwrap_or_else(|| "HEAD".to_string());
    let mut menu_item = move |action: Option<GitAction>| {
        menu_open.set(false);
        if let Some(action) = action {
            run_action(action);
        }
    };

    rsx! {
        div {
            class: "rv",
            div {
                class: if *menu_open.read() { "rv-bar menu-open" } else { "rv-bar" },
                {icon_git_branch()}
                span { class: "rv-ref", title: "{branch}", "{branch}" }
                span { class: "rv-arrow", {icon_arrow_right()} }
                span { class: "rv-ref dim", "{store.t(\"git.working_tree\")}" }
                span { class: "rv-spacer" }
                if has_files {
                    span {
                        class: "rv-count",
                        if file_list.len() == 1 { "{store.t(\"git.one_file\")}" } else { "{store.tf(\"git.files_count\", &[&file_list.len().to_string()])}" }
                    }
                    span { class: "diff-stat add", "+{added}" }
                    span { class: "diff-stat del", "−{removed}" }
                }
                button {
                    class: "icon-btn rv-menu-btn",
                    title: store.t("git.more_actions"),
                    "aria-label": store.t("git.more_actions"),
                    "aria-expanded": "{menu_open.read()}",
                    onclick: move |_| menu_open.toggle(),
                    {icon_dots_horizontal()}
                }
                if *menu_open.read() {
                    div { class: "popover-backdrop", onclick: move |_| menu_open.set(false) }
                    div {
                        class: "rv-menu",
                        role: "menu",
                        button { role: "menuitem", disabled: !has_files, onclick: move |_| menu_item(Some(GitAction::Stage(Vec::new()))), "{store.t(\"git.stage_all\")}" }
                        button { role: "menuitem", disabled: !has_files, onclick: move |_| menu_item(Some(GitAction::Unstage(Vec::new()))), "{store.t(\"git.unstage_all\")}" }
                        button { role: "menuitem", title: store.t("git.undo_title"), onclick: move |_| menu_item(Some(GitAction::Undo)), "{store.t(\"git.undo\")}" }
                        div { class: "rv-menu-sep" }
                        button {
                            role: "menuitem",
                            disabled: *busy.read(),
                            onclick: move |_| {
                                menu_item(None);
                                push(store, busy);
                            },
                            "{store.t(\"git.push\")}"
                        }
                        button {
                            role: "menuitem",
                            class: if *show_worktrees.read() { "on" } else { "" },
                            onclick: move |_| {
                                menu_item(None);
                                show_worktrees.toggle();
                            },
                            "{store.t(\"git.worktrees\")}"
                        }
                    }
                }
            }
            if *show_worktrees.read() {
                WorktreeList { worktrees: worktrees.read().clone(), on_change: move |_| bump() }
            }
            if !has_files {
                super::right_panel::EmptyState { icon: icon_check(), title: store.t("git.clean"), hint: store.t("git.clean_hint").to_string() }
            }
            for (index, file) in file_list.iter().enumerate() {
                ReviewFileSection {
                    key: "{file.diff.path()}",
                    file: file.clone(),
                    default_open: opens_by_default(index, &file.diff),
                    on_stage: move |(path, stage): (String, bool)| {
                        run_action(if stage { GitAction::Stage(vec![path]) } else { GitAction::Unstage(vec![path]) });
                    },
                }
            }
            if has_files {
                div {
                    class: "rv-commit",
                    input {
                        placeholder: store.t("git.commit_message"),
                        value: "{message.read()}",
                        oninput: move |event| message.set(event.value()),
                        onkeydown: move |event| {
                            if event.key() == Key::Enter && !message.read().trim().is_empty() && !*busy.read() {
                                commit(store, busy, message, bump);
                            }
                        },
                    }
                    button {
                        class: "primary small",
                        disabled: *busy.read() || message.read().trim().is_empty(),
                        onclick: move |_| commit(store, busy, message, bump),
                        "{store.t(\"git.commit\")}"
                    }
                }
            }
        }
    }
}

/// A repo action from a menu.
#[derive(Clone)]
enum GitAction {
    /// Paths to stage; empty stages everything.
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Undo,
}

/// Commits everything with the typed message.
fn commit(store: UiStore, mut busy: Signal<bool>, mut message: Signal<String>, bump: impl FnOnce() + 'static) {
    let Some(path) = store.current_project() else { return };
    let text = message.read().trim().to_string();
    busy.set(true);
    spawn(async move {
        let input = locagens_core::api::projects::GitCommitInput { path: Some(path), message: Some(text), action: Some("commit".to_string()) };
        match locagens_core::api::projects::git_commit(&core(), input).await {
            Ok(()) => {
                store.toast(store.t("toast.committed").to_string());
                message.set(String::new());
            }
            Err(err) => store.toast(store.tf("toast.commit_failed", &[&err.to_string()])),
        }
        busy.set(false);
        bump();
    });
}

/// Pushes the current branch.
fn push(store: UiStore, mut busy: Signal<bool>) {
    let Some(path) = store.current_project() else { return };
    busy.set(true);
    spawn(async move {
        let input = locagens_core::api::projects::GitCommitInput { path: Some(path), message: None, action: Some("push".to_string()) };
        match locagens_core::api::projects::git_commit(&core(), input).await {
            Ok(()) => store.toast(store.t("toast.pushed").to_string()),
            Err(err) => store.toast(store.tf("toast.push_failed", &[&err.to_string()])),
        }
        busy.set(false);
    });
}

/// The current turn's diff, for projects without git history.
#[component]
fn TurnDiffReview() -> Element {
    let store: UiStore = use_context();
    let Some((diff, count)) = store.turn_diff.read().clone().filter(|(diff, _)| !diff.trim().is_empty()) else {
        return rsx! {};
    };
    let files: Vec<Rc<ReviewFile>> = parse_unified_diff(&diff)
        .into_iter()
        .map(|diff| Rc::new(ReviewFile { diff, content: None, staged: false }))
        .collect();
    rsx! {
        div {
            class: "rv",
            div {
                class: "rv-bar",
                span { class: "rv-ref dim", "{store.tf(\"git.turn_diff\", &[&count.to_string()])}" }
            }
            for (index, file) in files.iter().enumerate() {
                ReviewFileSection {
                    key: "{file.diff.path()}",
                    file: file.clone(),
                    default_open: opens_by_default(index, &file.diff),
                    readonly: true,
                    on_stage: move |_| {},
                }
            }
        }
    }
}

/// One file: sticky header (chevron, icon, path, `+N −M`, "⋯") and, when
/// open, its diff rows.
#[component]
fn ReviewFileSection(file: Rc<ReviewFile>, default_open: bool, #[props(default)] readonly: bool, on_stage: EventHandler<(String, bool)>) -> Element {
    let store: UiStore = use_context();
    let mut open = use_signal(|| default_open);
    let mut limit = use_signal(|| LINE_CAP);
    let mut expanded = use_signal(HashSet::<usize>::new);
    let mut menu_open = use_signal(|| false);
    // The menu opens upward when its button sits in the lower part of the window.
    let mut menu_up = use_signal(|| false);
    let diff = &file.diff;
    let path = diff.path().to_string();
    let (dir, name) = split_path(&path);
    let (dir, name) = (dir.to_string(), name.to_string());
    let is_open = *open.read();
    let tag = match diff.status {
        FileStatus::Added => Some(store.t("git.kind_created")),
        FileStatus::Deleted => Some(store.t("git.kind_deleted")),
        FileStatus::Renamed => Some(store.t("git.kind_renamed")),
        FileStatus::Modified => None,
    };
    let renamed_from = diff.old_path.clone().filter(|old| diff.status == FileStatus::Renamed && *old != path);
    let title = match &renamed_from {
        Some(old) => format!("{old} → {path}"),
        None => path.clone(),
    };

    let (rows, hidden) = if is_open {
        review_rows(diff, file.content.as_deref(), &expanded.read(), *limit.read())
    } else {
        (Vec::new(), 0)
    };
    // Gutter width from the largest line number shown.
    let widest = rows
        .iter()
        .filter_map(|row| match row {
            Row::Line(line) => line.old.max(line.new),
            Row::Gap { .. } => None,
        })
        .max()
        .unwrap_or(1);
    let gutter = widest.to_string().len().max(2);
    let body_note = if diff.binary {
        Some(store.t("git.binary"))
    } else if diff.hunks.is_empty() {
        Some(store.t("git.no_content_changes"))
    } else {
        None
    };
    let stage_path = path.clone();
    let unstage_path = path.clone();
    let copy_path = path.clone();

    rsx! {
        section {
            class: if is_open { "rv-file open" } else { "rv-file" },
            div {
                class: if *menu_open.read() { "rv-file-head menu-open" } else { "rv-file-head" },
                role: "button",
                tabindex: "0",
                "aria-expanded": "{is_open}",
                title: "{title}",
                onclick: move |_| open.toggle(),
                onkeydown: move |event| {
                    if event.key() == Key::Enter || event.key() == Key::Character(" ".into()) {
                        event.prevent_default();
                        open.toggle();
                    }
                },
                span { class: "rv-chevron", {icon_chevron_right()} }
                span { class: "rv-file-icon", {icon_file()} }
                span {
                    class: "rv-path",
                    if !dir.is_empty() {
                        span { class: "rv-dir", "{dir}" }
                    }
                    span { class: "rv-name", "{name}" }
                }
                if let Some(tag) = tag {
                    span { class: "rv-tag", "{tag}" }
                }
                if file.staged {
                    span { class: "rv-tag staged", "{store.t(\"git.staged\")}" }
                }
                span { class: "rv-spacer" }
                if diff.added > 0 {
                    span { class: "diff-stat add", "+{diff.added}" }
                }
                if diff.removed > 0 {
                    span { class: "diff-stat del", "−{diff.removed}" }
                }
                button {
                    class: "icon-btn rv-file-menu-btn",
                    title: store.t("git.file_actions"),
                    "aria-label": store.t("git.file_actions"),
                    onclick: move |event| {
                        event.stop_propagation();
                        let window = dioxus::desktop::window();
                        let height = window.inner_size().to_logical::<f64>(window.scale_factor()).height;
                        menu_up.set(event.client_coordinates().y > height - 180.0);
                        menu_open.toggle();
                    },
                    onkeydown: move |event| event.stop_propagation(),
                    {icon_dots_horizontal()}
                }
                if *menu_open.read() {
                    div {
                        class: "popover-backdrop",
                        onclick: move |event| {
                            event.stop_propagation();
                            menu_open.set(false);
                        },
                    }
                    div {
                        class: if *menu_up.read() { "rv-menu up" } else { "rv-menu" },
                        role: "menu",
                        onclick: move |event| event.stop_propagation(),
                        if !readonly {
                            button {
                                role: "menuitem",
                                onclick: move |_| {
                                    menu_open.set(false);
                                    on_stage.call((stage_path.clone(), true));
                                },
                                "{store.t(\"git.stage\")}"
                            }
                            button {
                                role: "menuitem",
                                disabled: !file.staged,
                                onclick: move |_| {
                                    menu_open.set(false);
                                    on_stage.call((unstage_path.clone(), false));
                                },
                                "{store.t(\"git.unstage\")}"
                            }
                        }
                        button {
                            role: "menuitem",
                            onclick: move |_| {
                                menu_open.set(false);
                                let path = copy_path.clone();
                                spawn(async move {
                                    if !crate::clipboard::copy_text(&path).await {
                                        store.toast(store.t("toast.copy_failed").to_string());
                                    }
                                });
                            },
                            "{store.t(\"git.copy_path\")}"
                        }
                    }
                }
            }
            if is_open {
                div {
                    class: "rv-diff",
                    style: "--ln-w: {gutter}ch;",
                    if let Some(note) = body_note {
                        div { class: "rv-note", "{note}" }
                    }
                    for (index, row) in rows.into_iter().enumerate() {
                        match row {
                            Row::Gap { index: gap, count, section, expandable } => rsx! {
                                div {
                                    key: "g{index}",
                                    class: if expandable { "rv-gap expandable" } else { "rv-gap" },
                                    role: if expandable { "button" } else { "presentation" },
                                    tabindex: if expandable { "0" } else { "-1" },
                                    onclick: move |_| {
                                        if expandable {
                                            expanded.with_mut(|set| { set.insert(gap); });
                                        }
                                    },
                                    span { class: "rv-gap-icon", {icon_unfold()} }
                                    span { class: "rv-gap-count", "{store.tf(\"git.unmodified\", &[&thousands(&store, count)])}" }
                                    if !section.is_empty() {
                                        span { class: "rv-gap-section", "{section}" }
                                    }
                                }
                            },
                            Row::Line(line) => {
                                let class = match line.kind {
                                    LineKind::Added => "rv-line add",
                                    LineKind::Removed => "rv-line del",
                                    LineKind::Context => "rv-line",
                                };
                                let old = line.old.map(|n| n.to_string()).unwrap_or_default();
                                let new = line.new.map(|n| n.to_string()).unwrap_or_default();
                                rsx! {
                                    div {
                                        key: "l{index}",
                                        class: "{class}",
                                        span { class: "rv-ln", "{old}" }
                                        span { class: "rv-ln", "{new}" }
                                        span { class: "rv-code", "{line.text}" }
                                    }
                                }
                            }
                        }
                    }
                    if hidden > 0 {
                        button {
                            class: "rv-more",
                            onclick: move |_| limit.with_mut(|limit| *limit += LINE_STEP),
                            "{store.tf(\"git.show_more\", &[&thousands(&store, hidden.min(LINE_STEP) as u32)])}"
                        }
                    }
                }
            }
        }
    }
}

/// Worktrees of the repository: switch to one, remove one, add a branch.
#[component]
fn WorktreeList(worktrees: Vec<GitWorktree>, on_change: EventHandler<()>) -> Element {
    let store: UiStore = use_context();
    let mut branch = use_signal(String::new);
    let create = move || {
        let Some(project) = store.current_project() else { return };
        let name = branch.read().trim().to_string();
        if name.is_empty() {
            return;
        }
        spawn(async move {
            match locagens_core::api::projects::create_worktree(&core(), &project, &name, true).await {
                Ok(_) => {
                    branch.set(String::new());
                    store.toast(store.tf("toast.worktree_created", &[&name]));
                    store.refresh_projects();
                }
                Err(err) => store.toast(store.tf("toast.worktree_failed", &[&err.to_string()])),
            }
            on_change.call(());
        });
    };
    rsx! {
        div {
            class: "rv-worktrees",
            for worktree in worktrees.iter() {
                {
                    let switch_path = worktree.path.clone();
                    let remove_path = worktree.path.clone();
                    let label = worktree.branch.clone().unwrap_or_else(|| store.t("git.detached").to_string());
                    rsx! {
                        div {
                            key: "{worktree.path}",
                            class: "rv-wt",
                            div {
                                class: "rv-wt-main",
                                role: "button",
                                tabindex: "0",
                                title: "{worktree.path}",
                                onclick: move |_| {
                                    let path = switch_path.clone();
                                    spawn(async move {
                                        let mut store = store;
                                        let input = locagens_core::api::projects::CreateProjectInput { path: Some(path.clone()), name: None };
                                        if let Err(err) = locagens_core::api::projects::create_project(&core(), input).await {
                                            store.toast(store.tf("toast.switch_failed", &[&err.to_string()]));
                                            return;
                                        }
                                        store.refresh_projects();
                                        store.selected_project.set(Some(path));
                                    });
                                },
                                span { class: "rv-wt-branch", "{label}" }
                                if worktree.is_main {
                                    span { class: "rv-tag", "{store.t(\"git.main\")}" }
                                }
                                span { class: "rv-wt-path", "{worktree.path}" }
                            }
                            if !worktree.is_main {
                                button {
                                    class: "icon-btn danger",
                                    title: store.t("git.remove_worktree"),
                                    onclick: move |_| {
                                        let path = remove_path.clone();
                                        spawn(async move {
                                            let Some(project) = store.current_project() else { return };
                                            match locagens_core::api::projects::remove_worktree(&core(), &project, &path, false).await {
                                                Ok(()) => {
                                                    store.toast(store.t("toast.worktree_removed").to_string());
                                                    store.refresh_projects();
                                                }
                                                Err(err) => store.toast(store.tf("toast.remove_failed", &[&err.to_string()])),
                                            }
                                            on_change.call(());
                                        });
                                    },
                                    {icon_x()}
                                }
                            }
                        }
                    }
                }
            }
            if worktrees.is_empty() {
                div { class: "rv-note", "{store.t(\"git.no_worktrees\")}" }
            }
            div {
                class: "rv-wt-new",
                input {
                    placeholder: store.t("git.branch"),
                    value: "{branch.read()}",
                    oninput: move |event| branch.set(event.value()),
                    onkeydown: move |event| {
                        if event.key() == Key::Enter {
                            create();
                        }
                    },
                }
                button {
                    class: "small",
                    disabled: branch.read().trim().is_empty(),
                    onclick: move |_| create(),
                    {icon_plus()}
                    "{store.t(\"git.worktree\")}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(path: &str, kind: &str, new_text: &str) -> GitFileDiff {
        GitFileDiff { path: path.into(), kind: kind.into(), old_text: String::new(), new_text: new_text.into() }
    }

    #[test]
    fn review_files_merge_diff_staged_set_and_untracked() {
        let worktree = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let staged = worktree;
        let details = [detail("notes.txt", "created", "one\ntwo\n"), detail("src/a.rs", "edited", "y\n")];
        let files = review_files(worktree, staged, &details);
        assert_eq!(files.len(), 2);
        // Details order; the untracked file is all added.
        assert_eq!((files[0].diff.path(), files[0].diff.added, files[0].staged), ("notes.txt", 2, false));
        assert_eq!((files[1].diff.path(), files[1].staged), ("src/a.rs", true));
        assert_eq!(files[1].content.as_deref(), Some("y\n"));
        assert_eq!(split_path("src/a.rs"), ("src/", "a.rs"));
        assert_eq!(split_path("x"), ("", "x"));
    }

    #[test]
    fn huge_and_generated_files_start_collapsed() {
        let small = FileDiff::whole_file("src/a.rs", "a\nb\n", false);
        let lock = FileDiff::whole_file("Cargo.lock", "a\n", false);
        let big = FileDiff::whole_file("src/big.rs", &"x\n".repeat(700), false);
        assert!(opens_by_default(0, &small) && opens_by_default(20, &small));
        assert!(!opens_by_default(0, &lock));
        assert!(!opens_by_default(0, &big));
    }
}
