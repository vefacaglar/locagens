/**
 * Shared inline SVG strings for places that render icons into HTML strings
 * (e.g. markdown code-block copy buttons). Vue components should prefer
 * `ui/CopyButton.vue`, which renders the same icons as templates.
 */

export const COPY_ICON_SVG =
  '<svg class="copy-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
  '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>' +
  '<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>' +
  '</svg>';

export const CHECK_ICON_SVG =
  '<svg class="check-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">' +
  '<path d="M20 6 9 17l-5-5"/>' +
  '</svg>';
