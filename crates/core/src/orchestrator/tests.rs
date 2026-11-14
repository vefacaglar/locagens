use super::*;
use super::drive::{cost_of, reasoning_supported};
use super::gating::{extract_command_str, standing_grant_matches};
use super::interactive::{normalize_plan_args, normalize_request_user_input};
use crate::db::{PermissionRule, RunMessage};

fn message(role: &str, content: &str, raw: Option<&str>, agent_role: Option<&str>) -> RunMessage {
    RunMessage {
        id: format!("m-{}", uuid::Uuid::new_v4()),
        run_id: "r".into(),
        role: role.into(),
        content: content.into(),
        raw_response: raw.map(str::to_string),
        agent_role: agent_role.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn history_seeding_pairs_tool_calls_with_responses() {
    let messages = vec![
        message("user", "do it", None, None),
        message("assistant", "working", Some(r#"[{"id":"c1","function":{"name":"t","arguments":"{}"}}]"#), None),
        message("tool", "result", Some("c1"), None),
        // Sub-agent messages are folded away.
        message("assistant", "sub output", None, Some("coder")),
        message("assistant", "final", None, None),
    ];
    let history = seed_history(&messages);
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].tool_calls.as_ref().unwrap()[0]["id"], "c1");
    assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(history[3].content, "final");
}

/// FR-001 / FR-002 / FR-067: a failed round's empty placeholder and a
/// cancelled tool loop replay as a provider-valid history.
#[test]
fn history_seeding_repairs_failed_and_cancelled_rounds() {
    let calls = r#"[{"id":"c1","function":{"name":"t","arguments":"{}"}},{"id":"c2","function":{"name":"t","arguments":"{}"}}]"#;
    let messages = vec![
        message("user", "first question", None, None),
        message("assistant", "", None, None), // round failed before any delta
        message("user", "second question", None, None),
        message("assistant", "", Some(calls), None),
        message("tool", "one", Some("c1"), None), // cancelled before c2 ran
        message("tool", "stray", Some("c9"), None),
        message("user", "third", None, None),
    ];
    let history = seed_history(&messages);
    let shape: Vec<String> = history
        .iter()
        .map(|m| format!("{}:{}", m.role, m.tool_call_id.clone().unwrap_or_else(|| m.content.clone())))
        .collect();
    assert_eq!(shape, ["user:first question", "user:second question", "assistant:", "tool:c1", "tool:c2", "user:third"]);
    assert_eq!(history[4].content, history::INTERRUPTED_TOOL_RESULT);
}

/// FR-005: a persisted compaction marker replaces the history before the
/// first message it kept.
#[test]
fn history_seeding_honours_the_latest_compaction_marker() {
    let mut messages: Vec<RunMessage> = (0..6).map(|i| message("user", &format!("u{i}"), None, None)).collect();
    let kept = messages[4].id.clone();
    let mut marker = message("system", &compact::summary_block("S"), None, None);
    marker.raw_response = Some(compact::marker_raw(Some(&kept)));
    messages.push(marker);
    messages.push(message("user", "after", None, None));
    let history = seed_history(&messages);
    let contents: Vec<&str> = history.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(contents, [compact::summary_block("S").as_str(), "u4", "u5", "after"]);
}

#[test]
fn standing_grants_match_scope_tool_command_and_domains() {
    let rule = PermissionRule {
        id: 1,
        scope: "project".into(),
        project_path: "C:/ws".into(),
        tool: "run_command".into(),
        command: "npm test".into(),
        network_domains: vec!["registry.npmjs.org".into()],
        status: "allow_always".into(),
    };
    assert!(standing_grant_matches(&rule, "run_command", "npm test", &["registry.npmjs.org".to_string()], "C:/ws"));
    assert!(!standing_grant_matches(&rule, "run_command", "npm publish", &[], "C:/ws"));
    assert!(!standing_grant_matches(&rule, "run_command", "npm test", &["evil.com".to_string()], "C:/ws"));
    assert!(!standing_grant_matches(&rule, "run_command", "npm test", &["registry.npmjs.org".to_string()], "C:/other"));
    assert!(!standing_grant_matches(&rule, "fetch_url", "", &[], "C:/ws"));

    let global = PermissionRule {
        scope: "global".into(),
        tool: "fetch_url".into(),
        command: String::new(),
        network_domains: vec![],
        status: "allow_run".into(),
        ..rule.clone()
    };
    // A fetch_url grant is bound to the host it was given for (FR-032).
    let global = PermissionRule { command: "example.com".into(), ..global };
    assert!(standing_grant_matches(&global, "fetch_url", "example.com", &[], "C:/anywhere"));
    assert!(!standing_grant_matches(&global, "fetch_url", "evil.com", &[], "C:/anywhere"));
    let unkeyed = PermissionRule { command: String::new(), ..global.clone() };
    assert!(!standing_grant_matches(&unkeyed, "fetch_url", "", &[], "C:/anywhere"));
    assert_eq!(super::gating::grant_key("fetch_url", &json!({ "url": "https://Example.com/a/b?x=1" })), "example.com");
    let denied = PermissionRule { status: "deny".into(), ..global };
    assert!(!standing_grant_matches(&denied, "fetch_url", "example.com", &[], "C:/anywhere"));

    // A command grant saved without domains does not cover a call that adds hosts.
    let no_domains = PermissionRule { network_domains: vec![], ..rule };
    assert!(standing_grant_matches(&no_domains, "run_command", "npm test", &[], "C:/ws"));
    assert!(!standing_grant_matches(&no_domains, "run_command", "npm test", &["evil.com".to_string()], "C:/ws"));
}

#[test]
fn standing_grants_match_exec_command_by_cmd() {
    let rule = PermissionRule {
        id: 2,
        scope: "global".into(),
        project_path: String::new(),
        tool: "exec_command".into(),
        command: "cargo test".into(),
        network_domains: vec![],
        status: "allow_always".into(),
    };
    // `cmd` argument form matches the saved command.
    assert!(standing_grant_matches(&rule, "exec_command", "cargo test", &[], "C:/ws"));
    assert!(!standing_grant_matches(&rule, "exec_command", "cargo publish", &[], "C:/ws"));
    // A different tool never inherits a command rule.
    assert!(!standing_grant_matches(&rule, "run_command", "cargo test", &[], "C:/ws"));

    // Empty saved command must NOT act as a wildcard for exec_command
    // (the historical bug: exec_command saved `command: ""`).
    let wildcard = PermissionRule { command: String::new(), ..rule.clone() };
    assert!(!standing_grant_matches(&wildcard, "exec_command", "anything", &[], "C:/ws"));
    // And an empty live command never matches either.
    let live = PermissionRule { command: "cargo test".into(), ..rule };
    assert!(!standing_grant_matches(&live, "exec_command", "", &[], "C:/ws"));
}

#[test]
fn extract_command_reads_command_then_cmd() {
    assert_eq!(extract_command_str(&json!({ "command": "npm test" })), "npm test");
    assert_eq!(extract_command_str(&json!({ "cmd": "cargo check" })), "cargo check");
    assert_eq!(extract_command_str(&json!({})), "");
}

#[test]
fn workers_get_web_search_and_explorers_do_not() {
    let names = |defs: Vec<Value>| -> Vec<String> {
        defs.iter().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect()
    };
    let worker = names(worker_tool_defs());
    assert!(worker.iter().any(|n| n == "web_search") && worker.iter().any(|n| n == "fetch_url"));
    let explorer = names(tools::readonly_tool_defs(false));
    assert!(!explorer.iter().any(|n| n == "web_search" || n == "fetch_url"));
}

#[test]
fn tool_names_follow_mode() {
    let chat = tool_names_for_mode("chat");
    assert!(chat.contains(&"read_file"));
    assert!(chat.contains(&"compact_context"));
    assert!(!chat.contains(&"update_plan"));
    assert!(!chat.contains(&"fetch_url"));
    assert!(!chat.contains(&"web_search"));
    assert!(!chat.contains(&"spawn_agent"));
    assert!(!chat.contains(&"write_file"));
    assert!(!chat.contains(&"run_command"));
    assert!(!chat.contains(&"exec_command"));
    assert!(!chat.contains(&"write_stdin"));
    assert!(!chat.contains(&"apply_patch"));

    let plan = tool_names_for_mode("plan");
    for chat_tool in chat {
        assert!(plan.contains(&chat_tool), "plan must keep chat tool {chat_tool}");
    }
    assert!(plan.contains(&"update_plan"));
    assert!(plan.contains(&"fetch_url"));
    assert!(plan.contains(&"web_search"));
    assert!(plan.contains(&"spawn_agent"));
    assert!(plan.contains(&"spawn_agents"));
    assert!(!plan.contains(&"write_file"));
    assert!(!plan.contains(&"apply_patch"));
    assert!(!plan.contains(&"run_command"));
    assert!(!plan.contains(&"exec_command"));
    assert!(!plan.contains(&"write_stdin"));

    let full = tool_names_for_mode("accept_edits");
    assert!(full.contains(&"write_file"));
    assert!(full.contains(&"exec_command"));
    assert!(full.contains(&"web_search"));
    assert!(full.contains(&"write_stdin"));
    assert!(full.contains(&"apply_patch"));
    assert!(full.contains(&"spawn_agents"));
    let auto = tool_names_for_mode("auto");
    assert!(auto.contains(&"run_command"));
}

#[test]
fn sub_agent_roles_are_read_only_in_readonly_modes() {
    // `spawn_agents` and `spawn_agent` share `resolve_role` (QA-033).
    let roles = agent_roles::builtin_roles();
    assert!(agent_roles::resolve_role(&roles, Some("worker"), "plan").unwrap().readonly);
    assert!(!agent_roles::resolve_role(&roles, Some("worker"), "auto").unwrap().readonly);
    assert_eq!(agent_roles::resolve_role(&roles, None, "auto").unwrap().name, "explorer");
}

#[test]
fn cost_computation_uses_per_million_pricing() {
    let info = crate::pi_providers::ModelInfo {
        id: "m".into(),
        name: "m".into(),
        cost: Some(json!({
            "currency": "USD",
            "inputPerMillion": 3.0,
            "outputPerMillion": 15.0,
            "cacheReadPerMillion": 0.3,
            "cacheWritePerMillion": 3.75
        })),
        ..Default::default()
    };
    let usage = llm::Usage {
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        cache_read_tokens: 500_000,
        cache_write_tokens: 0,
        provider_cost: None,
    };
    let cost = cost_of(&usage, Some(&info));
    // Only the 500k uncached input tokens are billed at the input rate.
    assert!((cost - (1.5 + 1.5 + 0.15)).abs() < 1e-9);
    assert_eq!(cost_of(&usage, None), 0.0);
}

#[test]
fn update_plan_normalizes_both_schemas() {
    // Locagens schema.
    let (title, body, tasks) = normalize_plan_args(&json!({
        "title": "Ship it",
        "body": "rationale",
        "tasks": ["one", { "text": "two", "status": "completed" }]
    }));
    assert_eq!(title, "Ship it");
    assert_eq!(body.as_deref(), Some("rationale"));
    assert_eq!(tasks[0]["text"], "one");
    assert_eq!(tasks[0]["status"], "pending");
    assert_eq!(tasks[1]["text"], "two");
    assert_eq!(tasks[1]["status"], "completed");

    // Codex schema: explanation + plan[{step,status}], no title.
    let (title, body, tasks) = normalize_plan_args(&json!({
        "explanation": "why",
        "plan": [
            { "step": "research", "status": "completed" },
            { "step": "implement", "status": "in_progress" },
            { "step": "verify" }
        ]
    }));
    assert_eq!(title, "research");
    assert_eq!(body.as_deref(), Some("why"));
    assert_eq!(tasks[0]["text"], "research");
    assert_eq!(tasks[0]["status"], "completed");
    assert_eq!(tasks[2]["status"], "pending");
}

#[test]
fn request_user_input_normalization_fills_defaults() {
    let normalized = normalize_request_user_input(&json!({
        "questions": [
            { "id": "q1", "question": "Proceed?" },
            { "question": "Which db?", "options": [{ "label": "SQLite" }] }
        ]
    }));
    let questions = normalized["questions"].as_array().unwrap();
    assert_eq!(questions[0]["header"], "Choice");
    assert_eq!(questions[0]["options"][0]["label"], "Yes");
    assert_eq!(questions[0]["question"], "Proceed?");
    assert_eq!(questions[1]["header"], "Choice");
    assert_eq!(questions[1]["options"][0]["label"], "SQLite");

    // FR-016: strings become questions, other non-objects are skipped.
    let loose = normalize_request_user_input(&json!({ "questions": ["Proceed?", 3, null] }));
    let questions = loose["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["question"], "Proceed?");
    assert_eq!(questions[0]["options"][1]["label"], "No");
}

/// FR-041: keyless providers are fine on this machine only.
#[test]
fn local_base_urls_are_recognized() {
    for url in ["http://localhost:1234/v1", "http://127.0.0.1:8080", "http://[::1]:11434/v1", "http://lm.localhost/v1"] {
        assert!(super::drive::is_local_base_url(url), "{url}");
    }
    for url in ["https://api.example.com/v1", "http://192.168.1.5:8080", "http://localhost.evil.com", "not a url"] {
        assert!(!super::drive::is_local_base_url(url), "{url}");
    }
}

#[test]
fn reasoning_support_heuristic() {
    assert!(reasoning_supported("glm-5.3-flash", None));
    assert!(reasoning_supported("gpt-5-mini", None));
    assert!(!reasoning_supported("text-embedding-3", None));
    // FR-076: the o-series markers only match as the leading name segment.
    assert!(reasoning_supported("o3-mini", None));
    assert!(reasoning_supported("openai/o4-mini", None));
    assert!(reasoning_supported("o1", None));
    assert!(!reasoning_supported("yolo1-chat", None));
    assert!(!reasoning_supported("gpt-4o3x", None));
    assert!(!reasoning_supported("pro1-model", None));
    let info = crate::pi_providers::ModelInfo {
        id: "whatever".into(),
        name: "whatever".into(),
        reasoning: Some(true),
        ..Default::default()
    };
    assert!(reasoning_supported("whatever", Some(&info)));
}
