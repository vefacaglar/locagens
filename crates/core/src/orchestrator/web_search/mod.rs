//! `web_search` tool: a local web search that works with every model
//! provider (Codex uses OpenAI's hosted tool, which only OpenAI models get).
//!
//! Backends, first configured wins: Tavily (`tavily` / `TAVILY_API_KEY`),
//! Brave Search (`brave-search` / `BRAVE_API_KEY`), Exa (`exa` /
//! `EXA_API_KEY`). Keys resolve like model-provider keys (`auth.json`
//! literal, `!command`, `$ENV_VAR`, or the variable). Without any key the
//! keyless DuckDuckGo HTML fallback ([`duckduckgo`]) answers. Only these
//! fixed hosts are contacted; the tool returns a numbered list (title, URL,
//! snippet) and the model reads pages with `fetch_url`. Permission-gated
//! like `fetch_url` (see `tools::needs_permission`).

mod duckduckgo;
mod keyed_backends;

use crate::pi_providers::PiProviderManager;
use serde_json::{json, Value};

pub const TOOL_NAME: &str = "web_search";

/// Default and largest number of results one call returns.
const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 10;
/// Byte budgets per result (cut on char boundaries).
const TITLE_BYTES: usize = 200;
const URL_BYTES: usize = 500;
const SNIPPET_BYTES: usize = 400;
/// Bytes of a backend response read before it stops.
const RESPONSE_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Every backend request gives up after this long.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn tool_def() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Search the web and return a numbered list of results (title, URL, snippet). Read a result with fetch_url. Uses Tavily, Brave Search or Exa when an API key is configured (TAVILY_API_KEY / BRAVE_API_KEY / EXA_API_KEY or `lcgn auth login`), otherwise a keyless DuckDuckGo fallback with weaker results.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query." },
                    "max_results": { "type": "integer", "description": "Number of results, 1..10 (default 5)." }
                },
                "required": ["query"]
            }
        }
    })
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Which service answers a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackend {
    Tavily,
    BraveSearch,
    Exa,
    DuckDuckGo,
}

/// Keyed backends in the order they are tried.
const KEYED_BACKENDS: [SearchBackend; 3] = [SearchBackend::Tavily, SearchBackend::BraveSearch, SearchBackend::Exa];

impl SearchBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tavily => "Tavily",
            Self::BraveSearch => "Brave Search",
            Self::Exa => "Exa",
            Self::DuckDuckGo => "DuckDuckGo",
        }
    }

    /// The `auth.json` / search-provider id of a keyed backend.
    fn provider_id(self) -> Option<&'static str> {
        match self {
            Self::Tavily => Some("tavily"),
            Self::BraveSearch => Some("brave-search"),
            Self::Exa => Some("exa"),
            Self::DuckDuckGo => None,
        }
    }
}

/// The first keyed backend that has a key, else DuckDuckGo (no key).
/// `keys` lists each keyed backend with its resolved key, in try order.
pub fn select_backend(keys: Vec<(SearchBackend, Option<String>)>) -> (SearchBackend, Option<String>) {
    keys.into_iter()
        .find_map(|(backend, key)| key.filter(|key| !key.trim().is_empty()).map(|key| (backend, Some(key))))
        .unwrap_or((SearchBackend::DuckDuckGo, None))
}

/// `max_results` clamped to 1..=10 (default 5).
pub fn clamp_max_results(args: &Value) -> usize {
    match args.get("max_results").and_then(Value::as_i64) {
        Some(count) => count.clamp(1, MAX_RESULTS as i64) as usize,
        None => DEFAULT_RESULTS,
    }
}

/// Runs the tool: picks the backend, queries it, formats the hits.
pub async fn execute(pi: &PiProviderManager, args: &Value) -> Result<String, String> {
    let query = args.get("query").and_then(Value::as_str).unwrap_or_default().trim();
    if query.is_empty() {
        return Err("web_search: query is empty.".to_string());
    }
    let max_results = clamp_max_results(args);
    let mut keys = Vec::new();
    for backend in KEYED_BACKENDS {
        let key = match backend.provider_id() {
            Some(id) => pi.resolve_api_key(id, None).await,
            None => None,
        };
        keys.push((backend, key));
    }
    let (backend, key) = select_backend(keys);
    let client = http_client()?;
    let mut hits = match (backend, key) {
        (SearchBackend::DuckDuckGo, _) | (_, None) => duckduckgo::search(&client, query).await?,
        (keyed, Some(key)) => keyed_backends::search(&client, keyed, &key, query, max_results).await?,
    };
    hits.truncate(max_results);
    Ok(format_results(query, backend, &hits))
}

