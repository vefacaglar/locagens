import type { McpToolDefinition } from "./types.js";

/**
 * Converts MCP tool definitions into standard OpenAI-compatible Tool schemas
 * for completion requests.
 */
export function adaptMcpToolsToSchemas(tools: McpToolDefinition[]): any[] {
  return tools.map((tool) => ({
    type: "function",
    function: {
      name: tool.name,
      description: tool.description || `MCP tool "${tool.originalName}" from server "${tool.serverName}".`,
      parameters: tool.inputSchema || {
        type: "object",
        properties: {}
      }
    }
  }));
}

/**
 * Formats MCP tool call response into a string for the message thread.
 */
export function formatMcpToolResult(result: {
  content: Array<{ type: string; text?: string; data?: string; mimeType?: string }>;
  isError?: boolean;
}): string {
  if (result.isError) {
    const errorText = result.content.map((c) => c.text || JSON.stringify(c)).join("\n");
    return JSON.stringify({
      success: false,
      error: errorText || "MCP tool returned an error."
    });
  }

  const textParts = result.content
    .map((c) => {
      if (c.type === "text") return c.text || "";
      if (c.type === "image") return `[Image: ${c.mimeType || "image"}]`;
      return JSON.stringify(c);
    })
    .filter(Boolean);

  if (textParts.length === 0) {
    return JSON.stringify({ success: true, result: "Tool executed successfully." });
  }

  return textParts.join("\n\n");
}
