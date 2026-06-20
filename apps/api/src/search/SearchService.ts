import type { SearchEngine, SearchSettings } from "@locagens/shared";
import type { ProviderSecretStore } from "../providers/ProviderSecretStore.js";
import type { ResolvedSearchSettings } from "./SearchConfig.js";

export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}

export interface SearchResponse {
  success: boolean;
  source: SearchEngine;
  query: string;
  results: SearchResult[];
  error?: string;
  status?: number;
}

const SEARCH_KEYCHAIN_PREFIX = "macos-keychain:";
const BRAVE_REF = "search:brave";
const GOOGLE_REF = "search:google";
const REQUEST_TIMEOUT_MS = 30_000;

function resolveKey(value: string | undefined, ref: string | undefined, store: ProviderSecretStore): string {
  if (value && value.trim()) return value.trim();
  if (ref?.startsWith(SEARCH_KEYCHAIN_PREFIX)) {
    return store.get(ref) || "";
  }
  return ref ? store.get(ref) || "" : "";
}

function normalizeMaxResults(raw?: number): number {
  return Math.min(Math.max(Math.floor(raw ?? 5), 1), 10);
}

function isAbortError(err: unknown): err is DOMException {
  return err instanceof Error && err.name === "AbortError";
}

async function withTimeout<T>(fn: (signal: AbortSignal) => Promise<T>): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    return await fn(controller.signal);
  } finally {
    clearTimeout(timeout);
  }
}

function createFailureResponse(source: SearchEngine, query: string, error: string, status?: number): SearchResponse {
  return { success: false, source, query, results: [], error, status };
}

export class SearchService {
  constructor(
    private settings: ResolvedSearchSettings,
    private secretStore: ProviderSecretStore
  ) {}

  async search(rawQuery: string, rawMaxResults?: number): Promise<SearchResponse> {
    const query = rawQuery.trim();
    if (!query) {
      return createFailureResponse(this.settings.engine, query, "Missing parameter: query");
    }

    const maxResults = normalizeMaxResults(rawMaxResults);
    const engine = this.settings.engine === "disabled" ? "duckduckgo" : this.settings.engine;

    try {
      if (engine === "brave") {
        const apiKey = resolveKey(this.settings.braveApiKey, this.settings.braveApiKeyRef, this.secretStore);
        if (apiKey) {
          return await searchBrave(query, maxResults, apiKey);
        }
      }

      if (engine === "google") {
        const apiKey = resolveKey(this.settings.googleApiKey, this.settings.googleApiKeyRef, this.secretStore);
        if (apiKey && this.settings.googleSearchEngineId) {
          return await searchGoogle(query, maxResults, apiKey, this.settings.googleSearchEngineId);
        }
      }

      return await searchDuckDuckGo(query, maxResults);
    } catch (err) {
      return createFailureResponse(engine, query, isAbortError(err) ? "Search timed out after 30s." : errorMessage(err));
    }
  }

  static editableSettings(settings: ResolvedSearchSettings, store: ProviderSecretStore): SearchSettings & { hasBraveApiKey: boolean; hasGoogleApiKey: boolean } {
    return {
      engine: settings.engine,
      braveApiKey: "",
      googleApiKey: "",
      googleSearchEngineId: settings.googleSearchEngineId || "",
      hasBraveApiKey: !!resolveKey(settings.braveApiKey, settings.braveApiKeyRef, store),
      hasGoogleApiKey: !!resolveKey(settings.googleApiKey, settings.googleApiKeyRef, store)
    };
  }
}

async function searchDuckDuckGo(query: string, maxResults: number): Promise<SearchResponse> {
  const searchUrl = `https://html.duckduckgo.com/html/?${new URLSearchParams({ q: query }).toString()}`;

  try {
    const response = await withTimeout((signal) => fetch(searchUrl, {
      method: "GET",
      redirect: "follow",
      signal,
      headers: { "User-Agent": "Locagens/1.0 (+local workspace assistant)" }
    }));

    const body = await response.text();
    const results = parseDuckDuckGoResults(body).slice(0, maxResults);
    return {
      success: response.ok,
      source: "duckduckgo",
      query,
      status: response.status,
      results
    };
  } catch (err) {
    return createFailureResponse("duckduckgo", query, isAbortError(err) ? "Search timed out after 30s." : errorMessage(err));
  }
}

function parseDuckDuckGoResults(html: string): SearchResult[] {
  const results: SearchResult[] = [];
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

interface BraveSearchPayload {
  web?: { results?: unknown[] };
}

async function searchBrave(query: string, maxResults: number, apiKey: string): Promise<SearchResponse> {
  const url = new URL("https://api.search.brave.com/res/v1/web/search");
  url.searchParams.set("q", query);
  url.searchParams.set("count", String(Math.min(maxResults, 20)));
  url.searchParams.set("offset", "0");

  try {
    const response = await withTimeout((signal) => fetch(url.toString(), {
      method: "GET",
      signal,
      headers: {
        "Accept": "application/json",
        "Accept-Encoding": "gzip",
        "X-Subscription-Token": apiKey
      }
    }));

    if (!response.ok) {
      const text = await response.text().catch(() => "Unknown error");
      return createFailureResponse("brave", query, `Brave API returned ${response.status}: ${text}`, response.status);
    }

    const data = (await response.json()) as BraveSearchPayload;
    const rawResults = data.web?.results ?? [];
    const results = rawResults
      .map(toBraveSearchResult)
      .filter((r) => Boolean(r.title && r.url));

    return { success: true, source: "brave", query, status: response.status, results: results.slice(0, maxResults) };
  } catch (err) {
    return createFailureResponse("brave", query, isAbortError(err) ? "Search timed out after 30s." : errorMessage(err));
  }
}

function toBraveSearchResult(raw: unknown): SearchResult {
  const r = raw as Record<string, unknown>;
  return {
    title: String(r.title ?? ""),
    url: String(r.url ?? ""),
    snippet: String(r.description ?? "")
  };
}

interface GoogleSearchPayload {
  items?: unknown[];
}

async function searchGoogle(query: string, maxResults: number, apiKey: string, searchEngineId: string): Promise<SearchResponse> {
  const url = new URL("https://www.googleapis.com/customsearch/v1");
  url.searchParams.set("key", apiKey);
  url.searchParams.set("cx", searchEngineId);
  url.searchParams.set("q", query);
  url.searchParams.set("num", String(Math.min(maxResults, 10)));

  try {
    const response = await withTimeout((signal) => fetch(url.toString(), {
      method: "GET",
      signal,
      headers: { "Accept": "application/json" }
    }));

    if (!response.ok) {
      const text = await response.text().catch(() => "Unknown error");
      return createFailureResponse("google", query, `Google API returned ${response.status}: ${text}`, response.status);
    }

    const data = (await response.json()) as GoogleSearchPayload;
    const rawResults = data.items ?? [];
    const results = rawResults
      .map(toGoogleSearchResult)
      .filter((r) => Boolean(r.title && r.url));

    return { success: true, source: "google", query, status: response.status, results: results.slice(0, maxResults) };
  } catch (err) {
    return createFailureResponse("google", query, isAbortError(err) ? "Search timed out after 30s." : errorMessage(err));
  }
}

function toGoogleSearchResult(raw: unknown): SearchResult {
  const r = raw as Record<string, unknown>;
  return {
    title: String(r.title ?? ""),
    url: String(r.link ?? ""),
    snippet: String(r.snippet ?? "")
  };
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

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return "Search failed.";
}
