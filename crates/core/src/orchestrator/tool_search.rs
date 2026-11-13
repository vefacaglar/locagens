//! Deferred tool loading (`tool_search`), modelled on Codex's tool search.
//!
//! Codex keeps MCP/app tools out of the first request: they are *deferred*,
//! the model gets a `tool_search` tool, and the specs of whatever it finds are
//! added to the following requests. Ranking is BM25 over each tool's name,
//! description and argument names.
//!
//! Threshold: Codex defers every eligible MCP tool whenever the model supports
//! search (it has no tool-count cutoff). Locagens providers do not advertise
//! native search, so the loader is client-side and only kicks in when it can
//! help: more than [`DEFER_MCP_TOOL_THRESHOLD`] MCP tools (Codex's default
//! `tool_search` result limit, [`DEFAULT_SEARCH_LIMIT`]) — with fewer, a search
//! could not narrow anything. When it kicks in, *all* MCP tools are deferred,
//! as in Codex.

use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

pub const TOOL_SEARCH_NAME: &str = "tool_search";
/// Codex's `TOOL_SEARCH_DEFAULT_LIMIT`.
pub const DEFAULT_SEARCH_LIMIT: usize = 8;
/// More MCP tools than this are deferred behind `tool_search`.
pub const DEFER_MCP_TOOL_THRESHOLD: usize = DEFAULT_SEARCH_LIMIT;

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

fn tool_name(def: &Value) -> &str {
    def.pointer("/function/name").and_then(Value::as_str).unwrap_or_default()
}

fn is_mcp_tool(def: &Value) -> bool {
    tool_name(def).starts_with("mcp__")
}

/// Splits `tools` into `(direct, deferred)`. When anything is deferred the
/// `tool_search` tool is appended to `direct`.
pub fn split_deferred(tools: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let mcp_count = tools.iter().filter(|def| is_mcp_tool(def)).count();
    if mcp_count <= DEFER_MCP_TOOL_THRESHOLD {
        return (tools, Vec::new());
    }
    let (deferred, mut direct): (Vec<Value>, Vec<Value>) = tools.into_iter().partition(is_mcp_tool);
    direct.push(tool_def(&deferred));
    (direct, deferred)
}

/// `tool_search` spec; lists the MCP servers whose tools are deferred
/// (Codex's "source listing").
pub fn tool_def(deferred: &[Value]) -> Value {
    let servers: BTreeSet<&str> = deferred.iter().filter_map(|def| server_of(tool_name(def))).collect();
    let listing = if servers.is_empty() {
        "None currently enabled.".to_string()
    } else {
        servers.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")
    };
    json!({
        "type": "function",
        "function": {
            "name": TOOL_SEARCH_NAME,
            "description": format!(
                "Search deferred tools and load the best matches so you can call them on your next step. \
Only the tools from these sources are deferred:\n{listing}\n\
Describe the capability you need (e.g. \"create github issue\"). Matching tool specs are added to the available tools."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query for deferred tools." },
                    "limit": { "type": "integer", "description": format!("Maximum number of tools to return. Defaults to {DEFAULT_SEARCH_LIMIT}.") }
                },
                "required": ["query"]
            }
        }
    })
}

/// `mcp__<server>__<tool>` -> `<server>`.
fn server_of(name: &str) -> Option<&str> {
    name.strip_prefix("mcp__")?.split("__").next().filter(|s| !s.is_empty())
}

// ----------------------------------------------------------------------
// BM25 ranking
// ----------------------------------------------------------------------

/// Lower-cased alphanumeric tokens; camelCase boundaries split; a trailing
/// plural `s` is dropped so `issues` finds `issue`.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in text.chars() {
        if !ch.is_alphanumeric() {
            push_token(&mut tokens, &mut current);
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower {
            push_token(&mut tokens, &mut current);
        }
        prev_lower = ch.is_lowercase() || ch.is_numeric();
        current.extend(ch.to_lowercase());
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }
    let mut token = std::mem::take(current);
    if token.len() > 3 && token.ends_with('s') && !token.ends_with("ss") {
        token.pop();
    }
    tokens.push(token);
}

/// Searchable text of one tool def: name (counted twice — it is the strongest
/// signal), description and top-level argument names.
fn search_tokens(def: &Value) -> Vec<String> {
    let mut tokens = tokenize(tool_name(def));
    tokens.extend(tokens.clone());
    if let Some(description) = def.pointer("/function/description").and_then(Value::as_str) {
        tokens.extend(tokenize(description));
    }
    if let Some(props) = def.pointer("/function/parameters/properties").and_then(Value::as_object) {
        for key in props.keys() {
            tokens.extend(tokenize(key));
        }
    }
    tokens
}

