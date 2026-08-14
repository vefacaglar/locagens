import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { REASONING_EFFORTS, type ModelOption } from './useComposerSettings';
import { STORAGE_KEYS } from '../lib/storageKeys';

/**
 * State and behavior of the composer's model-picker dropdown: search,
 * per-provider grouping/collapsing, favorites, keyboard navigation, and
 * left/right reasoning-effort cycling. The host component keeps the menu
 * open/close state and the surrounding template; this composable owns
 * everything inside the list.
 */

export interface ProviderGroup {
  providerId: string;
  providerDisplayName: string;
  models: ModelOption[];
}

export interface NavigableItem {
  type: 'action' | 'model';
  id: string;
  label: string;
  option?: ModelOption;
}

/** The callbacks the picker needs from its host component. */
export interface ModelPickerHost {
  modelOptions: () => ModelOption[];
  selectedModel: () => string;
  selectedReasoningEffort: () => string;
  /** Commit a model change WITHOUT closing the menu (used while cycling). */
  commitModel: (value: string) => void;
  /** Commit a reasoning-effort change (menu stays open). */
  commitReasoningEffort: (value: string) => void;
}

export function useModelPicker(host: ModelPickerHost) {
  const searchQuery = ref('');
  const searchInputRef = ref<HTMLInputElement | null>(null);
  const scrollContainerRef = ref<HTMLElement | null>(null);
  const collapsedProviders = ref<Record<string, boolean>>({});
  const favoriteModels = ref<string[]>([]);

  onMounted(() => {
    const stored = localStorage.getItem(STORAGE_KEYS.favoriteModels);
    if (stored) {
      try {
        favoriteModels.value = JSON.parse(stored);
      } catch (e) {
        console.error(e);
      }
    }
  });

  function toggleFavorite(val: string) {
    const idx = favoriteModels.value.indexOf(val);
    if (idx > -1) {
      favoriteModels.value.splice(idx, 1);
    } else {
      favoriteModels.value.push(val);
    }
    localStorage.setItem(STORAGE_KEYS.favoriteModels, JSON.stringify(favoriteModels.value));
  }

  function isStarred(val: string): boolean {
    return favoriteModels.value.includes(val);
  }

  function toggleProviderCollapse(providerId: string) {
    collapsedProviders.value[providerId] = !collapsedProviders.value[providerId];
  }

  function isProviderCollapsed(providerId: string): boolean {
    return !!collapsedProviders.value[providerId];
  }

  const groupedModels = computed<ProviderGroup[]>(() => {
    const groups: Record<string, ProviderGroup> = {};
    const query = searchQuery.value.trim().toLowerCase();

    const filtered = host.modelOptions().filter(opt => {
      if (!query) return true;
      return (
        opt.model.toLowerCase().includes(query) ||
        opt.label.toLowerCase().includes(query) ||
        opt.providerId.toLowerCase().includes(query)
      );
    });

    for (const option of filtered) {
      const providerId = option.providerId;
      const providerDisplayName = option.label.split(' / ')[0] || providerId;
      if (!groups[providerId]) {
        groups[providerId] = {
          providerId,
          providerDisplayName,
          models: []
        };
      }
      groups[providerId].models.push(option);
    }

    // Also auto-expand providers if there is search query
    if (query) {
      for (const pid of Object.keys(groups)) {
        collapsedProviders.value[pid] = false;
      }
    }

    return Object.values(groups).sort((a, b) => {
      const nameA = a.providerDisplayName.toLowerCase();
      const nameB = b.providerDisplayName.toLowerCase();
      if (nameA.includes('deepseek') && !nameB.includes('deepseek')) return -1;
      if (!nameA.includes('deepseek') && nameB.includes('deepseek')) return 1;
      if (nameA.includes('opencode') && !nameB.includes('opencode')) return -1;
      if (!nameA.includes('opencode') && nameB.includes('opencode')) return 1;
      return a.providerDisplayName.localeCompare(b.providerDisplayName);
    });
  });

  const navigableItems = computed<NavigableItem[]>(() => {
    const items: NavigableItem[] = [];
    items.push({
      type: 'action',
      id: 'add-provider',
      label: '+ Add new provider'
    });

    for (const group of groupedModels.value) {
      if (!isProviderCollapsed(group.providerId)) {
        for (const model of group.models) {
          items.push({
            type: 'model',
            id: model.value,
            label: model.model,
            option: model
          });
        }
      }
    }
    return items;
  });

  const focusedIndex = ref(0);
  const focusedItem = computed<NavigableItem | null>(() => navigableItems.value[focusedIndex.value] ?? null);

  function isItemFocused(id: string): boolean {
    const item = navigableItems.value[focusedIndex.value];
    return item ? item.id === id : false;
  }

  function setFocusedItem(id: string) {
    const idx = navigableItems.value.findIndex(item => item.id === id);
    if (idx > -1) {
      focusedIndex.value = idx;
    }
  }

  watch(searchQuery, () => {
    focusedIndex.value = navigableItems.value.length > 1 ? 1 : 0;
  });

  function scrollToFocusedItem() {
    nextTick(() => {
      const container = scrollContainerRef.value;
      if (!container) return;
      const focusedEl = container.querySelector('.focused');
      if (!focusedEl) return;

      const containerTop = container.scrollTop;
      const containerBottom = containerTop + container.clientHeight;
      const elemTop = (focusedEl as HTMLElement).offsetTop;
      const elemBottom = elemTop + (focusedEl as HTMLElement).offsetHeight;

      if (elemTop < containerTop) {
        container.scrollTop = elemTop;
      } else if (elemBottom > containerBottom) {
        container.scrollTop = elemBottom - container.clientHeight;
      }
    });
  }

  /** Arrow-key navigation: moves the focus ring up/down through the list. */
  function moveFocus(delta: number) {
    if (navigableItems.value.length === 0) return;
    focusedIndex.value = (focusedIndex.value + delta + navigableItems.value.length) % navigableItems.value.length;
    scrollToFocusedItem();
  }

  function cycleReasoningEffort(option: ModelOption, direction: number) {
    if (!option.reasoningOptions || option.reasoningOptions.length === 0) return;

    const options = [
      { id: 'default', label: 'Default' },
      ...option.reasoningOptions.map(opt => ({
        id: opt.id,
        label: opt.label || REASONING_EFFORTS.find(x => x.id === opt.id)?.label || opt.id
      }))
    ];

    if (host.selectedModel() !== option.value) {
      host.commitModel(option.value);
    }

    const currentIndex = options.findIndex(x => x.id === host.selectedReasoningEffort());
    const nextIndex = (Math.max(0, currentIndex) + direction + options.length) % options.length;
    host.commitReasoningEffort(options[nextIndex].id);
  }

  /** Left/right keyboard cycling on whichever model row is focused. */
  function cycleReasoningOnFocused(direction: number) {
    const item = focusedItem.value;
    if (item && item.type === 'model' && item.option) {
      cycleReasoningEffort(item.option, direction);
    }
  }

  /** Reset for a fresh menu open: clear search, focus it, jump to the active model. */
  async function onMenuOpened() {
    searchQuery.value = '';
    await nextTick();
    searchInputRef.value?.focus();

    const activeIdx = navigableItems.value.findIndex(item => item.type === 'model' && item.id === host.selectedModel());
    if (activeIdx > -1) {
      focusedIndex.value = activeIdx;
      scrollToFocusedItem();
    } else {
      focusedIndex.value = navigableItems.value.length > 1 ? 1 : 0;
    }
  }

  return {
    searchQuery,
    searchInputRef,
    scrollContainerRef,
    groupedModels,
    focusedItem,
    isItemFocused,
    setFocusedItem,
    moveFocus,
    toggleFavorite,
    isStarred,
    toggleProviderCollapse,
    isProviderCollapsed,
    cycleReasoningOnFocused,
    onMenuOpened
  };
}
