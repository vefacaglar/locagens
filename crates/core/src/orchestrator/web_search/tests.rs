use super::duckduckgo::{decode_result_link, parse_results};
use super::keyed_backends::{parse_brave, parse_exa, parse_tavily};
use super::*;

fn hit(title: &str, url: &str, snippet: &str) -> SearchHit {
    SearchHit { title: title.into(), url: url.into(), snippet: snippet.into() }
}

#[test]
fn parses_tavily_results() {
    let body = json!({
        "query": "rust",
        "results": [
            { "title": "Rust Programming Language", "url": "https://www.rust-lang.org/", "content": "A language empowering\n everyone.", "score": 0.9 },
            { "title": "", "url": "https://doc.rust-lang.org/book/", "content": "The Book" },
            { "title": "no url" },
            { "title": "bad scheme", "url": "javascript:alert(1)", "content": "x" }
        ]
    });
    assert_eq!(
        parse_tavily(&body),
        vec![
            hit("Rust Programming Language", "https://www.rust-lang.org/", "A language empowering everyone."),
            hit("https://doc.rust-lang.org/book/", "https://doc.rust-lang.org/book/", "The Book"),
        ]
    );
    assert!(parse_tavily(&json!({ "detail": "unauthorized" })).is_empty());
}

#[test]
fn parses_brave_results_and_strips_highlights() {
    let body = json!({
        "type": "search",
        "web": { "results": [
            { "title": "The <strong>Rust</strong> Book", "url": "https://doc.rust-lang.org/book/", "description": "Learn <strong>Rust</strong> &amp; more &#39;fast&#x27;" }
        ] }
    });
    assert_eq!(parse_brave(&body), vec![hit("The Rust Book", "https://doc.rust-lang.org/book/", "Learn Rust & more 'fast'")]);
    assert!(parse_brave(&json!({ "type": "search" })).is_empty());
}

#[test]
fn parses_exa_results_with_text_or_highlights() {
    let body = json!({
        "requestId": "r1",
        "results": [
            { "title": "Rust", "url": "https://www.rust-lang.org/", "text": "Fast, reliable, productive." },
            { "title": "Crates", "url": "https://crates.io/", "highlights": ["The Rust package registry"] },
            { "title": "Bare", "url": "https://example.com/" }
        ]
    });
    assert_eq!(
        parse_exa(&body),
        vec![
            hit("Rust", "https://www.rust-lang.org/", "Fast, reliable, productive."),
            hit("Crates", "https://crates.io/", "The Rust package registry"),
            hit("Bare", "https://example.com/", ""),
        ]
    );
}

const DUCKDUCKGO_PAGE: &str = r#"
<div class="result results_links results_links_deep result--ad">
  <h2 class="result__title"><a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=ads.example&amp;u3=x">Sponsored</a></h2>
  <a class="result__snippet" href="https://duckduckgo.com/y.js?x">Buy now</a>
</div>
<div class="result results_links results_links_deep web-result ">
  <div class="links_main links_deep result__body">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2Flearn%3Fa%3D1%26b%3D2&amp;rut=abc123">Learn <b>Rust</b></a>
    </h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x">A language empowering <b>everyone</b> to build reliable &amp; efficient software.</a>
  </div>
</div>
<div class="result results_links web-result">
  <h2 class="result__title"><a href="https://en.wikipedia.org/wiki/Rust_(programming_language)" class="result__a">Rust (programming language) - Wikipedia</a></h2>
</div>
<div class="result results_links web-result">
  <h2 class="result__title"><a class="result__a" href="https://crates.io/">crates.io</a></h2>
  <div class="result__snippet">The Rust community&#x27;s crate registry</div>
</div>
"#;

#[test]
fn parses_duckduckgo_html_and_skips_ads() {
    assert_eq!(
        parse_results(DUCKDUCKGO_PAGE),
        vec![
            hit("Learn Rust", "https://www.rust-lang.org/learn?a=1&b=2", "A language empowering everyone to build reliable & efficient software."),
            // No snippet of its own: the next result's snippet is not borrowed.
            hit("Rust (programming language) - Wikipedia", "https://en.wikipedia.org/wiki/Rust_(programming_language)", ""),
            hit("crates.io", "https://crates.io/", "The Rust community's crate registry"),
        ]
    );
    assert!(parse_results("<html><body>No results.</body></html>").is_empty());
}

