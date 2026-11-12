//! Model/provider resolution and the `--list-models` table.

use crate::pi_providers::{PiProviderManager, ProviderMetadata};

/// A provider/model pair resolved from CLI flags.
#[derive(Clone, Debug)]
pub struct ResolvedModel {
    pub provider_id: String,
    pub model: String,
}

impl ResolvedModel {
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider_id, self.model)
    }
}

/// Resolves `--model` / `--provider` into an explicit pair.
pub async fn resolve_model(
    pi: &PiProviderManager,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<ResolvedModel, String> {
    let providers = pi.list_providers().await;
    let provider_flag = provider.map(str::trim).filter(|p| !p.is_empty());

    let raw = match model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(raw) => raw,
        None => {
            return Err(
                "No model specified. Use --model provider/model, e.g. --model opencode-go/glm-5.3-flash (see `lcgn --list-models`)."
                    .to_string(),
            )
        }
    };

    // provider/id form.
    if let Some((flagged_provider, model_id)) = raw.split_once('/') {
        if let Some(explicit) = provider_flag {
            if explicit != flagged_provider.trim() {
                return Err(format!(
                    "--provider \"{explicit}\" conflicts with --model \"{raw}\"."
                ));
            }
        }
        validate(&providers, flagged_provider.trim(), model_id)?;
        return Ok(ResolvedModel {
            provider_id: flagged_provider.trim().to_string(),
            model: model_id.to_string(),
        });
    }

    // Bare model id with an explicit provider.
    if let Some(explicit) = provider_flag {
        validate(&providers, explicit, raw)?;
        return Ok(ResolvedModel {
            provider_id: explicit.to_string(),
            model: raw.to_string(),
        });
    }

    // Bare model id alone: must be unambiguous.
    let matching: Vec<&ProviderMetadata> = providers
        .iter()
        .filter(|candidate| candidate.models.iter().any(|m| m == raw))
        .collect();
    match matching.len() {
        0 => Err(format!(
            "Model \"{raw}\" is not offered by any authenticated provider. Run `lcgn --list-models` to see available models."
        )),
        1 => Ok(ResolvedModel {
            provider_id: matching[0].id.clone(),
            model: raw.to_string(),
        }),
        _ => {
            let names: Vec<String> = matching
                .iter()
                .map(|candidate| format!("{}/{}", candidate.id, raw))
                .collect();
            Err(format!(
                "Model \"{raw}\" is ambiguous ({names}). Use --model provider/model to pick one.",
                names = names.join(", ")
            ))
        }
    }
}

fn validate(providers: &[ProviderMetadata], provider_id: &str, model: &str) -> Result<(), String> {
    let Some(entry) = providers.iter().find(|p| p.id == provider_id) else {
        return Err(format!(
            "Provider \"{provider_id}\" is not configured. Run `lcgn auth login {provider_id}` or `lcgn --list-models`."
        ));
    };
    if entry.models.iter().any(|m| m == model) {
        Ok(())
    } else {
        Err(format!(
            "Provider \"{provider_id}\" has no model \"{model}\". Available: {}.",
            entry.models.join(", ")
        ))
    }
}

fn cell(value: Option<i64>) -> String {
    match value {
        Some(n) => n.to_string(),
        None => "-".to_string(),
    }
}

