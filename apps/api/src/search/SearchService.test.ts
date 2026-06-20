import test from "node:test";
import assert from "node:assert";
import { SearchService } from "./SearchService.js";
import { InMemoryProviderSecretStore } from "../providers/ProviderSecretStore.js";

const originalFetch = globalThis.fetch;

function mockFetch(handler: (url: string, options?: RequestInit) => Promise<{ ok: boolean; status: number; text?: () => Promise<string>; json?: () => Promise<unknown> }>) {
  globalThis.fetch = async (url: unknown, options?: unknown) => handler(String(url), options as RequestInit | undefined) as any;
}

// ResolvedSearchSettings types braveApiKey/googleApiKey as `never` to prevent
// plain-text key leakage in production configs.  Tests intentionally pass
// inline keys, so we suppress the type check with a narrow cast.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const testCfg = (cfg: Record<string, unknown>) => cfg as any;

test("SearchService - uses DuckDuckGo fallback by default", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  mockFetch(async (url) => {
    assert.ok(url.includes("duckduckgo.com"));
    return {
      ok: true,
      status: 200,
      text: async () => `
        <div class="result">
          <a class="result__a" href="https://example.com">Example</a>
          <a class="result__snippet">An example snippet.</a>
        </div>
        </body>
      `
    };
  });

  const service = new SearchService(testCfg({ engine: "duckduckgo" }), new InMemoryProviderSecretStore());
  const res = await service.search("example");

  assert.strictEqual(res.success, true);
  assert.strictEqual(res.source, "duckduckgo");
  assert.strictEqual(res.results.length, 1);
  assert.strictEqual(res.results[0]?.title, "Example");
  assert.strictEqual(res.results[0]?.url, "https://example.com/");
});

test("SearchService - returns empty results for unparseable DuckDuckGo HTML", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  mockFetch(async () => ({ ok: true, status: 200, text: async () => "<html><body>no results</body></html>" }));

  const service = new SearchService(testCfg({ engine: "duckduckgo" }), new InMemoryProviderSecretStore());
  const res = await service.search("example");

  assert.strictEqual(res.success, true);
  assert.strictEqual(res.results.length, 0);
});

test("SearchService - calls Brave API when key is provided", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let fetchedUrl: string | null = null;
  mockFetch(async (url, options) => {
    fetchedUrl = url;
    const headers = options?.headers as Record<string, string> | undefined;
    assert.strictEqual(headers?.["X-Subscription-Token"], "brave-key");
    return {
      ok: true,
      status: 200,
      json: async () => ({
        web: {
          results: [
            { title: "Brave Result", url: "https://brave.com/result", description: "From Brave" }
          ]
        }
      })
    };
  });

  const service = new SearchService(testCfg({ engine: "brave", braveApiKey: "brave-key" }), new InMemoryProviderSecretStore());
  const res = await service.search("test");

  assert.ok(String(fetchedUrl).includes("api.search.brave.com"));
  assert.strictEqual(res.success, true);
  assert.strictEqual(res.source, "brave");
  assert.strictEqual(res.results[0]?.title, "Brave Result");
});

test("SearchService - Brave API error surfaces status and message", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  mockFetch(async () => ({ ok: false, status: 401, text: async () => "Unauthorized" }));

  const service = new SearchService(testCfg({ engine: "brave", braveApiKey: "brave-key" }), new InMemoryProviderSecretStore());
  const res = await service.search("test");

  assert.strictEqual(res.success, false);
  assert.strictEqual(res.status, 401);
  assert.ok(typeof res.error === "string" && res.error.includes("Brave API returned 401"));
});

test("SearchService - resolves Brave key from secret store ref", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  const store = new InMemoryProviderSecretStore();
  const ref = store.set("search:brave", "stored-brave-key");

  mockFetch(async (_url, options) => {
    const headers = options?.headers as Record<string, string> | undefined;
    assert.strictEqual(headers?.["X-Subscription-Token"], "stored-brave-key");
    return {
      ok: true,
      status: 200,
      json: async () => ({ web: { results: [] } })
    };
  });

  const service = new SearchService(testCfg({ engine: "brave", braveApiKeyRef: ref }), store);
  const res = await service.search("x");
  assert.strictEqual(res.success, true);
  assert.strictEqual(res.source, "brave");
});

test("SearchService - falls back to DuckDuckGo when Brave key is missing", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  mockFetch(async (url) => {
    assert.ok(url.includes("duckduckgo.com"));
    return { ok: true, status: 200, text: async () => "" };
  });

  const service = new SearchService(testCfg({ engine: "brave" }), new InMemoryProviderSecretStore());
  const res = await service.search("x");

  assert.strictEqual(res.success, true);
  assert.strictEqual(res.source, "duckduckgo");
});

test("SearchService - calls Google API when key and cx are provided", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let fetchedUrl: string | null = null;
  mockFetch(async (url) => {
    fetchedUrl = url;
    return {
      ok: true,
      status: 200,
      json: async () => ({
        items: [
          { title: "Google Result", link: "https://google.com/result", snippet: "From Google" }
        ]
      })
    };
  });

  const service = new SearchService(testCfg({
    engine: "google",
    googleApiKey: "google-key",
    googleSearchEngineId: "cx-123"
  }), new InMemoryProviderSecretStore());

  const res = await service.search("test");

  assert.ok(String(fetchedUrl).includes("googleapis.com"));
  assert.ok(String(fetchedUrl).includes("key=google-key"));
  assert.ok(String(fetchedUrl).includes("cx=cx-123"));
  assert.strictEqual(res.success, true);
  assert.strictEqual(res.source, "google");
  assert.strictEqual(res.results[0]?.title, "Google Result");
});

test("SearchService - Google API error surfaces status and message", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  mockFetch(async () => ({ ok: false, status: 400, text: async () => "Bad Request" }));

  const service = new SearchService(testCfg({
    engine: "google",
    googleApiKey: "google-key",
    googleSearchEngineId: "cx-123"
  }), new InMemoryProviderSecretStore());

  const res = await service.search("test");

  assert.strictEqual(res.success, false);
  assert.strictEqual(res.status, 400);
  assert.ok(typeof res.error === "string" && res.error.includes("Google API returned 400"));
});

test("SearchService - treats disabled engine as DuckDuckGo", async (t) => {
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  mockFetch(async (url) => {
    assert.ok(url.includes("duckduckgo.com"));
    return { ok: true, status: 200, text: async () => "" };
  });

  const service = new SearchService(testCfg({ engine: "disabled" }), new InMemoryProviderSecretStore());
  const res = await service.search("hello");

  assert.strictEqual(res.success, true);
  assert.strictEqual(res.source, "duckduckgo");
});

test("SearchService - returns error for empty query", async () => {
  const service = new SearchService(testCfg({ engine: "duckduckgo" }), new InMemoryProviderSecretStore());
  const res = await service.search("  ");
  assert.strictEqual(res.success, false);
  assert.ok(typeof res.error === "string" && res.error.includes("Missing parameter"));
});

test("SearchService - editableSettings hides raw keys", () => {
  const store = new InMemoryProviderSecretStore();
  store.set("search:brave", "secret-brave");

  const settings = SearchService.editableSettings(testCfg({ engine: "brave", braveApiKeyRef: "memory:search:brave" }), store);

  assert.strictEqual(settings.engine, "brave");
  assert.strictEqual(settings.braveApiKey, "");
  assert.strictEqual(settings.hasBraveApiKey, true);
  assert.strictEqual(settings.hasGoogleApiKey, false);
});
