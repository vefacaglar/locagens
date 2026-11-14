use super::*;
use serde_json::json;

fn app() -> App {
    App::new(
        PathBuf::from("C:/proj"),
        Some(ResolvedModel {
            provider_id: "prov".into(),
            model: "model-x".into(),
        }),
        None,
        "accept_edits".into(),
        false,
        None,
    )
}

#[test]
fn patch_progress_sets_and_clears_the_status_label() {
    let mut app = app();
    app.busy = true;
    app.on_run_event(&json!({"type": "patch_progress", "complete": false,
        "files": [{"path": "src/a.rs", "op": "update", "added": 3, "removed": 1}]}));
    assert_eq!(app.patch_progress.as_deref(), Some("Writing changes · src/a.rs +3 -1"));
    app.on_run_event(&json!({"type": "patch_progress", "complete": true, "files": []}));
    assert!(app.patch_progress.is_none());
}


fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

fn type_text(app: &mut App, text: &str) -> Vec<Action> {
    let mut actions = Vec::new();
    for ch in text.chars() {
        actions.extend(app.on_key(press(KeyCode::Char(ch))));
    }
    actions
}

fn assistant(id: &str, content: &str) -> Value {
    json!({"type": "message_updated", "message": {"id": id, "role": "assistant", "content": content}})
}

#[test]
fn created_then_updated_replaces_assistant_content() {
    let mut app = app();
    app.on_run_event(&json!({"type": "message_created",
        "message": {"id": "m1", "role": "assistant", "content": ""}}));
    assert!(app.cells.is_empty());
    app.on_run_event(&assistant("m1", "Hel"));
    app.on_run_event(&assistant("m1", "Hello world"));
    assert_eq!(app.cells.len(), 1);
    assert_eq!(
        app.cells[0].kind(),
        &CellKind::Assistant { text: "Hello world".into() }
    );
}

#[test]
fn reasoning_cell_is_placed_before_the_answer() {
    let mut app = app();
    app.on_run_event(&assistant("m1", "answer"));
    app.on_run_event(&json!({"type": "message_updated",
        "message": {"id": "m1", "role": "assistant", "content": "answer", "reasoningContent": "hmm"}}));
    assert!(matches!(app.cells[0].kind(), CellKind::Reasoning { .. }));
    assert!(matches!(app.cells[1].kind(), CellKind::Assistant { .. }));
}

