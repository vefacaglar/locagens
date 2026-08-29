import os from "node:os";
import path from "node:path";

const APP_DIR_NAME = "Locagens";

/** Platform app-support directory (macOS Application Support / Windows APPDATA / XDG). */
export function appSupportDir(): string {
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Application Support", APP_DIR_NAME);
  }
  if (process.platform === "win32") {
    return path.join(process.env.APPDATA || path.join(os.homedir(), "AppData", "Roaming"), APP_DIR_NAME);
  }
  return path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "locagens");
}

export function defaultSettingsPath(): string {
  if (process.env.LOCAGENS_SETTINGS_PATH) {
    return process.env.LOCAGENS_SETTINGS_PATH;
  }
  return path.join(appSupportDir(), "settings.json");
}

export function defaultProviderConfigPath(): string {
  if (process.env.LOCAGENS_PROVIDER_CONFIG_PATH) {
    return process.env.LOCAGENS_PROVIDER_CONFIG_PATH;
  }
  return path.join(appSupportDir(), "providers.json");
}
