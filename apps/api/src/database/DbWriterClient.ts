import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";

interface PendingRequest {
  resolve: (value: any) => void;
  reject: (err: Error) => void;
  timer: NodeJS.Timeout;
  request: DbWriteRequest;
}

export interface DbWriteRequest {
  op: string;
  args?: Record<string, unknown>;
}

function findWorkspaceRoot(): string {
  let currentDir = process.cwd();
  const rootDir = path.parse(currentDir).root;
  while (currentDir !== rootDir) {
    if (fs.existsSync(path.join(currentDir, "pnpm-workspace.yaml"))) {
      return currentDir;
    }
    currentDir = path.dirname(currentDir);
  }
  return process.cwd();
}

function bundledWriterPath(): string | null {
  const explicit = process.env.LOCAGENS_DB_WRITER_PATH;
  return explicit && fs.existsSync(explicit) ? explicit : null;
}

/** Resolves the tsx CLI shipped with the db-writer package (dev only). */
function devTsxCli(wsRoot: string): string {
  const requireFromWriter = createRequire(path.join(wsRoot, "apps/db-writer/package.json"));
  return requireFromWriter.resolve("tsx/cli");
}

export class DbWriterClient {
  private child: ChildProcessWithoutNullStreams | null = null;
  private pending = new Map<string, PendingRequest>();
  private nextId = 1;
  private stderrTail = "";
  private closing = false;

  constructor(private dbPath: string, private timeoutMs = 15000) {
    // Run recovery in the background on startup
    this.recoverPendingWrites().catch(err => {
      console.error("[DbWriterClient] Background recovery failed:", err);
    });

    const cleanUp = () => {
      this.close();
    };

    process.on("exit", cleanUp);
    process.on("SIGINT", () => {
      this.close();
      process.exit(0);
    });
    process.on("SIGTERM", () => {
      this.close();
      process.exit(0);
    });
    process.on("SIGHUP", () => {
      this.close();
      process.exit(0);
    });
  }

  async write<T = unknown>(request: DbWriteRequest): Promise<T> {
    if (this.closing) {
      throw new Error("DB writer is closing, cannot accept new writes.");
    }
    const child = this.ensureStarted();
    const id = String(this.nextId++);
    const payload = JSON.stringify({ id, ...request }) + "\n";

    return await new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`DB writer timed out for ${request.op}.`));
      }, this.timeoutMs);

      this.pending.set(id, {
        resolve,
        reject,
        timer,
        request
      });

      child.stdin.write(payload, "utf8", (err) => {
        if (!err) return;
        const pending = this.pending.get(id);
        if (!pending) return;
        clearTimeout(pending.timer);
        this.pending.delete(id);
        pending.reject(err);
      });
    });
  }

  close(): void {
    if (this.closing) return;
    this.closing = true;
    this.savePendingWrites();
    if (!this.child) return;
    this.child.stdin.end();
    this.child.kill();
    this.child = null;
  }

  private savePendingWrites(): void {
    if (this.pending.size === 0) return;
    const writes = Array.from(this.pending.values()).map(p => p.request);
    const backupPath = this.dbPath + ".pending-writes.json";
    try {
      fs.writeFileSync(backupPath, JSON.stringify(writes, null, 2), "utf8");
      console.log(`[DbWriterClient] Saved ${writes.length} pending writes to ${backupPath}`);
    } catch (err) {
      console.error(`[DbWriterClient] Failed to save pending writes:`, err);
    }
  }

  private async recoverPendingWrites(): Promise<void> {
    const backupPath = this.dbPath + ".pending-writes.json";
    if (!fs.existsSync(backupPath)) return;

    try {
      const data = fs.readFileSync(backupPath, "utf8");
      const writes: DbWriteRequest[] = JSON.parse(data);
      console.log(`[DbWriterClient] Recovering ${writes.length} pending database writes...`);

      for (const request of writes) {
        try {
          await this.write(request);
        } catch (err: any) {
          if (err.message && err.message.includes("UNIQUE constraint failed")) {
            console.log(`[DbWriterClient] Recovered write for ${request.op} already exists, skipping.`);
          } else {
            console.error(`[DbWriterClient] Failed to execute recovered write for ${request.op}:`, err);
          }
        }
      }

      fs.unlinkSync(backupPath);
      console.log("[DbWriterClient] Pending database writes recovery completed.");
    } catch (err) {
      console.error("[DbWriterClient] Error during pending database writes recovery:", err);
    }
  }

  private ensureStarted(): ChildProcessWithoutNullStreams {
    if (this.child && !this.child.killed) return this.child;

    // The writer is a Node.js sidecar. Prod passes the bundled script via
    // LOCAGENS_DB_WRITER_PATH; dev runs the TypeScript source through tsx.
    // process.execPath is the Electron binary in packaged builds, so
    // ELECTRON_RUN_AS_NODE makes it behave as plain Node (harmless on real Node).
    const explicit = bundledWriterPath();
    const args = explicit
      ? [explicit]
      : [devTsxCli(findWorkspaceRoot()), path.join(findWorkspaceRoot(), "apps/db-writer/src/worker.ts")];

    const child = spawn(process.execPath, args, {
      env: {
        ...process.env,
        ELECTRON_RUN_AS_NODE: "1",
        LOCAGENS_DB_PATH: this.dbPath
      },
      stdio: ["pipe", "pipe", "pipe"]
    });
    this.child = child;

    const lines = readline.createInterface({ input: child.stdout });
    lines.on("line", (line) => this.handleLine(line));
    child.stderr.on("data", (chunk) => {
      this.stderrTail = (this.stderrTail + chunk.toString()).slice(-4000);
    });
    child.on("exit", (code, signal) => {
      const err = new Error(`DB writer exited (${signal ?? code ?? "unknown"}). ${this.stderrTail}`.trim());
      for (const [id, pending] of this.pending) {
        clearTimeout(pending.timer);
        pending.reject(err);
        this.pending.delete(id);
      }
      if (this.child === child) this.child = null;
    });

    return child;
  }

  private handleLine(line: string): void {
    let response: any;
    try {
      response = JSON.parse(line);
    } catch {
      return;
    }

    const id = String(response.id ?? "");
    const pending = this.pending.get(id);
    if (!pending) return;

    clearTimeout(pending.timer);
    this.pending.delete(id);

    if (response.ok) {
      pending.resolve(response.result);
    } else {
      pending.reject(new Error(response.error || "DB writer request failed."));
    }
  }
}