#[test]
fn tool_call_cell_from_raw_response_gets_its_result() {
    let mut app = app();
    let calls = json!([{"id": "c1", "function": {"name": "run_command", "arguments": "{\"command\":\"ls\"}"}}]);
    app.on_run_event(&json!({"type": "message_updated", "message": {
        "id": "m1", "role": "assistant", "content": "", "rawResponse": calls.to_string()}}));
    assert_eq!(app.running_tool().as_deref(), Some("Ran ls"));
    app.on_run_event(&json!({"type": "message_created", "message": {
        "id": "m2", "role": "tool", "content": "a\nb", "rawResponse": "c1"}}));
    match app.cells[0].kind() {
        CellKind::Tool { name, result: Some(result), .. } => {
            assert_eq!(name, "run_command");
            assert!(result.ok);
            assert_eq!(result.output, "a\nb");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(app.running_tool().is_none());
    app.on_run_event(&json!({"type": "message_created", "message": {
        "id": "m3", "role": "tool", "content": "Tool error: boom", "rawResponse": "c-unknown"}}));
    assert!(matches!(app.cells[1].kind(), CellKind::Tool { result: Some(r), .. } if !r.ok));
}

#[test]
fn sub_agent_messages_are_tagged() {
    let mut app = app();
    app.on_run_event(&json!({"type": "message_updated", "message": {
        "id": "s1", "role": "assistant", "content": "found it",
        "agentRole": "explorer", "agentName": "scout"}}));
    assert_eq!(app.cells[0].agent.as_deref(), Some("explorer scout"));
}

#[test]
fn permission_requested_opens_overlay_and_decision_closes_it() {
    let mut app = app();
    app.begin_turn();
    app.on_run_event(&json!({"type": "permission_requested", "toolCall":
        {"id": "c1", "function": {"name": "run_command", "arguments": "{}"}},
        "preview": {"action": "command", "command": "rm x"}}));
    assert!(matches!(app.overlay, Some(Overlay::Approval(_))));
    // Typed-ahead keys are ignored while the prompt arms.
    assert!(app.on_key(press(KeyCode::Char('r'))).is_empty());
    assert!(app.overlay.is_some());
    if let Some(Overlay::Approval(approval)) = app.overlay.as_mut() {
        approval.armed_at = std::time::Instant::now();
    }
    let actions = app.on_key(press(KeyCode::Char('r')));
    assert!(matches!(actions.as_slice(), [Action::Decide("allow_run")]));
    assert!(app.overlay.is_none());
}

#[test]
fn altgr_characters_are_typed_into_the_composer() {
    let mut app = app();
    let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
    for ch in ['@', '\\', '{', '|'] {
        app.on_key(KeyEvent::new(KeyCode::Char(ch), altgr));
    }
    if cfg!(windows) {
        assert_eq!(app.composer.text(), "@\\{|");
    } else {
        assert_eq!(app.composer.text(), "");
    }
}

#[test]
fn question_requested_opens_overlay_and_answers() {
    let mut app = app();
    app.on_run_event(&json!({"type": "question_requested",
        "questions": [{"question": "Which?", "options": ["a", "b"]}]}));
    assert!(matches!(app.overlay, Some(Overlay::Question(_))));
    let actions = app.on_key(press(KeyCode::Enter));
    match actions.as_slice() {
        [Action::Answer(answer)] => assert_eq!(answer.selections, vec![vec!["a".to_string()]]),
        _ => panic!("expected an answer"),
    }
}

#[test]
fn run_completed_clears_busy_and_adds_diff_and_done_line() {
    let mut app = app();
    app.begin_turn();
    app.on_run_event(&json!({"type": "turn_diff_updated", "filesChanged": 2, "diff": "-a\n+b\n"}));
    app.on_run_event(&json!({"type": "run_completed", "finalOutput": "ok"}));
    assert!(!app.busy);
    assert!(app.run_finished);
    assert!(matches!(app.cells.first().map(Cell::kind), Some(CellKind::Diff { title, .. }) if title == "Changes: 2 files"));
    assert!(matches!(app.cells.last().map(Cell::kind), Some(CellKind::Completed(_))));
}

#[test]
fn run_failed_shows_error_even_after_failed_status() {
    let mut app = app();
    app.begin_turn();
    app.on_run_event(&json!({"type": "status_changed", "status": "failed"}));
    app.on_run_event(&json!({"type": "run_failed", "errorMessage": "boom"}));
    assert!(!app.busy);
    assert!(app.cells.iter().any(|c| matches!(c.kind(), CellKind::Error(m) if m == "boom")));
}

#[test]
fn cancel_marks_pending_tools_interrupted() {
    let mut app = app();
    app.begin_turn();
    let calls = json!([{"id": "c1", "function": {"name": "run_command", "arguments": "{}"}}]);
    app.on_run_event(&json!({"type": "message_updated", "message": {
        "id": "m1", "role": "assistant", "content": "", "rawResponse": calls.to_string()}}));
    app.on_run_event(&json!({"type": "status_changed", "status": "cancelled"}));
    assert!(!app.busy);
    assert!(matches!(app.cells[0].kind(), CellKind::Tool { result: Some(r), .. } if !r.ok));
}

#[test]
fn user_events_dedupe_and_hide_environment_context() {
    let mut app = app();
    let user = json!({"type": "message_created", "message": {"id": "u1", "role": "user", "content": "hi"}});
    app.on_run_event(&user);
    app.on_run_event(&user);
    app.on_run_event(&json!({"type": "message_created", "message": {
        "id": "u2", "role": "user", "content": "<environment_context>x</environment_context>"}}));
    assert_eq!(app.cells.len(), 1);
    assert_eq!(app.last_user_text().as_deref(), Some("hi"));
}

#[test]
fn queued_message_is_unmarked_when_the_run_folds_it() {
    let mut app = app();
    app.begin_turn();
    app.note_queued("later", "later");
    assert!(matches!(app.cells[0].kind(), CellKind::User { queued: true, .. }));
    app.on_run_event(&json!({"type": "message_created", "message": {"id": "u9", "role": "user", "content": "later"}}));
    assert_eq!(app.cells.len(), 1);
    assert!(matches!(app.cells[0].kind(), CellKind::User { queued: false, .. }));
}

#[test]
fn leftover_queue_is_handed_back_after_completion() {
    let mut app = app();
    app.begin_turn();
    app.note_queued("later", "later");
    app.on_run_event(&json!({"type": "run_completed", "finalOutput": ""}));
    assert_eq!(app.take_leftover_queued().as_deref(), Some("later"));
    assert!(!app.cells.iter().any(|c| matches!(c.kind(), CellKind::User { .. })));
}

#[test]
fn enter_submits_or_queues_and_slash_becomes_a_command() {
    let mut app = app();
    let actions = type_text(&mut app, "hello");
    assert!(actions.is_empty());
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Submit(t)] if t == "hello"));
    app.begin_turn();
    type_text(&mut app, "more");
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Queue(t)] if t == "more"));
    type_text(&mut app, "/usage");
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/usage"));
    type_text(&mut app, "/etc/hosts please");
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Queue(_)]));
}

#[test]
fn newline_shortcuts_do_not_submit() {
    let mut app = app();
    type_text(&mut app, "a");
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut app, "b");
    app.on_key(ctrl('j'));
    type_text(&mut app, "c");
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_text(&mut app, "d\\");
    assert!(app.on_key(press(KeyCode::Enter)).is_empty());
    assert_eq!(app.composer.text(), "a\nb\nc\nd\n");
}

#[test]
fn release_events_are_ignored() {
    let mut app = app();
    let mut release = press(KeyCode::Char('x'));
    release.kind = KeyEventKind::Release;
    app.on_key(release);
    assert!(app.composer.is_empty());
}

