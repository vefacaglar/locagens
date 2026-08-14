import { app } from "electron";
import fs from "node:fs";
import path from "node:path";

/** Dev = running unpackaged (electron .); prod = packaged .app. */
export const isDev = !app.isPackaged;

export const DEFAULT_PORT = 4321;

/**
 * Root directory holding settings.json + locagens.db for the current
 * environment. Dev and prod are deliberately separate so the installed app and
 * a local dev build never share data (and can run at the same time):
 *   - dev:  <repo>/.locagens-dev   (git-ignored)
 *   - prod: app userData           (~/Library/Application Support/Locagens)
 */
export function dataDir(): string {
  if (isDev) {
    // dist/main.js -> ../ = dist, ../../ = apps/desktop, ../../../ = repo root
    return path.join(path.resolve(__dirname, "..", "..", ".."), ".locagens-dev");
  }
  return app.getPath("userData");
}

export function settingsPath(): string {
  return path.join(dataDir(), "settings.json");
}

export function dbPath(): string {
  return path.join(dataDir(), "locagens.db");
}

/**
 * Writable user overlay: the user's custom providers, their edits to predefined
 * ones, and removal tombstones. Lives in the data dir so it survives app updates
 * (the predefined base ships read-only in the bundle and is refreshed on update).
 */
export function userProviderConfig(): string {
  return path.join(dataDir(), "providers.user.json");
}

/** The committed predefined catalog bundled into the app (prod, read-only). */
export function bundledProviderConfig(): string {
  return path.join(process.resourcesPath, "config", "providers.json");
}

/** Bundled backend entry (prod only — forked as a child process). */
export function backendScript(): string {
  return path.join(process.resourcesPath, "api", "server.bundle.cjs");
}

/** Bundled Go SQLite writer sidecar (prod only). */
export function dbWriterBinary(): string {
  return path.join(process.resourcesPath, "db-writer", "db-writer");
}

export function sandboxRuntimeDir(): string {
  return path.join(process.resourcesPath, "sandbox-runtime", "vendor");
}

/** Built web app entry (prod only). */
export function webIndex(): string {
  return path.join(process.resourcesPath, "web", "index.html");
}

/** Reads the configured port from settings.json, falling back to the default. */
export function resolvePort(): number {
  try {
    const parsed = JSON.parse(fs.readFileSync(settingsPath(), "utf-8"));
    const port = Number(parsed?.port);
    if (Number.isInteger(port) && port >= 1 && port <= 65535) return port;
  } catch {
    // missing/invalid file -> default
  }
  return DEFAULT_PORT;
}
