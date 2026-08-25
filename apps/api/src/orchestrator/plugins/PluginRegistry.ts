import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type {
  PluginManifest,
  PluginScope,
  PluginSummary,
  PluginTemplate,
  InstallPluginPayload
} from "./types.js";

const APP_DIR_NAME = "Locagens";
const MANIFEST_FILENAME = "plugin.json";
const PROJECT_PLUGINS_SEGMENTS = [".locagens", "plugins"] as const;

export const BUILTIN_TEMPLATES: PluginTemplate[] = [
  {
    id: "context-mode",
    name: "Context Mode",
    description: "Context window optimization for AI coding agents. Sandboxes tool output (98% reduction), persists session memory, and indexes session events with SQLite FTS5/BM25.",
    author: "mksglu",
    homepage: "https://github.com/mksglu/context-mode",
    manifest: {
      id: "context-mode",
      name: "Context Mode",
      version: "1.0.0",
      description: "Context window optimization for AI coding agents. Sandboxes tool output (98% reduction), persists session memory, and indexes session events with SQLite FTS5/BM25.",
      author: "mksglu",
      homepage: "https://github.com/mksglu/context-mode",
      mcpServers: {
        "context-mode": {
          transport: "stdio",
          command: "npx",
          args: ["-y", "context-mode"],
          enabled: true
        }
      },
      hooks: {
        onSessionStart: "internal:context-mode-init",
        postToolUse: "internal:context-mode-sandbox"
      },
      systemPrompt: "Context Mode is active. Large tool outputs and workspace operations are sandboxed and indexed in persistent session memory. You can use ctx_* MCP tools (ctx_execute, ctx_search, ctx_index) when needed."
    }
  },
  {
    id: "github-mcp",
    name: "GitHub Tools",
    description: "Inspect repositories, pull requests, issues, and branches directly via GitHub MCP Server.",
    author: "modelcontextprotocol",
    homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/github",
    manifest: {
      id: "github-mcp",
      name: "GitHub Tools",
      version: "1.0.0",
      description: "Inspect repositories, pull requests, issues, and branches directly via GitHub MCP Server.",
      author: "modelcontextprotocol",
      homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/github",
      mcpServers: {
        "github": {
          transport: "stdio",
          command: "npx",
          args: ["-y", "@modelcontextprotocol/server-github"],
          env: {
            "GITHUB_PERSONAL_ACCESS_TOKEN": ""
          },
          enabled: true
        }
      },
      systemPrompt: "GitHub MCP is connected. You can inspect GitHub issues, pull requests, commits, and repos."
    }
  },
  {
    id: "sqlite-explorer",
    name: "SQLite Explorer",
    description: "Read, inspect schemas, and query SQLite databases safely via MCP.",
    author: "modelcontextprotocol",
    homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/sqlite",
    manifest: {
      id: "sqlite-explorer",
      name: "SQLite Explorer",
      version: "1.0.0",
      description: "Read, inspect schemas, and query SQLite databases safely via MCP.",
      author: "modelcontextprotocol",
      homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/sqlite",
      mcpServers: {
        "sqlite": {
          transport: "stdio",
          command: "npx",
          args: ["-y", "@modelcontextprotocol/server-sqlite", "--db-path", "./data.db"],
          enabled: true
        }
      }
    }
  }
];

export class PluginRegistry {
  private readonly userRootOverride?: string;

  constructor(userRootOverride?: string) {
    this.userRootOverride = userRootOverride;
  }

