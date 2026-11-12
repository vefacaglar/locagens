//! Standing permission list / revoke facade (Settings → Permissions).

use crate::api::error::{ApiError, ApiResult};
use crate::AppState;

/// Lists every persisted permission rule (`GET /api/permissions`).
pub async fn list_permissions(state: &AppState) -> ApiResult<Vec<crate::db::PermissionRule>> {
    Ok(state.db.list_permissions())
}

/// Deletes a permission rule; 404 when the id does not exist
/// (`DELETE /api/permissions/:id`).
pub async fn delete_permission(state: &AppState, id: i64) -> ApiResult<()> {
    if !state.db.delete_permission(id) {
        return Err(ApiError::not_found(format!(
            "Permission with id \"{id}\" not found"
        )));
    }
    Ok(())
}

/// Deletes every permission rule (`DELETE /api/permissions`).
pub async fn clear_permissions(state: &AppState) -> ApiResult<()> {
    state.db.clear_permissions();
    Ok(())
}