#[test]
fn decodes_duckduckgo_redirect_links() {
    assert_eq!(
        decode_result_link("//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Fserde&amp;rut=1").as_deref(),
        Some("https://docs.rs/serde")
    );
    assert_eq!(
        decode_result_link("https://duckduckgo.com/l/?kh=-1&uddg=http%3A%2F%2Fexample.com%2Fa%20b").as_deref(),
        Some("http://example.com/a%20b")
    );
    assert_eq!(decode_result_link("https://example.org/page").as_deref(), Some("https://example.org/page"));
    // Ads, other DuckDuckGo pages, missing targets and non-http schemes.
    assert_eq!(decode_result_link("https://duckduckgo.com/y.js?ad_domain=x"), None);
    assert_eq!(decode_result_link("//duckduckgo.com/l/?rut=1"), None);
    assert_eq!(decode_result_link("//duckduckgo.com/l/?uddg=javascript%3Aalert(1)"), None);
    assert_eq!(decode_result_link("/relative"), None);
}

#[test]
fn backend_selection_follows_the_configured_order() {
    let keys = |tavily: Option<&str>, brave: Option<&str>, exa: Option<&str>| {
        vec![
            (SearchBackend::Tavily, tavily.map(str::to_string)),
            (SearchBackend::BraveSearch, brave.map(str::to_string)),
            (SearchBackend::Exa, exa.map(str::to_string)),
        ]
    };
    assert_eq!(select_backend(keys(Some("t"), Some("b"), Some("e"))), (SearchBackend::Tavily, Some("t".into())));
    assert_eq!(select_backend(keys(None, Some("b"), Some("e"))), (SearchBackend::BraveSearch, Some("b".into())));
    assert_eq!(select_backend(keys(Some("  "), None, Some("e"))), (SearchBackend::Exa, Some("e".into())));
    assert_eq!(select_backend(keys(None, None, None)), (SearchBackend::DuckDuckGo, None));
    let order: Vec<_> = KEYED_BACKENDS.iter().map(|backend| backend.provider_id().unwrap()).collect();
    assert_eq!(order, ["tavily", "brave-search", "exa"]);
}

#[test]
fn max_results_is_clamped() {
    assert_eq!(clamp_max_results(&json!({})), 5);
    assert_eq!(clamp_max_results(&json!({ "max_results": 0 })), 1);
    assert_eq!(clamp_max_results(&json!({ "max_results": -3 })), 1);
    assert_eq!(clamp_max_results(&json!({ "max_results": 7 })), 7);
    assert_eq!(clamp_max_results(&json!({ "max_results": 50 })), 10);
    assert_eq!(clamp_max_results(&json!({ "max_results": "3" })), 5);
}

#[test]
fn results_are_numbered_and_byte_budgeted() {
    let long = "é".repeat(1000);
    let text = format_results("rust", SearchBackend::Tavily, &[hit("A", "https://a.example/", &long), hit("B", "https://b.example/", "")]);
    assert!(text.starts_with("Web search results for \"rust\" (via Tavily):"), "{text}");
    assert!(text.contains("\n1. A\n   https://a.example/\n   é"));
    assert!(text.contains("\n2. B\n   https://b.example/\n"));
    assert!(text.len() < 1000, "snippet must be cut: {}", text.len());
    assert!(!text.contains("DuckDuckGo"));
    let fallback = format_results("rust", SearchBackend::DuckDuckGo, &[]);
    assert!(fallback.contains("via DuckDuckGo") && fallback.contains("TAVILY_API_KEY") && fallback.ends_with("No results."));
}

#[test]
fn errors_never_show_the_key() {
    assert_eq!(mask("HTTP 401: bad key sk-123 given", Some("sk-123")), "HTTP 401: bad key *** given");
    assert_eq!(mask("plain", None), "plain");
}

#[tokio::test]
async fn empty_query_is_refused_without_network() {
    let tmp = tempfile::tempdir().unwrap();
    let pi = PiProviderManager::with_dirs(tmp.path().join("home"), tmp.path().join("support"));
    let err = execute(&pi, &json!({ "query": "   " })).await.unwrap_err();
    assert!(err.contains("query is empty"));
}

/// One live check of the keyless path (network): run with
/// `cargo test -p locagens-core duckduckgo_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn duckduckgo_live_search_returns_results() {
    let client = http_client().unwrap();
    let hits = duckduckgo::search(&client, "rust programming language").await.unwrap();
    println!("{}", format_results("rust programming language", SearchBackend::DuckDuckGo, &hits));
    assert!(!hits.is_empty());
}
