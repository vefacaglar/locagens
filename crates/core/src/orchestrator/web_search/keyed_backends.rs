//! The API-key search backends: Tavily, Brave Search and Exa. Each has a
//! fixed endpoint, one request shape and one response parser.

use super::{plain_text, send_for_body, SearchBackend, SearchHit};
use serde_json::{json, Value};

const TAVILY_URL: &str = "https://api.tavily.com/search";
const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const EXA_URL: &str = "https://api.exa.ai/search";
/// Characters of page text Exa returns per result (the snippet).
const EXA_TEXT_CHARS: usize = 500;

/// Queries one keyed backend and parses its answer.
pub(super) async fn search(
    client: &reqwest::Client,
    backend: SearchBackend,
    key: &str,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchHit>, String> {
    let request = match backend {
        SearchBackend::Tavily => client
            .post(TAVILY_URL)
            .bearer_auth(key)
            .json(&json!({ "query": query, "max_results": max_results, "search_depth": "basic" })),
        SearchBackend::BraveSearch => client
            .get(BRAVE_URL)
            .query(&[("q", query), ("count", &max_results.to_string())])
            .header("Accept", "application/json")
            .header("X-Subscription-Token", key),
        SearchBackend::Exa => client.post(EXA_URL).header("x-api-key", key).json(&json!({
            "query": query,
            "numResults": max_results,
            "contents": { "text": { "maxCharacters": EXA_TEXT_CHARS } }
        })),
        SearchBackend::DuckDuckGo => return Err("web_search: DuckDuckGo takes no key.".to_string()),
    };
    let body = send_for_body(backend, request, Some(key)).await?;
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|err| format!("web_search: {} sent an unreadable answer: {err}", backend.label()))?;
    Ok(match backend {
        SearchBackend::Tavily => parse_tavily(&parsed),
        SearchBackend::BraveSearch => parse_brave(&parsed),
        _ => parse_exa(&parsed),
    })
}

/// Tavily: `{ "results": [{ "title", "url", "content" }] }`.
pub fn parse_tavily(body: &Value) -> Vec<SearchHit> {
    hits_from(body.get("results"), &["content"])
}

/// Brave: `{ "web": { "results": [{ "title", "url", "description" }] } }`
/// (titles and descriptions carry `<strong>` highlights).
pub fn parse_brave(body: &Value) -> Vec<SearchHit> {
    hits_from(body.pointer("/web/results"), &["description"])
}

/// Exa: `{ "results": [{ "title", "url", "text" | "highlights" | "summary" }] }`.
pub fn parse_exa(body: &Value) -> Vec<SearchHit> {
    let Some(results) = body.get("results").and_then(Value::as_array) else { return Vec::new() };
    results
        .iter()
        .filter_map(|result| {
            let highlight = result.pointer("/highlights/0").and_then(Value::as_str);
            let snippet = text_field(result, &["text", "summary"]).or(highlight).unwrap_or_default();
            hit(result, snippet)
        })
        .collect()
}

/// Hits of a JSON result array whose items have `title`, `url` and the
/// first of `snippet_keys`.
fn hits_from(results: Option<&Value>, snippet_keys: &[&str]) -> Vec<SearchHit> {
    let Some(results) = results.and_then(Value::as_array) else { return Vec::new() };
    results.iter().filter_map(|result| hit(result, text_field(result, snippet_keys).unwrap_or_default())).collect()
}

fn text_field<'a>(result: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| result.get(*key).and_then(Value::as_str).filter(|text| !text.trim().is_empty()))
}

/// A hit with a usable http(s) URL; the title falls back to the URL.
fn hit(result: &Value, snippet: &str) -> Option<SearchHit> {
    let url = result.get("url").and_then(Value::as_str)?.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let title = plain_text(result.get("title").and_then(Value::as_str).unwrap_or_default());
    Some(SearchHit {
        title: if title.is_empty() { url.to_string() } else { title },
        url: url.to_string(),
        snippet: plain_text(snippet),
    })
}