/// The client every backend request uses: fetch_url's conventions (30 s
/// timeout, a user agent without personal data) and no redirects, so a key
/// header never follows a redirect to another origin.
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("Locagens/1.0")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| format!("web_search: {err}"))
}

/// Sends a request and returns the body of a 2xx answer. `secret` (the API
/// key) is masked in any error text.
async fn send_for_body(backend: SearchBackend, request: reqwest::RequestBuilder, secret: Option<&str>) -> Result<String, String> {
    let name = backend.label();
    let response = request.send().await.map_err(|err| mask(&format!("web_search: {name} request failed: {err}"), secret))?;
    let status = response.status();
    let (bytes, _cut) = super::tools::read_capped_body(response, RESPONSE_MAX_BYTES).await;
    let body = String::from_utf8_lossy(&bytes).to_string();
    if !status.is_success() {
        let detail = plain_text(locagens_protocol::text::prefix_at_char_boundary(&body, 300));
        return Err(mask(&format!("web_search: {name} returned HTTP {status}: {detail}"), secret));
    }
    Ok(body)
}

fn mask(text: &str, secret: Option<&str>) -> String {
    match secret.filter(|secret| !secret.is_empty()) {
        Some(secret) => text.replace(secret, "***"),
        None => text.to_string(),
    }
}

/// The numbered list the model sees.
pub fn format_results(query: &str, backend: SearchBackend, hits: &[SearchHit]) -> String {
    let mut out = format!("Web search results for \"{query}\" (via {}):\n", backend.label());
    if backend == SearchBackend::DuckDuckGo {
        out.push_str("(keyless DuckDuckGo fallback; a TAVILY_API_KEY, BRAVE_API_KEY or EXA_API_KEY gives better results)\n");
    }
    if hits.is_empty() {
        out.push_str("\nNo results.");
        return out;
    }
    for (index, hit) in hits.iter().enumerate() {
        let cut = locagens_protocol::text::prefix_at_char_boundary;
        out.push_str(&format!("\n{}. {}\n   {}\n", index + 1, cut(&hit.title, TITLE_BYTES), cut(&hit.url, URL_BYTES)));
        let snippet = cut(&hit.snippet, SNIPPET_BYTES);
        if !snippet.is_empty() {
            out.push_str(&format!("   {snippet}\n"));
        }
    }
    out.push_str("\nUse fetch_url to read a result.");
    out
}

/// Text of an HTML fragment: highlight tags (`<b>`, `<strong>` …) dropped,
/// other tags turned into spaces, common entities decoded, whitespace
/// collapsed.
pub fn plain_text(html: &str) -> String {
    let replace = |text: &str, pattern: &str, with: &str| {
        regex::Regex::new(pattern)
            .map(|tags| tags.replace_all(text, with).to_string())
            .unwrap_or_else(|_| text.to_string())
    };
    let unhighlighted = replace(html, r"(?i)</?(b|strong|em|i|mark)>", "");
    let without_tags = replace(&unhighlighted, r"<[^>]*>", " ");
    decode_entities(&without_tags).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decodes named (`&amp;` …) and numeric (`&#39;`, `&#x27;`) entities.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest.find(';').filter(|end| *end <= 10).and_then(|end| Some((entity_char(&rest[1..end])?, end)));
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity_char(name: &str) -> Option<char> {
    let numeric = |digits: &str, radix: u32| u32::from_str_radix(digits, radix).ok().and_then(char::from_u32);
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => match name.strip_prefix('#') {
            Some(hex) if hex.starts_with(['x', 'X']) => numeric(&hex[1..], 16),
            Some(decimal) => numeric(decimal, 10),
            None => None,
        },
    }
}

#[cfg(test)]
mod tests;
