//! Automatic chat titles: on a run's first turn one small tool-free model
//! call names the chat from the user's first message, in parallel with the
//! turn. A title set meanwhile (the `set_chat_title` tool, a rename, a
//! review run) wins: the generated one only replaces the placeholder.
//! Interactive frontends opt in per run (`CreateRunPayload::auto_title`);
//! headless `lcgn -p` runs skip the extra call.

use super::drive::{cost_of, endpoint_http, resolve_endpoint};
use super::llm::{self, LlmMessage};
use super::RunHandle;
use crate::db::{Run, UsageLogEntry};
use crate::AppState;
use serde_json::json;
use std::sync::Arc;

const MAX_TITLE_CHARS: usize = 60;
/// Characters of the first message sent to the title call.
const MAX_PROMPT_CHARS: usize = 2_000;
/// Usage-log role of the title call.
pub const TITLE_ROLE: &str = "title";

pub const TITLE_PROMPT: &str = "You name chats in a coding assistant. Reply with a short title \
(2 to 6 words) for the conversation that starts with the user's message. Write it in the \
language of the message. Reply with the title only: no quotes, no markdown, no trailing period.";

/// The placeholder title of a new run: the task's first line, cut at
/// [`MAX_TITLE_CHARS`].
pub fn fallback_title(task: &str) -> String {
    let first_line = task.lines().next().unwrap_or("").trim();
    let mut title: String = first_line.chars().take(MAX_TITLE_CHARS).collect();
    if first_line.chars().count() > MAX_TITLE_CHARS {
        title.push('…');
    }
    if title.is_empty() {
        title = "New chat".to_string();
    }
    title
}

/// A clean one-line title from the model's reply, if it has one.
pub fn title_from_reply(reply: &str) -> Option<String> {
    let reply = match (reply.find("<think>"), reply.find("</think>")) {
        (Some(_), Some(end)) => &reply[end + "</think>".len()..],
        _ => reply,
    };
    let line = reply.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line.trim_start_matches(['#', '*', '-', '>', ' ']);
    let line = ["title:", "Title:", "TITLE:", "Başlık:"]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .unwrap_or(line);
    let line = line.trim().trim_matches(['"', '\'', '`', '*', '“', '”', '‘', '’']).trim();
    let line = line.trim_end_matches(['.', '。']).trim();
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let mut title: String = words.join(" ");
    if title.chars().count() > MAX_TITLE_CHARS {
        title = title.chars().take(MAX_TITLE_CHARS).collect::<String>().trim_end().to_string();
        title.push('…');
    }
    Some(title)
}

/// Starts the title call when `run` is on its first turn and still has its
/// placeholder title. The drive awaits the task before the turn ends, so
/// the `run_title_changed` event reaches the live frontends.
pub(super) fn start(state: &Arc<AppState>, handle: &Arc<RunHandle>, run: &Run) -> Option<tokio::task::JoinHandle<()>> {
    if !handle.auto_title.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let user_turns = state.db.list_messages(&run.id).iter().filter(|message| message.role == "user").count();
    if user_turns != 1 || run.title != fallback_title(&run.task) || run.task.trim().is_empty() {
        return None;
    }
    let (state, handle, run) = (state.clone(), handle.clone(), run.clone());
    Some(tokio::spawn(async move {
        if let Err(err) = generate(&state, &handle, &run).await {
            tracing::debug!("Run {}: automatic title skipped: {err}", run.id);
        }
    }))
}

async fn generate(state: &AppState, handle: &RunHandle, run: &Run) -> Result<(), String> {
    let endpoint = resolve_endpoint(&state.pi, &run.provider_id, &run.model, None, &run.id).await?;
    let mut task = run.task.clone();
    locagens_protocol::text::truncate_at_char_boundary(&mut task, MAX_PROMPT_CHARS);
    let messages = vec![LlmMessage { role: "user".into(), content: task, ..Default::default() }];
    let mut ignore_deltas = |_: &str, _: &str| {};
    let should_stop = || handle.is_cancelled();
    let outcome = llm::stream_chat(&endpoint_http(state), &endpoint, TITLE_PROMPT, &messages, &[], &mut ignore_deltas, &should_stop)
        .await
        .map_err(|err| err.to_string())?;
    if let Some(usage) = outcome.usage {
        let info = state.pi.model_info(&run.provider_id, &run.model);
        state.db.append_usage(UsageLogEntry {
            id: 0,
            run_id: run.id.clone(),
            agent_role: Some(TITLE_ROLE.to_string()),
            provider_id: run.provider_id.clone(),
            model: run.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            cache_hit_rate: 0.0,
            cost: cost_of(&usage, info.as_ref()),
            created_at: crate::util::now_iso(),
            duration_ms: None,
        });
        handle.emit(json!({ "type": "usage_recorded", "runId": run.id, "agentRole": TITLE_ROLE }));
    }
    let title = title_from_reply(&outcome.content).ok_or("empty reply")?;
    let placeholder = fallback_title(&run.task);
    let updated = state.db.patch_run(&run.id, |current| {
        if current.title == placeholder {
            current.title = title.clone();
        }
    });
    if updated.is_some_and(|current| current.title == title) {
        handle.emit(json!({ "type": "run_title_changed", "runId": run.id, "title": title }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_cleaned_into_one_line_titles() {
        assert_eq!(title_from_reply("\"Fix the login bug.\"").as_deref(), Some("Fix the login bug"));
        assert_eq!(title_from_reply("Title: Refactor  TUI\nextra").as_deref(), Some("Refactor TUI"));
        assert_eq!(title_from_reply("<think>hmm</think>\n\n# Model seçici").as_deref(), Some("Model seçici"));
        assert_eq!(title_from_reply("**Rust sandbox**").as_deref(), Some("Rust sandbox"));
        assert!(title_from_reply("  \n ").is_none());
        let long = title_from_reply(&"word ".repeat(40)).unwrap();
        assert!(long.ends_with('…') && long.chars().count() <= MAX_TITLE_CHARS + 1);
    }

    #[test]
    fn fallback_is_the_cut_first_line() {
        assert_eq!(fallback_title("short task\nmore"), "short task");
        assert_eq!(fallback_title("  "), "New chat");
        let title = fallback_title(&"x".repeat(100));
        assert_eq!(title.chars().count(), 61);
        assert!(title.ends_with('…'));
    }
}
