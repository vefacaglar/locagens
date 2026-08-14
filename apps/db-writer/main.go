package main

import (
	"bufio"
	"database/sql"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"

	_ "modernc.org/sqlite"
)

type request struct {
	ID   string                 `json:"id"`
	Op   string                 `json:"op"`
	Args map[string]interface{} `json:"args"`
}

type response struct {
	ID     string      `json:"id"`
	OK     bool        `json:"ok"`
	Result interface{} `json:"result,omitempty"`
	Error  string      `json:"error,omitempty"`
}

type workItem struct {
	req  request
	done chan response
}

func main() {
	dbPath := os.Getenv("LOCAGENS_DB_PATH")
	if dbPath == "" {
		log.Fatal("LOCAGENS_DB_PATH is required")
	}
	if err := os.MkdirAll(filepath.Dir(dbPath), 0o755); err != nil {
		log.Fatalf("create db dir: %v", err)
	}

	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		log.Fatalf("open sqlite: %v", err)
	}
	defer db.Close()
	db.SetMaxOpenConns(1)

	if err := initDB(db); err != nil {
		log.Fatalf("init db: %v", err)
	}

	queue := make(chan workItem, 1024)
	go writerLoop(db, queue)

	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var req request
		if err := json.Unmarshal([]byte(line), &req); err != nil {
			writeResponse(response{OK: false, Error: "invalid json: " + err.Error()})
			continue
		}
		item := workItem{req: req, done: make(chan response, 1)}
		queue <- item
		writeResponse(<-item.done)
	}
	if err := scanner.Err(); err != nil {
		log.Printf("stdin scanner: %v", err)
	}
}

func writerLoop(db *sql.DB, queue <-chan workItem) {
	for item := range queue {
		result, err := execute(db, item.req)
		if err != nil {
			item.done <- response{ID: item.req.ID, OK: false, Error: err.Error()}
		} else {
			item.done <- response{ID: item.req.ID, OK: true, Result: result}
		}
	}
}

func writeResponse(res response) {
	_ = json.NewEncoder(os.Stdout).Encode(res)
}

func initDB(db *sql.DB) error {
	stmts := []string{
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
		"CREATE INDEX IF NOT EXISTS idx_usage_logs_run ON usage_logs(run_id)",
	}
	for _, stmt := range stmts {
		if _, err := db.Exec(stmt); err != nil {
			return err
		}
	}
	for _, col := range []string{"mode TEXT NOT NULL DEFAULT 'accept_edits'", "last_active_at TEXT", "coder_provider_id TEXT", "coder_model TEXT", "agent_preset TEXT", "utility_provider_id TEXT", "utility_model TEXT", "coder_reasoning_effort TEXT", "utility_reasoning_effort TEXT", "reasoning_effort TEXT"} {
		_, _ = db.Exec("ALTER TABLE runs ADD COLUMN " + col)
	}
	for _, col := range []string{"reasoning_content TEXT", "agent_name TEXT"} {
		_, _ = db.Exec("ALTER TABLE messages ADD COLUMN " + col)
	}
	_, _ = db.Exec("UPDATE runs SET last_active_at = COALESCE(updated_at, created_at) WHERE last_active_at IS NULL")
	_, _ = db.Exec("ALTER TABLE usage_logs ADD COLUMN duration_ms INTEGER")
	return nil
}

