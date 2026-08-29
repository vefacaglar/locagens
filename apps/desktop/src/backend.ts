import { utilityProcess, type UtilityProcess } from "electron";
import {
  backendScript,
  bundledProviderConfig,
  dbPath,
  dbWriterBinary,
  providerConfig,
  settingsPath,
  sandboxRuntimeDir,
} from "./paths";

let child: UtilityProcess | null = null;
let readyPromise: Promise<void> | null = null;

/**
 * Env passed to the forked backend. The settings file is the source of truth
 * for the port; PORT is passed too as a belt-and-suspenders default. DB lives
 * next to settings in the env-specific data dir. Provider config is a single
 * writable file in OS userData; the bundled catalog is only a first-run seed.
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
    LOCAGENS_PROVIDER_CONFIG_PATH: providerConfig(),
    LOCAGENS_PROVIDER_SEED_PATH: bundledProviderConfig(),
  };
}

/** Forks the bundled backend as a child process (prod). */
export function startBackend(port: number, apiToken: string): Promise<void> {
  if (child) return readyPromise ?? Promise.resolve();
  child = utilityProcess.fork(backendScript(), [], {
    env: backendEnv(port, apiToken),
    stdio: "pipe",
  });
  const current = child;
  current.stdout?.on("data", (d) => process.stdout.write(`[backend] ${d}`));
  current.stderr?.on("data", (d) => process.stderr.write(`[backend] ${d}`));
  readyPromise = new Promise<void>((resolve, reject) => {
    let settled = false;
    const cleanup = () => {
      clearTimeout(timeout);
      current.removeListener("message", onMessage);
      current.removeListener("exit", onEarlyExit);
    };
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (error) reject(error);
      else resolve();
    };
    const onMessage = (message: unknown) => {
      if ((message as any)?.type !== "locagens:backend-ready" || (message as any)?.port !== port) return;
      finish();
    };
    const onEarlyExit = (code: number) => {
      finish(new Error(`Backend exited before readiness (code ${code}).`));
    };
    const timeout = setTimeout(() => {
      current.kill();
      finish(new Error("Backend readiness timed out."));
    }, 15_000);
    current.on("message", onMessage);
    current.once("exit", onEarlyExit);
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
