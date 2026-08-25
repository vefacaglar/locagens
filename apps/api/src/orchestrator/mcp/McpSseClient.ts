import type {
  McpClient,
  McpConnectionStatus,
  McpServerConfig,
  McpToolDefinition,
  JsonRpcRequest,
  JsonRpcResponse
} from "./types.js";

const DEFAULT_TIMEOUT_MS = 30_000;
const PROTOCOL_VERSION = "2024-11-05";

export class McpSseClient implements McpClient {
  private readonly config: McpServerConfig;
  private status: McpConnectionStatus = "disconnected";
  private lastError: string | null = null;
  private requestId = 0;
  private endpointUrl: string | null = null;
  private serverTools: McpToolDefinition[] = [];
  private abortController: AbortController | null = null;

  constructor(config: McpServerConfig) {
    this.config = config;
  }

  getStatus(): McpConnectionStatus {
    return this.status;
  }

  getLastError(): string | null {
    return this.lastError;
  }

  async connect(): Promise<void> {
    if (this.status === "connected") return;
    this.status = "connecting";
    this.lastError = null;

    const url = this.config.url;
    if (!url) {
      this.status = "error";
      this.lastError = "No URL specified for SSE MCP server.";
      throw new Error(this.lastError);
    }

    try {
      this.endpointUrl = url;
      this.abortController = new AbortController();

      // Handshake: initialize
      await this.request("initialize", {
        protocolVersion: PROTOCOL_VERSION,
        capabilities: {},
        clientInfo: { name: "locagens", version: "0.1.0" }
      });

      this.status = "connected";
      await this.listTools();
    } catch (err: any) {
      this.status = "error";
      this.lastError = err?.message ?? "Failed to connect to SSE MCP server.";
      throw err;
    }
  }

  async listTools(): Promise<McpToolDefinition[]> {
    if (this.status !== "connected") {
      await this.connect();
    }

    try {
      const response = await this.request("tools/list", {});
      const rawTools: any[] = Array.isArray(response?.tools) ? response.tools : [];

      this.serverTools = rawTools.map((tool) => {
        const origName = String(tool.name || "").trim();
        const namespaced = `mcp__${this.config.name}__${origName}`;
        return {
          name: namespaced,
          originalName: origName,
          serverName: this.config.name,
          description: tool.description || `Tool ${origName} from MCP server ${this.config.name}`,
          inputSchema: tool.inputSchema || { type: "object", properties: {} }
        };
      });

      return this.serverTools;
    } catch (err: any) {
      this.lastError = err?.message ?? "Failed to list tools.";
      throw err;
    }
  }

  async callTool(
    name: string,
    args: Record<string, any>
  ): Promise<{ content: Array<{ type: string; text?: string; data?: string }>; isError?: boolean }> {
    if (this.status !== "connected") {
      await this.connect();
    }

    const prefix = `mcp__${this.config.name}__`;
    const toolName = name.startsWith(prefix) ? name.slice(prefix.length) : name;

    try {
      const result = await this.request("tools/call", {
        name: toolName,
        arguments: args || {}
      });

      const content = Array.isArray(result?.content)
        ? result.content
        : [{ type: "text", text: typeof result === "string" ? result : JSON.stringify(result ?? {}) }];

      return {
        content,
        isError: !!result?.isError
      };
    } catch (err: any) {
      return {
        content: [{ type: "text", text: `MCP Tool Error: ${err?.message ?? "Execution failed"}` }],
        isError: true
      };
    }
  }

  async ping(): Promise<boolean> {
    if (this.status !== "connected") return false;
    try {
      await this.request("ping", {});
      return true;
    } catch {
      return false;
    }
  }

  async close(): Promise<void> {
    this.status = "disconnected";
    if (this.abortController) {
      this.abortController.abort();
      this.abortController = null;
    }
  }

  private async request(method: string, params?: Record<string, any>, timeoutMs = DEFAULT_TIMEOUT_MS): Promise<any> {
    if (!this.endpointUrl) throw new Error("SSE endpoint URL not set.");

    const id = ++this.requestId;
    const req: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params
    };

    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...(this.config.env || {})
    };

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);

    try {
      const res = await fetch(this.endpointUrl, {
        method: "POST",
        headers,
        body: JSON.stringify(req),
        signal: controller.signal
      });

      if (!res.ok) {
        throw new Error(`HTTP error ${res.status}: ${res.statusText}`);
      }

      const json = (await res.json()) as JsonRpcResponse;
      if (json.error) {
        throw new Error(json.error.message || `JSON-RPC error ${json.error.code}`);
      }
      return json.result;
    } finally {
      clearTimeout(timer);
    }
  }
}
