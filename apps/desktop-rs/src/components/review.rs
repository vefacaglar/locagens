//! Code review in the desktop UI: the findings card shown in the thread, the
//! `/review` composer command and the header action that starts a review.

use super::icons::{icon_check, icon_list_checks, icon_x};
use crate::markdown::render_markdown;
use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::review::{review_of_message, start_review, StartReviewPayload};
use locagens_core::db::RunMessage;
use locagens_core::protocol::review::{ReviewFinding, ReviewPayload, ReviewTarget};

/// Styles of the findings card; token colours only (no hard-coded hex).
pub const REVIEW_CSS: &str = r#"
.review-card {
  display: flex; flex-direction: column; gap: 10px;
  padding: 12px 14px; border: 1px solid var(--border); border-radius: 10px;
  background: var(--bg-elevated); box-shadow: var(--shadow-card);
}
.review-head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 13px; }
.review-head .review-name { font-weight: 600; display: inline-flex; align-items: center; gap: 6px; }
.review-head .review-hint { color: var(--text-2); }
.review-verdict {
  margin-left: auto; display: inline-flex; align-items: center; gap: 4px;
  padding: 2px 8px; border-radius: 999px; font-size: 12px; background: var(--chip-bg);
}
.review-verdict.ok { color: var(--ok); }
.review-verdict.bad { color: var(--err); }
.review-summary { margin: 0; font-size: 13px; line-height: 1.55; color: var(--text-2); }
.review-finding {
  display: flex; flex-direction: column; gap: 6px; padding: 10px 12px;
  border: 1px solid var(--rule); border-radius: 8px; background: var(--bg-panel);
}
.review-finding-head { display: flex; align-items: center; gap: 8px; }
.review-finding-title { font-weight: 600; font-size: 13px; word-break: break-word; }
.review-chip {
  flex: none; padding: 1px 7px; border-radius: 6px; font-size: 11px; font-weight: 600;
  background: var(--chip-bg); color: var(--text-2);
}
.review-chip.p0, .review-chip.p1 { color: var(--err); }
.review-chip.p2 { color: var(--warn); }
.review-loc { font-family: var(--font-mono); font-size: 12px; color: var(--text-3); word-break: break-all; }
.review-body { font-size: 13px; line-height: 1.55; word-break: break-word; }
.review-body p { margin: 0 0 8px; }
.review-body p:last-child { margin-bottom: 0; }
.review-empty { font-size: 13px; color: var(--text-2); }
.review-pending { color: var(--text-3); font-size: 13px; }
"#;

/// What the card shows for one finding. The body is rendered markdown with
/// raw HTML escaped (see `crate::markdown`); every other field is plain text
/// that the UI escapes when it renders it.
#[derive(Clone, Debug, PartialEq)]
pub struct FindingView {
    pub chip_class: &'static str,
    pub chip_text: String,
    pub title: String,
    pub location: String,
    pub body_html: String,
}

pub fn finding_view(finding: &ReviewFinding) -> FindingView {
    let chip_class = match finding.priority {
        Some(0) => "review-chip p0",
        Some(1) => "review-chip p1",
        Some(2) => "review-chip p2",
        Some(3) => "review-chip p3",
        _ => "review-chip",
    };
    FindingView {
        chip_class,
        chip_text: finding.priority_label(),
        title: finding.plain_title(),
        location: finding.location_label(),
        body_html: render_markdown(&finding.body),
    }
}

/// `true` when the reviewer judged the patch correct, `false` when incorrect,
/// `None` when it gave no verdict.
pub fn verdict_ok(payload: &ReviewPayload) -> Option<bool> {
    let verdict = payload.output.overall_correctness.trim().to_lowercase();
    if verdict.is_empty() {
        None
    } else {
        Some(!verdict.contains("incorrect"))
    }
}

/// Body of an assistant message: the findings card for a review answer, a
/// placeholder while a review streams its JSON, the markdown bubble otherwise.
#[component]
pub fn AssistantBody(message: RunMessage) -> Element {
    let store: UiStore = use_context();
    if let Some(payload) = review_of_message(&message) {
        return rsx! { ReviewCard { payload } };
    }
    if message.content.is_empty() {
        return rsx! {};
    }
    let streaming_review = store
        .active_run
        .read()
        .as_ref()
        .map(|run| {
            locagens_core::orchestrator::review::is_review_run(run)
                && super::run_active(Some(run))
        })
        .unwrap_or(false);
    let trimmed = message.content.trim_start();
    if streaming_review && (trimmed.starts_with('{') || trimmed.starts_with("```")) {
        return rsx! { div { class: "review-pending", "{store.t(\"review.writing\")}" } };
    }
    rsx! {
        div { class: "bubble md", dangerous_inner_html: render_markdown(&message.content) }
    }
}

#[component]
pub fn ReviewCard(payload: ReviewPayload) -> Element {
    let store: UiStore = use_context();
    let output = &payload.output;
    let verdict = verdict_ok(&payload);
    let findings: Vec<FindingView> = output.findings.iter().map(finding_view).collect();
    let summary = output.overall_explanation.trim().to_string();
    rsx! {
        div {
            class: "review-card",
            div {
                class: "review-head",
                span { class: "review-name", {icon_list_checks()} "{store.t(\"review.title\")}" }
                span { class: "review-hint", "{payload.hint}" }
                if let Some(ok) = verdict {
                    span {
                        class: if ok { "review-verdict ok" } else { "review-verdict bad" },
                        if ok { {icon_check()} } else { {icon_x()} }
                        "{output.overall_correctness.trim()}"
                    }
                }
            }
            if !summary.is_empty() {
                p { class: "review-summary", "{summary}" }
            }
            if findings.is_empty() {
                div { class: "review-empty", "{store.t(\"review.none\")}" }
            }
            for (index, finding) in findings.into_iter().enumerate() {
                div {
                    class: "review-finding",
                    key: "{index}",
                    div {
                        class: "review-finding-head",
                        span { class: "{finding.chip_class}", "{finding.chip_text}" }
                        span { class: "review-finding-title", "{finding.title}" }
                    }
                    if !finding.location.is_empty() {
                        div { class: "review-loc", "{finding.location}" }
                    }
                    div { class: "review-body md", dangerous_inner_html: finding.body_html }
                }
            }
        }
    }
}

