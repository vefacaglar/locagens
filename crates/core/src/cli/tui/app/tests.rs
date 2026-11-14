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
fn run_completed_clears_busy_and_ends_with_one_summary_cell() {
    let mut app = app();
    app.begin_turn();
    app.on_run_event(&json!({"type": "turn_diff_updated", "filesChanged": 2, "diff": "-a\n+b\n"}));
    app.on_run_event(&json!({"type": "run_completed", "finalOutput": "ok"}));
    assert!(!app.busy);
    assert!(app.run_finished);
    assert_eq!(app.cells.len(), 1, "diff and done line are one summary");
    assert!(matches!(
        app.cells[0].kind(),
        CellKind::TurnSummary { ok: true, stats, diff: Some(_), .. } if stats.is_empty()
    ));
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
fn queued_message_waits_in_the_strip_until_the_run_folds_it() {
    let mut app = app();
    app.begin_turn();
    app.note_queued("later", "later");
    assert!(app.cells.is_empty(), "queued text is not in the transcript");
    assert_eq!(app.queued_displays(), ["later"]);
    app.on_run_event(&json!({"type": "message_created", "message": {"id": "u9", "role": "user", "content": "later"}}));
    assert!(app.queued_displays().is_empty());
    assert_eq!(app.cells.len(), 1);
    assert!(matches!(app.cells[0].kind(), CellKind::User { text } if text == "later"));
}

#[test]
fn leftover_queue_is_handed_back_after_completion() {
    let mut app = app();
    app.begin_turn();
    app.note_queued("later", "later");
    app.on_run_event(&json!({"type": "run_completed", "finalOutput": ""}));
    assert_eq!(app.take_leftover_queued().as_deref(), Some("later"));
    assert!(!app.cells.iter().any(|c| matches!(c.kind(), CellKind::User { .. })));
    assert!(app.queued_displays().is_empty());
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
fn ctrl_o_toggles_details_and_shift_tab_cycles_build_plan_full_access() {
    let mut app = app();
    app.on_key(ctrl('o'));
    assert!(app.details);
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "plan");
    assert_eq!(app.flash_text(), Some("Mode: Plan"));
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "auto");
    assert_eq!(app.flash_text(), Some("Mode: Full Access"));
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "accept_edits");
}

#[test]
fn shift_tab_is_ignored_while_a_popup_or_overlay_is_open() {
    let mut app = app();
    type_text(&mut app, "/pl");
    assert!(app.popup.is_some());
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "accept_edits");
    app.composer.clear();
    app.popup = None;
    app.open_mode_picker();
    app.on_key(press(KeyCode::BackTab));
    assert_eq!(app.mode, "accept_edits");
}

#[test]
fn effort_picker_marks_the_current_level_and_applies_the_pick() {
    let mut app = app();
    app.thinking = Some("high".into());
    app.open_effort_picker();
    let Some(Overlay::Effort(picker)) = app.overlay.as_ref() else { panic!("no effort picker") };
    assert_eq!(picker.options[picker.selected].value, "high");
    app.on_key(press(KeyCode::Down));
    let actions = app.on_key(press(KeyCode::Enter));
    assert!(matches!(actions.as_slice(), [Action::Slash(t)] if t == "/thinking xhigh"));
    assert!(app.overlay.is_none());
    app.open_effort_picker();
    assert!(app.on_key(press(KeyCode::Esc)).is_empty());
    assert!(app.overlay.is_none());
}

#[test]
fn mode_picker_lists_five_modes_and_switches_through_permissions() {
    let mut app = app();
    app.open_mode_picker();
    let Some(Overlay::Mode(picker)) = app.overlay.as_ref() else { panic!("no mode picker") };
    let values: Vec<&str> = picker.options.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(values, ["chat", "plan", "build", "ask", "auto"]);
    assert_eq!(picker.options[picker.selected].value, "build");
    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::Down));
    let actions = app.on_key(press(KeyCode::Enter));
    assert!(matches!(actions.as_slice(), [Action::Slash(t)] if t == "/permissions auto"));
}

#[test]
fn bare_thinking_and_permissions_run_from_the_slash_popup() {
    let mut app = app();
    type_text(&mut app, "/perm");
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/permissions"));
    type_text(&mut app, "/thinki");
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/thinking"));
}

