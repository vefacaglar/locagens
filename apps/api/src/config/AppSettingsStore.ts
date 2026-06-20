import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { AppSettings, SearchSettings } from "@locagens/shared";
import { normalizeSearchSettings, SEARCH_ENGINE_OPTIONS, type PersistedSearchSettings, type ResolvedSearchSettings } from "../search/SearchConfig.js";
import { MacOSKeychainProviderSecretStore, type ProviderSecretStore } from "../providers/ProviderSecretStore.js";

export const DEFAULT_PORT = 4321;
export const SEARCH_PRESERVE_API_KEY_VALUE = "__LOCAGENS_PRESERVE_API_KEY__";
const MIN_PORT = 1;
const MAX_PORT = 65535;

function createSearchSecretStore(): ProviderSecretStore {
  if (process.platform === "darwin") {
    return new MacOSKeychainProviderSecretStore();
  }
  return {
    supportsSecurePersistence: false,
    get: () => "",
    set: () => { throw new Error("Secure search secret storage is only available on macOS Keychain."); }
  };
}

function createFileProvider(filePath: string): SettingsFileProvider {
  return {
    read: () => {
      try {
        return JSON.parse(fs.readFileSync(filePath, "utf-8")) as unknown;
      } catch {
        return undefined;
      }
    },
    write: (data) => {
      fs.mkdirSync(path.dirname(filePath), { recursive: true });
      fs.writeFileSync(filePath, JSON.stringify(data, null, 2), "utf-8");
    }
  };
}

interface SettingsFileProvider {
  read(): unknown;
  write(data: AppSettings): void;
}

/**
 * Reads and writes local application settings (backend port + search config)
 * from a small JSON file living next to the provider config in the platform's
 * app-support directory. Provider/search secrets never touch this file — only
 * non-sensitive settings and keychain references are persisted.
 */
export class AppSettingsStore {
  private readonly fileProvider: SettingsFileProvider;
  private readonly secretStore: ProviderSecretStore;

  constructor(filePathOverride?: string, secretStore?: ProviderSecretStore) {
    const filePath = filePathOverride || AppSettingsStore.defaultPath();
    this.fileProvider = createFileProvider(filePath);
    this.secretStore = secretStore || createSearchSecretStore();
  }