/// Header button that reviews the uncommitted changes of the current project.
#[component]
pub fn ReviewButton() -> Element {
    let store: UiStore = use_context();
    rsx! {
        button {
            class: "icon-btn",
            title: store.t("review.button"),
            onclick: move |_| start(&store, ReviewTarget::Uncommitted),
            {icon_list_checks()}
        }
    }
}

/// Composer `/review [base <branch> | commit <sha> | <instructions>]`.
pub fn start_from_command(store: &UiStore, argument: &str) {
    match locagens_core::cli::review::target_from_slash(argument) {
        Ok(target) => {
            let mut store = *store;
            store.prompt.set(String::new());
            start(&store, target);
        }
        Err(message) => store.toast(message),
    }
}

/// Starts a review run on the selected model and shows it in the thread.
pub fn start(store: &UiStore, target: ReviewTarget) {
    let mut store = *store;
    let provider_id = store.selected_provider.read().clone();
    let model = store.selected_model.read().clone();
    if provider_id.is_empty() || model.is_empty() {
        store.toast(store.t("toast.select_provider").to_string());
        return;
    }
    let project_path = store
        .selected_project
        .read()
        .clone()
        .or_else(|| store.active_run.read().as_ref().map(|run| run.project_path.clone()));
    let effort = store.selected_effort.read().clone();
    let payload = StartReviewPayload {
        target,
        project_path,
        provider_id,
        model,
        reasoning_effort: Some(effort).filter(|e| !e.is_empty()),
    };
    spawn(async move {
        let state = core();
        match start_review(&state, &payload).await {
            Ok(started) => {
                let run_id = started.run.id.clone();
                store.active_run_id.set(Some(run_id.clone()));
                store.active_run.set(Some(started.run));
                let messages = locagens_core::api::runs::list_messages(&state, &run_id)
                    .await
                    .unwrap_or_default();
                store.messages.set(messages);
                store.plan.set(None);
                store.run_usage.set(None);
                store.context_usage.set(None);
                store.pending_permission.set(None);
                store.pending_question.set(None);
                store.turn_diff.set(None);
                store.spawned_agents.set(Vec::new());
                store.attach_event_stream(run_id, started.events);
                store.refresh_runs();
            }
            Err(err) => store.toast(store.tf("review.failed", &[&err.message])),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use locagens_core::protocol::review::parse_review_output;

    fn finding(title: &str, body: &str, priority: Option<i32>) -> ReviewFinding {
        let mut finding = ReviewFinding {
            title: title.to_string(),
            body: body.to_string(),
            priority,
            ..Default::default()
        };
        finding.code_location.absolute_file_path = "src/a.rs".to_string();
        finding.code_location.line_range.start = 4;
        finding.code_location.line_range.end = 9;
        finding
    }

    #[test]
    fn finding_view_escapes_model_html_in_the_body() {
        let view = finding_view(&finding(
            "[P1] Unchecked input",
            "Calls <script>alert(1)</script> and <img src=x onerror=boom()>.",
            Some(1),
        ));
        assert!(!view.body_html.contains("<script"), "{}", view.body_html);
        assert!(!view.body_html.contains("<img"), "{}", view.body_html);
        assert!(view.body_html.contains("&lt;script&gt;"));
        assert_eq!(view.chip_text, "P1");
        assert_eq!(view.chip_class, "review-chip p1");
        assert_eq!(view.title, "Unchecked input");
        assert_eq!(view.location, "src/a.rs:4-9");
    }

    #[test]
    fn finding_view_handles_unknown_priority_and_code_in_body() {
        let view = finding_view(&finding("Plain", "Use `a < b` here.", None));
        assert_eq!(view.chip_text, "P?");
        assert_eq!(view.chip_class, "review-chip");
        assert!(view.body_html.contains("<code>"));
        assert!(view.body_html.contains("a &lt; b"));
    }

    #[test]
    fn verdict_follows_the_reviewer_text() {
        let correct = ReviewPayload::new(
            ReviewTarget::Uncommitted,
            parse_review_output(r#"{"findings":[],"overall_correctness":"patch is correct"}"#),
        );
        assert_eq!(verdict_ok(&correct), Some(true));
        let incorrect = ReviewPayload::new(
            ReviewTarget::Uncommitted,
            parse_review_output(r#"{"findings":[],"overall_correctness":"patch is incorrect"}"#),
        );
        assert_eq!(verdict_ok(&incorrect), Some(false));
        let none = ReviewPayload::new(ReviewTarget::Uncommitted, parse_review_output("free text"));
        assert_eq!(verdict_ok(&none), None);
    }

    #[test]
    fn review_css_uses_tokens_only() {
        let hex = REVIEW_CSS
            .split('#')
            .skip(1)
            .filter(|rest| rest.chars().take_while(|c| c.is_ascii_hexdigit()).count() >= 3)
            .count();
        assert_eq!(hex, 0, "review styles must not hard-code colours");
    }
}
