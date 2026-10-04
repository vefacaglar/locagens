import type { ChatMessage, RunMessage } from "@locagens/shared";

/** Roles whose messages live inside a delegation tool call, never the main thread. */
const SUB_AGENT_ROLES = new Set(["coder", "utility", "explorer", "worker"]);

/**
 * Reconstructs the provider-facing message list from persisted messages,
 * pairing each assistant tool_call with its results so the history is valid
 * for strict providers ("a tool result must immediately follow its call").
 *
 * Replays ONLY top-level (architect/main) messages: coder/utility/explorer/worker
 * sub-agent messages are internal to a delegate_tasks/delegate_to_utility/
 * spawn_agents call — the main agent never saw them, only the summarized tool
 * result — and including them would interleave their messages between the
 * delegating tool_call and its result.
 */
export function rebuildHistory(allMessages: RunMessage[], task: string): ChatMessage[] {
  const messages = allMessages.filter(m => !SUB_AGENT_ROLES.has(m.agentRole ?? ""));
  const chatMessages: ChatMessage[] = [{ role: "user", content: task }];

  for (let i = 0; i < messages.length; i++) {
    const m = messages[i];

    // A tool message reached here directly is an orphan result: its assistant
    // tool_call (if any) consumes its results in the assistant branch below, so
    // anything left over has no matching call. Drop it — sending it would make
    // a provider reject the whole request.
    if (m.role === "tool") continue;

    if (m.role === "assistant") {
      let toolCalls: any[] | undefined;
      if (m.rawResponse) {
        try {
          const parsed = JSON.parse(m.rawResponse);
          if (Array.isArray(parsed) && parsed.length > 0) toolCalls = parsed;
        } catch { /* ignore malformed raw response */ }
      }

      if (!toolCalls) {
        chatMessages.push({ role: "assistant", content: m.content });
        continue;
      }

      // The N tool results for this call are the next N consecutive tool
      // messages. Collect them in order.
      const results: RunMessage[] = [];
      let j = i + 1;
      while (j < messages.length && messages[j].role === "tool" && results.length < toolCalls.length) {
        results.push(messages[j]);
        j++;
      }

      if (results.length < toolCalls.length) {
        // Results are missing (e.g. the run was cancelled mid tool execution).
        // Drop the tool_calls rather than send calls without results.
        chatMessages.push({ role: "assistant", content: m.content });
        continue;
      }

      chatMessages.push({ role: "assistant", content: m.content || "", toolCalls });
      for (let k = 0; k < toolCalls.length; k++) {
        chatMessages.push({
          role: "tool",
          content: results[k].content,
          tool_call_id: toolCalls[k].id,
          name: toolCalls[k].function?.name
        });
      }
      i = j - 1; // skip the consumed result messages
      continue;
    }

    // system / user messages pass through unchanged
    chatMessages.push({ role: m.role, content: m.content });
  }

  return chatMessages;
}
