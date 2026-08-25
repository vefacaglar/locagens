import type {
  McpServerConfig,
  McpServerInfo,
  McpToolDefinition,
  McpConnectionStatus,
  McpTransportType,
  McpServerScope
} from "@locagens/shared";

export type {
  McpServerConfig,
  McpServerInfo,
  McpToolDefinition,
  McpConnectionStatus,
  McpTransportType,
  McpServerScope
};

export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: string | number;
  method: string;
  params?: Record<string, any>;
}

export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: Record<string, any>;
}

export interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: string | number;
  result?: any;
  error?: {
    code: number;
    message: string;
    data?: any;
  };
}

export interface McpClient {
  connect(): Promise<void>;
  listTools(): Promise<McpToolDefinition[]>;
  callTool(name: string, args: Record<string, any>): Promise<{ content: Array<{ type: string; text?: string; data?: string }>; isError?: boolean }>;
  ping(): Promise<boolean>;
  close(): Promise<void>;
  getStatus(): McpConnectionStatus;
  getLastError(): string | null;
}
