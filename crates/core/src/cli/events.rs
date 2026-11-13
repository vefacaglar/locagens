//! Run-event pump shared by `lcgn -p`, the REPL and the TUI (QA-046, QA-049).
//!
//! A broadcast receiver that falls behind drops events. If a dropped event was
//! `permission_requested`, `question_requested` or a terminal event, the run
//! waits on a prompt nobody shows (or a frontend waits for an end that never
//! comes). [`RunEvents`] therefore treats `RecvError::Lagged` (and the
//! `{"type":"lagged"}` marker that `api::runs::subscribe_run_events` forwards)
//! as a signal to rebuild state from the store: it yields [`Pumped::Resynced`]
//! with the persisted messages, then replays the still-pending requests and,
//! for a run that already ended, its terminal event.

use crate::api::runs::{resync_run, RunResync};
use crate::api::{EventRx, RunEvent};
use crate::state::AppState;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// What the pump hands to a frontend.
#[derive(Debug)]
pub enum Pumped {
    /// A run event, in order.
    Event(RunEvent),
    /// Events were missed; the persisted state follows. Frontends that keep
    /// a transcript re-apply `messages`; the replayed requests and terminal
    /// event arrive as ordinary [`Pumped::Event`]s right after.
    Resynced(Box<RunResync>),
    /// The channel closed.
    Closed,
}

/// A run's event receiver plus the resync-on-lag behaviour.
pub struct RunEvents {
    state: Arc<AppState>,
    run_id: String,
    rx: EventRx,
    queued: VecDeque<Pumped>,
    /// Set while a resync is owed (kept across a dropped `next()` future so
    /// the pump stays cancel-safe inside `select!`).
    resync_owed: bool,
}

impl RunEvents {
    pub fn new(state: Arc<AppState>, run_id: impl Into<String>, rx: EventRx) -> Self {
        Self { state, run_id: run_id.into(), rx, queued: VecDeque::new(), resync_owed: false }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The next thing to show. Cancel-safe.
    pub async fn next(&mut self) -> Pumped {
        loop {
            if let Some(item) = self.queued.pop_front() {
                return item;
            }
            if self.resync_owed {
                let mut resync = resync_run(&self.state, &self.run_id).await;
                let events = std::mem::take(&mut resync.events);
                self.queued.push_back(Pumped::Resynced(Box::new(resync)));
                self.queued.extend(events.into_iter().map(Pumped::Event));
                self.resync_owed = false;
                continue;
            }
            match self.rx.recv().await {
                Ok(event) if is_lagged_marker(&event) => self.resync_owed = true,
                Ok(event) => return Pumped::Event(event),
                Err(RecvError::Lagged(_)) => self.resync_owed = true,
                Err(RecvError::Closed) => return Pumped::Closed,
            }
        }
    }
}

/// `api::runs::subscribe_run_events` reports lag as `{"type":"lagged"}`.
pub fn is_lagged_marker(event: &RunEvent) -> bool {
    event.get("type").and_then(|kind| kind.as_str()) == Some("lagged")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::Orchestrator;
    use serde_json::json;

    fn state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("lcgn-events-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(AppState {
            db: Arc::new(crate::db::Db::open(&dir).unwrap()),
            pi: Arc::new(crate::pi_providers::PiProviderManager::with_dirs(dir.clone(), dir.clone())),
            orchestrator: Arc::new(Orchestrator::new()),
            settings_path: dir.join("settings.json"),
            default_project_path: dir.clone(),
            processes: Arc::new(crate::services::processes::ProcessManager::new()),
        })
    }

    #[tokio::test]
    async fn a_lagged_receiver_resyncs_pending_prompts_from_the_run_handle() {
        let state = state();
        state.db.create_run(crate::db::Run {
            id: "run-lag".into(),
            status: "awaiting_permission".into(),
            created_at: crate::util::now_iso(),
            ..Default::default()
        });
        state.db.append_message(crate::db::RunMessage {
            id: "m1".into(),
            run_id: "run-lag".into(),
            role: "user".into(),
            content: "hi".into(),
            created_at: crate::util::now_iso(),
            ..Default::default()
        });
        let handle = state.orchestrator.ensure_handle("run-lag");
        // A permission request is pending on the handle…
        {
            let mut slot = handle.permission.lock().await;
            slot.pending = Some(crate::orchestrator::PendingPermission {
                tool_call: json!({ "function": { "name": "run_command" } }),
                preview: Some(json!({ "action": "command", "command": "ls" })),
            });
        }
        // …but the receiver was so slow that its request event was dropped.
        let rx = handle.events.subscribe();
        for i in 0..2000 {
            handle.emit(json!({ "type": "message_updated", "i": i }));
        }

        let mut events = RunEvents::new(state.clone(), "run-lag", rx);
        let mut saw_resync = false;
        let mut saw_permission = false;
        for _ in 0..2100 {
            match events.next().await {
                Pumped::Resynced(resync) => {
                    saw_resync = true;
                    assert_eq!(resync.messages.len(), 1, "persisted messages are handed over");
                }
                Pumped::Event(event) if event["type"] == "permission_requested" => {
                    assert_eq!(event["toolCall"]["function"]["name"], "run_command");
                    saw_permission = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_resync && saw_permission, "the lost prompt must be replayed");
    }

    #[tokio::test]
    async fn the_lagged_marker_and_a_finished_run_yield_the_terminal_event() {
        let state = state();
        state.db.create_run(crate::db::Run {
            id: "run-end".into(),
            status: "done".into(),
            created_at: crate::util::now_iso(),
            ..Default::default()
        });
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        tx.send(json!({ "type": "lagged" })).unwrap();
        let mut events = RunEvents::new(state, "run-end", rx);
        assert!(matches!(events.next().await, Pumped::Resynced(_)));
        match events.next().await {
            Pumped::Event(event) => assert_eq!(event["type"], "run_completed"),
            other => panic!("expected the terminal event, got {other:?}"),
        }
        drop(tx);
        assert!(matches!(events.next().await, Pumped::Closed));
    }
}
