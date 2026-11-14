//! Keyless fallback: DuckDuckGo's HTML endpoint, used when no search key is
//! configured. Kept on its own so it is easy to drop: it scrapes a page
//! meant for browsers, so it is weaker and can stop working any time.
//!
//! The query is sent as a POST form, so it stays out of URLs and logs.

use super::{decode_entities, plain_text, send_for_body, SearchBackend, SearchHit};
use regex::Regex;

const DUCKDUCKGO_URL: &str = "https://html.duckduckgo.com/html/";

pub(super) async fn search(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>, String> {
    let request = client.post(DUCKDUCKGO_URL).form(&[("q", query)]);
    let page = send_for_body(SearchBackend::DuckDuckGo, request, None).await?;
    let hits = parse_results(&page);
    if hits.is_empty() && is_bot_check(&page) {
        return Err("web_search: DuckDuckGo refused the automated request (bot check). Configure a Tavily, Brave Search or Exa key (`lcgn auth login tavily`, or TAVILY_API_KEY / BRAVE_API_KEY / EXA_API_KEY).".to_string());
    }
    Ok(hits)
}

/// Whether the page is DuckDuckGo's "unusual traffic" challenge.
fn is_bot_check(page: &str) -> bool {
    let lower = page.to_lowercase();
    lower.contains("anomaly-modal") || lower.contains("unusual traffic") || lower.contains("challenge-form")
}

/// Results of a DuckDuckGo HTML page: every `result__a` link (title + link)
/// with the `result__snippet` that follows it, before the next result.
/// Ads and links that do not decode to an http(s) URL are skipped.
pub fn parse_results(page: &str) -> Vec<SearchHit> {
    let (Ok(anchor), Ok(href), Ok(snippet)) = (
        Regex::new(r#"(?is)<a\s([^>]*\bclass="[^"]*\bresult__a\b[^"]*"[^>]*)>(.*?)</a>"#),
        Regex::new(r#"(?i)\bhref="([^"]*)""#),
        Regex::new(r#"(?is)\bclass="[^"]*\bresult__snippet\b[^"]*"[^>]*>(.*?)</(?:a|div|td|span)>"#),
    ) else {
        return Vec::new();
    };
    let anchors: Vec<regex::Captures> = anchor.captures_iter(page).collect();
    let mut hits = Vec::new();
    for (index, captures) in anchors.iter().enumerate() {
        let whole = captures.get(0).map_or(0..0, |m| m.range());
        let next_start = anchors.get(index + 1).and_then(|next| next.get(0)).map_or(page.len(), |m| m.start());
        let Some(link) = href.captures(&captures[1]).and_then(|h| decode_result_link(&h[1])) else { continue };
        let between = &page[whole.end..next_start];
        let text = snippet.captures(between).map(|s| plain_text(&s[1])).unwrap_or_default();
        let title = plain_text(&captures[2]);
        hits.push(SearchHit { title: if title.is_empty() { link.clone() } else { title }, url: link, snippet: text });
    }
    hits
}

/// The target of a result link: `//duckduckgo.com/l/?uddg=<encoded>`
/// redirect links decode to `<encoded>`, plain http(s) links pass, ads
/// (`/y.js`) and anything else are `None`.
pub fn decode_result_link(raw_href: &str) -> Option<String> {
    let href = decode_entities(raw_href.trim());
    let absolute = if href.starts_with("//") { format!("https:{href}") } else { href };
    let parsed = url::Url::parse(&absolute).ok()?;
    let host = parsed.host_str().unwrap_or_default();
    let target = if host == "duckduckgo.com" || host.ends_with(".duckduckgo.com") {
        if parsed.path() != "/l/" {
            return None;
        }
        let (_, target) = parsed.query_pairs().find(|(name, _)| name == "uddg")?;
        url::Url::parse(&target).ok()?
    } else {
        parsed
    };
    matches!(target.scheme(), "http" | "https").then(|| target.to_string())
}
