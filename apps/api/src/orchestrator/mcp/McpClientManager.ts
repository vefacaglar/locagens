import type { McpClient, McpServerConfig, McpServerInfo, McpToolDefinition } from "./types.js";
import { McpConfigStore } from "./McpConfigStore.js";
import { McpStdioClient } from "./McpStdioClient.js";
import { McpSseClient } from "./McpSseClient.js";
import { formatMcpToolResult } from "./mcpToolAdapter.js";

export class McpClientManager {
  private readonly configStore: McpConfigStore;
  private readonly clients = new Map<string, McpClient>();

  constructor(configStore: McpConfigStore) {
    this.configStore = configStore;
  }

  getConfigStore(): McpConfigStore {
    return this.configStore;
  }

  /**
   * Retrieves all tools exposed by currently enabled MCP servers for the active context.
   */
  async getAllActiveTools(projectPath?: string | null): Promise<McpToolDefinition[]> {
    const configs = this.configStore.listConfigs(projectPath);
    const enabledConfigs = configs.filter((c) => c.enabled);
    const allTools: McpToolDefinition[] = [];

    await Promise.all(
      enabledConfigs.map(async (config) => {
        try {
          const client = this.getOrCreateClient(config);
          const tools = await client.listTools();
          allTools.push(...tools);
        } catch (err: any) {
          console.error(`[MCP] Failed to load tools from server "${config.name}":`, err.message);
        }
      })
    );

    return allTools.sort((a, b) => a.name.localeCompare(b.name));
  }

  /**
   * Invokes an MCP tool by its namespaced name (e.g. mcp__github__create_issue).
   */
  async callTool(
    namespacedToolName: string,
    args: Record<string, any>,
    projectPath?: string | null
  ): Promise<string> {
    const parts = namespacedToolName.split("__");
    if (parts.length < 3 || parts[0] !== "mcp") {
      return JSON.stringify({ success: false, error: `Invalid MCP tool name: "${namespacedToolName}"` });
    }

    const serverName = parts[1];
    const toolName = parts.slice(2).join("__");

    const configs = this.configStore.listConfigs(projectPath);
    const config = configs.find((c) => c.name === serverName);

    if (!config) {
      return JSON.stringify({ success: false, error: `MCP server "${serverName}" is not configured.` });
    }

    if (!config.enabled) {
      return JSON.stringify({ success: false, error: `MCP server "${serverName}" is currently disabled.` });
    }

    try {
      const client = this.getOrCreateClient(config);
      const result = await client.callTool(toolName, args);
      return formatMcpToolResult(result);
    } catch (err: any) {
      return JSON.stringify({
        success: false,
        error: `MCP invocation error on server "${serverName}": ${err?.message ?? "Execution failed"}`
      });
    }
  }

  /**
   * Returns current status and tool definitions for all configured MCP servers.
   */
  async getStatusList(projectPath?: string | null): Promise<McpServerInfo[]> {
    const configs = this.configStore.listConfigs(projectPath);
    const result: McpServerInfo[] = [];

    for (const config of configs) {
      if (!config.enabled) {
        result.push({
          config,
          status: "disabled",
          error: null,
          tools: []
        });
        continue;
      }

      try {
        const client = this.getOrCreateClient(config);
        const tools = await client.listTools();
        result.push({
          config,
          status: client.getStatus(),
          error: client.getLastError(),
          tools
        });
      } catch (err: any) {
        result.push({
          config,
          status: "error",
          error: err?.message ?? "Failed to connect",
          tools: []
        });
      }
    }

    return result;
  }

  /**
   * Restarts a specific MCP server process/connection.
   */
  async restartServer(name: string, projectPath?: string | null): Promise<McpServerInfo> {
    const rawName = String(name || "").trim().toLowerCase();
    const existing = this.clients.get(rawName);
    if (existing) {
      await existing.close();
      this.clients.delete(rawName);
    }

    const configs = this.configStore.listConfigs(projectPath);
    const config = configs.find((c) => c.name === rawName);

    if (!config) {
      throw new Error(`MCP server "${rawName}" not found.`);
    }

    if (!config.enabled) {
      return {
        config,
        status: "disabled",
        error: null,
        tools: []
      };
    }

    try {
      const client = this.getOrCreateClient(config);
      const tools = await client.listTools();
      return {
        config,
        status: client.getStatus(),
        error: client.getLastError(),
        tools
      };
    } catch (err: any) {
      return {
        config,
        status: "error",
        error: err?.message ?? "Failed to connect",
        tools: []
      };
    }
  }

  /**
   * Enables or disables an MCP server.
   */
  async toggleServer(name: string, enabled: boolean, projectPath?: string | null): Promise<McpServerInfo> {
    const rawName = String(name || "").trim().toLowerCase();
    const configs = this.configStore.listConfigs(projectPath);
    const config = configs.find((c) => c.name === rawName);

    if (!config) {
      throw new Error(`MCP server "${rawName}" not found.`);
    }

    this.configStore.toggleConfig(rawName, enabled, config.scope, projectPath);
    config.enabled = enabled;

    if (!enabled) {
      const client = this.clients.get(rawName);
      if (client) {
        await client.close();
        this.clients.delete(rawName);
      }
      return {
        config,
        status: "disabled",
        error: null,
        tools: []
      };
    }

    return this.restartServer(rawName, projectPath);
  }

  /**
   * Deletes an MCP server configuration and terminates its client.
   */
  async deleteServer(name: string, projectPath?: string | null): Promise<boolean> {
    const rawName = String(name || "").trim().toLowerCase();
    const configs = this.configStore.listConfigs(projectPath);
    const config = configs.find((c) => c.name === rawName);
    if (!config) return false;

    const client = this.clients.get(rawName);
    if (client) {
      await client.close();
      this.clients.delete(rawName);
    }

    return this.configStore.deleteConfig(rawName, config.scope, projectPath);
  }

  /**
   * Closes all active MCP clients.
   */
  async closeAll(): Promise<void> {
    for (const [, client] of this.clients) {
      await client.close();
    }
    this.clients.clear();
  }

  private getOrCreateClient(config: McpServerConfig): McpClient {
    const existing = this.clients.get(config.name);
    if (existing) return existing;

    const client: McpClient =
      config.transport === "sse" ? new McpSseClient(config) : new McpStdioClient(config);

    this.clients.set(config.name, client);
    return client;
  }
}
