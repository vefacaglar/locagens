import { useMediaQuery } from '@vueuse/core';

/** Single source of truth for the app's 760px mobile breakpoint. */
export function useIsMobile() {
  return useMediaQuery('(max-width: 760px)');
}
