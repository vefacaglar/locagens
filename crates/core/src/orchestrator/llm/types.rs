//! Provider-neutral request/response types and endpoint resolution.

use super::*;

/// One chat message in provider-neutral form. Tool calls are kept in the
/// OpenAI shape (`{ id, function: { name, arguments } }`) — the normalized
/// shape used in `RunMessage.rawResponse` — and converted per API protocol
/// when the request body is built.
#[derive(Clone, Debug, Default)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
    /// `tool_call_id` for `role == "tool"` responses.
    ///
    /// (`call_id` for `function_call_output` items under the Responses API.)
    pub tool_call_id: Option<String>,
    /// Tool calls requested by an assistant message.
    pub tool_calls: Option<Vec<Value>>,
    /// Anthropic `thinking` / `redacted_thinking` blocks of an assistant
    /// message, replayed unchanged (signature included).
    pub thinking_blocks: Option<Vec<Value>>,
    /// Id of the persisted `RunMessage` this message came from (compaction
    /// markers point at the first message they kept).
    pub source_id: Option<String>,
    /// Images attached to a user message.
    pub images: Vec<crate::db::ImageAttachment>,
}

/// Token accounting for one completion call.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    /// The whole prompt, cached tokens included, for every protocol
    /// (Anthropic's uncached count is normalized at parse time).
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Provider-reported cost (when the upstream bills it directly); the
    /// orchestrator's per-million pricing is only a fallback.
    pub provider_cost: Option<f64>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }
}

/// Everything a streaming completion produced.
#[derive(Clone, Debug, Default)]
pub struct StreamOutcome {
    pub content: String,
    pub reasoning: String,
    /// Normalized tool calls: `[{ id, function: { name, arguments(str) } }]`.
    pub tool_calls: Vec<Value>,
    pub usage: Option<Usage>,
    /// Anthropic thinking blocks (with signatures) to replay.
    pub thinking_blocks: Vec<Value>,
    /// Generation stopped at the output-token limit.
    pub truncated: bool,
}

/// Resolved endpoint for one loop iteration.
#[derive(Clone, Debug, Default)]
pub struct Endpoint {
    pub base_url: String,
    /// `"openai-compatible"` (chat/completions), `"openai-responses"` /
    /// `"responses"` (Codex `/v1/responses` WireAPI) or `"anthropic"`.
    pub api: String,
    pub api_key: String,
    /// Per-conversation routing id (OpenCode requires `x-opencode-session`).
    pub session_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub max_tokens: Option<i64>,
    /// Whether the model accepts a `reasoning_effort` parameter.
    pub reasoning_supported: bool,
    /// Provider-specific request differences (headers, URLs, body fields).
    pub quirk: ProviderQuirk,
}

/// Providers whose requests differ from the plain protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderQuirk {
    #[default]
    None,
    /// GitHub Copilot: editor identity headers, bearer auth for every API,
    /// no `/v1` prefix on its chat and Responses URLs.
    GithubCopilot,
    /// "Sign in with ChatGPT" token against `api.openai.com/v1`: the
    /// Responses body must be `store: false` without `max_output_tokens`.
    ChatgptOauth,
}

const OPENAI_API_URL: &str = "https://api.openai.com/v1";

impl ProviderQuirk {
    /// Detects the quirk from the resolved provider, base URL and credential.
    /// ChatGPT sign-in is `openai` on exactly `api.openai.com/v1` with a
    /// bearer that is not an `sk-` API key (pi's detection).
    pub fn detect(provider_id: &str, base_url: &str, api_key: &str) -> Self {
        if provider_id == "github-copilot" {
            Self::GithubCopilot
        } else if provider_id == "openai"
            && base_url.trim_end_matches('/') == OPENAI_API_URL
            && !api_key.starts_with("sk-")
        {
            Self::ChatgptOauth
        } else {
            Self::None
        }
    }
}

impl Endpoint {
    /// `{base}/messages`; Copilot serves Anthropic Messages at `/v1/messages`.
    pub(super) fn messages_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if self.quirk == ProviderQuirk::GithubCopilot {
            format!("{base}/v1/messages")
        } else {
            format!("{base}/messages")
        }
    }

    /// The Responses URL; Copilot has no `/v1` prefix.
    pub(super) fn responses_endpoint_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if self.quirk == ProviderQuirk::GithubCopilot {
            format!("{base}/responses")
        } else {
            responses_url(base)
        }
    }

    pub fn is_anthropic(&self) -> bool {
        self.api == "anthropic"
            || self.api == "anthropic-messages"
            || self.base_url.contains("anthropic")
    }

    /// True when the endpoint speaks Codex's native OpenAI Responses WireAPI
    /// (`/v1/responses`) instead of chat completions.
    pub fn is_openai_responses(&self) -> bool {
        self.api == "openai-responses" || self.api == "responses"
    }
}
