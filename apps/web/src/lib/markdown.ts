import { Marked } from 'marked';
import { COPY_ICON_SVG, CHECK_ICON_SVG } from './icons';
// The core build + explicit grammar registration keeps ~175 unused grammars
// (roughly 70% of the app bundle) out of the build. Unregistered languages
// degrade gracefully to plain escaped code blocks below.
import hljs from 'highlight.js/lib/core';
import javascript from 'highlight.js/lib/languages/javascript';
import typescript from 'highlight.js/lib/languages/typescript';
import python from 'highlight.js/lib/languages/python';
import json from 'highlight.js/lib/languages/json';
import bash from 'highlight.js/lib/languages/bash';
import shell from 'highlight.js/lib/languages/shell';
import xml from 'highlight.js/lib/languages/xml';
import css from 'highlight.js/lib/languages/css';
import sql from 'highlight.js/lib/languages/sql';
import java from 'highlight.js/lib/languages/java';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import go from 'highlight.js/lib/languages/go';
import rust from 'highlight.js/lib/languages/rust';
import yaml from 'highlight.js/lib/languages/yaml';
import markdown from 'highlight.js/lib/languages/markdown';

hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('python', python);
hljs.registerLanguage('json', json);
hljs.registerLanguage('bash', bash);
hljs.registerLanguage('shell', shell);
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('css', css);
hljs.registerLanguage('sql', sql);
hljs.registerLanguage('java', java);
hljs.registerLanguage('c', c);
hljs.registerLanguage('cpp', cpp);
hljs.registerLanguage('go', go);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('yaml', yaml);
hljs.registerLanguage('markdown', markdown);

const marked = new Marked({ gfm: true, breaks: true });

// highlightAuto tries every registered language, so it is by far the most
// expensive path in rendering — and during streaming an unlabeled code block is
// re-detected on every frame (the cache only hits once content stops changing).
// Bound the cost: only auto-detect short blocks, and only against the languages
// that realistically appear in this tool's output.
const AUTO_HIGHLIGHT_MAX_CHARS = 3000;
const AUTO_HIGHLIGHT_LANGUAGES = [
  'javascript', 'typescript', 'python', 'json', 'bash', 'shell',
  'xml', 'css', 'sql', 'java', 'c', 'cpp', 'go', 'rust', 'yaml', 'markdown'
].filter((lang) => hljs.getLanguage(lang));

let codeBlockCounter = 0;
let currentPrefix = '';

