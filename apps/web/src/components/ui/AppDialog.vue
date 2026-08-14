<script setup lang="ts">
import type { DialogRequest } from '../../composables/useCustomDialog';

defineProps<{
  dialog: DialogRequest | null;
}>();
</script>

<template>
  <Transition name="fade">
    <div v-if="dialog" class="modal-overlay" @click="dialog.resolve(false)">
      <div class="modal-card dialog-modal-card" @click.stop>
        <header class="modal-header">
          <h3>{{ dialog.title || (dialog.type === 'confirm' ? 'Confirmation' : 'Notification') }}</h3>
          <button class="close-modal-btn" @click="dialog.resolve(false)">Close</button>
        </header>
        <div class="modal-body dialog-modal-body">
          <div class="dialog-content-wrapper">
            <div class="dialog-icon-wrap" :class="dialog.type">
              <svg v-if="dialog.type === 'confirm'" xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="10"></circle>
                <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"></path>
                <line x1="12" y1="17" x2="12.01" y2="17"></line>
              </svg>
              <svg v-else xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z"></path>
                <line x1="12" y1="9" x2="12" y2="13"></line>
                <line x1="12" y1="17" x2="12.01" y2="17"></line>
              </svg>
            </div>
            <p class="dialog-message">{{ dialog.message }}</p>
          </div>
        </div>
        <footer class="modal-footer">
          <button v-if="dialog.type === 'confirm'" class="ghost-button" @click="dialog.resolve(false)">
            Cancel
          </button>
          <button class="primary-button" @click="dialog.resolve(true)">
            {{ dialog.type === 'confirm' ? 'Confirm' : 'OK' }}
          </button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.dialog-modal-card {
  width: min(420px, 90%);
}

.dialog-content-wrapper {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 8px 0;
}

.dialog-icon-wrap {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: 50%;
  flex-shrink: 0;
}

.dialog-icon-wrap.confirm {
  background: var(--dialog-confirm-icon-bg);
  color: var(--dialog-confirm-icon-color);
}

.dialog-icon-wrap.alert {
  background: var(--dialog-alert-icon-bg);
  color: var(--dialog-alert-icon-color);
}

.dialog-message {
  margin: 0;
  font-size: 0.95rem;
  line-height: 1.5;
  color: var(--text);
  padding-top: 8px;
}
</style>
