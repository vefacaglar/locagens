//! Interactive tools: `update_plan` and `ask_user_question` /
//! `request_user_input`.

use super::*;

impl AgentLoop {
    pub(super) async fn update_plan(&self, args: &Value) -> Result<String, AgentError> {
        // Accepts BOTH schemas:
        // - Codex:   { explanation?, plan: [{ step, status }] }
        // - Locagens: { title, body?, tasks: [{ text, status }] }
        let (title, body, tasks) = normalize_plan_args(args);
        if title.is_empty() {
            return Err(AgentError::Failed("update_plan: title is empty.".into()));
        }

        let existing = self.state.db.get_plan(&self.run_id);
        let now = crate::util::now_iso();
        let plan = match existing {
            Some(mut plan) => {
                plan.title = title;
                plan.body = body;
                plan.tasks = tasks;
                plan.version += 1;
                plan.updated_at = now;
                plan
            }
            None => Plan {
                id: format!("plan-{}", uuid::Uuid::new_v4()),
                run_id: self.run_id.clone(),
                title,
                body,
                tasks,
                status: "active".to_string(),
                version: 1,
                created_at: now.clone(),
                updated_at: now,
            },
        };
        self.state.db.set_plan(plan.clone());
        self.handle.emit(json!({
            "type": "plan_updated",
            "plan": serde_json::to_value(&plan).unwrap_or(Value::Null),
        }));
        Ok("Plan updated.".to_string())
    }

    pub(super) async fn ask_user_question(&self, args: &Value) -> Result<String, AgentError> {
        let questions = args.get("questions").cloned().unwrap_or(Value::Null);
        let valid = questions
            .as_array()
            .map(|items| !items.is_empty() && items.iter().all(|q| q.get("question").is_some() && q.get("options").map(Value::is_array).unwrap_or(false)))
            .unwrap_or(false);
        if !valid {
            return Err(AgentError::Failed(
                "ask_user_question: questions must be a non-empty array of {question, header, options}."
                    .to_string(),
            ));
        }

        // Pause the run until the user answers (`api::runs::answer_question`).
        self.notification_hook("elicitation_dialog", "Locagens is waiting for your answer").await;
        self.state.db.patch_run(&self.run_id, |run| run.status = "awaiting_input".into());
        self.handle.emit(json!({ "type": "status_changed", "status": "awaiting_input" }));
        self.handle.emit(json!({
            "type": "question_requested",
            "runId": self.run_id,
            "questions": questions,
        }));

        let (sender, receiver) = oneshot::channel::<Value>();
        {
            let mut slot = self.handle.question.lock().await;
            slot.pending = Some(questions.clone());
            slot.sender = Some(sender);
        }
        let answer = tokio::select! {
            answer = receiver => answer.ok(),
            _ = self.handle.cancelled() => None,
        };
        {
            let mut slot = self.handle.question.lock().await;
            slot.pending = None;
            slot.sender = None;
        }
        if answer.is_none() {
            return Err(AgentError::Cancelled);
        }

        self.state.db.patch_run(&self.run_id, |run| run.status = "generating".into());
        self.handle.emit(json!({ "type": "status_changed", "status": "generating" }));

        Ok(format!(
            "The user answered your questions:\n{}",
            serde_json::to_string_pretty(&answer.unwrap()).unwrap_or_else(|_| "{}".into())
        ))
    }
}

/// Normalizes both `update_plan` schemas into `(title, body, tasks)` where
/// tasks are Locagens `{ text, status }` objects.
pub(super) fn normalize_plan_args(args: &Value) -> (String, Option<String>, Value) {
    let items = args
        .get("tasks")
        .or_else(|| args.get("plan"))
        .cloned()
        .unwrap_or(json!([]));
    let mut first_text: Option<String> = None;
    let tasks = match items {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .filter_map(|item| match item {
                    Value::String(text) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            None
                        } else {
                            first_text.get_or_insert_with(|| text.clone());
                            Some(json!({ "text": text, "status": "pending" }))
                        }
                    }
                    Value::Object(_) => {
                        let text = item
                            .get("text")
                            .or_else(|| item.get("step"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        if text.is_empty() {
                            None
                        } else {
                            first_text.get_or_insert_with(|| text.clone());
                            let status = item
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("pending")
                                .to_string();
                            let status = ["pending", "in_progress", "completed"]
                                .iter()
                                .any(|valid| *valid == status)
                                .then_some(status.clone())
                                .unwrap_or_else(|| "pending".to_string());
                            Some(json!({ "text": text, "status": status }))
                        }
                    }
                    _ => None,
                })
                .collect(),
        ),
        _ => json!([]),
    };
    let title = args
        .get("title")
        .or_else(|| args.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .or_else(|| first_text.take())
        .unwrap_or_default();
    let body = args
        .get("body")
        .or_else(|| args.get("explanation"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|body| !body.trim().is_empty());
    (title, body, tasks)
}

/// Normalizes Codex `request_user_input` questions for the QuestionCard flow:
/// every question ends up with a header and at least two options.
pub(super) fn normalize_request_user_input(args: &Value) -> Value {
    let questions = args.get("questions").cloned().unwrap_or(json!([]));
    let normalized = questions
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|question| {
                    let header = question
                        .get("header")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|h| !h.is_empty())
                        .unwrap_or("Choice")
                        .to_string();
                    let options = match question.get("options") {
                        Some(options) if options.is_array() && !options.as_array().unwrap().is_empty() => {
                            options.clone()
                        }
                        _ => json!([{ "label": "Yes" }, { "label": "No" }]),
                    };
                    let mut question = question.clone();
                    let object = question.as_object_mut().unwrap();
                    object.insert("header".into(), json!(header));
                    object.insert("options".into(), options);
                    question
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({ "questions": normalized })
}