marked.use({
  renderer: {
    code(token: { text: string; lang?: string; escaped?: boolean }) {
      const code = token.text;
      const lang = token.lang || '';

      let highlightedCode = '';
      let hasHighlighting = false;

      if (lang && hljs.getLanguage(lang)) {
        try {
          highlightedCode = hljs.highlight(code, { language: lang }).value;
          hasHighlighting = true;
        } catch (e) {
          console.error('highlight.js error:', e);
        }
      } else if (!lang && code.length <= AUTO_HIGHLIGHT_MAX_CHARS) {
        try {
          const result = hljs.highlightAuto(code, AUTO_HIGHLIGHT_LANGUAGES);
          highlightedCode = result.value;
          hasHighlighting = true;
        } catch (e) {
          // ignore
        }
      }

      if (hasHighlighting) {
        highlightedCode = highlightedCode.replace(
          /<span class="hljs-string">(?:&quot;&quot;&quot;|&#x27;&#x27;&#x27;|&#x39;&#x39;&#x39;|&#39;&#39;&#39;)[\s\S]*?(?:&quot;&quot;&quot;|&#x27;&#x27;&#x27;|&#x39;&#x39;&#x39;|&#39;&#39;&#39;)<\/span>/g,
          (match) => match.replace('hljs-string', 'hljs-comment')
        );
      }

      // Escape HTML entities as fallback to prevent rendering issues in raw code blocks.
      if (!hasHighlighting) {
        highlightedCode = code
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;')
          .replace(/"/g, '&quot;')
          .replace(/'/g, '&#039;');
      }

      const displayLang = lang || 'code';
      const blockId = currentPrefix ? `${currentPrefix}-code-${codeBlockCounter++}` : `code-block-${codeBlockCounter++}`;

      return `
        <div class="code-block-wrapper">
          <div class="code-block-header">
            <span class="code-block-lang">${displayLang}</span>
            <button class="code-block-copy-btn" onclick="
              const codeText = this.closest('.code-block-wrapper').querySelector('code').innerText;
              navigator.clipboard.writeText(codeText);
              this.classList.add('copied');
              this.querySelector('.copy-icon').style.display = 'none';
              this.querySelector('.check-icon').style.display = 'inline-block';
              setTimeout(() => {
                this.classList.remove('copied');
                this.querySelector('.copy-icon').style.display = 'inline-block';
                this.querySelector('.check-icon').style.display = 'none';
              }, 2000);
            " title="Copy code">
              ${COPY_ICON_SVG}
              ${CHECK_ICON_SVG.replace('class="check-icon"', 'class="check-icon" style="display: none;"')}
            </button>
          </div>
          <pre id="${blockId}"><code class="hljs language-${displayLang}">${highlightedCode}</code></pre>
        </div>
      `;
    }
  }
});

const markdownCache = new Map<string, { content: string; rendered: string }>();
const MAX_CACHE_SIZE = 500;

/** Renders markdown to HTML, falling back to raw text on parse errors. */
export function renderMarkdown(content: string, idPrefix?: string): string {
  if (!content) return '';

  // One cache slot per idPrefix (message id): a streaming message overwrites its
  // own slot instead of minting a new key per token, which used to flood the
  // FIFO and evict every stable message's rendered HTML.
  const cacheKey = idPrefix || null;
  if (cacheKey) {
    const cached = markdownCache.get(cacheKey);
    if (cached && cached.content === content) {
      return cached.rendered;
    }
  }

  codeBlockCounter = 0;
  currentPrefix = idPrefix || '';
  try {
    const rendered = marked.parse(content) as string;
    if (cacheKey) {
      if (markdownCache.size >= MAX_CACHE_SIZE) {
        const firstKey = markdownCache.keys().next().value;
        if (firstKey !== undefined) {
          markdownCache.delete(firstKey);
        }
      }
      markdownCache.set(cacheKey, { content, rendered });
    }
    return rendered.trimEnd();
  } catch (err) {
    console.error('Markdown parsing error:', err);
    return content;
  }
}

/** Strips internal <plan>...</plan> and <task_list>...</task_list> blocks before displaying assistant text.
 *  Also strips a not-yet-closed block (open tag with no closing tag) so a partial block being
 *  streamed in is hidden instead of rendering raw markdown until generation finishes. */
export function cleanMessageContent(content: string): string {
  return content
    .replace(/<plan>[\s\S]*?<\/plan>/g, '')
    .replace(/<task_list>[\s\S]*?<\/task_list>/g, '')
    .replace(/<plan>[\s\S]*$/g, '')
    .replace(/<task_list>[\s\S]*$/g, '')
    .replace(/<\/?confirm\b[^>]*>/ig, '')
    .trim();
}

/** Parses and cleans up system error messages that might contain raw provider JSON */
export function formatSystemErrorMessage(content: string): string {
  if (!content) return '';
  const jsonMatch = content.match(/(\{[\s\S]*\})/);
  if (jsonMatch) {
    const rawJson = jsonMatch[1];
    try {
      const parsed = JSON.parse(rawJson);
      let cleanMsg = "";
      if (parsed.error) {
        if (typeof parsed.error === "object" && typeof parsed.error.message === "string") {
          cleanMsg = parsed.error.message;
        } else if (typeof parsed.error === "string") {
          cleanMsg = parsed.error;
        }
      } else if (typeof parsed.message === "string") {
        cleanMsg = parsed.message;
      } else if (typeof parsed.msg === "string") {
        cleanMsg = parsed.msg;
      }
      
      if (cleanMsg) {
        return content.replace(rawJson, cleanMsg);
      }
    } catch {
      // ignore
    }
  }
  return content;
}

