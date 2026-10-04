import type { CompletionRequest, CompletionResponse, ChatMessage } from "@locagens/shared";
import type { ModelProvider } from "./ModelProvider.js";
import { DEFAULT_MAX_TOKENS } from "./ModelProvider.js";

// Max time to wait for the (non-streamed) response before aborting. Generous so
// long generations aren't cut off prematurely.
const REQUEST_TIMEOUT_MS = 300_000; // 5 minutes

/**
 * Converts OpenAI-style tool definitions into Anthropic's tool schema.
 * (function.parameters -> input_schema)
 */
function toAnthropicTools(tools?: any[]) {
  if (!tools || tools.length === 0) return undefined;
  return tools.map(tool => ({
    name: tool.function.name,
    description: tool.function.description,
    input_schema: tool.function.parameters
  }));
}

/**
 * Maps the internal (OpenAI-shaped) message list to the Anthropic Messages
 * format. Plain text turns use string content; tool calls become `tool_use`
 * blocks on the assistant turn and `tool_result` blocks on a following user
 * turn. Consecutive tool results are merged into a single user message so the
 * conversation keeps alternating user/assistant turns.
 */
function parseAnthropicMessageContent(content: string): any {
  if (!content) return "";
  const regex = /!\[([^\]]*)\]\((data:image\/([a-zA-Z+.-]+);base64,([^)]+))\)/g;
  const matches = [...content.matchAll(regex)];
  if (matches.length === 0) {
    return content;
  }

  const parts: any[] = [];
  let lastIndex = 0;

  for (const match of matches) {
    const textPart = content.slice(lastIndex, match.index).trim();
    if (textPart) {
      parts.push({ type: "text", text: textPart });
    }
    const mediaType = `image/${match[3]}`;
    const base64Data = match[4];
    parts.push({
      type: "image",
      source: {
        type: "base64",
        media_type: mediaType,
        data: base64Data
      }
    });
    lastIndex = (match.index ?? 0) + match[0].length;
  }

  const remainingText = content.slice(lastIndex).trim();
  if (remainingText) {
    parts.push({ type: "text", text: remainingText });
  }

  return parts;
}

function toAnthropicMessages(messages: ChatMessage[]): any[] {
  const out: any[] = [];
  let i = 0;

  while (i < messages.length) {
    const m = messages[i];

    if (m.role === "system") {
      // System content is sent via the top-level `system` field instead.
      i++;
    } else if (m.role === "user") {
      const content = parseAnthropicMessageContent(m.content);
      const previous = out[out.length - 1];
      if (previous?.role === "user" && Array.isArray(previous.content)) {
        // A user message right after tool results (e.g. one sent mid-run) joins
        // that tool_result turn, keeping user/assistant turns alternating.
        previous.content.push(...(typeof content === "string" ? [{ type: "text", text: content }] : content));
      } else {
        out.push({ role: "user", content });
      }
      i++;
    } else if (m.role === "assistant") {
      if (m.toolCalls && m.toolCalls.length > 0) {
        const blocks: any[] = [];
        if (m.content && m.content.trim()) {
          blocks.push({ type: "text", text: m.content });
        }
        for (const tc of m.toolCalls) {
          let input: unknown = {};
          try {
            input = JSON.parse(tc.function.arguments || "{}");
          } catch {
            input = {};
          }
          blocks.push({ type: "tool_use", id: tc.id, name: tc.function.name, input });
        }
        out.push({ role: "assistant", content: blocks });
      } else {
        out.push({ role: "assistant", content: parseAnthropicMessageContent(m.content) });
      }
      i++;
    } else if (m.role === "tool") {
      const blocks: any[] = [];
      while (i < messages.length && messages[i].role === "tool") {
        const t = messages[i];
        blocks.push({ type: "tool_result", tool_use_id: t.tool_call_id, content: t.content });
        i++;
      }
      out.push({ role: "user", content: blocks });
    } else {
      i++;
    }
  }

  return out;
}

/**
 * Adds a rolling prompt-cache breakpoint to the END of the conversation by
 * tagging the last content block of the last message with `cache_control`.
 * Combined with the breakpoint on the system block, this lets every turn of the
 * agent loop read the entire prior prefix (tools + system + all earlier
 * messages) from cache instead of re-billing it at full rate. Mutates in place.
 */