  static defaultPath(): string {
    if (process.env.LOCAGENS_SETTINGS_PATH) {
      return process.env.LOCAGENS_SETTINGS_PATH;
    }

    const appDirName = "Locagens";
    if (process.platform === "darwin") {
      return path.join(os.homedir(), "Library", "Application Support", appDirName, "settings.json");
    }
    if (process.platform === "win32") {
      return path.join(process.env.APPDATA || path.join(os.homedir(), "AppData", "Roaming"), appDirName, "settings.json");
    }
    return path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "locagens", "settings.json");
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
    return this.readPort() ?? AppSettingsStore.normalizePort(process.env.PORT) ?? DEFAULT_PORT;
  }

  read(): AppSettings {
    const file = this.readFile();
    const search = this.readSearchFromFile(file);
    return {
      port: this.readPortFromFile(file) ?? DEFAULT_PORT,
      search: {
        engine: search.engine,
        braveApiKey: "",
        googleApiKey: "",
        googleSearchEngineId: search.googleSearchEngineId,
        hasBraveApiKey: this.hasKey(search.braveApiKeyRef),
        hasGoogleApiKey: this.hasKey(search.googleApiKeyRef)
      }
    };
  }

  readResolvedSearch(): ResolvedSearchSettings {
    const file = this.readFile();
    const search = this.readSearchFromFile(file);
    return {
      engine: search.engine,
      braveApiKeyRef: search.braveApiKeyRef,
      googleApiKeyRef: search.googleApiKeyRef,
      googleSearchEngineId: search.googleSearchEngineId
    };
  }

  /** Persists settings, returning the stored (normalized) values. */
  save(settings: AppSettings): AppSettings {
    const port = AppSettingsStore.normalizePort(settings.port);
    if (!port) {
      throw new Error(`Invalid port: ${settings.port}. Use an integer between ${MIN_PORT} and ${MAX_PORT}.`);
    }

    const file = this.readFile();
    const next: AppSettings = { port };
    const search = this.persistSearchSettings(settings.search, file.search);
    if (search) next.search = search;

    this.fileProvider.write(next);
    return next;
  }

  private readFile(): Partial<AppSettings> {
    const data = this.fileProvider.read();
    if (!data || typeof data !== "object") return {};
    return data as Partial<AppSettings>;
  }

  private readPort(): number | undefined {
    return this.readPortFromFile(this.readFile());
  }

  private readPortFromFile(file: Partial<AppSettings>): number | undefined {
    return AppSettingsStore.normalizePort(file.port);
  }

  private readSearchFromFile(file: Partial<AppSettings>): PersistedSearchSettings {
    const raw = file.search;
    if (!raw || typeof raw !== "object") {
      return { engine: "duckduckgo" };
    }
    const cast = raw as unknown as Record<string, unknown>;
    return {
      engine: SEARCH_ENGINE_OPTIONS.includes(cast.engine as any) ? (cast.engine as any) : "duckduckgo",
      braveApiKeyRef: typeof cast.braveApiKeyRef === "string" ? cast.braveApiKeyRef : undefined,
      googleApiKeyRef: typeof cast.googleApiKeyRef === "string" ? cast.googleApiKeyRef : undefined,
      googleSearchEngineId: typeof cast.googleSearchEngineId === "string" ? cast.googleSearchEngineId : undefined
    };
  }

  private hasKey(ref: string | undefined): boolean {
    if (!ref) return false;
    return !!this.secretStore.get(ref);
  }

  private persistSearchSettings(search: SearchSettings | undefined, existingSearch: SearchSettings | undefined): SearchSettings | undefined {
    if (!search) return undefined;

    const normalized = normalizeSearchSettings(search);
    const persisted: PersistedSearchSettings = { engine: normalized.engine };

    if (normalized.googleSearchEngineId) {
      persisted.googleSearchEngineId = normalized.googleSearchEngineId;
    }

    if (this.secretStore.supportsSecurePersistence) {
      if (normalized.braveApiKey && normalized.braveApiKey !== SEARCH_PRESERVE_API_KEY_VALUE) {
        persisted.braveApiKeyRef = this.secretStore.set("search:brave", normalized.braveApiKey);
      }
      if (normalized.googleApiKey && normalized.googleApiKey !== SEARCH_PRESERVE_API_KEY_VALUE) {
        persisted.googleApiKeyRef = this.secretStore.set("search:google", normalized.googleApiKey);
      }
    }

    const existingPersisted = this.toPersistedRefs(existingSearch);
    if (normalized.braveApiKey === SEARCH_PRESERVE_API_KEY_VALUE) {
      persisted.braveApiKeyRef = existingPersisted.braveApiKeyRef;
    }
    if (normalized.googleApiKey === SEARCH_PRESERVE_API_KEY_VALUE) {
      persisted.googleApiKeyRef = existingPersisted.googleApiKeyRef;
    }

    return {
      engine: persisted.engine,
      googleSearchEngineId: persisted.googleSearchEngineId
    };
  }

  private toPersistedRefs(existing?: SearchSettings): Pick<PersistedSearchSettings, "braveApiKeyRef" | "googleApiKeyRef"> {
    if (!existing || typeof existing !== "object") return {};
    const cast = existing as unknown as Record<string, unknown>;
    return {
      braveApiKeyRef: typeof cast.braveApiKeyRef === "string" ? cast.braveApiKeyRef : undefined,
      googleApiKeyRef: typeof cast.googleApiKeyRef === "string" ? cast.googleApiKeyRef : undefined
    };
  }
}
