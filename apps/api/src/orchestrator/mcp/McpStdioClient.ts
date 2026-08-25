import { spawn, type ChildProcess } from "node:child_process";
import type {
  McpClient,
  McpConnectionStatus,
  McpServerConfig,
  McpToolDefinition,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcNotification
} from "./types.js";

const DEFAULT_TIMEOUT_MS = 30_000;
const PROTOCOL_VERSION = "2024-11-05";

export class McpStdioClient implements McpClient {
  private readonly config: McpServerConfig;
  private process: ChildProcess | null = null;
  private status: McpConnectionStatus = "disconnected";
  private lastError: string | null = null;
  private requestId = 0;
  private pendingRequests = new Map<
    string | number,
    { resolve: (res: any) => void; reject: (err: any) => void; timer: NodeJS.Timeout }
  >();
  private buffer = "";
  private serverTools: McpToolDefinition[] = [];

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
    if (this.status === "connected" && this.process) {
      return;
    }

    this.status = "connecting";
    this.lastError = null;

    const command = this.config.command;
    if (!command) {
      this.status = "error";
      this.lastError = "No command specified for stdio MCP server.";
      throw new Error(this.lastError);
    }

    try {
      const args = this.config.args || [];
      const env = {
        ...process.env,
        ...(this.config.env || {})
      };

      const cwd = this.config.projectPath || process.cwd();

      this.process = spawn(command, args, {
        cwd,
        env,
        stdio: ["pipe", "pipe", "pipe"]
      });

      this.process.on("error", (err) => {
        this.status = "error";
        this.lastError = `Process error: ${err.message}`;
        this.rejectAllPending(new Error(this.lastError));
      });

      this.process.on("exit", (code, signal) => {
        if (this.status !== "disconnected") {
          this.status = "disconnected";
          if (code !== 0 && code !== null) {
            this.lastError = `Process exited with code ${code} (${signal || "no signal"})`;
          }
        }
        this.rejectAllPending(new Error(this.lastError || "MCP server process exited."));
        this.process = null;
      });

      this.process.stdout?.on("data", (chunk: Buffer) => {
        this.handleStdoutData(chunk);
      });

      let stderrLog = "";
      this.process.stderr?.on("data", (chunk: Buffer) => {
        const str = chunk.toString("utf-8");
        stderrLog += str;
        if (stderrLog.length > 2000) {
          stderrLog = stderrLog.slice(-2000);
        }
      });

      // Handshake: initialize
      const initResult = await this.request("initialize", {
        protocolVersion: PROTOCOL_VERSION,
        capabilities: {},
        clientInfo: {
          name: "locagens",
          version: "0.1.0"
        }
      });

      // Send initialized notification
      this.notify("notifications/initialized", {});

      this.status = "connected";

      // Cache tools
      await this.listTools();
    } catch (err: any) {
      this.status = "error";
      this.lastError = err?.message ?? "Failed to connect to MCP server.";
      await this.close();
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

    // Strip namespace if present
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
    this.rejectAllPending(new Error("Client closed."));

    if (this.process) {
      const proc = this.process;
      this.process = null;
      try {
        proc.kill("SIGTERM");
        setTimeout(() => {
          try {
            proc.kill("SIGKILL");
          } catch {
            /* ignore */
          }
        }, 1000).unref();
      } catch {
        /* ignore */
      }
    }
  }

  private request(method: string, params?: Record<string, any>, timeoutMs = DEFAULT_TIMEOUT_MS): Promise<any> {
    return new Promise((resolve, reject) => {
      if (!this.process || !this.process.stdin || this.process.killed) {
        return reject(new Error("MCP server process is not running."));
      }

      const id = ++this.requestId;
      const req: JsonRpcRequest = {
        jsonrpc: "2.0",
        id,
        method,
        params
      };

      const timer = setTimeout(() => {
        this.pendingRequests.delete(id);
        reject(new Error(`MCP request "${method}" timed out after ${timeoutMs}ms.`));
      }, timeoutMs);

      this.pendingRequests.set(id, { resolve, reject, timer });

      try {
        const payload = JSON.stringify(req) + "\n";
        this.process.stdin.write(payload, "utf-8");
      } catch (err) {
        clearTimeout(timer);
        this.pendingRequests.delete(id);
        reject(err);
      }
    });
  }

  private notify(method: string, params?: Record<string, any>): void {
    if (!this.process || !this.process.stdin || this.process.killed) return;
    const notification: JsonRpcNotification = {
      jsonrpc: "2.0",
      method,
      params
    };
    try {
      this.process.stdin.write(JSON.stringify(notification) + "\n", "utf-8");
    } catch {
      /* ignore */
    }
  }

  private handleStdoutData(chunk: Buffer): void {
    this.buffer += chunk.toString("utf-8");
    const lines = this.buffer.split(/\r?\n/);
    this.buffer = lines.pop() || "";

    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const msg = JSON.parse(trimmed) as JsonRpcResponse;
        if (msg.id !== undefined && this.pendingRequests.has(msg.id)) {
          const pending = this.pendingRequests.get(msg.id)!;
          this.pendingRequests.delete(msg.id);
          clearTimeout(pending.timer);

          if (msg.error) {
            pending.reject(new Error(msg.error.message || `JSON-RPC error code ${msg.error.code}`));
          } else {
            pending.resolve(msg.result);
          }
        }
      } catch {
        // Non-JSON or log line on stdout, ignore
      }
    }
  }

  private rejectAllPending(err: Error): void {
    for (const [, pending] of this.pendingRequests) {
      clearTimeout(pending.timer);
      pending.reject(err);
    }
    this.pendingRequests.clear();
  }
}
