import { ref, type Ref } from 'vue';

/**
 * Tiny load + isLoading + try/finally wrapper shared by the list-style
 * composables. The fetcher's null result (getJson's error signal) leaves the
 * current data untouched, matching the previous hand-written loaders.
 */
export function useAsyncResource<T>(fetcher: () => Promise<T | null>, initial: T) {
  const data = ref(initial) as Ref<T>;
  const isLoading = ref(false);

  async function load() {
    isLoading.value = true;
    try {
      const result = await fetcher();
      if (result) data.value = result;
    } finally {
      isLoading.value = false;
    }
  }

  return { data, isLoading, load };
}
