import { nextTick, onBeforeUnmount, watch, type Ref } from 'vue';
import type { RunMessage } from '@locagens/shared';

export function useChatAutoScroll(
  container: Ref<HTMLElement | null>,
  messages: Ref<RunMessage[]>,
  isRunning: Ref<boolean>,
  activeRunId: Ref<string | null>
) {
  let isSticky = true;
  let isProgrammaticScrolling = false;
  let programmaticScrollTimeout: number | null = null;
  const stickyBottomThreshold = 24;

  function setProgrammaticScrollActive(duration = 800) {
    isProgrammaticScrolling = true;
    if (programmaticScrollTimeout !== null) {
      clearTimeout(programmaticScrollTimeout);
    }
    programmaticScrollTimeout = window.setTimeout(() => {
      isProgrammaticScrolling = false;
      programmaticScrollTimeout = null;
    }, duration);
  }

  function isAtBottom(el: HTMLElement, threshold = stickyBottomThreshold) {
    return el.scrollHeight - el.scrollTop - el.clientHeight <= threshold;
  }

  function scrollToBottom() {
    if (!container.value) return;

    // Temporarily disable smooth scroll to perform instant scroll
    const el = container.value;
    const originalScrollBehavior = el.style.scrollBehavior;
    el.style.scrollBehavior = 'auto';
    el.scrollTop = el.scrollHeight;
    el.style.scrollBehavior = originalScrollBehavior;
  }

  function scrollToUserMessage() {
    if (!container.value) return;
    const userContainers = container.value.querySelectorAll('.user-message-container');
    const lastUserContainer = userContainers[userContainers.length - 1] as HTMLElement | undefined;
    if (lastUserContainer) {
      // Put the new user turn as high as possible while leaving a small visual inset.
      const topOffset = lastUserContainer.getBoundingClientRect().top - container.value.getBoundingClientRect().top + container.value.scrollTop - 16;
      
      // Set programmatic scrolling active so that async smooth scroll events do not turn off stickiness
      setProgrammaticScrollActive(800);

      container.value.scrollTo({
        top: Math.max(0, topOffset),
        behavior: 'smooth'
      });
    }
  }

  function handleScroll() {
    if (!container.value || isProgrammaticScrolling) return;
    isSticky = isAtBottom(container.value);
  }

  // Watch activeRunId to force scroll when chat loads.
  watch(activeRunId, () => {
    isSticky = true;
    nextTick(scrollToBottom);
  });

  // Watch primitives (not a fresh object literal) so Vue's per-source
  // comparison suppresses the callback — and its forced-layout scroll —
  // when nothing scroll-relevant actually changed.
  watch(
    [
      () => messages.value.length,
      () => messages.value[messages.value.length - 1]?.id,
      () => messages.value[messages.value.length - 1]?.content?.length ?? 0,
      () => messages.value[messages.value.length - 1]?.reasoningContent?.length ?? 0
    ],
    () => {
      if (!container.value) return;

      const lastMsg = messages.value.length > 0 ? messages.value[messages.value.length - 1] : null;
      const isUserMsg = lastMsg && lastMsg.role === 'user';

      nextTick(() => {
        if (isUserMsg) {
          scrollToUserMessage();
          isSticky = true;
        } else if (isSticky) {
          scrollToBottom();
        }
      });
    }
  );

  // Watch container in case it gets mounted/unmounted.
  function removeContainerListeners(el: HTMLElement | null) {
    el?.removeEventListener('scroll', handleScroll);
  }

  function addContainerListeners(el: HTMLElement | null) {
    el?.addEventListener('scroll', handleScroll, { passive: true });
  }

  watch(container, (newVal, oldVal) => {
    removeContainerListeners(oldVal);
    addContainerListeners(newVal);
    if (newVal) {
      isSticky = true;
      nextTick(scrollToBottom);
    }
  });

  // Watch isRunning to auto-scroll on start/stop.
  watch(isRunning, () => {
    if (!container.value) return;
    nextTick(() => {
      if (isSticky) {
        scrollToBottom();
      }
    });
  });

  onBeforeUnmount(() => {
    removeContainerListeners(container.value);
    if (programmaticScrollTimeout !== null) {
      clearTimeout(programmaticScrollTimeout);
    }
  });

  return { scrollToBottom };
}
