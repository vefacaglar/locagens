import path from "node:path";
import type { Run, ToolCall } from "@locagens/shared";
import { truncateOutput } from "./pathGuards.js";
import { commandScansOutsideWorkspace } from "./permissionPreview.js";
import { executeWorkspaceTool } from "./fileToolExecutor.js";
import { safeFetchText } from "../../security/SafeHttpClient.js";
import { commandSandbox, normalizeCommandTimeout, normalizeNetworkDomains } from "../../security/CommandSandbox.js";
import { SymbolIndexer } from "../symbols/index.js";

/**
 * The async tools (shell + network) and the orchestrator's execution entry
 * point. run_command / fetch_url / search_web are inherently asynchronous, so
 * the orchestrator always executes tools through executeWorkspaceToolAsync;
 * synchronous filesystem tools fall through to executeWorkspaceTool.
 */

/**
 * Async variant of executeWorkspaceTool. Identical for synchronous tools, but
 * handles network tools by awaiting the HTTP request. The orchestrator should
 * call this so web fetches resolve.
 */
export async function executeWorkspaceToolAsync(run: Run, toolCall: ToolCall): Promise<string> {
  if (toolCall.function.name === "search_symbols") {
    try {
      const args = JSON.parse(toolCall.function.arguments);
      const baseDir = path.resolve(run.projectPath || process.cwd());
      const indexer = new SymbolIndexer();
      const matches = await indexer.search(
        baseDir,
        typeof args.query === "string" ? args.query : "",
        args.kind
      );
      return JSON.stringify({ success: true, count: matches.length, symbols: matches });
    } catch (err: any) {
      return JSON.stringify({ success: false, error: err.message });
    }
  }
  if (toolCall.function.name === "run_command") {
    try {
      const args = JSON.parse(toolCall.function.arguments);
      const baseDir = path.resolve(run.projectPath || process.cwd());
      return await runShellCommand(
        baseDir,
        typeof args.command === "string" ? args.command : "",
        args.network_domains,
        normalizeCommandTimeout(args.timeout_ms)
      );
    } catch (err: any) {
      return JSON.stringify({ success: false, error: err.message });
    }
  }
  if (toolCall.function.name !== "fetch_url") {
    if (toolCall.function.name === "search_web") {
      try {
        const args = JSON.parse(toolCall.function.arguments);
        return await searchWeb(
          typeof args.query === "string" ? args.query : "",
          typeof args.max_results === "number" ? args.max_results : undefined
        );
      } catch (err: any) {
        return JSON.stringify({ success: false, error: err.message });
      }
    }
    return executeWorkspaceTool(run, toolCall);
  }
  try {
    const args = JSON.parse(toolCall.function.arguments);
    return await fetchUrl(typeof args.url === "string" ? args.url : "");
  } catch (err: any) {
    return JSON.stringify({ success: false, error: err.message });
  }
}

/** Runs shell commands without blocking the orchestrator's event loop. */
async function runShellCommand(baseDir: string, rawCommand: string, rawDomains: unknown, timeoutMs: number): Promise<string> {
  const command = rawCommand.trim();
  if (command === "") {
    return JSON.stringify({ success: false, error: "Missing parameter: command" });
  }
  if (commandScansOutsideWorkspace(command)) {
    return JSON.stringify({
      success: false,
      error: "Refusing to scan outside the workspace. Search project-relative paths, or check installed tools with direct version/path commands."
    });
  }
  const domains = normalizeNetworkDomains(rawDomains);
  return JSON.stringify(await commandSandbox.run(baseDir, command, domains, timeoutMs));
}

/** Fetches an http(s) URL and returns its text body (truncated). */
async function fetchUrl(rawUrl: string): Promise<string> {
  const url = rawUrl.trim();
  if (url === "") {
    return JSON.stringify({ success: false, error: "Missing parameter: url" });
  }
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return JSON.stringify({ success: false, error: `Invalid URL: ${url}` });
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return JSON.stringify({ success: false, error: "Only http and https URLs are allowed." });
  }

  try {
    const response = await safeFetchText(url);
    return JSON.stringify({
      success: response.ok,
      status: response.status,
      contentType: response.contentType,
      finalUrl: response.finalUrl,
      content: truncateOutput(response.body)
    });
  } catch (err: any) {
    return JSON.stringify({ success: false, error: err?.message ?? "Fetch failed." });
  }
}

async function searchWeb(rawQuery: string, rawMaxResults?: number): Promise<string> {
  const query = rawQuery.trim();
  if (query === "") {
    return JSON.stringify({ success: false, error: "Missing parameter: query" });
  }
  const maxResults = Math.min(Math.max(Math.floor(rawMaxResults ?? 5), 1), 10);
  const searchUrl = `https://html.duckduckgo.com/html/?${new URLSearchParams({ q: query }).toString()}`;

  try {
    const response = await safeFetchText(searchUrl);
    const results = parseDuckDuckGoResults(response.body).slice(0, maxResults);
    return JSON.stringify({
      success: response.ok,
      status: response.status,
      query,
      source: "duckduckgo_html",
      results
    });
  } catch (err: any) {
    return JSON.stringify({ success: false, error: err?.message ?? "Search failed." });
  }
}

function parseDuckDuckGoResults(html: string): Array<{ title: string; url: string; snippet: string }> {
  const results: Array<{ title: string; url: string; snippet: string }> = [];
  const blocks = html.match(/<div class="result[\s\S]*?(?=<div class="result|<\/body>)/g) ?? [];
  for (const block of blocks) {
    const linkMatch = block.match(/<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/);
    if (!linkMatch) continue;
    const url = normalizeSearchResultUrl(decodeHtml(linkMatch[1]));
    const title = stripHtml(linkMatch[2]);
    if (!url || !title) continue;
    const snippetMatch = block.match(/<a[^>]*class="result__snippet"[^>]*>([\s\S]*?)<\/a>|<div[^>]*class="result__snippet"[^>]*>([\s\S]*?)<\/div>/);
    results.push({
      title,
      url,
      snippet: stripHtml(snippetMatch?.[1] ?? snippetMatch?.[2] ?? "")
    });
  }
  return results;
}

function normalizeSearchResultUrl(rawUrl: string): string {
  try {
    const parsed = new URL(rawUrl, "https://duckduckgo.com");
    const redirected = parsed.searchParams.get("uddg");
    return redirected ? decodeURIComponent(redirected) : parsed.toString();
  } catch {
    return rawUrl;
  }
}

function stripHtml(value: string): string {
  return decodeHtml(value.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim());
}

function decodeHtml(value: string): string {
  return value
    .replace(/&#(\d+);/g, (_m, code) => String.fromCharCode(Number(code)))
    .replace(/&#x([0-9a-f]+);/gi, (_m, code) => String.fromCharCode(Number.parseInt(code, 16)))
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">");
}
