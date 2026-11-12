import { app } from "electron";
import fs from "node:fs";
import path from "node:path";

/** Dev = running unpackaged (electron .); prod = packaged .app. */
export const isDev = !app.isPackaged;

export const DEFAULT_PORT = 4321;

/** Repo root (apps/desktop/dist -> ../../.. ). */
export function repoRoot(): string {
  return path.resolve(__dirname, "..", "..", "..");
}

/**
 * Root directory holding app data for the current environment. Dev and prod
 * are deliberately separate so the installed app and a local dev build never
 * share data (and can run at the same time):
 *   - dev:  <repo>/.locagens-dev   (git-ignored)
 *   - prod: app userData           (~/Library/Application Support/Locagens)
 * settings.json and providers.json always live in OS userData.
 */
export function dataDir(): string {
  if (isDev) {
    return path.join(repoRoot(), ".locagens-dev");
  }
  return app.getPath("userData");
}

/** Data directory passed to the backend via LOCAGENS_DATA_DIR. */
export function dataDirPath(): string {
  return path.join(dataDir(), "data");
}

export function settingsPath(): string {
  return path.join(app.getPath("userData"), "settings.json");
}

export function dbPath(): string {
  return path.join(dataDir(), "locagens.db");
}

/** Live provider config — always OS userData, never the project tree. */
export function providerConfig(): string {
  return path.join(app.getPath("userData"), "providers.json");
}

/** Bundled catalog used only to seed userData on first run. */
export function bundledProviderConfig(): string {
  return path.join(process.resourcesPath, "config", "providers.json");
}

/**
 * Bundled Rust backend binary. Prod lives in resources/bin; dev prefers the
 * release build, falling back to debug.
 */
export function backendBinary(): string {
  const exe = process.platform === "win32" ? "locagens-backend.exe" : "locagens-backend";
  if (isDev) {
    const release = path.join(repoRoot(), "target", "release", exe);
    if (fs.existsSync(release)) return release;
    return path.join(repoRoot(), "target", "debug", exe);
  }
  return path.join(process.resourcesPath, "bin", exe);
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
