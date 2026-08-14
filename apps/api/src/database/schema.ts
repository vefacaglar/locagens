import type { DatabaseSync } from "node:sqlite";

export function setupSchema(db: DatabaseSync, wsRoot: string, defaultProjectName: string) {
  // Create Runs Table
  db.exec(`
    CREATE TABLE IF NOT EXISTS runs (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      task TEXT NOT NULL,
      project_path TEXT NOT NULL DEFAULT '${wsRoot.replace(/'/g, "''")}',
      project_name TEXT NOT NULL DEFAULT '${defaultProjectName.replace(/'/g, "''")}',
      status TEXT NOT NULL,

      provider_id TEXT NOT NULL,
      provider_display_name TEXT NOT NULL,
      model TEXT NOT NULL,
      mode TEXT NOT NULL DEFAULT 'accept_edits',

      error_message TEXT,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      last_active_at TEXT
    );
  `);

  // Create Messages Table
  db.exec(`
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
  `);

  // Covers MessageRepository.listByRunId (WHERE run_id ... ORDER BY created_at):
  // messages is the fastest-growing table and is scanned on every run open.
  db.exec("CREATE INDEX IF NOT EXISTS idx_messages_run ON messages(run_id, created_at)");

  // Create Projects Table
  db.exec(`
    CREATE TABLE IF NOT EXISTS projects (
      path TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      created_at TEXT NOT NULL
    );
  `);

  // Create Permissions Table.
  db.exec(`
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
  `);

  // Create Plans Table.
  db.exec(`
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
  `);
  db.exec("CREATE INDEX IF NOT EXISTS idx_plans_run ON plans(run_id)");

  // Create Memory Table.
  db.exec(`
    CREATE TABLE IF NOT EXISTS memory (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      scope TEXT NOT NULL,
      project_path TEXT NOT NULL DEFAULT '',
      category TEXT NOT NULL DEFAULT 'project',
      content TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
  `);
  db.exec("CREATE INDEX IF NOT EXISTS idx_memory_scope_project ON memory(scope, project_path)");

  // Create Usage Logs Table.
  db.exec(`
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
  `);
  db.exec("CREATE INDEX IF NOT EXISTS idx_usage_logs_run ON usage_logs(run_id)");
}
