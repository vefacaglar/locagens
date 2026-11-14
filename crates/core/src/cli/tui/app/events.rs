//! Run-event reducer: turns `run_event`s into transcript cells.

use super::*;

impl App {
    // ---- run-event reducer ---------------------------------------------------

    pub fn on_run_event(&mut self, event: &Value) {
        self.dirty = true;
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "status_changed" => {
                if event.get("status").and_then(Value::as_str) == Some("cancelled") {
                    self.finish(Finish::Cancelled);
                }
            }
            "message_created" | "message_updated" => {
                if let Some(message) = event.get("message") {
                    self.on_message(message);
                }
            }
            "permission_requested" => self.open_approval(event),
            "question_requested" => {
                self.overlay = Some(Overlay::Question(QuestionFlow::from_event(event)));
            }
            "run_title_changed" => {
                self.title = event.get("title").and_then(Value::as_str).map(str::to_string);
            }
            "plan_updated" => self.on_plan(event),
            "turn_diff_updated" => {
                let diff = event.get("diff").and_then(Value::as_str).unwrap_or("");
                let files = event.get("filesChanged").and_then(Value::as_u64).unwrap_or(0);
                if !diff.is_empty() {
                    self.turn_diff = Some((diff.to_string(), files));
                }
            }
            "context_compacted" => {
                self.push_notice("Older messages were summarized to free up context.");
            }
            "agent_spawned" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("agent");
                let role = event.get("role").and_then(Value::as_str).unwrap_or("explorer");
                self.push_notice(format!("spawned {role} agent \"{name}\""));
            }
            "patch_progress" => {
                let complete = event.get("complete").and_then(Value::as_bool).unwrap_or(true);
                self.patch_progress = (!complete).then(|| patch_progress_label(event));
            }
            "run_completed" => self.finish(Finish::Completed),
            "run_failed" => {
                let message = event
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("Run failed.");
                self.push_error(message.to_string());
                self.push_notice("Send a message to continue · /undo reverts this turn's file changes");
                self.finish(Finish::Failed);
            }
            _ => {}
        }
    }

    pub(super) fn open_approval(&mut self, event: &Value) {
        let approval = Approval::from_event(event);
        let call_id = event.pointer("/toolCall/id").and_then(Value::as_str).unwrap_or("");
        if let Some(diff) = approval.diff.clone() {
            if let Some(cell) = self.cells.iter_mut().rev().find(|c| {
                matches!(c.kind(), CellKind::Tool { call_id: id, .. } if id == call_id)
            }) {
                cell.update(|kind| {
                    if let CellKind::Tool { diff: slot, .. } = kind {
                        *slot = Some(diff);
                    }
                });
            }
        }
        self.overlay = Some(Overlay::Approval(approval));
    }

    pub(super) fn on_plan(&mut self, event: &Value) {
        let title = event.pointer("/plan/title").and_then(Value::as_str).unwrap_or("Plan");
        let body = event.pointer("/plan/body").and_then(Value::as_str).unwrap_or("");
        let text = format!("# {title}\n\n{body}");
        let existing = self
            .cells
            .iter_mut()
            .rev()
            .find(|c| c.msg_id.as_deref() == Some("__plan__"));
        match existing {
            Some(cell) => cell.update(|kind| *kind = CellKind::Assistant { text }),
            None => self
                .cells
                .push(Cell::from_message("__plan__", None, CellKind::Assistant { text })),
        }
    }

    pub(super) fn on_message(&mut self, message: &Value) {
        let id = message.get("id").and_then(Value::as_str).unwrap_or("");
        let agent = agent_tag(message);
        match message.get("role").and_then(Value::as_str).unwrap_or("") {
            "assistant" => self.on_assistant_message(id, agent, message),
            "tool" => self.on_tool_message(message),
            "user" => self.on_user_message(id, message),
            _ => {}
        }
    }

    pub(super) fn on_assistant_message(&mut self, id: &str, agent: Option<String>, message: &Value) {
        let reasoning = message.get("reasoningContent").and_then(Value::as_str).unwrap_or("");
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        if !reasoning.trim().is_empty() {
            self.upsert_text(id, &agent, reasoning, true);
        }
        if !content.trim().is_empty() {
            self.upsert_text(id, &agent, content, false);
        }
        let calls = message
            .get("rawResponse")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
            .unwrap_or_default();
        for call in calls {
            self.upsert_tool_call(id, &agent, &call);
        }
    }

    /// Creates or updates the reasoning/assistant text cell of a message.
    pub(super) fn upsert_text(&mut self, id: &str, agent: &Option<String>, text: &str, reasoning: bool) {
        let is_target = |kind: &CellKind| match kind {
            CellKind::Reasoning { .. } => reasoning,
            CellKind::Assistant { .. } => !reasoning,
            _ => false,
        };
        let position = self
            .cells
            .iter()
            .rposition(|c| c.msg_id.as_deref() == Some(id) && is_target(c.kind()));
        let new_kind = || {
            if reasoning {
                CellKind::Reasoning { text: text.to_string() }
            } else {
                CellKind::Assistant { text: text.to_string() }
            }
        };
        match position {
            Some(index) => {
                let unchanged = matches!(self.cells[index].kind(),
                    CellKind::Reasoning { text: t } | CellKind::Assistant { text: t } if t == text);
                if !unchanged {
                    self.cells[index].update(|kind| *kind = new_kind());
                }
            }
            None => {
                let cell = Cell::from_message(id, agent.clone(), new_kind());
                // Reasoning always precedes the answer text of the same message.
                let before_answer = reasoning
                    .then(|| {
                        self.cells.iter().rposition(|c| {
                            c.msg_id.as_deref() == Some(id)
                                && matches!(c.kind(), CellKind::Assistant { .. })
                        })
                    })
                    .flatten();
                match before_answer {
                    Some(index) => self.cells.insert(index, cell),
                    None => self.cells.push(cell),
                }
            }
        }
    }

    pub(super) fn upsert_tool_call(&mut self, msg_id: &str, agent: &Option<String>, call: &Value) {
        let call_id = call.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool");
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .to_string();
        let existing = self.cells.iter_mut().rev().find(|c| {
            matches!(c.kind(), CellKind::Tool { call_id: id, .. } if *id == call_id)
        });
        match existing {
            Some(cell) => cell.update(|kind| {
                if let CellKind::Tool { arguments: slot, name: n, .. } = kind {
                    *slot = arguments;
                    *n = name.to_string();
                }
            }),
            None => self.cells.push(Cell::from_message(
                msg_id,
                agent.clone(),
                CellKind::Tool {
                    call_id,
                    name: name.to_string(),
                    arguments,
                    result: None,
                    diff: None,
                },
            )),
        }
    }

    pub(super) fn on_tool_message(&mut self, message: &Value) {
        let call_id = message.get("rawResponse").and_then(Value::as_str).unwrap_or("");
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        let result = ToolResult {
            ok: !(content.starts_with("Tool error")
                || content.starts_with("The user denied permission")),
            output: content.to_string(),
        };
        let existing = self.cells.iter_mut().rev().find(|c| {
            matches!(c.kind(), CellKind::Tool { call_id: id, .. } if id == call_id)
        });
        match existing {
            Some(cell) => cell.update(|kind| {
                if let CellKind::Tool { result: slot, .. } = kind {
                    *slot = Some(result);
                }
            }),
            None => self.push(CellKind::Tool {
                call_id: call_id.to_string(),
                name: "tool".to_string(),
                arguments: "{}".to_string(),
                result: Some(result),
                diff: None,
            }),
        }
    }

    pub(super) fn on_user_message(&mut self, id: &str, message: &Value) {
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        if content.starts_with("<environment_context>") {
            return;
        }
        // A queued message was folded into the run: it leaves the strip and
        // shows up in the transcript below.
        if self.queued.pop_front().is_some() {
            self.dirty = true;
        }
        let seen = !id.is_empty()
            && self
                .cells
                .iter()
                .any(|c| c.msg_id.as_deref() == Some(id) && matches!(c.kind(), CellKind::User { .. }));
        if !seen {
            self.cells.push(Cell::from_message(
                id,
                None,
                CellKind::User {
                    text: content.to_string(),
                },
            ));
        }
    }

    /// Replays persisted messages when resuming a session.
    pub fn load_history(&mut self, messages: &[Value]) {
        for message in messages {
            self.on_message(message);
        }
        self.viewport.follow = true;
        self.dirty = true;
    }

}
