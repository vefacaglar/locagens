import fs from "node:fs";
import path from "node:path";
import { defaultSettingsPath, type AppSettings } from "@locagens/shared";

export const DEFAULT_PORT = 4321;
const MIN_PORT = 1;
const MAX_PORT = 65535;

/**
 * Reads and writes local application settings (currently just the backend
 * port) from a small JSON file living next to the provider config in the
 * platform's app-support directory. This file is the single source of truth
 * for the port: the backend reads it on startup, and the eventual Electron
 * main process reads it to decide where to launch / connect the server.
 *
 * Provider secrets never touch this file — it only holds non-sensitive app
 * settings, so it can be read freely by the UI.
 */
export class AppSettingsStore {
  private readonly filePath: string;

  constructor(filePathOverride?: string) {
    this.filePath = filePathOverride || AppSettingsStore.defaultPath();
  }

  static defaultPath(): string {
    return defaultSettingsPath();
  }

  /** Validates a candidate port, returning a clamped integer or undefined. */
  static normalizePort(value: unknown): number | undefined {
    const port = Number(value);
    if (!Number.isInteger(port) || port < MIN_PORT || port > MAX_PORT) return undefined;
    return port;
  }

  /**
   * Resolves the effective port. The config file wins (it is what the UI
   * edits); `PORT` env is only a fallback for unconfigured installs, then the
   * built-in default.
   */
  resolvePort(): number {
    return this.readPortFromFile() ?? AppSettingsStore.normalizePort(process.env.PORT) ?? DEFAULT_PORT;
  }

  read(): AppSettings {
    return { port: this.readPortFromFile() ?? DEFAULT_PORT };
  }

  /** The port stored in the config file, or undefined when absent/invalid. */
  private readPortFromFile(): number | undefined {
    try {
      const parsed = JSON.parse(fs.readFileSync(this.filePath, "utf-8")) as Partial<AppSettings>;
      return AppSettingsStore.normalizePort(parsed?.port);
    } catch {
      return undefined;
    }
  }

  /** Persists settings, returning the stored (normalized) values. */
  save(settings: AppSettings): AppSettings {
    const port = AppSettingsStore.normalizePort(settings.port);
    if (!port) throw new Error(`Invalid port: ${settings.port}. Use an integer between ${MIN_PORT} and ${MAX_PORT}.`);
    const next: AppSettings = { port };
    fs.mkdirSync(path.dirname(this.filePath), { recursive: true });
    fs.writeFileSync(this.filePath, JSON.stringify(next, null, 2), "utf-8");
    return next;
  }
}
