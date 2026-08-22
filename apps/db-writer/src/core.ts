import { DatabaseSync } from "node:sqlite";

export interface DbWriterRequest {
  id: string;
  op: string;
  args?: Record<string, unknown>;
}

export interface DbWriterResponse {
  id: string;
  ok: boolean;
  result?: unknown;
  error?: string;
}

type Args = Record<string, unknown>;

export function initDB(db: DatabaseSync): void {
  const stmts = [
    "PRAGMA busy_timeout = 10000",
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    `CREATE TABLE IF NOT EXISTS runs (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      task TEXT NOT NULL,
      project_path TEXT NOT NULL DEFAULT '',
      project_name TEXT NOT NULL DEFAULT 'Locagens',
      status TEXT NOT NULL,
      provider_id TEXT NOT NULL,
      provider_display_name TEXT NOT NULL,
      model TEXT NOT NULL,
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
      utility_reasoning_effort TEXT,
      reasoning_effort TEXT
    )`,
    `CREATE TABLE IF NOT EXISTS messages (
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
    )`,
    `CREATE TABLE IF NOT EXISTS projects (
      path TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      created_at TEXT NOT NULL
    )`,
    `CREATE TABLE IF NOT EXISTS permissions (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      scope TEXT NOT NULL,
      project_path TEXT NOT NULL DEFAULT '',
      tool TEXT NOT NULL DEFAULT '',
      command TEXT NOT NULL DEFAULT '',
      network_domains TEXT NOT NULL DEFAULT '[]',
      status TEXT NOT NULL,
      UNIQUE(scope, project_path, tool, command, network_domains)
    )`,
    `CREATE TABLE IF NOT EXISTS plans (
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
    )`,
    "CREATE INDEX IF NOT EXISTS idx_plans_run ON plans(run_id)",
    `CREATE TABLE IF NOT EXISTS memory (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      scope TEXT NOT NULL,
      project_path TEXT NOT NULL DEFAULT '',
      category TEXT NOT NULL DEFAULT 'project',
      content TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    )`,
    "CREATE INDEX IF NOT EXISTS idx_memory_scope_project ON memory(scope, project_path)",
    `CREATE TABLE IF NOT EXISTS usage_logs (
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
    )`,
    "CREATE INDEX IF NOT EXISTS idx_usage_logs_run ON usage_logs(run_id)"
  ];
  for (const stmt of stmts) {
    db.exec(stmt);
  }
  const runColumns = ["mode TEXT NOT NULL DEFAULT 'accept_edits'", "last_active_at TEXT", "coder_provider_id TEXT", "coder_model TEXT", "agent_preset TEXT", "utility_provider_id TEXT", "utility_model TEXT", "coder_reasoning_effort TEXT", "utility_reasoning_effort TEXT", "reasoning_effort TEXT"];
  for (const col of runColumns) {
    try {
      db.exec("ALTER TABLE runs ADD COLUMN " + col);
    } catch {
      // Ignored if the column already exists
    }
  }
  for (const col of ["reasoning_content TEXT", "agent_name TEXT"]) {
    try {
      db.exec("ALTER TABLE messages ADD COLUMN " + col);
    } catch {
      // Ignored if the column already exists
    }
  }
  try {
    db.exec("UPDATE runs SET last_active_at = COALESCE(updated_at, created_at) WHERE last_active_at IS NULL");
  } catch {
    // Ignored
  }
  try {
    db.exec("ALTER TABLE usage_logs ADD COLUMN duration_ms INTEGER");
  } catch {
    // Ignored if the column already exists
  }
}

