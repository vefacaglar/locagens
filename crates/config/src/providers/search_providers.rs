//! Web search backends of the `web_search` tool (Tavily, Brave Search, Exa).
//!
//! They are API-key-only `auth.json` entries like any provider (literal,
//! `!command`, `$ENV_VAR`) so `lcgn /login` and Settings → Providers can
//! store their keys, but they are NOT model providers: they never appear in
//! `provider_ids`, `list_providers`, `get_configs` or model refreshes.

/// One web search backend that takes an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProvider {
    /// `auth.json` key (`tavily`, `brave-search`, `exa`).
    pub id: &'static str,
    pub name: &'static str,
    /// Environment variable that supplies the key when `auth.json` has none.
    pub env_var: &'static str,
}

/// Search backends in the order `web_search` tries them: the first one with
/// a key wins.
pub const SEARCH_PROVIDERS: &[SearchProvider] = &[
    SearchProvider { id: "tavily", name: "Tavily (web search)", env_var: "TAVILY_API_KEY" },
    SearchProvider { id: "brave-search", name: "Brave Search (web search)", env_var: "BRAVE_API_KEY" },
    SearchProvider { id: "exa", name: "Exa (web search)", env_var: "EXA_API_KEY" },
];

/// The search backend with this id, if it is one.
pub fn search_provider(id: &str) -> Option<&'static SearchProvider> {
    SEARCH_PROVIDERS.iter().find(|provider| provider.id == id)
}

/// Whether `id` names a web search backend rather than a model provider.
pub fn is_search_provider(id: &str) -> bool {
    search_provider(id).is_some()
}