func execute(db *sql.DB, req request) (interface{}, error) {
	switch req.Op {
	case "run.create":
		run := obj(req.Args, "run")
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
			str(run, "createdAt"), str(run, "updatedAt"), strDefault(run, "lastActiveAt", str(run, "createdAt")))
	case "run.update":
		return updateRun(db, str(req.Args, "id"), obj(req.Args, "updates"))
	case "message.create":
		msg := obj(req.Args, "message")
		tx, err := db.Begin()
		if err != nil {
			return nil, err
		}
		if _, err := tx.Exec(`INSERT INTO messages (
			id, run_id, role, agent_role, agent_name, provider_id, provider_display_name, model,
			content, reasoning_content, raw_response, created_at
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			str(msg, "id"), str(msg, "runId"), str(msg, "role"), nullable(msg, "agentRole"),
			nullable(msg, "agentName"), nullable(msg, "providerId"), nullable(msg, "providerDisplayName"),
			nullable(msg, "model"), str(msg, "content"), nullable(msg, "reasoningContent"),
			nullable(msg, "rawResponse"), str(msg, "createdAt")); err != nil {
			_ = tx.Rollback()
			return nil, err
		}
		if _, err := tx.Exec("UPDATE runs SET updated_at = ?, last_active_at = ? WHERE id = ?", str(msg, "createdAt"), str(msg, "createdAt"), str(msg, "runId")); err != nil {
			_ = tx.Rollback()
			return nil, err
		}
		return nil, tx.Commit()
	case "message.update":
		return updateMessage(db, str(req.Args, "id"), obj(req.Args, "updates"))
	case "plan.updateActive":
		return exec(db, "UPDATE plans SET title = ?, body = ?, tasks = ?, status = 'active', updated_at = ? WHERE id = ?",
			str(req.Args, "title"), req.Args["body"], str(req.Args, "tasksJson"), str(req.Args, "now"), str(req.Args, "existingId"))
	case "plan.complete":
		return exec(db, "UPDATE plans SET status = 'completed', updated_at = ? WHERE id = ?", str(req.Args, "updatedAt"), str(req.Args, "id"))
	case "plan.create":
		return exec(db, "INSERT INTO plans (id, run_id, title, body, tasks, status, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?)",
			str(req.Args, "id"), str(req.Args, "runId"), str(req.Args, "title"), req.Args["body"], str(req.Args, "tasksJson"), intValue(req.Args, "version"), str(req.Args, "now"), str(req.Args, "now"))
	case "project.create":
		project := obj(req.Args, "project")
		return exec(db, "INSERT OR REPLACE INTO projects (path, name, created_at) VALUES (?, ?, ?)", str(project, "path"), str(project, "name"), str(project, "createdAt"))
	case "project.delete":
		return exec(db, "DELETE FROM projects WHERE path = ?", str(req.Args, "path"))
	case "permission.allowProject":
		return exec(db, "INSERT OR REPLACE INTO permissions (scope, project_path, tool, command, network_domains, status) VALUES ('project', ?, ?, ?, ?, 'allowed')", str(req.Args, "projectPath"), str(req.Args, "tool"), str(req.Args, "command"), jsonValue(req.Args, "networkDomains", "[]"))
	case "permission.allowGlobal":
		return exec(db, "INSERT OR REPLACE INTO permissions (scope, project_path, tool, command, network_domains, status) VALUES ('global', '', ?, ?, ?, 'allowed')", str(req.Args, "tool"), str(req.Args, "command"), jsonValue(req.Args, "networkDomains", "[]"))
	case "permission.deleteById":
		return exec(db, "DELETE FROM permissions WHERE id = ?", intValue(req.Args, "id"))
	case "permission.clear":
		return exec(db, "DELETE FROM permissions")
	case "memory.create":
		res, err := db.Exec("INSERT INTO memory (scope, project_path, category, content, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
			str(req.Args, "scope"), str(req.Args, "projectPath"), str(req.Args, "category"), str(req.Args, "content"), str(req.Args, "now"), str(req.Args, "now"))
		if err != nil {
			return nil, err
		}
		id, _ := res.LastInsertId()
		return map[string]interface{}{"lastInsertRowid": id}, nil
	case "memory.update":
		return exec(db, "UPDATE memory SET content = ?, updated_at = ? WHERE id = ?", str(req.Args, "content"), str(req.Args, "now"), intValue(req.Args, "id"))
	case "memory.deleteById":
		return exec(db, "DELETE FROM memory WHERE id = ?", intValue(req.Args, "id"))
	case "memory.clear":
		return exec(db, "DELETE FROM memory")
	case "usage_logs.create":
		logObj := obj(req.Args, "log")
		return exec(db, `INSERT INTO usage_logs (
			run_id, agent_role, provider_id, model,
			input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
			cache_hit_rate, cost, created_at, duration_ms
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			str(logObj, "runId"), nullable(logObj, "agentRole"), str(logObj, "providerId"), str(logObj, "model"),
			intValue(logObj, "inputTokens"), intValue(logObj, "outputTokens"), intValue(logObj, "cacheReadTokens"), intValue(logObj, "cacheWriteTokens"),
			floatValue(logObj, "cacheHitRate"), floatValue(logObj, "cost"), str(logObj, "createdAt"), nullable(logObj, "durationMs"))
	default:
		return nil, fmt.Errorf("unknown op %q", req.Op)
	}
}