#[test]
fn shift_tab_cycle_respects_approve_and_other_modes() {
    let mut app = app();
    app.mode = "ask_permissions".into();
    app.cycle_mode();
    assert_eq!(app.mode, "plan", "any other mode enters the cycle at Plan");

    let mut approved = App::new(PathBuf::from("C:/proj"), None, None, "auto".into(), true, None);
    approved.cycle_mode();
    assert_eq!(approved.mode, "plan");
    approved.cycle_mode();
    assert_eq!(approved.mode, "auto", "with --approve Build means Full Access");
}

#[test]
fn context_used_is_derived_from_the_window() {
    assert_eq!(context_used_percent(25_000, 100_000), 25);
    assert_eq!(context_used_percent(500_000, 100_000), 100);
    assert_eq!(context_used_percent(10, 0), 0);
}

#[test]
fn tokens_format_compactly() {
    let shown: Vec<String> = [0, 999, 1_000, 6_400, 128_000, 999_999, 1_000_000, 1_500_000].map(compact_tokens).to_vec();
    assert_eq!(shown, ["0", "999", "1k", "6.4k", "128k", "999k", "1M", "1.5M"]);
}

#[test]
fn elapsed_formats_compactly() {
    assert_eq!(format_elapsed(Duration::from_secs(5)), "5s");
    assert_eq!(format_elapsed(Duration::from_secs(65)), "1m 05s");
    assert_eq!(format_elapsed(Duration::from_secs(3720)), "1h 02m");
}

#[test]
fn turn_summary_lists_files_and_counts() {
    let mut app = app();
    app.begin_turn();
    let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@\n-old\n+new\n+more\n--- a/b.rs\n+++ b/b.rs\n+x\n";
    app.on_run_event(&json!({"type": "turn_diff_updated", "filesChanged": 2, "diff": diff}));
    app.on_run_event(&json!({"type": "run_completed", "finalOutput": ""}));
    let CellKind::TurnSummary { stats, ok, .. } = app.cells[0].kind() else { panic!("expected a summary") };
    assert!(*ok);
    assert_eq!((stats[0].path.as_str(), stats[0].added, stats[0].removed), ("src/a.rs", 2, 1));
    assert_eq!(stats.len(), 2);
    let lines: Vec<String> = app.cells[0].lines(100, false).iter().map(crate::cli::tui::wrap::line_text).collect();
    assert!(lines[0].starts_with("✔ "), "{lines:?}");
    assert!(lines[0].contains("2 files changed +3 −1 · /diff to view · /undo to revert"), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("src/a.rs") && l.contains("+2")));
}

#[test]
fn up_on_an_empty_composer_withdraws_the_newest_queued_message() {
    let mut app = app();
    assert!(app.on_key(press(KeyCode::Up)).is_empty(), "idle: Up is history");
    app.begin_turn();
    assert!(app.on_key(press(KeyCode::Up)).is_empty(), "nothing queued");
    app.note_queued("first", "first");
    app.note_queued("second", "second");
    assert_eq!(app.last_queued_content(), Some("second"));
    assert!(matches!(app.on_key(press(KeyCode::Up)).as_slice(), [Action::Withdraw]));
    app.unqueue_last();
    assert_eq!(app.composer.text(), "second");
    assert_eq!(app.queued_displays(), ["first"]);
    // With text in the composer Up edits, it does not withdraw.
    assert!(app.on_key(press(KeyCode::Up)).is_empty());
}

#[test]
fn interrupted_run_puts_queued_messages_back_in_the_composer() {
    let mut app = app();
    app.begin_turn();
    app.note_queued("one", "one");
    app.note_queued("two", "two");
    app.on_run_event(&json!({"type": "status_changed", "status": "cancelled"}));
    assert_eq!(app.composer.text(), "one\n\ntwo");
    assert!(app.queued_displays().is_empty());
    assert_eq!(app.flash_text(), Some("Queued message restored to the composer."));
    assert!(!app.cells.iter().any(|c| matches!(c.kind(), CellKind::Notice(t) if t.contains("not delivered"))));
}

#[test]
fn question_mark_opens_help_only_on_an_empty_composer() {
    let mut app = app();
    app.on_key(press(KeyCode::Char('?')));
    assert!(matches!(app.overlay, Some(Overlay::Help(_))));
    app.on_key(press(KeyCode::Esc));
    assert!(app.overlay.is_none());
    type_text(&mut app, "why");
    app.on_key(press(KeyCode::Char('?')));
    assert_eq!(app.composer.text(), "why?");
    assert!(app.overlay.is_none());
}

