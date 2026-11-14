//! Mid-run message queue facade: what is waiting in a live run's inbox, and
//! withdrawing an entry before the model picks it up (UX-011). Queuing
//! itself is `runs::queue_message`.

use crate::api::error::ApiResult;
use crate::orchestrator::RunHandle;
use crate::AppState;

/// Messages queued on a live run, oldest first (empty when the run is not
/// live or nothing waits).
pub async fn queued_messages(state: &AppState, run_id: &str) -> ApiResult<Vec<String>> {
    let Some(handle) = state.orchestrator.handle(run_id) else { return Ok(Vec::new()) };
    let inbox = handle.inbox.lock().await;
    Ok(inbox.iter().cloned().collect())
}

/// Takes the newest queued message back out of the run's inbox when it is
/// still `content` (not yet picked up by the model). Returns whether it was
/// withdrawn.
pub async fn withdraw_queued_message(handle: &RunHandle, content: &str) -> bool {
    let mut inbox = handle.inbox.lock().await;
    if inbox.back().map(String::as_str) == Some(content.trim()) {
        inbox.pop_back();
        true
    } else {
        false
    }
}

/// Removes the oldest queued message equal to `content` (trimmed) from the
/// run `run_id`, wherever it sits in the queue. Returns `false` when it is
/// no longer queued: the model already took it, or the run ended.
pub async fn withdraw_queued(state: &AppState, run_id: &str, content: &str) -> ApiResult<bool> {
    let Some(handle) = state.orchestrator.handle(run_id) else { return Ok(false) };
    let content = content.trim();
    let mut inbox = handle.inbox.lock().await;
    match inbox.iter().position(|queued| queued == content) {
        Some(index) => {
            inbox.remove(index);
            Ok(true)
        }
        None => Ok(false),
    }
}
