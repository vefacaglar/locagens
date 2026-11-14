//! Withdrawing a message that was queued on a running run.

use crate::orchestrator::RunHandle;

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
