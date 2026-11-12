//! SQLite database layer (`locagens.db`) — a thread-safe `rusqlite::Connection`
//! wrapper plus schema creation and startup tasks, replacing `apps/db-writer`
//! and `apps/api/src/database`.

use crate::util::now_iso;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// A durable fact remembered via the `remember` tool / Settings → Memory.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: i64,
    pub scope: String,
    pub project_path: String,
    pub category: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A persisted standing approval for workspace tool calls.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub id: i64,
    pub scope: String,
    pub project_path: String,
    pub tool: String,
    pub command: String,
    pub network_domains: Vec<String>,
    pub status: String,
}

/// A registered project folder.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub path: String,
    pub name: String,
    pub created_at: String,
}

/// The columns of `runs` consumed by this backend phase.
#[derive(Clone, Default)]
pub struct RunRow {
    pub id: String,
    pub title: String,
    pub project_path: Option<String>,
    pub project_name: String,
    pub provider_id: String,
    pub provider_display_name: String,
    pub model: String,
    pub coder_provider_id: Option<String>,
    pub coder_model: Option<String>,
}

pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA_SQL: &str = r#"
PRAGMA busy_timeout = 10000;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    task TEXT NOT NULL,
    project_path TEXT NOT NULL DEFAULT '',
    project_name TEXT NOT NULL DEFAULT 'Locagens',
    status TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_display_name TEXT NOT NULL,
    model TEXT NOT NULL,
    reasoning_effort TEXT,
    mode TEXT NOT NULL DEFAULT 'accept_edits',
    error_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_active_at TEXT,
    coder_provider_id TEXT,
    coder_model TEXT,
    agent_preset TEXT,
    utility_provider_id TEXT,
    utility_model TEXT,
    coder_reasoning_effort TEXT,
    utility_reasoning_effort TEXT
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    role TEXT NOT NULL,
    agent_role TEXT,
    agent_name TEXT,
    provider_id TEXT,
    provider_display_name TEXT,
    model TEXT,
    content TEXT NOT NULL,
    reasoning_content TEXT,
    raw_response TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE INDEX IF NOT EXISTS idx_messages_run ON messages(run_id, created_at);

CREATE TABLE IF NOT EXISTS projects (
    path TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS permissions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scope TEXT NOT NULL,
    project_path TEXT NOT NULL DEFAULT '',
    tool TEXT NOT NULL DEFAULT '',
    command TEXT NOT NULL DEFAULT '',
    network_domains TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL,
    UNIQUE(scope, project_path, tool, command, network_domains)
);