/// Indices into `deferred`, best match first, score > 0 only.
pub fn rank(deferred: &[Value], query: &str, limit: usize) -> Vec<usize> {
    let docs: Vec<Vec<String>> = deferred.iter().map(search_tokens).collect();
    let query_tokens = tokenize(query);
    if docs.is_empty() || query_tokens.is_empty() {
        return Vec::new();
    }
    let avg_len = docs.iter().map(Vec::len).sum::<usize>() as f64 / docs.len() as f64;
    let mut doc_freq: HashMap<&str, usize> = HashMap::new();
    for doc in &docs {
        let unique: BTreeSet<&str> = doc.iter().map(String::as_str).collect();
        for token in unique {
            *doc_freq.entry(token).or_default() += 1;
        }
    }
    let total = docs.len() as f64;
    let mut scored: Vec<(usize, f64)> = docs
        .iter()
        .enumerate()
        .map(|(index, doc)| (index, bm25(doc, &query_tokens, &doc_freq, total, avg_len)))
        .filter(|(_, score)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    scored.into_iter().take(limit).map(|(index, _)| index).collect()
}

fn bm25(doc: &[String], query: &[String], doc_freq: &HashMap<&str, usize>, total: f64, avg_len: f64) -> f64 {
    let mut score = 0.0;
    for token in query {
        let tf = doc.iter().filter(|t| *t == token).count() as f64;
        if tf == 0.0 {
            continue;
        }
        let df = doc_freq.get(token.as_str()).copied().unwrap_or(0) as f64;
        let idf = ((total - df + 0.5) / (df + 0.5) + 1.0).ln();
        let norm = 1.0 - BM25_B + BM25_B * doc.len() as f64 / avg_len.max(1.0);
        score += idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * norm);
    }
    score
}

// ----------------------------------------------------------------------
// Tool execution
// ----------------------------------------------------------------------

