//! Standing permission list / revoke facade (Settings → Permissions).

use crate::api::error::{ApiError, ApiResult};
use crate::AppState;

/// Lists every persisted permission rule.
pub async fn list_permissions(state: &AppState) -> ApiResult<Vec<crate::db::PermissionRule>> {
    Ok(state.db.list_permissions())
}

/// Deletes a permission rule; 404 when the id does not exist.
pub async fn delete_permission(state: &AppState, id: i64) -> ApiResult<()> {
    if !state.db.delete_permission(id) {
        return Err(ApiError::not_found(format!(
            "Permission with id \"{id}\" not found"
        )));
    }
    Ok(())
}

/// Deletes every permission rule.
pub async fn clear_permissions(state: &AppState) -> ApiResult<()> {
    state.db.clear_permissions();
    Ok(())
}

/// Re-creates a rule that was just revoked (the desktop's "Undo" after
/// Revoke). The rule gets a new id; the returned value carries it.
pub async fn restore_permission(state: &AppState, rule: &crate::db::PermissionRule) -> ApiResult<crate::db::PermissionRule> {
    Ok(state.db.upsert_permission(
        &rule.scope,
        &rule.project_path,
        &rule.tool,
        &rule.command,
        rule.network_domains.clone(),
        &rule.status,
    ))
}

/// Answers permission prompt `prompt_id` of a run (the `promptId` of its
/// `permission_requested` event). Unlike `runs::permission_decision` it never
/// answers a different prompt: when that prompt is no longer pending (already
/// answered, cancelled, or replaced by the next queued one) it is a conflict.
/// Unknown decisions are rejected; the asking side persists grants.
pub async fn answer_permission_prompt(state: &AppState, run_id: &str, prompt_id: u64, decision: &str) -> ApiResult<()> {
    use crate::orchestrator::permission_decision::PermissionDecision;
    let Some(decision) = PermissionDecision::parse(decision.trim()) else {
        return Err(ApiError::bad_request("Invalid permission decision."));
    };
    let Some(handle) = state.orchestrator.handle(run_id) else {
        return Err(ApiError::conflict("No pending permission request for this run."));
    };
    if handle.answer_permission(prompt_id, decision.as_str()).await {
        Ok(())
    } else {
        Err(ApiError::conflict("This permission request was already answered."))
    }
}