CREATE TABLE IF NOT EXISTS plans (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT 'Plan',
    body TEXT,
    tasks TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'active',
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE INDEX IF NOT EXISTS idx_plans_run ON plans(run_id);

CREATE TABLE IF NOT EXISTS memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scope TEXT NOT NULL,
    project_path TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT 'project',
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memory_scope_project ON memory(scope, project_path);

CREATE TABLE IF NOT EXISTS usage_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    agent_role TEXT,
    provider_id TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_hit_rate REAL NOT NULL DEFAULT 0.0,
    cost REAL NOT NULL DEFAULT 0.0,
    created_at TEXT NOT NULL,
    duration_ms INTEGER,
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE INDEX IF NOT EXISTS idx_usage_logs_run ON usage_logs(run_id);
"#;

impl Db {
    /// Opens (creating if needed) the SQLite database at `path` with WAL mode.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Serialized access to the connection.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("database mutex poisoned")
    }

    /// Seeds the default project and fails any run left active by a restart.
    pub fn run_startup_tasks(&self, ws_root: &Path, default_project_name: &str) {
        let conn = self.lock();
        let ws_root_str = ws_root.to_string_lossy().to_string();

        // Seed default project if the table is empty.
        let count: i64 = conn
            .query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
            .unwrap_or(0);
        if count == 0 {
            let _ = conn.execute(
                "INSERT INTO projects (path, name, created_at) VALUES (?, ?, ?)",
                rusqlite::params![ws_root_str, default_project_name, now_iso()],
            );
        }

        // Reset any runs left in active states by the previous session.
        let stuck: Vec<String> = {
            let mut stmt = match conn.prepare(
                "SELECT id FROM runs WHERE status IN ('created', 'generating', 'awaiting_permission', 'awaiting_input')",
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    tracing::error!("[Database] Failed to query stuck runs: {err}");
                    return;
                }
            };
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map(|mapped| mapped.filter_map(Result::ok).collect::<Vec<_>>())
                .unwrap_or_default();
            rows
        };

        for run_id in stuck {
            let _ = conn.execute(
                "UPDATE runs SET status = 'failed', error_message = 'Session interrupted due to server restart.' WHERE id = ?",
                rusqlite::params![run_id],
            );
            let _ = conn.execute(
                "INSERT INTO messages (id, run_id, role, content, created_at) VALUES (?, ?, 'system', 'Session interrupted due to server restart.', ?)",
                rusqlite::params![
                    format!("msg-err-restart-{}-{}", run_id, chrono::Utc::now().timestamp_millis()),
                    run_id,
                    now_iso()
                ],
            );
        }
    }

    // ------------------------------------------------------------------
    // Projects
    // ------------------------------------------------------------------

    pub fn list_projects(&self) -> Vec<Project> {
        let conn = self.lock();
        let mut stmt = match conn.prepare("SELECT path, name, created_at FROM projects") {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            Ok(Project {
                path: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    pub fn get_project(&self, path: &str) -> Option<Project> {
        let conn = self.lock();
        conn.query_row(
            "SELECT path, name, created_at FROM projects WHERE path = ?1",
            [path],
            |row| {
                Ok(Project {
                    path: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .ok()
    }

    pub fn create_project(&self, path: &str, name: &str) -> Project {
        let created_at = now_iso();
        let conn = self.lock();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO projects (path, name, created_at) VALUES (?, ?, ?)",
            rusqlite::params![path, name, created_at],
        );
        Project {
            path: path.to_string(),
            name: name.to_string(),
            created_at,
        }
    }

    pub fn delete_project(&self, path: &str) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM projects WHERE path = ?1", [path]);
    }

    // ------------------------------------------------------------------
    // Memories
    // ------------------------------------------------------------------

    const MEMORY_COLUMNS: &'static str = "id, scope, project_path, category, content, created_at, updated_at";

    fn memory_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
        Ok(Memory {
            id: row.get(0)?,
            scope: row.get(1)?,
            project_path: row.get(2)?,
            category: row.get(3)?,
            content: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }

    pub fn list_memories(&self) -> Vec<Memory> {
        let conn = self.lock();
        let mut stmt = match conn.prepare(&format!(
            "SELECT {} FROM memory ORDER BY updated_at DESC",
            Self::MEMORY_COLUMNS
        )) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], Self::memory_from_row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn create_memory(
        &self,
        scope: &str,
        project_path: &str,
        category: &str,
        content: &str,
    ) -> anyhow::Result<Memory> {
        let now = now_iso();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO memory (scope, project_path, category, content, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![scope, project_path, category, content, now, now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Memory {
            id,
            scope: scope.to_string(),
            project_path: project_path.to_string(),
            category: category.to_string(),
            content: content.to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn update_memory(&self, id: i64, content: &str) -> Option<Memory> {
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE memory SET content = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![content, now_iso(), id],
            )
            .unwrap_or(0);
        if changed == 0 {
            return None;
        }
        conn.query_row(
            &format!("SELECT {} FROM memory WHERE id = ?1", Self::MEMORY_COLUMNS),
            [id],
            Self::memory_from_row,
        )
        .ok()
    }

    pub fn delete_memory(&self, id: i64) -> bool {
        let conn = self.lock();
        conn.execute("DELETE FROM memory WHERE id = ?1", [id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn clear_memories(&self) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM memory", []);
    }

    // ------------------------------------------------------------------
    // Permissions
    // ------------------------------------------------------------------

    fn permission_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PermissionRule> {
        let network_domains_raw: String = row.get(5)?;
        let network_domains =
            serde_json::from_str::<Vec<String>>(&network_domains_raw).unwrap_or_default();
        Ok(PermissionRule {
            id: row.get(0)?,
            scope: row.get(1)?,
            project_path: row.get(2)?,
            tool: row.get(3)?,
            command: row.get(4)?,
            network_domains,
            status: row.get(6)?,
        })
    }

    pub fn list_permissions(&self) -> Vec<PermissionRule> {
        let conn = self.lock();
        let mut stmt = match conn.prepare(
            "SELECT id, scope, project_path, tool, command, network_domains, status FROM permissions ORDER BY id",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], Self::permission_from_row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn delete_permission(&self, id: i64) -> bool {
        let conn = self.lock();
        conn.execute("DELETE FROM permissions WHERE id = ?1", [id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn clear_permissions(&self) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM permissions", []);
    }

    // ------------------------------------------------------------------
    // Runs (read-only in this phase)
    // ------------------------------------------------------------------

    pub fn get_run(&self, id: &str) -> Option<RunRow> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, title, project_path, project_name, provider_id, provider_display_name, model, coder_provider_id, coder_model FROM runs WHERE id = ?1",
            [id],
            |row| {
                Ok(RunRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    project_path: row.get(2)?,
                    project_name: row.get(3)?,
                    provider_id: row.get(4)?,
                    provider_display_name: row.get(5)?,
                    model: row.get(6)?,
                    coder_provider_id: row.get(7)?,
                    coder_model: row.get(8)?,
                })
            },
        )
        .ok()
    }
}

/// Resolve the database file path: `LOCAGENS_DB_PATH` or `<cwd>/locagens.db`.
pub fn resolve_db_path() -> PathBuf {
    if let Ok(path) = std::env::var("LOCAGENS_DB_PATH") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("locagens.db")
}