  /** Absolute user-level plugins directory. */
  userRoot(): string {
    if (this.userRootOverride) return path.resolve(this.userRootOverride);
    if (process.env.LOCAGENS_PLUGINS_PATH) {
      return path.resolve(process.env.LOCAGENS_PLUGINS_PATH);
    }
    if (process.platform === "darwin") {
      return path.join(os.homedir(), "Library", "Application Support", APP_DIR_NAME, "plugins");
    }
    if (process.platform === "win32") {
      return path.join(process.env.APPDATA || path.join(os.homedir(), "AppData", "Roaming"), APP_DIR_NAME, "plugins");
    }
    return path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "locagens", "plugins");
  }

  /** Project plugins root, or null when no project path is set. */
  projectRoot(projectPath?: string | null): string | null {
    if (!projectPath?.trim()) return null;
    try {
      const base = fs.realpathSync.native(projectPath.trim());
      return path.join(base, ...PROJECT_PLUGINS_SEGMENTS);
    } catch {
      return null;
    }
  }

  /** Ensures user plugins directory exists. */
  ensureUserRoot(): string {
    const root = this.userRoot();
    fs.mkdirSync(root, { recursive: true });
    return fs.realpathSync.native(root);
  }

  /** Ensures project plugins directory exists. */
  ensureProjectRoot(projectPath: string): string | null {
    const root = this.projectRoot(projectPath);
    if (!root) return null;
    fs.mkdirSync(root, { recursive: true });
    try {
      return fs.realpathSync.native(root);
    } catch {
      return null;
    }
  }

  /**
   * Scans a root directory for plugin directories containing plugin.json.
   */
  private scanRoot(root: string, scope: PluginScope): PluginManifest[] {
    if (!fs.existsSync(root)) return [];
    const plugins: PluginManifest[] = [];

    try {
      const entries = fs.readdirSync(root, { withFileTypes: true });
      for (const entry of entries) {
        if (!entry.isDirectory()) continue;
        const pluginDir = path.join(root, entry.name);
        const manifestPath = path.join(pluginDir, MANIFEST_FILENAME);
        if (!fs.existsSync(manifestPath)) continue;

        try {
          const raw = fs.readFileSync(manifestPath, "utf8");
          const parsed = JSON.parse(raw) as Partial<PluginManifest>;
          if (parsed.id && parsed.name) {
            plugins.push({
              id: parsed.id,
              name: parsed.name,
              version: parsed.version || "1.0.0",
              description: parsed.description || "",
              author: parsed.author,
              homepage: parsed.homepage,
              scope,
              enabled: parsed.enabled !== false,
              mcpServers: parsed.mcpServers || {},
              tools: parsed.tools || [],
              hooks: parsed.hooks || {},
              systemPrompt: parsed.systemPrompt,
              dir: pluginDir,
              installedAt: parsed.installedAt || new Date().toISOString()
            });
          }
        } catch (err) {
          console.error(`[PluginRegistry] Failed to parse manifest in ${pluginDir}:`, err);
        }
      }
    } catch (err) {
      console.error(`[PluginRegistry] Failed to scan root ${root}:`, err);
    }

    return plugins;
  }

  /**
   * Discovers all plugins for a run/project.
   * Project-level plugins override user-level plugins with the same ID.
   */
  discover(projectPath?: string | null): PluginManifest[] {
    const byId = new Map<string, PluginManifest>();

    // 1. User plugins
    for (const plugin of this.scanRoot(this.userRoot(), "user")) {
      byId.set(plugin.id, plugin);
    }

    // 2. Project plugins
    const projRoot = this.projectRoot(projectPath);
    if (projRoot) {
      for (const plugin of this.scanRoot(projRoot, "project")) {
        byId.set(plugin.id, plugin);
      }
    }

    return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
  }

  /** Returns active (enabled) plugins. */
  getActivePlugins(projectPath?: string | null): PluginManifest[] {
    return this.discover(projectPath).filter(p => p.enabled);
  }

  /** Returns summary rows for API and UI. */
  listSummaries(projectPath?: string | null): PluginSummary[] {
    return this.discover(projectPath).map(p => ({
      id: p.id,
      name: p.name,
      version: p.version,
      description: p.description,
      author: p.author,
      homepage: p.homepage,
      scope: p.scope,
      enabled: p.enabled,
      toolCount: (p.tools?.length || 0) + Object.keys(p.mcpServers || {}).length,
      hasHooks: Object.keys(p.hooks || {}).length > 0,
      hasMcp: Object.keys(p.mcpServers || {}).length > 0,
      hasSystemPrompt: !!p.systemPrompt
    }));
  }

  /** Returns available built-in templates. */
  getTemplates(): PluginTemplate[] {
    return BUILTIN_TEMPLATES;
  }

  /** Installs a plugin into user or project scope. */
  async install(payload: InstallPluginPayload): Promise<PluginManifest> {
    const targetDir = payload.scope === "project" && payload.projectPath
      ? this.ensureProjectRoot(payload.projectPath)
      : this.ensureUserRoot();

    if (!targetDir) {
      throw new Error(`Failed to resolve target directory for ${payload.scope} plugin.`);
    }

    let manifest: PluginManifest;

    if (payload.source === "template") {
      const template = BUILTIN_TEMPLATES.find(t => t.id === payload.uri);
      if (!template) {
        throw new Error(`Unknown plugin template "${payload.uri}".`);
      }
      manifest = {
        ...template.manifest,
        scope: payload.scope,
        enabled: true,
        installedAt: new Date().toISOString()
      };
    } else if (payload.source === "github") {
      const repo = payload.uri.replace(/^https:\/\/github\.com\//, "").replace(/\.git$/, "").trim();
      const parts = repo.split("/");
      const id = parts[1] || parts[0] || "github-plugin";
      
      // If it's context-mode or known github repo, match template if available
      const matchedTemplate = BUILTIN_TEMPLATES.find(t => t.id === id || t.homepage?.includes(repo));
      if (matchedTemplate) {
        manifest = {
          ...matchedTemplate.manifest,
          scope: payload.scope,
          enabled: true,
          installedAt: new Date().toISOString()
        };
      } else {
        manifest = {
          id,
          name: parts[1] ? `${parts[1]} (${parts[0]})` : id,
          version: "1.0.0",
          description: `Plugin installed from GitHub: ${repo}`,
          homepage: `https://github.com/${repo}`,
          scope: payload.scope,
          enabled: true,
          mcpServers: {
            [id]: {
              transport: "stdio",
              command: "npx",
              args: ["-y", id],
              enabled: true
            }
          },
          installedAt: new Date().toISOString(),
          ...(payload.customManifest || {})
        };
      }
    } else if (payload.source === "npm") {
      const pkg = payload.uri.trim();
      const id = pkg.replace(/^@/, "").replace(/\//g, "-");
      manifest = {
        id,
        name: pkg,
        version: "1.0.0",
        description: `Plugin installed from NPM package: ${pkg}`,
        scope: payload.scope,
        enabled: true,
        mcpServers: {
          [id]: {
            transport: "stdio",
            command: "npx",
            args: ["-y", pkg],
            enabled: true
          }
        },
        installedAt: new Date().toISOString(),
        ...(payload.customManifest || {})
      };
    } else {
      // Local or custom manifest
      if (!payload.customManifest?.id || !payload.customManifest?.name) {
        throw new Error("Plugin ID and name are required for custom installation.");
      }
      manifest = {
        id: payload.customManifest.id,
        name: payload.customManifest.name,
        version: payload.customManifest.version || "1.0.0",
        description: payload.customManifest.description || "",
        author: payload.customManifest.author,
        homepage: payload.customManifest.homepage,
        scope: payload.scope,
        enabled: payload.customManifest.enabled !== false,
        mcpServers: payload.customManifest.mcpServers || {},
        tools: payload.customManifest.tools || [],
        hooks: payload.customManifest.hooks || {},
        systemPrompt: payload.customManifest.systemPrompt,
        installedAt: new Date().toISOString()
      };
    }

    const pluginFolder = path.join(targetDir, manifest.id);
    fs.mkdirSync(pluginFolder, { recursive: true });
    manifest.dir = pluginFolder;

    const manifestPath = path.join(pluginFolder, MANIFEST_FILENAME);
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2), "utf8");

    return manifest;
  }

  /** Toggles a plugin's enabled status. */
  toggle(id: string, enabled: boolean, scope: PluginScope = "user", projectPath?: string): PluginManifest | null {
    const targetRoot = scope === "project" && projectPath
      ? this.projectRoot(projectPath)
      : this.userRoot();

    if (!targetRoot) return null;
    const manifestPath = path.join(targetRoot, id, MANIFEST_FILENAME);
    if (!fs.existsSync(manifestPath)) return null;

    try {
      const raw = fs.readFileSync(manifestPath, "utf8");
      const manifest = JSON.parse(raw) as PluginManifest;
      manifest.enabled = enabled;
      fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2), "utf8");
      manifest.scope = scope;
      manifest.dir = path.join(targetRoot, id);
      return manifest;
    } catch (err) {
      console.error(`[PluginRegistry] Failed to toggle plugin ${id}:`, err);
      return null;
    }
  }

  /** Deletes a plugin directory. */
  delete(id: string, scope: PluginScope = "user", projectPath?: string): boolean {
    const targetRoot = scope === "project" && projectPath
      ? this.projectRoot(projectPath)
      : this.userRoot();

    if (!targetRoot) return false;
    const pluginFolder = path.join(targetRoot, id);
    if (!fs.existsSync(pluginFolder)) return false;

    try {
      fs.rmSync(pluginFolder, { recursive: true, force: true });
      return true;
    } catch (err) {
      console.error(`[PluginRegistry] Failed to delete plugin ${id}:`, err);
      return false;
    }
  }
}
