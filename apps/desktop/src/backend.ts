import { spawn, type ChildProcess } from "node:child_process";
import readline from "node:readline";
import {
  backendBinary,
  bundledProviderConfig,
  dataDirPath,
  providerConfig,
  settingsPath,
} from "./paths";

let child: ChildProcess | null = null;
let readyPromise: Promise<void> | null = null;

/**
 * Env passed to the spawned backend. The settings file is the source of truth
 * for the port; PORT is passed too as a belt-and-suspenders default. Provider
 * config is a single writable file in OS userData; the bundled catalog is only
 * a first-run seed.
 */
function backendEnv(port: number, apiToken: string): Record<string, string> {
  return {
    ...(process.env as Record<string, string>),
    PORT: String(port),
    LOCAGENS_API_TOKEN: apiToken,
    LOCAGENS_SETTINGS_PATH: settingsPath(),
    LOCAGENS_DATA_DIR: dataDirPath(),
    LOCAGENS_PROVIDER_CONFIG_PATH: providerConfig(),
    LOCAGENS_PROVIDER_SEED_PATH: bundledProviderConfig(),
  };
}

/** Spawns the bundled Rust backend binary as a child process. */
export function startBackend(port: number, apiToken: string): Promise<void> {
  if (child) return readyPromise ?? Promise.resolve();
  child = spawn(backendBinary(), [], {
    env: backendEnv(port, apiToken),
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  const current = child;
  current.stdout?.on("data", (d) => process.stdout.write(`[backend] ${d}`));
  current.stderr?.on("data", (d) => process.stderr.write(`[backend] ${d}`));
  readyPromise = new Promise<void>((resolve, reject) => {
    let settled = false;
    const rl = readline.createInterface({ input: current.stdout! });
    const cleanup = () => {
      clearTimeout(timeout);
      rl.close();
      rl.removeListener("line", onLine);
      current.removeListener("exit", onEarlyExit);
      current.removeListener("error", onError);
    };
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (error) reject(error);
      else resolve();
    };
    const onLine = (line: string) => {
      if (line.includes("[locagens:backend-ready]") && line.includes(`port=${port}`)) {
        finish();
      }
    };
    const onEarlyExit = (code: number | null) => {
      finish(new Error(`Backend exited before readiness (code ${code}).`));
    };
    const onError = (error: Error) => {
      finish(error);
    };
    const timeout = setTimeout(() => {
      current.kill();
      finish(new Error("Backend readiness timed out."));
    }, 15_000);
    rl.on("line", onLine);
    current.once("exit", onEarlyExit);
    current.once("error", onError);
  });
  current.once("exit", () => {
    child = null;
    readyPromise = null;
  });
  return readyPromise;
}

/** Stops the running backend child, resolving once it has exited. */
export function stopBackend(): Promise<void> {
  return new Promise((resolve) => {
    if (!child) return resolve();
    const current = child;
    child = null;
    readyPromise = null;
    current.once("exit", () => resolve());
    current.kill();
  });
}
