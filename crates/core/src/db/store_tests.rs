//! FR-017 / FR-021 / FR-068 regressions: run ownership across stores,
//! broken shared files, slim `run_update` lines.

use super::*;

fn temp_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locagens-store-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(id: &str, status: &str) -> Run {
    Run {
        id: id.into(),
        task: "a long task ".repeat(200),
        system_prompt: Some("custom system prompt".into()),
        status: status.into(),
        created_at: now_iso(),
        updated_at: now_iso(),
        ..Default::default()
    }
}

/// FR-017 / FR-077: a run claimed by another store is owned elsewhere;
/// this store's own claim is not.
#[test]
fn run_ownership_is_seen_across_stores() {
    let root = temp_root("owner");
    let driver = Db::open(&root).unwrap();
    driver.create_run(run("run-a", "generating"));
    let other = Db::open(&root).unwrap();
    assert!(other.run_owned_elsewhere("run-a"));
    assert!(!driver.run_owned_elsewhere("run-a"));
    driver.patch_run("run-a", |run| run.status = "done".into());
    assert!(!other.run_owned_elsewhere("run-a"));
    drop((driver, other));
    let _ = std::fs::remove_dir_all(&root);
}

/// FR-021: a broken permissions.json is kept aside before it is rewritten.
#[test]
fn broken_shared_files_are_backed_up() {
    let root = temp_root("corrupt");
    std::fs::write(root.join("permissions.json"), "[{\"id\": 1, oops").unwrap();
    let db = Db::open(&root).unwrap();
    assert!(db.list_permissions().is_empty());
    let backup = std::fs::read_to_string(root.join("permissions.json.corrupt")).unwrap();
    assert_eq!(backup, "[{\"id\": 1, oops");
    drop(db);
    let _ = std::fs::remove_dir_all(&root);
}

/// FR-068: status updates do not repeat the task and system prompt, and
/// replay still restores them (including a prompt cleared later).
#[test]
fn run_updates_do_not_repeat_heavy_fields() {
    let root = temp_root("slim");
    let db = Db::open(&root).unwrap();
    db.create_run(run("run-s", "generating"));
    for status in ["awaiting_permission", "generating", "awaiting_permission", "generating"] {
        db.patch_run("run-s", |run| run.status = status.into());
    }
    db.patch_run("run-s", |run| {
        run.status = "done".into();
        run.system_prompt = None;
    });
    let mut rollouts = Vec::new();
    collect_rollouts(&root.join("sessions"), &mut rollouts);
    let text = std::fs::read_to_string(&rollouts[0]).unwrap();
    assert_eq!(text.matches("a long task").count(), 200, "the task is written once");

    let reopened = Db::open(&root).unwrap();
    let replayed = reopened.get_run_full("run-s").unwrap();
    assert_eq!(replayed.task, "a long task ".repeat(200));
    assert_eq!(replayed.status, "done");
    assert_eq!(replayed.system_prompt, None);
    drop((db, reopened));
    let _ = std::fs::remove_dir_all(&root);
}
