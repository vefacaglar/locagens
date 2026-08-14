import { ref } from 'vue';

/**
 * Keyed expand/collapse state shared by the accordion-style components.
 * Keys with no explicit entry fall back to `defaultOpen`.
 */
export function useExpandedMap<K extends string | number = string>(defaultOpen = false) {
  const map = ref<Record<K, boolean>>({} as Record<K, boolean>);

  function isOpen(key: K): boolean {
    return map.value[key] ?? defaultOpen;
  }

  function toggle(key: K) {
    map.value[key] = !isOpen(key);
  }

  function set(key: K, value: boolean) {
    map.value[key] = value;
  }

  return { map, isOpen, toggle, set };
}