#[test]
fn ctrl_r_searches_the_seeded_prompt_history() {
    let mut app = app();
    app.seed_history(vec!["fix parser".into(), "write docs".into(), "fix lexer".into()]);
    app.on_key(ctrl('r'));
    assert!(app.history_search.is_some());
    type_text(&mut app, "fix");
    // Newest match first; Up/Ctrl+R steps to older ones.
    app.on_key(ctrl('r'));
    app.on_key(press(KeyCode::Enter));
    assert!(app.history_search.is_none());
    assert_eq!(app.composer.text(), "fix parser");
    app.on_key(ctrl('r'));
    type_text(&mut app, "zzz");
    app.on_key(press(KeyCode::Enter));
    assert_eq!(app.composer.text(), "fix parser", "no match leaves the composer alone");
    app.on_key(ctrl('r'));
    app.on_key(press(KeyCode::Esc));
    assert!(app.history_search.is_none());
}

#[test]
fn enter_on_the_empty_first_run_screen_opens_login() {
    let mut app = app();
    assert!(app.on_key(press(KeyCode::Enter)).is_empty());
    app.no_providers = true;
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/login"));
    app.push_notice("something");
    assert!(app.on_key(press(KeyCode::Enter)).is_empty(), "only on the welcome screen");
}

#[test]
fn first_drag_shows_the_selection_hint_once() {
    use crossterm::event::MouseButton;
    let mut app = app();
    let drag = MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), column: 3, row: 3, modifiers: KeyModifiers::NONE };
    app.on_mouse(drag);
    assert!(app.flash_text().is_some_and(|t| t.contains("Hold Shift")));
    app.flash = None;
    app.on_mouse(drag);
    assert!(app.flash_text().is_none(), "shown once per session");
}

#[test]
fn reduced_motion_slows_the_spinner_to_a_crawl() {
    let mut app = app();
    app.begin_turn();
    app.reduced_motion = true;
    for _ in 0..(SPINNER_TICKS * 2) {
        app.tick();
    }
    assert_eq!(app.spinner, 0);
    let mut animated = super::tests::app();
    animated.begin_turn();
    for _ in 0..SPINNER_TICKS {
        animated.tick();
    }
    assert_eq!(animated.spinner, 1);
}

#[test]
fn compaction_notice_uses_plain_words() {
    let mut app = app();
    app.on_run_event(&json!({"type": "context_compacted", "messagesBefore": 42, "messagesAfter": 9}));
    assert!(matches!(app.cells[0].kind(), CellKind::Notice(t) if t == "Older messages were summarized to free up context."));
}

#[test]
fn failed_run_adds_the_next_step_hint() {
    let mut app = app();
    app.begin_turn();
    app.on_run_event(&json!({"type": "run_failed", "errorMessage": "boom"}));
    assert!(app.cells.iter().any(|c| matches!(c.kind(), CellKind::Notice(t) if t.contains("Send a message to continue"))));
}

#[test]
fn session_picker_enter_asks_the_driver_to_resume() {
    use crate::cli::tui::sessions::SessionPicker;
    let run = crate::db::Run { id: "r1".into(), title: "Fix".into(), ..Default::default() };
    let mut app = app();
    app.overlay = Some(Overlay::Sessions(SessionPicker::new(&[run], None, chrono::Utc::now())));
    assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Resume(id)] if id == "r1"));
    assert!(app.overlay.is_none());
}

#[test]
fn login_overlay_routes_keys_and_pastes() {
    use crate::cli::tui::login::{LoginFlow, LoginRow};
    let row = LoginRow {
        id: "anthropic".into(),
        name: "Anthropic".into(),
        method: "API key".into(),
        status: String::new(),
        has_oauth: false,
        keyless: false,
    };
    let mut app = app();
    app.overlay = Some(Overlay::Login(LoginFlow::new(vec![row])));
    app.on_key(press(KeyCode::Enter));
    assert!(app.on_paste(" sk-secret\n").is_empty());
    let actions = app.on_key(press(KeyCode::Enter));
    assert!(matches!(actions.as_slice(), [Action::Login(crate::cli::tui::login::LoginAction::SaveKey { key, .. })] if key == "sk-secret"));
    // Esc on the picker closes the overlay.
    app.overlay = Some(Overlay::Login(LoginFlow::new(Vec::new())));
    assert!(app.on_key(press(KeyCode::Esc)).is_empty());
    assert!(app.overlay.is_none());
}
