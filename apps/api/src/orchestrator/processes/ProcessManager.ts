import { spawn, type ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import type { ProcessInfo, ProcessLogEntry, ProcessStatus, SpawnOptions } from "./types.js";

const MAX_LOGS_PER_PROCESS = 2000;

interface ManagedProcess {
  info: ProcessInfo;
  child: ChildProcess | null;
  logs: ProcessLogEntry[];
  spawnOpts: SpawnOptions;
}

export class ProcessManager extends EventEmitter {
  private processes = new Map<string, ManagedProcess>();
  private idCounter = 1;

  constructor() {
    super();
    // Clean up all child processes on parent exit
    process.on("exit", () => {
      this.killAllSync();
    });
  }

  /**
   * Spawns a background process and immediately returns its tracking info.
   */
  public spawnProcess(options: SpawnOptions): ProcessInfo {
    const id = `proc-${Date.now()}-${this.idCounter++}`;
    const command = options.command.trim();

    const info: ProcessInfo = {
      id,
      command,
      projectPath: options.projectPath,
      status: "running",
      startedAt: new Date().toISOString(),
      stoppedAt: null,
      exitCode: null
    };

    const managed: ManagedProcess = {
      info,
      child: null,
      logs: [],
      spawnOpts: options
    };

    this.processes.set(id, managed);
    this.startManagedChild(managed);

    return { ...managed.info };
  }

  private startManagedChild(managed: ManagedProcess): void {
    const { command, projectPath, env } = managed.spawnOpts;

    try {
      const child = spawn(command, {
        cwd: projectPath,
        shell: true,
        env: {
          ...process.env,
          ...env,
          FORCE_COLOR: "1"
        }
      });

      managed.child = child;
      managed.info.pid = child.pid;
      managed.info.status = "running";

      this.appendLog(managed, {
        timestamp: new Date().toISOString(),
        stream: "system",
        text: `[Process started with PID ${child.pid}]`
      });

      child.stdout?.on("data", (chunk: Buffer) => {
        const text = chunk.toString("utf-8");
        this.appendLog(managed, {
          timestamp: new Date().toISOString(),
          stream: "stdout",
          text
        });
      });

      child.stderr?.on("data", (chunk: Buffer) => {
        const text = chunk.toString("utf-8");
        this.appendLog(managed, {
          timestamp: new Date().toISOString(),
          stream: "stderr",
          text
        });
      });

      child.on("error", (err: Error) => {
        managed.info.status = "error";
        managed.info.stoppedAt = new Date().toISOString();
        this.appendLog(managed, {
          timestamp: new Date().toISOString(),
          stream: "system",
          text: `[Process error: ${err.message}]`
        });
        this.emit("change", managed.info);
      });

      child.on("close", (code: number | null) => {
        managed.child = null;
        managed.info.status = "stopped";
        managed.info.exitCode = code;
        managed.info.stoppedAt = new Date().toISOString();
        this.appendLog(managed, {
          timestamp: new Date().toISOString(),
          stream: "system",
          text: `[Process exited with code ${code ?? "null"}]`
        });
        this.emit("change", managed.info);
      });

      this.emit("change", managed.info);
    } catch (err: any) {
      managed.info.status = "error";
      managed.info.stoppedAt = new Date().toISOString();
      this.appendLog(managed, {
        timestamp: new Date().toISOString(),
        stream: "system",
        text: `[Failed to spawn process: ${err?.message || err}]`
      });
      this.emit("change", managed.info);
    }
  }

  private appendLog(managed: ManagedProcess, entry: ProcessLogEntry): void {
    managed.logs.push(entry);
    if (managed.logs.length > MAX_LOGS_PER_PROCESS) {
      managed.logs.shift();
    }
    this.emit("log", { processId: managed.info.id, entry });
  }

  public list(projectPath?: string): ProcessInfo[] {
    const list: ProcessInfo[] = [];
    for (const managed of this.processes.values()) {
      if (!projectPath || managed.info.projectPath === projectPath) {
        list.push({ ...managed.info });
      }
    }
    return list.sort((a, b) => (b.startedAt > a.startedAt ? 1 : -1));
  }

  public get(id: string): ProcessInfo | null {
    const managed = this.processes.get(id);
    return managed ? { ...managed.info } : null;
  }

  public getLogs(id: string): ProcessLogEntry[] {
    const managed = this.processes.get(id);
    return managed ? [...managed.logs] : [];
  }

  public async kill(id: string, signal: NodeJS.Signals = "SIGTERM"): Promise<boolean> {
    const managed = this.processes.get(id);
    if (!managed || !managed.child) {
      if (managed) {
        managed.info.status = "stopped";
      }
      return false;
    }

    return new Promise<boolean>((resolve) => {
      try {
        managed.child?.kill(signal);
        setTimeout(() => {
          if (managed.child) {
            try {
              managed.child.kill("SIGKILL");
            } catch {
              /* ignore */
            }
          }
          managed.info.status = "stopped";
          resolve(true);
        }, 1500);
      } catch {
        resolve(false);
      }
    });
  }

  public async restart(id: string): Promise<ProcessInfo | null> {
    const managed = this.processes.get(id);
    if (!managed) return null;

    if (managed.child && managed.info.status === "running") {
      await this.kill(id);
    }

    managed.info.status = "running";
    managed.info.startedAt = new Date().toISOString();
    managed.info.stoppedAt = null;
    managed.info.exitCode = null;

    this.startManagedChild(managed);
    return { ...managed.info };
  }

  public killAllSync(): void {
    for (const managed of this.processes.values()) {
      if (managed.child) {
        try {
          managed.child.kill("SIGKILL");
        } catch {
          /* ignore */
        }
      }
    }
  }
}
