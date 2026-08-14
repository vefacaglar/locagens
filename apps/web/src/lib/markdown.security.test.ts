import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

describe('markdown security', () => {
  it('renders provider HTML as inert text', () => {
    const rendered = renderMarkdown('<img src=x onerror="window.__LOCAGENS_DESKTOP__.apiRequest({})">', 'security-test');
    expect(rendered).toContain('&lt;img');
    expect(rendered).not.toContain('<img');
    expect(rendered).not.toContain('onclick=');
  });

  it('does not emit active URLs or code-fence attribute injection', () => {
    const rendered = renderMarkdown('[click](javascript:alert(1))\n\n```\"><img src=x onerror=alert(1)>\ncode\n```', 'x\" onmouseover="alert(1)');
    expect(rendered).not.toContain('javascript:');
    expect(rendered).not.toContain('<img');
    expect(rendered).not.toContain('onmouseover=');
  });
});
