import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { McpServerConfig, McpServerScope, McpTransportType } from "./types.js";

const APP_DIR_NAME = "Locagens";
const USER_CONFIG_FILENAME = "mcp_servers.json";
const PROJECT_CONFIG_SEGMENTS = [".locagens", "mcp.json"] as const;

export interface RawMcpServerEntry {
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  transport?: McpTransportType;
  disabled?: boolean;
}

export interface RawMcpConfigFile {
  mcpServers?: Record<string, RawMcpServerEntry>;
}

export class McpConfigStore {
  private readonly userConfigPathOverride?: string;

  constructor(userConfigPathOverride?: string) {
    this.userConfigPathOverride = userConfigPathOverride;
  }

  userConfigPath(): string {
    if (this.userConfigPathOverride) return path.resolve(this.userConfigPathOverride);
    if (process.env.LOCAGENS_MCP_CONFIG_PATH) {
      return path.resolve(process.env.LOCAGENS_MCP_CONFIG_PATH);
    }
    if (process.platform === "darwin") {
      return path.join(os.homedir(), "Library", "Application Support", APP_DIR_NAME, USER_CONFIG_FILENAME);
    }
    if (process.platform === "win32") {
      return path.join(process.env.APPDATA || path.join(os.homedir(), "AppData", "Roaming"), APP_DIR_NAME, USER_CONFIG_FILENAME);
    }
    return path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "locagens", USER_CONFIG_FILENAME);
  }

  projectConfigPath(projectPath?: string | null): string | null {
    if (!projectPath?.trim()) return null;
    try {
      const base = fs.realpathSync.native(projectPath.trim());
      return path.join(base, ...PROJECT_CONFIG_SEGMENTS);
    } catch {
      return null;
    }
  }

  listConfigs(projectPath?: string | null): McpServerConfig[] {
    const configs: McpServerConfig[] = [];
    const namesSeen = new Set<string>();

    // 1. Project configs take precedence (override same-named user configs)
    const projPath = this.projectConfigPath(projectPath);
    if (projPath && fs.existsSync(projPath)) {
      const projConfigs = this.readConfigFile(projPath, "project", projectPath || undefined);
      for (const cfg of projConfigs) {
        configs.push(cfg);
        namesSeen.add(cfg.name);
      }
    }

    // 2. User configs
    const userPath = this.userConfigPath();
    if (fs.existsSync(userPath)) {
      const userConfigs = this.readConfigFile(userPath, "user");
      for (const cfg of userConfigs) {
        if (!namesSeen.has(cfg.name)) {
          configs.push(cfg);
          namesSeen.add(cfg.name);
        }
      }
    }

    return configs.sort((a, b) => a.name.localeCompare(b.name));
  }

  saveConfig(config: McpServerConfig, projectPath?: string | null): McpServerConfig {
    const rawName = String(config.name || "").trim().toLowerCase();
    if (!rawName || !/^[a-z0-9][a-z0-9_-]*$/.test(rawName)) {
      throw new Error("Invalid MCP server name. Must use lowercase alphanumeric, dash or underscore.");
    }

    const scope: McpServerScope = config.scope === "project" ? "project" : "user";
    const filePath =
      scope === "project"
        ? (() => {
            const p = this.projectConfigPath(projectPath);
            if (!p) throw new Error("projectPath is required for project MCP servers.");
            return p;
          })()
        : this.userConfigPath();

    const dir = path.dirname(filePath);
    fs.mkdirSync(dir, { recursive: true });

    let rawFile: RawMcpConfigFile = {};
    if (fs.existsSync(filePath)) {
      try {
        rawFile = JSON.parse(fs.readFileSync(filePath, "utf-8"));
      } catch {
        rawFile = {};
      }
    }
    if (!rawFile.mcpServers || typeof rawFile.mcpServers !== "object") {
      rawFile.mcpServers = {};
    }

    const transport = config.transport === "sse" ? "sse" : "stdio";
    const entry: RawMcpServerEntry = {
      transport,
      disabled: !config.enabled
    };

    if (transport === "stdio") {
      entry.command = config.command?.trim();
      entry.args = Array.isArray(config.args) ? config.args : [];
      if (config.env && Object.keys(config.env).length > 0) {
        entry.env = config.env;
      }
    } else {
      entry.url = config.url?.trim();
      if (config.env && Object.keys(config.env).length > 0) {
        entry.env = config.env;
      }
    }

    rawFile.mcpServers[rawName] = entry;
    fs.writeFileSync(filePath, JSON.stringify(rawFile, null, 2), "utf-8");

    return {
      name: rawName,
      scope,
      transport,
      command: entry.command,
      args: entry.args,
      env: entry.env,
      url: entry.url,
      enabled: !entry.disabled,
      projectPath: scope === "project" ? projectPath || undefined : undefined
    };
  }

  deleteConfig(name: string, scope: McpServerScope, projectPath?: string | null): boolean {
    const rawName = String(name || "").trim().toLowerCase();
    if (!rawName) return false;

    const filePath = scope === "project" ? this.projectConfigPath(projectPath) : this.userConfigPath();
    if (!filePath || !fs.existsSync(filePath)) return false;

    try {
      const rawFile: RawMcpConfigFile = JSON.parse(fs.readFileSync(filePath, "utf-8"));
      if (rawFile.mcpServers && rawFile.mcpServers[rawName]) {
        delete rawFile.mcpServers[rawName];
        fs.writeFileSync(filePath, JSON.stringify(rawFile, null, 2), "utf-8");
        return true;
      }
    } catch {
      return false;
    }

    return false;
  }

  toggleConfig(name: string, enabled: boolean, scope: McpServerScope, projectPath?: string | null): boolean {
    const rawName = String(name || "").trim().toLowerCase();
    if (!rawName) return false;

    const filePath = scope === "project" ? this.projectConfigPath(projectPath) : this.userConfigPath();
    if (!filePath || !fs.existsSync(filePath)) return false;

    try {
      const rawFile: RawMcpConfigFile = JSON.parse(fs.readFileSync(filePath, "utf-8"));
      if (rawFile.mcpServers && rawFile.mcpServers[rawName]) {
        rawFile.mcpServers[rawName].disabled = !enabled;
        fs.writeFileSync(filePath, JSON.stringify(rawFile, null, 2), "utf-8");
        return true;
      }
    } catch {
      return false;
    }

    return false;
  }

  private readConfigFile(filePath: string, scope: McpServerScope, projectPath?: string): McpServerConfig[] {
    try {
      const text = fs.readFileSync(filePath, "utf-8");
      const json: RawMcpConfigFile = JSON.parse(text);
      const servers = json.mcpServers;
      if (!servers || typeof servers !== "object") return [];

      const result: McpServerConfig[] = [];
      for (const [rawName, entry] of Object.entries(servers)) {
        const name = rawName.trim().toLowerCase();
        if (!name || typeof entry !== "object" || entry === null) continue;

        const transport: McpTransportType = entry.transport === "sse" || entry.url ? "sse" : "stdio";
        result.push({
          name,
          scope,
          transport,
          command: entry.command,
          args: Array.isArray(entry.args) ? entry.args : [],
          env: entry.env && typeof entry.env === "object" ? entry.env : undefined,
          url: entry.url,
          enabled: !entry.disabled,
          projectPath: scope === "project" ? projectPath : undefined
        });
      }
      return result;
    } catch {
      return [];
    }
  }
}
