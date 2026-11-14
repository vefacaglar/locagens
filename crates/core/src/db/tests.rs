use super::*;

/// QA-065: the main agent logs `main`; legacy `planner` lines read back
/// as `main`, and entries without a role belong to the main agent.
#[test]
fn legacy_planner_usage_role_reads_back_as_main() {
    let legacy: UsageLogEntry = serde_json::from_value(serde_json::json!({ "runId": "r", "agentRole": "planner" })).unwrap();
    assert_eq!(legacy.agent_role.as_deref(), Some(MAIN_AGENT_ROLE));
    let explorer: UsageLogEntry = serde_json::from_value(serde_json::json!({ "agentRole": "explorer" })).unwrap();
    assert_eq!(explorer.role(), "explorer");
    let unset: UsageLogEntry = serde_json::from_value(serde_json::json!({ "agentRole": null })).unwrap();
    assert_eq!(unset.agent_role, None);
    assert_eq!(unset.role(), MAIN_AGENT_ROLE);
}

fn temp_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "locagens-db-test-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rollouts_in(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rollouts(&root.join("sessions"), &mut out);
    out.sort();
    out
}

fn lines_of(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn kinds_of(path: &Path) -> Vec<String> {
    lines_of(path)
        .iter()
        .map(|line| line["type"].as_str().unwrap().to_string())
        .collect()
}

fn sample_run(id: &str) -> Run {
    Run {
        id: id.into(),
        title: "Test".into(),
        task: "do things".into(),
        status: "done".into(),
        provider_id: "p".into(),
        model: "m".into(),
        created_at: "2026-03-07T10:20:30.000Z".into(),
        updated_at: "2026-03-07T10:20:30.000Z".into(),
        ..Default::default()
    }
}

fn msg(run: &str, id: &str, role: &str, content: &str) -> RunMessage {
    RunMessage {
        id: id.into(),
        run_id: run.into(),
        role: role.into(),
        content: content.into(),
        created_at: now_iso(),
        ..Default::default()
    }
}

#[test]
fn projects_and_memories_persist_across_reopen() {
    let root = temp_root("persist");
    let db = Db::open(&root).unwrap();
    db.run_startup_tasks(Path::new("C:/ws"), "Locagens");
    assert_eq!(db.list_projects().len(), 1);
    assert_eq!(db.list_projects()[0].name, "Locagens");

    db.create_memory("project", "C:/ws", "reference", "hello fact").unwrap();
    db.create_project("C:/other", "Other");
    let memory = db.list_memories()[0].clone();
    let updated = db.update_memory(memory.id, "edited fact").unwrap();
    assert_eq!(updated.content, "edited fact");
    drop(db);

    let db2 = Db::open(&root).unwrap();
    db2.run_startup_tasks(Path::new("C:/ws"), "Locagens");
    assert_eq!(db2.list_projects().len(), 2);
    let memories = db2.list_memories();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content, "edited fact");
    assert_eq!(memories[0].id, memory.id);

    assert!(db2.delete_memory(memory.id));
    assert!(!db2.delete_memory(memory.id));
    db2.clear_memories();
    assert!(db2.list_memories().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn permission_dedup_and_ids() {
    let root = temp_root("perms");
    let db = Db::open(&root).unwrap();
    let first = db.upsert_permission("run", "C:/ws", "run_command", "git status", vec![], "allow_once");
    let second = db.upsert_permission("run", "C:/ws", "run_command", "git status", vec![], "allow_always");
    let other = db.upsert_permission("run", "C:/ws", "run_command", "git push", vec![], "allow_once");
    assert_eq!(first.id, 1);
    assert_eq!(second.id, 1, "duplicate rule is deduplicated");
    assert_eq!(second.status, "allow_always");
    assert_eq!(other.id, 2);
    assert_eq!(db.list_permissions().len(), 2);
    assert!(db.delete_permission(1));
    assert_eq!(db.list_permissions().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stuck_runs_fail_on_startup_with_system_message() {
    let root = temp_root("stuck");
    let db = Db::open(&root).unwrap();
    db.create_run(Run {
        status: "generating".into(),
        ..sample_run("run-1")
    });
    drop(db);

    let db2 = Db::open(&root).unwrap();
    db2.run_startup_tasks(Path::new("C:/ws"), "Locagens");
    let run = db2.get_run_full("run-1").unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(
        run.error_message.as_deref(),
        Some("Session interrupted due to server restart.")
    );
    let messages = db2.list_messages("run-1");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "system");

    // The failure is appended, not rewritten, and survives a reopen.
    let files = rollouts_in(&root);
    assert_eq!(kinds_of(&files[0]), ["session_meta", "run_update", "message"]);
    drop(db2);
    let db3 = Db::open(&root).unwrap();
    assert_eq!(db3.get_run_full("run-1").unwrap().status, "failed");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rollout_round_trip_layout_and_history() {
    let root = temp_root("roundtrip");
    let db = Db::open(&root).unwrap();
    db.create_run(sample_run("run-a"));
    db.append_message(msg("run-a", "m1", "user", "hello there"));
    db.append_message(RunMessage {
        agent_role: Some("explorer".into()),
        ..msg("run-a", "m2", "user", "sub-agent prompt")
    });
    db.append_message(msg("run-a", "m3", "assistant", "hi"));
    db.set_plan(Plan {
        id: "plan-1".into(),
        run_id: "run-a".into(),
        title: "Plan".into(),
        tasks: json!([{"text": "step"}]),
        version: 1,
        ..Default::default()
    });
    db.append_usage(UsageLogEntry {
        run_id: "run-a".into(),
        provider_id: "p".into(),
        model: "m".into(),
        input_tokens: 10,
        cost: 0.5,
        ..Default::default()
    });
    drop(db);

    // One date-sharded rollout named after the run's creation time.
    let files = rollouts_in(&root);
    assert_eq!(files.len(), 1);
    let rel = files[0].strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
    assert_eq!(rel, "sessions/2026/03/07/rollout-2026-03-07T10-20-30-run-a.jsonl");
    assert_eq!(
        kinds_of(&files[0]),
        ["session_meta", "message", "message", "message", "plan", "usage"]
    );
    let first = &lines_of(&files[0])[0];
    assert!(first["timestamp"].is_string());
    assert_eq!(first["payload"]["id"], "run-a");

    let db2 = Db::open(&root).unwrap();
    assert_eq!(db2.get_run_full("run-a").unwrap(), sample_run("run-a"));
    assert_eq!(db2.list_messages("run-a").len(), 3);
    assert_eq!(db2.get_plan("run-a").unwrap().title, "Plan");
    assert_eq!(db2.list_usage(Some("run-a")).len(), 1);
    assert_eq!(db2.list_usage(None)[0].cost, 0.5);
    assert!(!root.join("usage_logs.jsonl").exists());

    // Index: last line per id wins; history: only the main-agent prompt.
    let history: Vec<Value> = read_jsonl_or(&root.join(HISTORY_FILE));
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["session_id"], "run-a");
    assert_eq!(history[0]["text"], "hello there");
    assert!(history[0]["ts"].is_u64());
    db2.patch_run("run-a", |run| run.title = "Renamed".into());
    let index: Vec<Value> = read_jsonl_or(&root.join(SESSION_INDEX_FILE));
    assert_eq!(index.first().unwrap()["title"], "Test");
    assert_eq!(index.last().unwrap()["id"], "run-a");
    assert_eq!(index.last().unwrap()["title"], "Renamed");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn updates_are_appended_and_replayed_last_write_wins() {
    let root = temp_root("messages");
    let db = Db::open(&root).unwrap();
    db.append_message(msg("run-x", "m1", "assistant", "first"));
    db.append_message(msg("run-x", "m2", "user", "second"));
    let updated = db.update_message("run-x", "m1", "rewritten", Some("thinking"));
    assert_eq!(updated.unwrap().content, "rewritten");
    db.update_message_full("run-x", "m1", "rewritten again", None, Some("[]"));

    let files = rollouts_in(&root);
    assert_eq!(
        kinds_of(&files[0]),
        ["message", "message", "message_update", "message_update"],
        "updates append lines instead of rewriting"
    );
    let original = std::fs::read_to_string(&files[0]).unwrap();
    assert!(original.lines().next().unwrap().contains("\"first\""));

    drop(db);
    let db2 = Db::open(&root).unwrap();
    let messages = db2.list_messages("run-x");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "rewritten again");
    assert_eq!(messages[0].reasoning_content.as_deref(), Some("thinking"));
    assert_eq!(messages[0].raw_response.as_deref(), Some("[]"));

    let entry = db2.append_usage(UsageLogEntry {
        run_id: "run-x".into(),
        provider_id: "opencode-go".into(),
        model: "glm-5.3-flash".into(),
        input_tokens: 10,
        output_tokens: 5,
        ..Default::default()
    });
    assert_eq!(entry.id, 1);
    assert_eq!(db2.list_usage(Some("run-x")).len(), 1);
    assert_eq!(db2.list_usage(Some("run-y")).len(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn run_updates_replay_to_latest_snapshot() {
    let root = temp_root("runupdates");
    let db = Db::open(&root).unwrap();
    db.create_run(sample_run("run-u"));
    db.update_run_status("run-u", "failed", Some("boom"));
    db.patch_run("run-u", |run| run.title = "New title".into());
    drop(db);

    let files = rollouts_in(&root);
    assert_eq!(kinds_of(&files[0]), ["session_meta", "run_update", "run_update"]);
    let db2 = Db::open(&root).unwrap();
    let run = db2.get_run_full("run-u").unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(run.error_message.as_deref(), Some("boom"));
    assert_eq!(run.title, "New title");
    assert_eq!(db2.list_runs().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn fork_run_writes_new_rollout_with_copied_lines() {
    let root = temp_root("fork");
    let db = Db::open(&root).unwrap();
    let source = db.create_run(Run {
        title: "Original".into(),
        ..sample_run("run-src")
    });
    for (id, role, content) in [
        ("m1", "user", "first"),
        ("m2", "assistant", "answer"),
        ("m3", "user", "second"),
    ] {
        db.append_message(msg("run-src", id, role, content));
    }
    db.set_plan(Plan {
        id: "plan-1".into(),
        run_id: "run-src".into(),
        title: "Plan".into(),
        tasks: json!([{"text": "step"}]),
        version: 1,
        ..Default::default()
    });

    // Fork up to (and including) the middle message.
    let fork = db.fork_run("run-src", Some("m2")).expect("fork created");
    assert_ne!(fork.id, source.id);
    assert_eq!(fork.title, "Original (fork)");
    assert_eq!(fork.status, "done");
    assert_eq!(fork.error_message, None);
    let fork_messages = db.list_messages(&fork.id);
    assert_eq!(fork_messages.len(), 2);
    assert_eq!(fork_messages[0].content, "first");
    assert_eq!(fork_messages[1].content, "answer");
    assert!(fork_messages.iter().all(|message| message.run_id == fork.id));
    assert!(fork_messages.iter().all(|message| message.id != "m1" && message.id != "m2"));
    let fork_plan = db.get_plan(&fork.id).expect("plan cloned");
    assert_eq!(fork_plan.title, "Plan");
    assert_eq!(fork_plan.run_id, fork.id);

    // It is its own rollout file, marked as forked.
    assert_eq!(rollouts_in(&root).len(), 2);
    let fork_file = rollouts_in(&root)
        .into_iter()
        .find(|path| path.to_string_lossy().contains(&fork.id))
        .unwrap();
    assert_eq!(kinds_of(&fork_file), ["session_meta", "message", "message", "plan"]);
    assert_eq!(lines_of(&fork_file)[0]["payload"]["forked_from"], "run-src");

    // Fork is fully independent: mutations on either side don't leak.
    db.append_message(msg("run-src", "m4", "user", "later"));
    db.append_message(msg(&fork.id, "mf", "user", "fork-only"));
    assert_eq!(db.list_messages("run-src").len(), 4);
    assert_eq!(db.list_messages(&fork.id).len(), 3);
    assert_eq!(db.get_plan("run-src").unwrap().run_id, "run-src");

    // Unknown source or unknown fork point → None.
    assert!(db.fork_run("nope", None).is_none());
    assert!(db.fork_run("run-src", Some("missing")).is_none());

    // Forking without a cut point copies everything.
    let full = db.fork_run("run-src", None).unwrap();
    assert_eq!(db.list_messages(&full.id).len(), 4);

    // And it all survives a reopen.
    drop(db);
    let db2 = Db::open(&root).unwrap();
    assert_eq!(db2.list_runs().len(), 3);
    assert_eq!(db2.list_messages(&fork.id).len(), 3);
    assert_eq!(db2.get_plan(&fork.id).unwrap().run_id, fork.id);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn delete_run_removes_rollout_and_empty_date_dirs() {
    let root = temp_root("delete");
    let db = Db::open(&root).unwrap();
    db.create_run(sample_run("run-d"));
    db.create_run(sample_run("run-keep"));
    db.append_message(msg("run-d", "m1", "user", "bye"));
    db.append_usage(UsageLogEntry {
        run_id: "run-d".into(),
        ..Default::default()
    });
    db.append_usage(UsageLogEntry {
        run_id: "run-keep".into(),
        ..Default::default()
    });
    assert_eq!(rollouts_in(&root).len(), 2);

    assert!(db.delete_run("run-d"));
    assert!(!db.delete_run("run-d"));
    assert_eq!(rollouts_in(&root).len(), 1);
    assert!(db.get_run_full("run-d").is_none());
    assert!(db.list_messages("run-d").is_empty());
    assert_eq!(db.list_usage(None).len(), 1);

    assert!(db.delete_run("run-keep"));
    assert!(!root.join("sessions").join("2026").exists(), "empty date dirs pruned");
    drop(db);
    let db2 = Db::open(&root).unwrap();
    assert!(db2.list_runs().is_empty());
    assert!(db2.list_usage(None).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn corrupt_or_truncated_last_line_is_skipped_and_appends_recover() {
    let root = temp_root("corrupt");
    let db = Db::open(&root).unwrap();
    db.create_run(sample_run("run-c"));
    db.append_message(msg("run-c", "m1", "user", "kept"));
    db.append_message(msg("run-c", "m2", "assistant", "will be cut"));
    drop(db);

    // Cut the file in the middle of the last line.
    let file = rollouts_in(&root).remove(0);
    let raw = std::fs::read(&file).unwrap();
    std::fs::write(&file, &raw[..raw.len() - 20]).unwrap();

    let db2 = Db::open(&root).unwrap();
    let messages = db2.list_messages("run-c");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "kept");
    assert!(db2.get_run_full("run-c").is_some());

    // A new append must not be glued onto the truncated line.
    db2.append_message(msg("run-c", "m3", "assistant", "after"));
    drop(db2);
    let db3 = Db::open(&root).unwrap();
    let contents: Vec<String> = db3.list_messages("run-c").into_iter().map(|m| m.content).collect();
    assert_eq!(contents, ["kept", "after"]);

    // Garbage in the middle is skipped too.
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.insert_str(text.find('\n').unwrap() + 1, "not json at all\n");
    std::fs::write(&file, text).unwrap();
    let db4 = Db::open(&root).unwrap();
    assert_eq!(db4.list_messages("run-c").len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn memories_are_markdown_files_with_escaped_frontmatter() {
    let root = temp_root("memmd");
    let db = Db::open(&root).unwrap();
    let nasty = "line one\n---\nid: 99\n\"quoted\" # not a comment: yes\r\nünïcode ✓\n\ntrailing  ";
    let global = db.create_memory("global", "", "we\"ird: cat\negory", nasty).unwrap();
    let project = db
        .create_memory("project", "C:\\Users\\me\\proj: x", "reference", "Use cargo nextest")
        .unwrap();

    let global_files: Vec<_> = std::fs::read_dir(root.join("memories")).unwrap().flatten().collect();
    assert_eq!(global_files.len(), 1);
    let text = std::fs::read_to_string(global_files[0].path()).unwrap();
    assert!(text.starts_with("---\nid: 1\n"));
    assert!(text.ends_with(nasty), "body is stored verbatim");
    let project_dir = root.join("projects").join("C--Users-me-proj--x").join("memories");
    let project_files: Vec<_> = std::fs::read_dir(&project_dir).unwrap().flatten().collect();
    assert_eq!(project_files.len(), 1);
    assert!(project_files[0].file_name().to_string_lossy().starts_with("use-cargo-nextest-"));
    assert!(!root.join("memory.json").exists());

    drop(db);
    let db2 = Db::open(&root).unwrap();
    let memories = db2.list_memories();
    assert_eq!(memories.len(), 2);
    let g = memories.iter().find(|m| m.id == global.id).unwrap();
    assert_eq!(g, &global);
    let p = memories.iter().find(|m| m.id == project.id).unwrap();
    assert_eq!(p, &project);

    // Updates rewrite the same file; deletes remove it.
    let edited = db2.update_memory(global.id, "short").unwrap();
    assert_eq!(edited.category, global.category);
    assert_eq!(std::fs::read_dir(root.join("memories")).unwrap().count(), 1);
    assert!(db2.delete_memory(project.id));
    assert_eq!(std::fs::read_dir(&project_dir).unwrap().count(), 0);
    drop(db2);
    let db3 = Db::open(&root).unwrap();
    assert_eq!(db3.list_memories().len(), 1);
    assert_eq!(db3.list_memories()[0].content, "short");
    assert_eq!(db3.create_memory("global", "", "c", "next").unwrap().id, 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn legacy_data_is_imported_once_and_left_untouched() {
    let legacy = temp_root("legacy-src");
    let root = temp_root("legacy-dst");

    // Build a legacy `data/` tree by hand.
    let run = sample_run("run-old");
    let dir = legacy.join("sessions").join("run-old");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.json"), serde_json::to_string_pretty(&run).unwrap()).unwrap();
    let m1 = RunMessage {
        created_at: "2026-03-07T10:21:00.000Z".into(),
        ..msg("run-old", "m1", "user", "old prompt")
    };
    let m2 = RunMessage {
        created_at: "2026-03-07T10:22:00.000Z".into(),
        ..msg("run-old", "m2", "assistant", "old answer")
    };
    let jsonl = [&m1, &m2]
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("messages.jsonl"), format!("{jsonl}\n")).unwrap();
    let plan = Plan {
        id: "plan-old".into(),
        run_id: "run-old".into(),
        title: "Old plan".into(),
        tasks: json!([]),
        version: 1,
        ..Default::default()
    };
    std::fs::write(dir.join("plan.json"), serde_json::to_string(&plan).unwrap()).unwrap();
    let usage = |id: i64, run_id: &str| UsageLogEntry {
        id,
        run_id: run_id.into(),
        provider_id: "p".into(),
        model: "m".into(),
        input_tokens: 7,
        ..Default::default()
    };
    std::fs::write(
        legacy.join("usage_logs.jsonl"),
        format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&usage(1, "run-old")).unwrap(),
            serde_json::to_string(&usage(2, "run-gone")).unwrap(),
            serde_json::to_string(&usage(3, "run-old")).unwrap(),
        ),
    )
    .unwrap();
    let memory = Memory {
        id: 5,
        scope: "project".into(),
        project_path: "C:/ws".into(),
        category: "reference".into(),
        content: "legacy fact: with colon".into(),
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-02T00:00:00.000Z".into(),
    };
    std::fs::write(legacy.join("memory.json"), serde_json::to_string(&vec![memory.clone()]).unwrap()).unwrap();
    std::fs::write(
        legacy.join("projects.json"),
        serde_json::to_string(&vec![Project {
            path: "C:/ws".into(),
            name: "WS".into(),
            created_at: now_iso(),
        }])
        .unwrap(),
    )
    .unwrap();
    std::fs::write(legacy.join("permissions.json"), "[]").unwrap();

    let snapshot = |dir: &Path| -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        walk(dir, &mut out);
        out.sort();
        out
    };
    let before = snapshot(&legacy);

    let db = Db::open_with_legacy(&root, Some(&legacy)).unwrap();
    assert_eq!(snapshot(&legacy), before, "legacy dir is never modified");
    assert!(root.join(MIGRATION_MARKER).exists());
    assert_eq!(db.get_run_full("run-old").unwrap(), run);
    let messages = db.list_messages("run-old");
    assert_eq!(messages, vec![m1.clone(), m2.clone()]);
    assert_eq!(db.get_plan("run-old").unwrap().title, "Old plan");
    let usage_ids: Vec<i64> = db.list_usage(None).iter().map(|u| u.id).collect();
    assert_eq!(usage_ids, [1, 3], "usage of vanished runs is dropped");
    assert_eq!(db.list_memories(), vec![memory.clone()]);
    assert_eq!(db.list_projects()[0].name, "WS");
    let history: Vec<Value> = read_jsonl_or(&root.join(HISTORY_FILE));
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["text"], "old prompt");
    assert_eq!(history[0]["ts"], unix_secs("2026-03-07T10:21:00.000Z"));
    let index: Vec<Value> = read_jsonl_or(&root.join(SESSION_INDEX_FILE));
    assert_eq!(index.len(), 1);
    assert_eq!(index[0]["id"], "run-old");
    // Fresh ids continue after the imported ones.
    assert_eq!(db.append_usage(usage(0, "run-old")).id, 4);
    assert_eq!(db.create_memory("global", "", "c", "new").unwrap().id, 6);
    drop(db);

    // Second open does not import again (even after changing the legacy dir).
    std::fs::write(legacy.join("memory.json"), "[]").unwrap();
    let db2 = Db::open_with_legacy(&root, Some(&legacy)).unwrap();
    assert_eq!(db2.list_memories().len(), 2);
    assert_eq!(db2.list_runs().len(), 1);
    assert_eq!(rollouts_in(&root).len(), 1);
    let history: Vec<Value> = read_jsonl_or(&root.join(HISTORY_FILE));
    assert_eq!(history.len(), 1);

    // A plain `open` on a custom root never looks at the real app data.
    assert!(default_legacy_dir_for(&root).is_none());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&legacy);
}

/// Two `Db` instances on one root (desktop + `lcgn`) must not overwrite
/// each other's shared files: mutations re-read them from disk under the
/// advisory lock before applying the change.
#[test]
fn two_instances_interleave_without_losing_writes() {
    let root = temp_root("cross");
    let db1 = Db::open(&root).unwrap();
    let db2 = Db::open(&root).unwrap();

    db1.create_project("C:/a", "A");
    db2.create_project("C:/b", "B");
    db1.create_memory("project", "C:/a", "reference", "one").unwrap();
    db2.create_memory("project", "C:/b", "reference", "two").unwrap();
    let p1 = db1.upsert_permission("run", "C:/a", "run_command", "git status", vec![], "allow_once");
    let p2 = db2.upsert_permission("run", "C:/b", "run_command", "git push", vec![], "allow_once");
    let u1 = db1.append_usage(UsageLogEntry {
        run_id: "r1".into(),
        provider_id: "p".into(),
        model: "m".into(),
        ..Default::default()
    });
    let u2 = db2.append_usage(UsageLogEntry {
        run_id: "r2".into(),
        provider_id: "p".into(),
        model: "m".into(),
        ..Default::default()
    });

    // Fresh instance sees everything both wrote, with no id collisions.
    let db3 = Db::open(&root).unwrap();
    let paths: Vec<String> = db3.list_projects().iter().map(|p| p.path.clone()).collect();
    assert!(paths.contains(&"C:/a".to_string()) && paths.contains(&"C:/b".to_string()));
    let memories = db3.list_memories();
    assert_eq!(memories.len(), 2);
    assert!(memories.iter().any(|m| m.content == "one"));
    assert!(memories.iter().any(|m| m.content == "two"));
    assert_eq!(db3.list_permissions().len(), 2);
    assert_ne!(p1.id, p2.id, "ids must not collide across processes");
    assert_eq!(db3.list_usage(None).len(), 2);
    assert_ne!(u1.id, u2.id, "usage ids must not collide across processes");

    // refresh_runs picks up another instance's runs.
    db1.create_run(sample_run("run-from-1"));
    assert!(db2.get_run_full("run-from-1").is_none());
    db2.refresh_runs();
    assert!(db2.get_run_full("run-from-1").is_some());

    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------
// QA-034..041 regression tests
// ------------------------------------------------------------------

fn active_run(id: &str) -> Run {
    Run {
        status: "generating".into(),
        ..sample_run(id)
    }
}

#[test]
fn deleted_run_is_never_resurrected_by_late_writes() {
    let root = temp_root("tombstone");
    let db = Db::open(&root).unwrap();
    db.create_run(active_run("run-del"));
    db.append_message(msg("run-del", "m1", "user", "hi"));
    assert_eq!(rollouts_in(&root).len(), 1);

    assert!(db.delete_run("run-del"));
    assert!(rollouts_in(&root).is_empty());

    // A drive task that is still unwinding keeps writing.
    db.append_message(msg("run-del", "m2", "assistant", "late"));
    db.update_run_status("run-del", "done", None);
    db.set_plan(Plan {
        run_id: "run-del".into(),
        ..Default::default()
    });
    db.append_usage(UsageLogEntry {
        run_id: "run-del".into(),
        ..Default::default()
    });
    assert!(db.stream_message("run-del", "m2", "x", None).is_none());
    assert!(rollouts_in(&root).is_empty(), "no headless rollout may reappear");
    assert!(db.list_messages("run-del").is_empty());
    assert!(db.list_usage(Some("run-del")).is_empty());
    assert!(db.get_plan("run-del").is_none());

    // A reopen shows nothing either.
    drop(db);
    assert!(Db::open(&root).unwrap().list_runs().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn streaming_updates_are_coalesced_but_the_final_text_persists() {
    let root = temp_root("stream");
    let db = Db::open(&root).unwrap();
    db.create_run(active_run("run-s"));
    db.append_message(msg("run-s", "a1", "assistant", ""));
    let mut text = String::new();
    for chunk in 0..200 {
        text.push_str(&format!("chunk {chunk} "));
        let live = db.stream_message("run-s", "a1", &text, Some("thinking")).unwrap();
        assert_eq!(live.content, text, "memory is always current");
    }
    assert_eq!(db.list_messages("run-s")[0].content, text);
    let streaming_lines = kinds_of(&rollouts_in(&root)[0])
        .iter()
        .filter(|kind| *kind == "message_update")
        .count();
    assert_eq!(streaming_lines, 0, "intermediate states stay in memory");

    db.update_message_full("run-s", "a1", &text, Some("thinking"), None);
    let kinds = kinds_of(&rollouts_in(&root)[0]);
    assert_eq!(kinds.iter().filter(|kind| *kind == "message_update").count(), 1);

    drop(db);
    let reopened = Db::open(&root).unwrap();
    assert_eq!(reopened.list_messages("run-s")[0].content, text);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn startup_leaves_runs_of_a_live_process_alone() {
    let root = temp_root("owner");
    let owner = Db::open(&root).unwrap();
    owner.create_run(active_run("run-live"));

    // A second process starts while the first still drives the run.
    let other = Db::open(&root).unwrap();
    other.run_startup_tasks(Path::new("C:/ws"), "Locagens");
    assert_eq!(other.get_run_full("run-live").unwrap().status, "generating");
    assert!(other.list_messages("run-live").is_empty(), "no restart message injected");

    // The owner finishes: the claim is released, nothing is stuck.
    owner.update_run_status("run-live", "done", None);
    assert!(!run_lock_path(&root, "run-live").exists());

    // An owner that died mid-run (lock released with the process) is failed.
    owner.create_run(active_run("run-dead"));
    drop(owner);
    let third = Db::open(&root).unwrap();
    third.run_startup_tasks(Path::new("C:/ws"), "Locagens");
    assert_eq!(third.get_run_full("run-dead").unwrap().status, "failed");
    assert_eq!(third.get_run_full("run-live").unwrap().status, "done");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_panic_inside_a_closure_does_not_kill_the_store() {
    let root = temp_root("poison");
    let db = Db::open(&root).unwrap();
    db.create_run(sample_run("run-p"));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.patch_run("run-p", |_| panic!("boom"));
    }));
    assert!(result.is_err());
    // Every later call still works.
    assert!(db.get_run_full("run-p").is_some());
    db.append_message(msg("run-p", "m", "user", "still alive"));
    assert_eq!(db.list_messages("run-p").len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unopenable_lock_file_degrades_instead_of_panicking() {
    let root = temp_root("nolock");
    // `.lock` is a directory, so it can never be opened as a file.
    std::fs::create_dir_all(root.join(".lock")).unwrap();
    let db = Db::open(&root).unwrap();
    let project = db.create_project("C:/x", "x");
    assert_eq!(project.path, "C:/x");
    assert_eq!(db.list_projects().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn changes_from_another_process_become_visible() {
    let root = temp_root("crossproc");
    let a = Db::open(&root).unwrap();
    let b = Db::open(&root).unwrap();

    // Permissions: a grant revoked by one instance disappears for the other.
    let rule = a.upsert_permission("global", "", "run_command", "ls", vec![], "allow_always");
    assert_eq!(b.list_permissions().len(), 1);
    a.delete_permission(rule.id);
    assert!(b.list_permissions().is_empty());

    // Projects and memories likewise.
    a.create_project("C:/shared", "shared");
    assert!(b.get_project("C:/shared").is_some());
    let memory = a.create_memory("global", "", "ref", "remember me").unwrap();
    assert_eq!(b.list_memories().len(), 1);
    a.update_memory(memory.id, "edited");
    assert_eq!(b.list_memories()[0].content, "edited");
    a.delete_memory(memory.id);
    assert!(b.list_memories().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn refresh_runs_is_incremental_and_spares_runs_this_process_drives() {
    let root = temp_root("refresh");
    let a = Db::open(&root).unwrap();
    let b = Db::open(&root).unwrap();

    // b is driving a run with live streaming state.
    b.create_run(active_run("run-mine"));
    b.append_message(msg("run-mine", "am", "assistant", ""));
    b.stream_message("run-mine", "am", "streamed so far", None);

    // a finishes one run, then continues it with another message.
    a.create_run(sample_run("run-theirs"));
    b.refresh_runs();
    assert!(b.get_run_full("run-theirs").is_some());
    assert!(b.list_messages("run-theirs").is_empty());
    a.append_message(msg("run-theirs", "t1", "user", "more"));
    b.refresh_runs();
    assert_eq!(b.list_messages("run-theirs").len(), 1);

    // The run b drives was not reverted to its (stale) on-disk state.
    assert_eq!(b.list_messages("run-mine")[0].content, "streamed so far");

    // A run deleted by another process disappears.
    a.delete_run("run-theirs");
    b.refresh_runs();
    assert!(b.get_run_full("run-theirs").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn no_session_runs_leave_no_index_or_history_and_are_swept_at_startup() {
    let root = temp_root("ephemeral");
    let db = Db::open(&root).unwrap();
    db.create_run(Run {
        ephemeral: true,
        ..active_run("run-eph")
    });
    db.append_message(msg("run-eph", "u1", "user", "secret prompt"));
    db.update_run_status("run-eph", "generating", None);
    assert!(!root.join(SESSION_INDEX_FILE).exists());
    assert!(!root.join(HISTORY_FILE).exists());

    // A crash left it behind: the next startup removes it entirely.
    drop(db);
    let reopened = Db::open(&root).unwrap();
    assert!(reopened.get_run_full("run-eph").is_some());
    reopened.run_startup_tasks(Path::new("C:/ws"), "Locagens");
    assert!(reopened.get_run_full("run-eph").is_none());
    assert!(rollouts_in(&root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn usage_ids_stay_unique_without_reloading_shared_files() {
    let root = temp_root("usage");
    let a = Db::open(&root).unwrap();
    let b = Db::open(&root).unwrap();
    let mut ids = HashSet::new();
    for i in 0..10 {
        let (db, run) = if i % 2 == 0 { (&a, "ra") } else { (&b, "rb") };
        let entry = db.append_usage(UsageLogEntry {
            run_id: run.into(),
            ..Default::default()
        });
        assert!(ids.insert(entry.id), "duplicate usage id {}", entry.id);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn user_message_images_persist_and_old_messages_still_load() {
    let root = temp_root("images");
    let db = Db::open(&root).unwrap();
    db.create_run(active_run("run-i"));
    let mut message = msg("run-i", "u1", "user", "look");
    message.images = vec![ImageAttachment { mime: "image/png".into(), data: "QUJD".into() }];
    db.append_message(message.clone());
    db.append_message(msg("run-i", "u2", "user", "plain"));
    drop(db);
    let reopened = Db::open(&root).unwrap();
    let messages = reopened.list_messages("run-i");
    assert_eq!(messages[0].images, message.images);
    assert!(messages[1].images.is_empty());
    // An old message without the field loads, and a text-only one omits it.
    let old: RunMessage = serde_json::from_value(serde_json::json!({ "id": "m", "role": "user", "content": "x" })).unwrap();
    assert!(old.images.is_empty());
    assert!(serde_json::to_value(&old).unwrap().get("images").is_none());
    let _ = std::fs::remove_dir_all(&root);
}
