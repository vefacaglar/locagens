export interface PreScrollState {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  wasAtBottom: boolean;
}

/**
 * Captures scroll state of pre elements inside a container. When messageId is
 * given, only that message's code blocks are measured (their block ids embed
 * the message id, e.g. `<id>-code-0` / `thought-<id>-code-0`) — reading
 * scrollHeight forces a synchronous layout, so during streaming the caller
 * scopes this to the one message whose content can actually change.
 */
export function capturePreScrollStates(container: HTMLElement | null, messageId?: string): Map<string, PreScrollState> {
  const states = new Map<string, PreScrollState>();
  if (!container) return states;

  const selector = messageId ? `pre[id*="${CSS.escape(messageId)}"]` : 'pre[id]';
  const preElements = container.querySelectorAll(selector);
  preElements.forEach((el) => {
    const pre = el as HTMLElement;
    const wasAtBottom = pre.scrollHeight - pre.scrollTop - pre.clientHeight <= 60;
    states.set(pre.id, {
      scrollTop: pre.scrollTop,
      scrollHeight: pre.scrollHeight,
      clientHeight: pre.clientHeight,
      wasAtBottom
    });
  });

  return states;
}

/**
 * Restores scroll state of pre elements inside a container, scoped the same
 * way as capturePreScrollStates.
 */
export function restorePreScrollStates(container: HTMLElement | null, states: Map<string, PreScrollState>, messageId?: string) {
  if (!container) return;

  const selector = messageId ? `pre[id*="${CSS.escape(messageId)}"]` : 'pre[id]';
  const preElements = container.querySelectorAll(selector);
  preElements.forEach((el) => {
    const pre = el as HTMLElement;
    if (states.has(pre.id)) {
      const state = states.get(pre.id)!;
      if (state.wasAtBottom) {
        pre.scrollTop = pre.scrollHeight;
      } else {
        pre.scrollTop = state.scrollTop;
      }
    } else {
      // New code block being generated: default to auto-scrolling to the bottom.
      pre.scrollTop = pre.scrollHeight;
    }
  });
}