#[test]
fn ctrl_c_clears_then_cancels_then_arms_quit() {
    let mut app = app();
    type_text(&mut app, "draft");
    assert!(app.on_key(ctrl('c')).is_empty());
    assert!(app.composer.is_empty());
    app.begin_turn();
    assert!(matches!(app.on_key(ctrl('c')).as_slice(), [Action::Cancel]));
    app.busy = false;
    assert!(app.on_key(ctrl('c')).is_empty());
    assert!(app.quit_armed());
    assert!(matches!(app.on_key(ctrl('c')).as_slice(), [Action::Quit]));
}

#[test]
fn ctrl_d_quits_only_on_empty_composer() {
    let mut app = app();
    type_text(&mut app, "x");
    assert!(app.on_key(ctrl('d')).is_empty());
    app.composer.clear();
    assert!(matches!(app.on_key(ctrl('d')).as_slice(), [Action::Quit]));
}

#[test]
fn escape_cancels_a_busy_run_but_first_closes_the_popup() {
    let mut app = app();
    app.begin_turn();
    type_text(&mut app, "/mo");
    assert!(app.popup.is_some());
    assert!(app.on_key(press(KeyCode::Esc)).is_empty());
    assert!(app.popup.is_none());
    assert!(matches!(app.on_key(press(KeyCode::Esc)).as_slice(), [Action::Cancel]));
}

#[test]
fn slash_popup_tab_completes_and_enter_runs_simple_commands() {
    let mut app = app();
    type_text(&mut app, "/mo");
    assert_eq!(app.popup.as_ref().unwrap().items[0].value, "/model");
    assert!(app.on_key(press(KeyCode::Tab)).is_empty());
    assert_eq!(app.composer.text(), "/model ");
    assert!(app.popup.is_none());
    app.composer.clear();
    type_text(&mut app, "/pla");
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/plan"));
}

#[test]
fn review_popup_lists_targets_and_enter_runs_the_review() {
    use super::super::popup::PopupItem;
    let mut app = app();
    app.review_targets = vec![
        PopupItem { value: "uncommitted".into(), label: "uncommitted".into(), detail: "changes".into() },
        PopupItem { value: "base main".into(), label: "base main".into(), detail: String::new() },
    ];
    type_text(&mut app, "/revi");
    // Tab completes the command and immediately opens the target list.
    assert!(app.on_key(press(KeyCode::Tab)).is_empty());
    assert_eq!(app.composer.text(), "/review ");
    let popup = app.popup.as_ref().expect("target popup");
    assert_eq!(popup.kind, PopupKind::Review);
    assert_eq!(popup.items.len(), 2);
    app.on_key(press(KeyCode::Down));
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/review base main"));
    // Free text matches no target: no popup, Enter submits it as custom instructions.
    type_text(&mut app, "/review check the parser");
    assert!(app.popup.is_none());
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/review check the parser"));
}

#[test]
fn file_popup_requests_a_search_and_inserts_the_mention() {
    let mut app = app();
    type_text(&mut app, "see ");
    let actions = type_text(&mut app, "@ma");
    assert!(actions.iter().any(|a| matches!(a, Action::SearchFiles(q) if q == "ma")));
    app.set_file_matches("ma", &["src/main.rs".to_string(), "README.md".to_string()]);
    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::Up));
    app.on_key(press(KeyCode::Tab));
    assert_eq!(app.composer.text(), "see @src/main.rs ");
    // Stale results for an older query are ignored.
    app.set_file_matches("m", &["x".to_string()]);
    assert!(app.popup.is_none());
}

#[test]
fn up_and_down_recall_history_at_the_edges() {
    let mut app = app();
    type_text(&mut app, "first");
    app.on_key(press(KeyCode::Enter));
    app.on_key(press(KeyCode::Up));
    assert_eq!(app.composer.text(), "first");
    app.on_key(press(KeyCode::Down));
    assert_eq!(app.composer.text(), "");
}

#[test]
fn scrolling_leaves_follow_mode_and_end_returns() {
    let mut app = app();
    app.viewport.total = 100;
    app.viewport.height = 10;
    app.on_key(press(KeyCode::PageUp));
    assert!(!app.viewport.follow);
    assert_eq!(app.viewport.top, 82);
    app.on_key(press(KeyCode::End));
    assert!(app.viewport.follow);
    assert_eq!(app.viewport.resolve_top(), 90);
}

#[test]
fn ctrl_o_toggles_reasoning_and_shift_tab_toggles_mode() {
    let mut app = app();
    app.on_key(ctrl('o'));
    assert!(app.show_reasoning);
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "plan");
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "accept_edits");
}

#[test]
fn context_left_is_derived_from_the_window() {
    assert_eq!(context_left_percent(25_000, 100_000), 75);
    assert_eq!(context_left_percent(500_000, 100_000), 0);
    assert_eq!(context_left_percent(10, 0), 100);
}

#[test]
fn elapsed_formats_compactly() {
    assert_eq!(format_elapsed(Duration::from_secs(5)), "5s");
    assert_eq!(format_elapsed(Duration::from_secs(65)), "1m 05s");
    assert_eq!(format_elapsed(Duration::from_secs(3720)), "1h 02m");
}