function markLastBlockForCaching(messages: any[]): void {
  const last = messages[messages.length - 1];
  if (!last) return;
  if (typeof last.content === "string") {
    // Never tag an empty text block — Anthropic rejects those.
    if (!last.content.trim()) return;
    last.content = [{ type: "text", text: last.content, cache_control: { type: "ephemeral" } }];
  } else if (Array.isArray(last.content) && last.content.length > 0) {
    const lastBlock = last.content[last.content.length - 1];
    last.content[last.content.length - 1] = { ...lastBlock, cache_control: { type: "ephemeral" } };
  }
}

export class AnthropicProvider implements ModelProvider {
  private baseUrl: string;
  private apiKey: string;

  constructor(baseUrl: string, apiKey: string) {
    this.baseUrl = baseUrl.replace(/\/$/, ""); // Strip trailing slash
    this.apiKey = apiKey;
  }

  async complete(
    request: CompletionRequest,
    onChunk?: (chunk: { content?: string; reasoningContent?: string }) => void
  ): Promise<CompletionResponse> {
    const url = `${this.baseUrl}/v1/messages`;

    // Prompt caching: tag the conversation tail so the whole prior prefix
    // (tools + system + earlier turns) is served from cache on later loop turns.
    const messages = toAnthropicMessages(request.messages);
    markLastBlockForCaching(messages);

    const body: any = {
      model: request.model,
      messages,
      max_tokens: request.maxTokens ?? DEFAULT_MAX_TOKENS, // Anthropic requires max_tokens
      temperature: request.temperature ?? 0.7
    };

    if (request.reasoning?.style === "anthropic-budget" && request.reasoning.budgetTokens) {
      body.thinking = { type: "enabled", budget_tokens: request.reasoning.budgetTokens };
      body.temperature = 1;
    }

    if (request.systemPrompt) {
      // Send the system prompt as a cacheable block. This breakpoint caches the
      // stable prefix (tools come before system in the cache, so they are
      // covered too); within the 5-minute TTL each agent-loop turn reads it back
      // at ~10% of the input cost instead of re-billing the full prompt.
      body.system = [
        { type: "text", text: request.systemPrompt, cache_control: { type: "ephemeral" } }
      ];
    }

    const tools = toAnthropicTools(request.tools);
    if (tools) {
      body.tools = tools;
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => {
      controller.abort();
    }, REQUEST_TIMEOUT_MS);

    try {
      const response = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "x-api-key": this.apiKey,
          "anthropic-version": "2023-06-01"
        },
        body: JSON.stringify(body),
        signal: controller.signal
      });

      if (!response.ok) {
        const errText = await response.text().catch(() => "Unknown error");
        throw new Error(`Anthropic API returned status ${response.status}: ${errText}`);
      }

      const data = await response.json() as any;

      if (!data.content || !Array.isArray(data.content)) {
        throw new Error("Anthropic API returned invalid response shape");
      }

      // Join all text blocks; collect tool_use blocks as tool calls.
      const content = data.content
        .filter((block: any) => block.type === "text")
        .map((block: any) => block.text)
        .join("\n");

      const reasoningContent = data.content
        .filter((block: any) => block.type === "thinking")
        .map((block: any) => block.thinking)
        .join("\n") || undefined;

      const toolUseBlocks = data.content.filter((block: any) => block.type === "tool_use");
      const toolCalls = toolUseBlocks.length > 0
        ? toolUseBlocks.map((block: any) => ({
            id: block.id,
            type: "function" as const,
            function: {
              name: block.name,
              arguments: JSON.stringify(block.input ?? {})
            }
          }))
        : undefined;

      if ((!content || !content.trim()) && (!reasoningContent || !reasoningContent.trim()) && (!toolCalls || toolCalls.length === 0)) {
        throw new Error("Provider returned an empty response");
      }

      const usage = data.usage;

      return {
        content,
        reasoningContent,
        toolCalls,
        raw: data,
        usage: usage ? {
          // Anthropic's input_tokens already EXCLUDES cached reads and cache
          // writes, so these three are disjoint buckets of prompt tokens.
          inputTokens: usage.input_tokens,
          outputTokens: usage.output_tokens,
          totalTokens: usage.input_tokens + usage.output_tokens,
          cacheReadInputTokens: usage.cache_read_input_tokens ?? undefined,
          cacheWriteInputTokens: usage.cache_creation_input_tokens ?? undefined
        } : undefined
      };
    } catch (error: any) {
      if (error.name === "AbortError" || error.message?.includes("aborted")) {
        throw new Error(`Request to provider timed out after ${REQUEST_TIMEOUT_MS}ms`);
      }
      throw new Error(`Failed to query Anthropic provider: ${error.message}`);
    } finally {
      clearTimeout(timeoutId);
    }
  }
}
