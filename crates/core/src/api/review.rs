//! Review facade: starts a code review (a read-only run over a git diff) and
//! lists the targets a UI can offer. Logic lives in `orchestrator::review`.

use crate::api::runs::{self, CreateRunPayload, StartedRun};
use crate::api::{ApiError, ApiResult};
use crate::db::RunMessage;
use crate::orchestrator::review as engine;
use crate::AppState;
use locagens_protocol::review::{ReviewPayload, ReviewTarget};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Body of `start_review`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StartReviewPayload {
    pub target: ReviewTarget,
    pub project_path: Option<String>,
    pub provider_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

/// What a UI can offer for the project: current branch, branches, recent
/// commits and whether there are uncommitted changes.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReviewChoices {
    pub is_git_repo: bool,
    pub current_branch: Option<String>,
    pub branches: Vec<String>,
    pub commits: Vec<ReviewCommit>,
    pub has_uncommitted_changes: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommit {
    pub sha: String,
    pub subject: String,
}

fn project_dir(state: &AppState, project_path: Option<&str>) -> PathBuf {
    project_path
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state.default_project_path.clone())
}

/// Starts a review run (mode `chat`: read-only tools, no commands) on the
/// selected model. The diff is gathered with read-only git before the run
/// starts, so an empty or invalid target fails fast with `bad_request`.
/// The run's last assistant message carries the structured `ReviewPayload`
/// in `rawResponse` (see `review_of_message`).
pub async fn start_review(state: &AppState, payload: &StartReviewPayload) -> ApiResult<StartedRun> {
    let project = project_dir(state, payload.project_path.as_deref());
    let target = payload.target.clone();
    let resolved = {
        let project = project.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || engine::resolve_review(&project, &target))
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?
            .map_err(ApiError::bad_request)?
    };
    let create = CreateRunPayload {
        task: resolved.task,
        project_path: Some(project.display().to_string()),
        provider_id: payload.provider_id.clone(),
        model: payload.model.clone(),
        reasoning_effort: payload.reasoning_effort.clone(),
        mode: Some(engine::REVIEW_RUN_MODE.to_string()),
        append_system_prompt: Some(engine::append_prompt(&target, &resolved.diff)),
        ..Default::default()
    };
    let mut started = runs::create_run(state, &create).await?;
    let title = format!("Review: {}", target.hint());
    state.db.patch_run(&started.run.id, |run| run.title = title.clone());
    started.handle.emit(serde_json::json!({
        "type": "run_title_changed",
        "runId": started.run.id,
        "title": title,
    }));
    started.run.title = title;
    Ok(started)
}

/// Branches, recent commits and change state for a target picker.
pub async fn review_choices(state: &AppState, project_path: Option<&str>) -> ApiResult<ReviewChoices> {
    let project = project_dir(state, project_path);
    tokio::task::spawn_blocking(move || {
        if !engine::is_git_repo(&project) {
            return ReviewChoices::default();
        }
        ReviewChoices {
            is_git_repo: true,
            current_branch: engine::current_branch(&project),
            branches: engine::local_branches(&project),
            commits: engine::recent_commits(&project, 20)
                .into_iter()
                .map(|(sha, subject)| ReviewCommit { sha, subject })
                .collect(),
            has_uncommitted_changes: engine::has_uncommitted_changes(&project),
        }
    })
    .await
    .map_err(|err| ApiError::internal(err.to_string()))
}

/// The structured review stored on an assistant message, if it is one.
pub fn review_of_message(message: &RunMessage) -> Option<ReviewPayload> {
    if message.role != "assistant" {
        return None;
    }
    ReviewPayload::from_raw(message.raw_response.as_deref()?)
}

/// Whether `run_id` is a review run.
pub fn is_review_run(state: &AppState, run_id: &str) -> bool {
    state.db.get_run_full(run_id).map(|run| engine::is_review_run(&run)).unwrap_or(false)
}
