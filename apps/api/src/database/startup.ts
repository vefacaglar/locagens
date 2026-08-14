import type { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import path from "path";

/**
 * Older Locagens versions stored project paths exactly as selected, including
 * trailing slashes and symlink spellings. Runs now use canonical real paths,
 * so normalize the project registry and every project-scoped reference to the
 * same spelling before repositories start serving data.
 */
export function normalizeStoredProjectPaths(db: DatabaseSync) {
  const projects = db.prepare("SELECT path FROM projects").all() as { path: string }[];

  for (const project of projects) {
    let canonicalPath: string;
    try {
      canonicalPath = fs.realpathSync.native(project.path);
      if (!fs.statSync(canonicalPath).isDirectory()) continue;
    } catch {
      // Keep inaccessible legacy projects visible so the user can remove them.
      continue;
    }

    if (canonicalPath === project.path) continue;

    db.exec("BEGIN IMMEDIATE");
    try {
      const canonicalProject = db.prepare("SELECT path FROM projects WHERE path = ?").get(canonicalPath);
      if (canonicalProject) {
        db.prepare("DELETE FROM projects WHERE path = ?").run(project.path);
      } else {
        db.prepare("UPDATE projects SET path = ? WHERE path = ?").run(canonicalPath, project.path);
      }

      db.prepare("UPDATE runs SET project_path = ? WHERE project_path = ?").run(canonicalPath, project.path);
      db.prepare("UPDATE memory SET project_path = ? WHERE project_path = ?").run(canonicalPath, project.path);

      // Two formerly different path spellings may describe the same exact
      // permission. Keep the canonical row and discard the duplicate legacy row.
      db.prepare("UPDATE OR IGNORE permissions SET project_path = ? WHERE project_path = ?").run(canonicalPath, project.path);
      db.prepare("DELETE FROM permissions WHERE project_path = ?").run(project.path);
      db.exec("COMMIT");
    } catch (error) {
      db.exec("ROLLBACK");
      throw error;
    }
  }
}

export function runStartupTasks(db: DatabaseSync, wsRoot: string, defaultProjectName: string) {
  // Seed default project if empty
  const projectCount = db.prepare("SELECT count(*) as count FROM projects").get() as { count: number };
  if (projectCount.count === 0) {
    db.prepare(`
      INSERT INTO projects (path, name, created_at)
      VALUES (?, ?, ?)
    `).run(wsRoot, defaultProjectName, new Date().toISOString());
  }

  normalizeStoredProjectPaths(db);

  db.prepare(`
    UPDATE projects
    SET name = ?
    WHERE path IN (?, ?) AND name = ?
  `).run(defaultProjectName, wsRoot, `${wsRoot}/`, path.basename(wsRoot));

  // Reset any runs left in active states on startup
  try {
    const stuckRuns = db.prepare(`
      SELECT id FROM runs 
      WHERE status IN ('created', 'generating', 'awaiting_permission')
    `).all() as { id: string }[];

    if (stuckRuns.length > 0) {
      const updateRun = db.prepare(`
        UPDATE runs 
        SET status = 'failed', error_message = 'Session interrupted due to server restart.' 
        WHERE id = ?
      `);
      const insertMessage = db.prepare(`
        INSERT INTO messages (id, run_id, role, content, created_at)
        VALUES (?, ?, 'system', 'Session interrupted due to server restart.', ?)
      `);

      for (const r of stuckRuns) {
        updateRun.run(r.id);
        insertMessage.run(`msg-err-restart-${r.id}-${Date.now()}`, r.id, new Date().toISOString());
      }
    }
  } catch (err: any) {
    console.error("[Database] Failed to clean up stuck runs:", err.message);
  }
}
