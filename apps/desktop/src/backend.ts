import { utilityProcess, type UtilityProcess } from "electron";
import {
  backendScript,
  bundledProviderConfig,
  dbPath,
  dbWriterBinary,
  settingsPath,
  sandboxRuntimeDir,
  userProviderConfig,
} from "./paths";

let child: UtilityProcess | null = null;

/**
 * Env passed to the forked backend. The settings file is the source of truth
 * for the port; PORT is passed too as a belt-and-suspenders default. DB lives
 * next to settings in the env-specific data dir. Provider config is two-layer:
 * the predefined catalog is read straight from the read-only bundle (refreshed
 * on every app update), while the user's own providers/edits go to a writable
 * overlay in the data dir that updates never touch.
 */
function backendEnv(port: number, apiToken: string): Record<string, string> {
  return {
    ...(process.env as Record<string, string>),
    PORT: String(port),
    LOCAGENS_API_TOKEN: apiToken,
    LOCAGENS_SETTINGS_PATH: settingsPath(),
    LOCAGENS_DB_PATH: dbPath(),
    LOCAGENS_DB_WRITER_PATH: dbWriterBinary(),
    LOCAGENS_SANDBOX_RUNTIME_DIR: sandboxRuntimeDir(),
    LOCAGENS_PROVIDER_CONFIG_PATH: bundledProviderConfig(),
    LOCAGENS_PROVIDER_USER_CONFIG_PATH: userProviderConfig(),
  };
}

/** Forks the bundled backend as a child process (prod). */
export function startBackend(port: number, apiToken: string): void {
  if (child) return;
  child = utilityProcess.fork(backendScript(), [], {
    env: backendEnv(port, apiToken),
    stdio: "pipe",
  });
  child.stdout?.on("data", (d) => process.stdout.write(`[backend] ${d}`));
  child.stderr?.on("data", (d) => process.stderr.write(`[backend] ${d}`));
  child.once("exit", () => {
    child = null;
  });
}

/** Stops the running backend child, resolving once it has exited. */
export function stopBackend(): Promise<void> {
  return new Promise((resolve) => {
    if (!child) return resolve();
    const current = child;
    child = null;
    current.once("exit", () => resolve());
    current.kill();
  });
}

/** Polls /ping until the backend answers or the timeout elapses. */
export async function waitForBackend(port: number, timeoutMs = 15000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/ping`);
      if (res.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Backend did not become ready on port ${port}`);
}