/// Builds the `--list-models` table (provider, model, context, max-out,
/// thinking, images).
pub async fn list_models_table(pi: &PiProviderManager, search: &str) -> String {
    let providers = pi.list_providers().await;
    let needle = search.trim().to_lowercase();
    let mut rows: Vec<[String; 6]> = Vec::new();
    for provider in &providers {
        for model_id in &provider.models {
            if !needle.is_empty()
                && !provider.id.to_lowercase().contains(&needle)
                && !model_id.to_lowercase().contains(&needle)
            {
                continue;
            }
            let info = pi.model_info(&provider.id, model_id);
            rows.push([
                provider.id.clone(),
                model_id.clone(),
                cell(info.as_ref().and_then(|i| i.context_window)),
                cell(info.as_ref().and_then(|i| i.max_tokens)),
                match info.as_ref().and_then(|i| i.reasoning) {
                    Some(true) => "yes".to_string(),
                    Some(false) => "no".to_string(),
                    None => "-".to_string(),
                },
                "-".to_string(),
            ]);
        }
    }
    if rows.is_empty() {
        return if needle.is_empty() {
            "No authenticated models found. Run `lcgn auth login <provider>` first.\n".to_string()
        } else {
            format!("No models match \"{needle}\".\n")
        };
    }

    let header = ["provider", "model", "context", "max-out", "thinking", "images"];
    let widths: [usize; 6] = std::array::from_fn(|i| {
        rows.iter()
            .map(|row| row[i].chars().count())
            .chain(std::iter::once(header[i].len()))
            .max()
            .unwrap_or(0)
    });

    let mut out = String::new();
    let line = |out: &mut String, cells: [String; 6]| {
        for (i, value) in cells.iter().enumerate() {
            out.push_str(value);
            let pad = widths[i].saturating_sub(value.chars().count());
            out.extend(std::iter::repeat(' ').take(pad + 2));
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
    };
    let header_cells: [String; 6] = header.map(str::to_string);
    line(&mut out, header_cells.clone());
    for width in widths {
        out.extend(std::iter::repeat('-').take(width + 2));
    }
    out.push('\n');
    for row in &rows {
        line(&mut out, row.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::pi_agent_dir;
    use std::path::PathBuf;

    fn temp_manager(label: &str) -> (PiProviderManager, PathBuf, PathBuf) {
        let unique = uuid::Uuid::new_v4().to_string();
        let agent = std::env::temp_dir().join(format!("lcgn-models-{label}-{unique}"));
        let support = std::env::temp_dir().join(format!("lcgn-models-{label}-{unique}-support"));
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        (
            PiProviderManager::with_dirs(agent.clone(), support.clone()),
            agent,
            support,
        )
    }

    #[tokio::test]
    async fn resolves_provider_model_pair() {
        let (manager, agent, support) = temp_manager("pair");
        std::fs::write(
            agent.join("models.json"),
            serde_json::json!({
                "providers": {
                    "mockprov": {
                        "baseUrl": "http://127.0.0.1:1/v1",
                        "api": "openai-compatible",
                        "models": [ { "id": "mock-1" }, { "id": "mock-2" } ]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let _ = pi_agent_dir(); // touch module for coverage parity

        let resolved = resolve_model(&manager, Some("mockprov/mock-1"), None)
            .await
            .unwrap();
        assert_eq!(resolved.provider_id, "mockprov");
        assert_eq!(resolved.model, "mock-1");

        let resolved = resolve_model(&manager, Some("mock-1"), Some("mockprov"))
            .await
            .unwrap();
        assert_eq!(resolved.provider_id, "mockprov");

        assert!(resolve_model(&manager, Some("mockprov/nope"), None)
            .await
            .is_err());
        assert!(resolve_model(&manager, Some("ghost/m-1"), None).await.is_err());
        assert!(resolve_model(&manager, None, None).await.is_err());

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[tokio::test]
    async fn bare_model_must_be_unambiguous() {
        let (manager, agent, support) = temp_manager("bare");
        let doc = serde_json::json!({
            "providers": {
                "one": { "baseUrl": "http://127.0.0.1:1/v1", "api": "openai-compatible", "models": [ { "id": "solo" } ] },
                "two": { "baseUrl": "http://127.0.0.1:1/v1", "api": "openai-compatible", "models": [ { "id": "dupe" }, { "id": "solo2" } ] }
            }
        });
        std::fs::write(agent.join("models.json"), doc.to_string()).unwrap();

        let resolved = resolve_model(&manager, Some("solo"), None).await.unwrap();
        assert_eq!(resolved.provider_id, "one");

        let resolved = resolve_model(&manager, Some("dupe"), None).await.unwrap();
        assert_eq!(resolved.provider_id, "two");

        // "solo2" is unique too.
        let resolved = resolve_model(&manager, Some("solo2"), None).await.unwrap();
        assert_eq!(resolved.provider_id, "two");

        assert!(resolve_model(&manager, Some("missing"), None).await.is_err());
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[tokio::test]
    async fn table_lists_models_with_columns() {
        let (manager, agent, support) = temp_manager("table");
        std::fs::write(
            agent.join("models.json"),
            serde_json::json!({
                "providers": {
                    "mockprov": {
                        "baseUrl": "http://127.0.0.1:1/v1",
                        "api": "openai-compatible",
                        "models": [ { "id": "mock-1", "contextWindow": 128000, "maxTokens": 8192, "reasoning": true } ]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let table = list_models_table(&manager, "").await;
        assert!(table.contains("provider"));
        assert!(table.contains("mockprov"));
        assert!(table.contains("mock-1"));
        assert!(table.contains("128000"));
        assert!(table.contains("8192"));
        assert!(table.contains("yes"));

        let filtered = list_models_table(&manager, "mock").await;
        assert!(filtered.contains("mock-1"));
        let empty = list_models_table(&manager, "zzz-nope").await;
        assert!(empty.contains("No models match"));

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }
}