export function execute(db: DatabaseSync, req: DbWriterRequest): unknown {
  const args = req.args ?? {};
  switch (req.op) {
    case "run.create": {
      const run = obj(args, "run");
      return exec(db, `INSERT INTO runs (
        id, title, task, project_path, project_name, status,
        provider_id, provider_display_name, model, reasoning_effort, mode,
        coder_provider_id, coder_model, coder_reasoning_effort, agent_preset,
        utility_provider_id, utility_model, utility_reasoning_effort,
        error_message, created_at, updated_at, last_active_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        str(run, "id"), str(run, "title"), str(run, "task"), strDefault(run, "projectPath", mustCwd()),
        strDefault(run, "projectName", "Workspace"), str(run, "status"), str(run, "providerId"),
        str(run, "providerDisplayName"), str(run, "model"), nullable(run, "reasoningEffort"),
        strDefault(run, "mode", "accept_edits"), nullable(run, "coderProviderId"), nullable(run, "coderModel"),
        nullable(run, "coderReasoningEffort"), nullable(run, "agentPreset"), nullable(run, "utilityProviderId"),
        nullable(run, "utilityModel"), nullable(run, "utilityReasoningEffort"), nullable(run, "errorMessage"),
        str(run, "createdAt"), str(run, "updatedAt"), strDefault(run, "lastActiveAt", str(run, "createdAt")));
    }
    case "run.update":
      return updateRun(db, str(args, "id"), obj(args, "updates"));
    case "message.create": {
      const msg = obj(args, "message");
      db.exec("BEGIN");
      try {
        db.prepare(`INSERT INTO messages (
          id, run_id, role, agent_role, agent_name, provider_id, provider_display_name, model,
          content, reasoning_content, raw_response, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(
          str(msg, "id"), str(msg, "runId"), str(msg, "role"), nullable(msg, "agentRole"),
          nullable(msg, "agentName"), nullable(msg, "providerId"), nullable(msg, "providerDisplayName"),
          nullable(msg, "model"), str(msg, "content"), nullable(msg, "reasoningContent"),
          nullable(msg, "rawResponse"), str(msg, "createdAt"));
        db.prepare("UPDATE runs SET updated_at = ?, last_active_at = ? WHERE id = ?").run(
          str(msg, "createdAt"), str(msg, "createdAt"), str(msg, "runId"));
      } catch (err) {
        db.exec("ROLLBACK");
        throw err;
      }
      db.exec("COMMIT");
      return null;
    }
    case "message.update":
      return updateMessage(db, str(args, "id"), obj(args, "updates"));
    case "plan.updateActive":
      return exec(db, "UPDATE plans SET title = ?, body = ?, tasks = ?, status = 'active', updated_at = ? WHERE id = ?",
        str(args, "title"), nullable(args, "body"), str(args, "tasksJson"), str(args, "now"), str(args, "existingId"));
    case "plan.complete":
      return exec(db, "UPDATE plans SET status = 'completed', updated_at = ? WHERE id = ?",
        str(args, "updatedAt"), str(args, "id"));
    case "plan.create":
      return exec(db, "INSERT INTO plans (id, run_id, title, body, tasks, status, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?)",
        str(args, "id"), str(args, "runId"), str(args, "title"), nullable(args, "body"), str(args, "tasksJson"), intValue(args, "version"), str(args, "now"), str(args, "now"));
    case "project.create": {
      const project = obj(args, "project");
      return exec(db, "INSERT OR REPLACE INTO projects (path, name, created_at) VALUES (?, ?, ?)",
        str(project, "path"), str(project, "name"), str(project, "createdAt"));
    }
    case "project.delete":
      return exec(db, "DELETE FROM projects WHERE path = ?", str(args, "path"));
    case "permission.allowProject":
      return exec(db, "INSERT OR REPLACE INTO permissions (scope, project_path, tool, command, network_domains, status) VALUES ('project', ?, ?, ?, ?, 'allowed')",
        str(args, "projectPath"), str(args, "tool"), str(args, "command"), jsonValue(args, "networkDomains", "[]"));
    case "permission.allowGlobal":
      return exec(db, "INSERT OR REPLACE INTO permissions (scope, project_path, tool, command, network_domains, status) VALUES ('global', '', ?, ?, ?, 'allowed')",
        str(args, "tool"), str(args, "command"), jsonValue(args, "networkDomains", "[]"));
    case "permission.deleteById":
      return exec(db, "DELETE FROM permissions WHERE id = ?", intValue(args, "id"));
    case "permission.clear":
      return exec(db, "DELETE FROM permissions");
    case "memory.create": {
      const res = db.prepare("INSERT INTO memory (scope, project_path, category, content, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)").run(
        str(args, "scope"), str(args, "projectPath"), str(args, "category"), str(args, "content"), str(args, "now"), str(args, "now"));
      return { lastInsertRowid: Number(res.lastInsertRowid) };
    }
    case "memory.update":
      return exec(db, "UPDATE memory SET content = ?, updated_at = ? WHERE id = ?",
        str(args, "content"), str(args, "now"), intValue(args, "id"));
    case "memory.deleteById":
      return exec(db, "DELETE FROM memory WHERE id = ?", intValue(args, "id"));
    case "memory.clear":
      return exec(db, "DELETE FROM memory");
    case "usage_logs.create": {
      const logObj = obj(args, "log");
      return exec(db, `INSERT INTO usage_logs (
        run_id, agent_role, provider_id, model,
        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
        cache_hit_rate, cost, created_at, duration_ms
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        str(logObj, "runId"), nullable(logObj, "agentRole"), str(logObj, "providerId"), str(logObj, "model"),
        intValue(logObj, "inputTokens"), intValue(logObj, "outputTokens"), intValue(logObj, "cacheReadTokens"), intValue(logObj, "cacheWriteTokens"),
        floatValue(logObj, "cacheHitRate"), floatValue(logObj, "cost"), str(logObj, "createdAt"), nullable(logObj, "durationMs"));
    }
    default:
      throw new Error(`unknown op ${JSON.stringify(req.op)}`);
  }
}

type BindValue = string | number | null;

function exec(db: DatabaseSync, sqlText: string, ...args: BindValue[]): { changes: number; lastInsertRowid: number } {
  const res = db.prepare(sqlText).run(...args);
  return { changes: Number(res.changes), lastInsertRowid: Number(res.lastInsertRowid) };
}

function updateRun(db: DatabaseSync, id: string, updates: Args): unknown {
  const mapping: Record<string, string> = {
    title: "title", task: "task", projectPath: "project_path", projectName: "project_name",
    status: "status", providerId: "provider_id", providerDisplayName: "provider_display_name",
    model: "model", reasoningEffort: "reasoning_effort", mode: "mode",
    coderProviderId: "coder_provider_id", coderModel: "coder_model", coderReasoningEffort: "coder_reasoning_effort",
    utilityProviderId: "utility_provider_id", utilityModel: "utility_model", utilityReasoningEffort: "utility_reasoning_effort",
    agentPreset: "agent_preset", errorMessage: "error_message", createdAt: "created_at", lastActiveAt: "last_active_at"
  };
  const fields: string[] = [];
  const args: BindValue[] = [];
  for (const [key, col] of Object.entries(mapping)) {
    if (Object.hasOwn(updates, key)) {
      fields.push(col + " = ?");
      args.push(nullUndefined(updates[key]));
    }
  }
  if (fields.length === 0) {
    return { changes: 0 };
  }
  fields.push("updated_at = ?");
  args.push(nullUndefined(updates["updatedAt"]));
  args.push(id);
  return exec(db, "UPDATE runs SET " + fields.join(", ") + " WHERE id = ?", ...args);
}

function updateMessage(db: DatabaseSync, id: string, updates: Args): unknown {
  const mapping: Record<string, string> = { content: "content", reasoningContent: "reasoning_content", rawResponse: "raw_response" };
  const fields: string[] = [];
  const args: BindValue[] = [];
  for (const [key, col] of Object.entries(mapping)) {
    if (Object.hasOwn(updates, key)) {
      fields.push(col + " = ?");
      args.push(nullUndefined(updates[key]));
    }
  }
  if (fields.length === 0) {
    return { changes: 0 };
  }
  args.push(id);
  return exec(db, "UPDATE messages SET " + fields.join(", ") + " WHERE id = ?", ...args);
}

/** Processes one raw input line and returns the protocol response. Never throws. */
export function handleLine(db: DatabaseSync, line: string): DbWriterResponse | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  let req: DbWriterRequest;
  try {
    req = JSON.parse(trimmed) as DbWriterRequest;
  } catch (err) {
    return { id: "", ok: false, error: "invalid json: " + errMessage(err) };
  }
  try {
    const result = execute(db, req);
    const res: DbWriterResponse = { id: String(req.id ?? ""), ok: true };
    if (result !== null && result !== undefined) res.result = result;
    return res;
  } catch (err) {
    return { id: String(req?.id ?? ""), ok: false, error: errMessage(err) };
  }
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function obj(args: Args, key: string): Args {
  const v = args[key];
  if (v !== null && typeof v === "object" && !Array.isArray(v)) {
    return v as Args;
  }
  return {};
}

function str(m: Args, key: string): string {
  const v = m[key];
  return typeof v === "string" ? v : "";
}

function strDefault(m: Args, key: string, fallback: string): string {
  const v = str(m, key);
  return v !== "" ? v : fallback;
}

function jsonValue(m: Args, key: string, fallback: string): string {
  if (!Object.hasOwn(m, key)) return fallback;
  const encoded = JSON.stringify(m[key]);
  return typeof encoded === "string" ? encoded : fallback;
}

function nullable(m: Args, key: string): BindValue {
  if (Object.hasOwn(m, key)) return nullUndefined(m[key]);
  return null;
}

function nullUndefined(v: unknown): BindValue {
  if (v === undefined || v === null) return null;
  if (typeof v === "string") return v;
  if (typeof v === "number" && Number.isFinite(v)) return v;
  if (typeof v === "bigint") return Number(v);
  return null;
}

function intValue(m: Args, key: string): number {
  const v = m[key];
  if (typeof v === "number" && Number.isFinite(v)) return Math.trunc(v);
  if (typeof v === "bigint") return Number(v);
  return 0;
}

function floatValue(m: Args, key: string): number {
  const v = m[key];
  if (typeof v === "number" && Number.isFinite(v)) return v;
  if (typeof v === "bigint") return Number(v);
  return 0;
}

function mustCwd(): string {
  try {
    return process.cwd();
  } catch {
    return "";
  }
}
