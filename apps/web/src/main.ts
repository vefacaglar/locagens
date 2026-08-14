import { createApp } from 'vue'
import './style.css'
import App from './App.vue'

const deferredFont = document.querySelector<HTMLLinkElement>('link[data-deferred-font]');
deferredFont?.addEventListener('load', () => { deferredFont.media = 'all'; }, { once: true });

document.addEventListener('click', (event) => {
  const anchor = (event.target as Element | null)?.closest<HTMLAnchorElement>('a[href]');
  if (anchor) {
    event.preventDefault();
    try {
      const url = new URL(anchor.href);
      if (url.protocol === 'https:' || url.protocol === 'http:') {
        window.open(url.toString(), '_blank', 'noopener,noreferrer');
      }
    } catch {
      // Invalid and non-web protocols remain inert.
    }
    return;
  }
  const button = (event.target as Element | null)?.closest<HTMLButtonElement>('[data-copy-code="true"]');
  if (!button) return;
  const code = button.closest('.code-block-wrapper')?.querySelector('code')?.textContent || '';
  void navigator.clipboard.writeText(code);
  button.classList.add('copied');
  const copyIcon = button.querySelector<HTMLElement>('.copy-icon');
  const checkIcon = button.querySelector<HTMLElement>('.check-icon');
  if (copyIcon) copyIcon.style.display = 'none';
  if (checkIcon) checkIcon.style.display = 'inline-block';
  window.setTimeout(() => {
    button.classList.remove('copied');
    if (copyIcon) copyIcon.style.display = 'inline-block';
    if (checkIcon) checkIcon.style.display = 'none';
  }, 2000);
});

// Tag the root when running inside the Electron desktop shell so the app can
// adapt its chrome (make header bars draggable, leave room for the macOS
// traffic lights) without affecting the plain browser build.
if (navigator.userAgent.includes('Electron') || (window as any).__LOCAGENS_DESKTOP__) {
  document.documentElement.classList.add('is-desktop')

  // Double-clicking the draggable header bar zooms the window (macOS title-bar
  // behavior), and again restores it. Ignore double-clicks on actual controls.
  document.addEventListener('dblclick', (e) => {
    const target = e.target as HTMLElement
    if (!target.closest('.chat-header, .sidebar-header')) return
    if (target.closest('button, a, input, textarea, select, [role="button"]')) return
    ;(window as any).__LOCAGENS_DESKTOP__?.toggleMaximize?.()
  })
}

createApp(App).mount('#app')

// Global scrollbar visibility handler during active scrolling
const scrollTimeouts = new WeakMap<HTMLElement, ReturnType<typeof setTimeout>>();

window.addEventListener('scroll', (event) => {
  const target = event.target as HTMLElement;
  if (!target || !target.classList || (target as any) === document || (target as any) === window) return;

  target.classList.add('is-scrolling');

  const prevTimeout = scrollTimeouts.get(target);
  if (prevTimeout) {
    clearTimeout(prevTimeout);
  }

  const timeout = setTimeout(() => {
    target.classList.remove('is-scrolling');
    scrollTimeouts.delete(target);
  }, 800); // Hide 800ms after scrolling stops

  scrollTimeouts.set(target, timeout);
}, true); // Capture phase is necessary since scroll events do not bubble