/// Runs a `tool_search` call: ranks `deferred`, appends the matching specs
/// to `active_tools` (skipping ones already loaded) so the next request
/// carries them, and returns the model-facing result.
pub fn apply_search(active_tools: &mut Vec<Value>, deferred: &[Value], args: &Value) -> Result<String, String> {
    let query = args.get("query").and_then(Value::as_str).unwrap_or_default().trim();
    if query.is_empty() {
        return Err("query must not be empty".into());
    }
    let limit = match args.get("limit").and_then(Value::as_u64) {
        Some(0) => return Err("limit must be greater than zero".into()),
        Some(limit) => limit as usize,
        None => DEFAULT_SEARCH_LIMIT,
    };
    let hits = rank(deferred, query, limit);
    let mut tools = Vec::new();
    for index in hits {
        let def = &deferred[index];
        let name = tool_name(def).to_string();
        let already_loaded = active_tools.iter().any(|active| tool_name(active) == name);
        if !already_loaded {
            active_tools.push(def.clone());
        }
        tools.push(json!({
            "name": name,
            "description": def.pointer("/function/description").cloned().unwrap_or(Value::Null),
            "loaded": true,
            "newly_loaded": !already_loaded,
        }));
    }
    let note = if tools.is_empty() {
        "No deferred tools matched; try different keywords."
    } else {
        "These tools are now available; call them directly."
    };
    Ok(serde_json::to_string_pretty(&json!({ "tools": tools, "note": note })).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp(server: &str, tool: &str, description: &str, props: &[&str]) -> Value {
        let properties: serde_json::Map<String, Value> =
            props.iter().map(|p| (p.to_string(), json!({ "type": "string" }))).collect();
        json!({ "type": "function", "function": {
            "name": format!("mcp__{server}__{tool}"),
            "description": description,
            "parameters": { "type": "object", "properties": properties, "required": [] }
        }})
    }

    fn catalog() -> Vec<Value> {
        vec![
            mcp("github", "create_issue", "Create a new issue in a GitHub repository", &["repo", "title", "body"]),
            mcp("github", "list_pull_requests", "List pull requests of a repository", &["repo", "state"]),
            mcp("github", "merge_pull_request", "Merge a pull request", &["repo", "number"]),
            mcp("slack", "post_message", "Post a message to a Slack channel", &["channel", "text"]),
            mcp("slack", "list_channels", "List Slack channels", &[]),
            mcp("jira", "create_ticket", "Create a Jira ticket in a project", &["project", "summary"]),
            mcp("db", "run_query", "Run a read-only SQL query against the database", &["sql"]),
            mcp("db", "list_tables", "List the tables in the database schema", &[]),
            mcp("files", "read_document", "Read a document by path", &["path"]),
            mcp("files", "search_documents", "Search documents by keyword", &["query"]),
        ]
    }

    fn builtin(name: &str) -> Value {
        json!({ "type": "function", "function": { "name": name, "description": "d", "parameters": {} } })
    }

    #[test]
    fn small_mcp_sets_stay_direct() {
        let mut tools = vec![builtin("read_file")];
        tools.extend(catalog().into_iter().take(DEFER_MCP_TOOL_THRESHOLD));
        let (direct, deferred) = split_deferred(tools.clone());
        assert!(deferred.is_empty());
        assert_eq!(direct, tools);
    }

    #[test]
    fn large_mcp_sets_are_deferred_and_tool_search_is_added() {
        let mut tools = vec![builtin("read_file")];
        tools.extend(catalog());
        assert!(catalog().len() > DEFER_MCP_TOOL_THRESHOLD);
        let (direct, deferred) = split_deferred(tools);
        assert_eq!(deferred.len(), 10);
        assert!(direct.iter().all(|d| !is_mcp_tool(d)));
        let names: Vec<&str> = direct.iter().map(tool_name).collect();
        assert_eq!(names, vec!["read_file", "tool_search"]);
        let description = direct[1]["function"]["description"].as_str().unwrap();
        for server in ["db", "files", "github", "jira", "slack"] {
            assert!(description.contains(&format!("- {server}")), "{server} missing");
        }
    }

    #[test]
    fn ranking_prefers_name_and_description_matches() {
        let deferred = catalog();
        let top = |q: &str| tool_name(&deferred[rank(&deferred, q, 3)[0]]).to_string();
        assert_eq!(top("create github issue"), "mcp__github__create_issue");
        assert_eq!(top("post slack message"), "mcp__slack__post_message");
        assert_eq!(top("sql query"), "mcp__db__run_query");
        assert_eq!(top("merge pull request"), "mcp__github__merge_pull_request");
        assert_eq!(top("jira tickets"), "mcp__jira__create_ticket");
    }

    #[test]
    fn plural_and_camel_case_tokens_match() {
        let mut deferred = catalog();
        deferred.push(mcp("crm", "getCustomerRecord", "Fetch one customer", &["customerId"]));
        let hits = rank(&deferred, "customers record", 1);
        assert_eq!(tool_name(&deferred[hits[0]]), "mcp__crm__getCustomerRecord");
    }

    #[test]
    fn limit_and_no_match_behaviour() {
        let deferred = catalog();
        assert_eq!(rank(&deferred, "list", 2).len(), 2);
        assert!(rank(&deferred, "zzzzqqqq", 5).is_empty());
        assert!(rank(&deferred, "   ", 5).is_empty());
        assert!(rank(&[], "list", 5).is_empty());
    }

    #[test]
    fn apply_search_injects_specs_once_and_reports_them() {
        let deferred = catalog();
        let mut active = vec![builtin("read_file")];
        let out = apply_search(&mut active, &deferred, &json!({ "query": "github pull request", "limit": 2 })).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["tools"].as_array().unwrap().len(), 2);
        assert_eq!(active.len(), 3, "two specs appended to the next request");
        assert_eq!(parsed["tools"][0]["newly_loaded"], true);

        // Same search again loads nothing new.
        let again = apply_search(&mut active, &deferred, &json!({ "query": "github pull request", "limit": 2 })).unwrap();
        let parsed: Value = serde_json::from_str(&again).unwrap();
        assert_eq!(active.len(), 3);
        assert_eq!(parsed["tools"][0]["newly_loaded"], false);
    }

    #[test]
    fn apply_search_validates_arguments() {
        let deferred = catalog();
        let mut active = Vec::new();
        assert!(apply_search(&mut active, &deferred, &json!({})).is_err());
        assert!(apply_search(&mut active, &deferred, &json!({ "query": "x", "limit": 0 })).is_err());
        let none = apply_search(&mut active, &deferred, &json!({ "query": "zzzzqqqq" })).unwrap();
        assert!(none.contains("No deferred tools matched"));
        assert!(active.is_empty());
    }
}
