//! Chat-model catalogs for the providers whose OAuth login unlocks a fixed
//! model list (ids and context windows verbatim from pi 1.5.2).

/// One catalog model: `api` is the wire protocol the provider serves it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: &'static str,
    pub api: &'static str,
    pub context_window: i64,
}

const fn model(id: &'static str, api: &'static str, context_window: i64) -> CatalogModel {
    CatalogModel { id, api, context_window }
}

const RESPONSES: &str = "openai-responses";
const COMPLETIONS: &str = "openai-completions";
const MESSAGES: &str = "anthropic-messages";

/// Models served to "Sign in with ChatGPT" (`openai`, Responses API).
pub const CHATGPT_MODELS: &[CatalogModel] = &[
    model("chat-latest", RESPONSES, 400_000),
    model("gpt-4", RESPONSES, 8_192),
    model("gpt-4-turbo", RESPONSES, 128_000),
    model("gpt-4.1", RESPONSES, 1_047_576),
    model("gpt-4.1-mini", RESPONSES, 1_047_576),
    model("gpt-4.1-nano", RESPONSES, 1_047_576),
    model("gpt-4o", RESPONSES, 128_000),
    model("gpt-4o-2024-05-13", RESPONSES, 128_000),
    model("gpt-4o-2024-08-06", RESPONSES, 128_000),
    model("gpt-4o-2024-11-20", RESPONSES, 128_000),
    model("gpt-4o-mini", RESPONSES, 128_000),
    model("gpt-5", RESPONSES, 400_000),
    model("gpt-5-chat-latest", RESPONSES, 128_000),
    model("gpt-5-mini", RESPONSES, 400_000),
    model("gpt-5-nano", RESPONSES, 400_000),
    model("gpt-5-pro", RESPONSES, 400_000),
    model("gpt-5.1", RESPONSES, 400_000),
    model("gpt-5.2", RESPONSES, 400_000),
    model("gpt-5.2-pro", RESPONSES, 400_000),
    model("gpt-5.3-codex", RESPONSES, 400_000),
    model("gpt-5.3-codex-spark", RESPONSES, 128_000),
    model("gpt-5.4", RESPONSES, 272_000),
    model("gpt-5.4-mini", RESPONSES, 400_000),
    model("gpt-5.4-nano", RESPONSES, 400_000),
    model("gpt-5.4-pro", RESPONSES, 1_050_000),
    model("gpt-5.5", RESPONSES, 272_000),
    model("gpt-5.5-pro", RESPONSES, 1_050_000),
    model("gpt-5.6-cyber", RESPONSES, 400_000),
    model("gpt-5.6-luna", RESPONSES, 272_000),
    model("gpt-5.6-sol", RESPONSES, 272_000),
    model("gpt-5.6-terra", RESPONSES, 272_000),
    model("gpt-6-astra", RESPONSES, 272_000),
    model("gpt-6-luna", RESPONSES, 272_000),
    model("gpt-6-sol", RESPONSES, 272_000),
    model("gpt-6.1-sol", RESPONSES, 272_000),
    model("gpt-6.1-terra", RESPONSES, 272_000),
    model("gpt-daybreak-blue-latest", RESPONSES, 1_050_000),
    model("gpt-daybreak-red-latest", RESPONSES, 400_000),
    model("o1", RESPONSES, 200_000),
    model("o1-pro", RESPONSES, 200_000),
    model("o3", RESPONSES, 200_000),
    model("o3-mini", RESPONSES, 200_000),
    model("o3-pro", RESPONSES, 200_000),
    model("o4-mini", RESPONSES, 200_000),
];

/// Models GitHub Copilot serves; `availableModelIds` of the credential
/// narrows the list to what the account may use.
pub const COPILOT_MODELS: &[CatalogModel] = &[
    model("claude-fable-5", MESSAGES, 1_000_000),
    model("claude-fable-5.1", MESSAGES, 1_000_000),
    model("claude-haiku-4.5", MESSAGES, 200_000),
    model("claude-haiku-5.5", MESSAGES, 1_000_000),
    model("claude-opus-4.8", MESSAGES, 1_000_000),
    model("claude-opus-5", MESSAGES, 1_000_000),
    model("claude-opus-5.5", MESSAGES, 1_000_000),
    model("claude-sonnet-4.6", MESSAGES, 1_000_000),
    model("claude-sonnet-5", MESSAGES, 1_000_000),
    model("claude-sonnet-5.5", MESSAGES, 1_000_000),
    model("gemini-3.8-flash", COMPLETIONS, 1_000_000),
    model("gemini-4-argon", COMPLETIONS, 1_048_576),
    model("kimi-k3", COMPLETIONS, 1_048_576),
    model("gpt-5.3-codex", RESPONSES, 1_000_000),
    model("gpt-5.4-nano", RESPONSES, 400_000),
    model("gpt-5.6-luna", RESPONSES, 1_050_000),
    model("gpt-5.6-sol", RESPONSES, 1_050_000),
    model("gpt-5.6-terra", RESPONSES, 1_050_000),
    model("gpt-6-astra", RESPONSES, 1_000_000),
    model("gpt-6-luna", RESPONSES, 1_000_000),
    model("gpt-6-sol", RESPONSES, 1_000_000),
    model("gpt-6.1-sol", RESPONSES, 1_050_000),
    model("gpt-6.1-terra", RESPONSES, 1_050_000),
    model("grok-4.5", RESPONSES, 500_000),
    model("grok-4.6", RESPONSES, 500_000),
    model("grok-4.7", RESPONSES, 500_000),
    model("mai-code-1.1-flash", RESPONSES, 256_000),
];

/// Kimi Code subscription models (OpenAI chat completions).
pub const KIMI_MODELS: &[CatalogModel] = &[
    model("k3", COMPLETIONS, 1_048_576),
    model("k3-256k", COMPLETIONS, 262_144),
    model("k3.1-pro", COMPLETIONS, 1_048_576),
    model("k3.1-pro-highspeed", COMPLETIONS, 1_048_576),
    model("kimi-for-coding", COMPLETIONS, 1_048_576),
    model("kimi-for-coding-highspeed", COMPLETIONS, 262_144),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_match_the_spec_counts() {
        assert_eq!(CHATGPT_MODELS.len(), 44);
        assert_eq!(COPILOT_MODELS.len(), 27);
        assert_eq!(KIMI_MODELS.len(), 6);
        assert_eq!(COPILOT_MODELS.iter().filter(|m| m.api == MESSAGES).count(), 10);
        assert_eq!(COPILOT_MODELS.iter().filter(|m| m.api == COMPLETIONS).count(), 3);
    }
}