func exec(db *sql.DB, sqlText string, args ...interface{}) (interface{}, error) {
	res, err := db.Exec(sqlText, args...)
	if err != nil {
		return nil, err
	}
	changes, _ := res.RowsAffected()
	lastID, _ := res.LastInsertId()
	return map[string]interface{}{"changes": changes, "lastInsertRowid": lastID}, nil
}

func updateRun(db *sql.DB, id string, updates map[string]interface{}) (interface{}, error) {
	mapping := map[string]string{
		"title": "title", "task": "task", "projectPath": "project_path", "projectName": "project_name",
		"status": "status", "providerId": "provider_id", "providerDisplayName": "provider_display_name",
		"model": "model", "reasoningEffort": "reasoning_effort", "mode": "mode",
		"coderProviderId": "coder_provider_id", "coderModel": "coder_model", "coderReasoningEffort": "coder_reasoning_effort",
		"utilityProviderId": "utility_provider_id", "utilityModel": "utility_model", "utilityReasoningEffort": "utility_reasoning_effort",
		"agentPreset": "agent_preset", "errorMessage": "error_message", "createdAt": "created_at", "lastActiveAt": "last_active_at",
	}
	fields := []string{}
	args := []interface{}{}
	for key, col := range mapping {
		if val, ok := updates[key]; ok {
			fields = append(fields, col+" = ?")
			args = append(args, nullUndefined(val))
		}
	}
	if len(fields) == 0 {
		return map[string]interface{}{"changes": 0}, nil
	}
	fields = append(fields, "updated_at = ?")
	args = append(args, nullUndefined(updates["updatedAt"]))
	args = append(args, id)
	return exec(db, "UPDATE runs SET "+strings.Join(fields, ", ")+" WHERE id = ?", args...)
}

func updateMessage(db *sql.DB, id string, updates map[string]interface{}) (interface{}, error) {
	mapping := map[string]string{"content": "content", "reasoningContent": "reasoning_content", "rawResponse": "raw_response"}
	fields := []string{}
	args := []interface{}{}
	for key, col := range mapping {
		if val, ok := updates[key]; ok {
			fields = append(fields, col+" = ?")
			args = append(args, nullUndefined(val))
		}
	}
	if len(fields) == 0 {
		return map[string]interface{}{"changes": 0}, nil
	}
	args = append(args, id)
	return exec(db, "UPDATE messages SET "+strings.Join(fields, ", ")+" WHERE id = ?", args...)
}

func obj(args map[string]interface{}, key string) map[string]interface{} {
	v, ok := args[key].(map[string]interface{})
	if !ok {
		return map[string]interface{}{}
	}
	return v
}

func str(m map[string]interface{}, key string) string {
	if v, ok := m[key].(string); ok {
		return v
	}
	return ""
}

func strDefault(m map[string]interface{}, key, fallback string) string {
	if v := str(m, key); v != "" {
		return v
	}
	return fallback
}

func jsonValue(m map[string]interface{}, key, fallback string) string {
	v, ok := m[key]
	if !ok {
		return fallback
	}
	encoded, err := json.Marshal(v)
	if err != nil {
		return fallback
	}
	return string(encoded)
}

func nullable(m map[string]interface{}, key string) interface{} {
	if v, ok := m[key]; ok {
		return nullUndefined(v)
	}
	return nil
}

func nullUndefined(v interface{}) interface{} {
	if v == nil {
		return nil
	}
	return v
}

func intValue(m map[string]interface{}, key string) int64 {
	switch v := m[key].(type) {
	case float64:
		return int64(v)
	case int64:
		return v
	case int:
		return int64(v)
	default:
		return 0
	}
}

func floatValue(m map[string]interface{}, key string) float64 {
	switch v := m[key].(type) {
	case float64:
		return v
	case float32:
		return float64(v)
	case int64:
		return float64(v)
	case int:
		return float64(v)
	default:
		return 0.0
	}
}

func mustCwd() string {
	cwd, err := os.Getwd()
	if err != nil {
		return ""
	}
	return cwd
}
