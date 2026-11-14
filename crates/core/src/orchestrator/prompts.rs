//! System prompts of the main agent and of sub-agents.

use super::*;

// ----------------------------------------------------------------------
// System prompts
// ----------------------------------------------------------------------

pub(super) fn memories_block(state: &AppState, project_path: &str) -> String {
    let all = state.db.list_memories();
    let memories: Vec<&crate::db::Memory> = all
        .iter()
        .filter(|memory| memory.scope == "global" || (memory.scope == "project" && memory.project_path == project_path))
        .collect();
    if memories.is_empty() {
        return String::new();
    }
    let mut block = String::from("\n\n## Long-term memory\n");
    for memory in memories {
        block.push_str(&format!("- [{}] {}\n", memory.category, memory.content));
    }
    block
}

pub(super) fn mode_rules(mode: &str) -> String {
    match mode {
        "chat" => "## Mode: chat\nConversation only. Do not proactively touch the workspace, run commands, or start sub-agents; answer from context and ask before acting.".to_string(),
        "plan" => "## Mode: plan\nPlan only. Do NOT modify files or execute commands. Research (read/list/search) is allowed; communicate the plan as text and maintain it via update_plan.".to_string(),
        "ask_permissions" => "## Mode: ask permissions\nEvery tool call needs user approval unless a standing grant covers it; file mutations additionally show a diff preview. Make each tool call self-explanatory.".to_string(),
        "auto" => "## Mode: full access\nWork autonomously: apply edits and run commands without waiting for approval. Still avoid destructive actions unless explicitly requested.".to_string(),
        // accept_edits
        _ => "## Mode: build\nApply file edits directly (write_file/edit_file/move_file need no approval). Running commands and network fetches still pause for approval unless granted.".to_string(),
    }
}

pub(super) fn system_prompt(state: &AppState, run: &crate::db::Run) -> String {
    // `--system-prompt` replaces the base prompt; `--append-system-prompt`
    // appends to whatever base ends up being used.
    let mut prompt = if run.system_prompt.as_deref().map(str::trim).map(str::is_empty) == Some(false)
        || run.append_system_prompt.is_some()
    {
        let mut prompt = match run.system_prompt.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(custom) => custom.to_string(),
            None => base_system_prompt(state, run),
        };
        if let Some(append) = run.append_system_prompt.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            prompt.push_str("\n\n");
            prompt.push_str(append);
        }
        prompt
    } else {
        base_system_prompt(state, run)
    };
    append_plugin_prompts(&mut prompt, &run.project_path);
    prompt
}

/// Appends the `systemPrompt` sections of every enabled plugin.
pub(super) fn append_plugin_prompts(prompt: &mut String, project_path: &str) {
    let prompts = crate::services::plugins::enabled_system_prompts(Some(project_path));
    if prompts.is_empty() {
        return;
    }
    prompt.push_str("\n\n## Active plugins\n");
    for (name, text) in prompts {
        prompt.push_str(&format!("\n### {name}\n{text}\n"));
    }
}

pub(super) fn base_system_prompt(state: &AppState, run: &crate::db::Run) -> String {
    let project = if run.project_path.is_empty() {
        "No project folder selected."
    } else {
        "Workspace root (all file tool paths are relative to it):"
    };
    format!(
        "You are the Locagens coding agent, working inside the user's project.\n\n\
{project} {path}\n{project_name}\n\n{mode}\n{agents_md}\n{memories}\n\n\
## Working style\n\
- Inspect before changing: read the relevant files first.\n\
- Prefer apply_patch/edit_file for small changes; write_file for new files.\n\
- Keep answers concise and concrete; summarize what you did.\n\
- Use update_plan for multi-step work so the user can follow progress.\n\
- Fan independent research or implementation sub-tasks out via spawn_agents / spawn_agent (explorer / worker).\n\
- Use ask_user_question when a decision genuinely needs the user.\n",
        project = project,
        path = run.project_path,
        project_name = {
            let name = &run.project_name;
            if name.is_empty() { String::new() } else { format!("(project: {name})") }
        },
        mode = mode_rules(&run.mode),
        agents_md = agents_md_block(&run.project_path),
        memories = memories_block(state, &run.project_path),
    )
}

/// Hierarchical AGENTS.md docs (Codex discovery: git root → cwd, 32 KiB
/// budget, AGENTS.override.md > AGENTS.md > CLAUDE.md > .agents.md).
pub(super) fn agents_md_block(project_path: &str) -> String {
    if project_path.trim().is_empty() {
        return String::new();
    }
    let cwd = PathBuf::from(project_path);
    let (docs, sources) = agents_md::load_agents_md_with(&cwd, &cwd, &crate::config_runtime::doc_options());
    if docs.trim().is_empty() {
        return String::new();
    }
    let source_list = sources
        .iter()
        .map(|source| source.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n## Project instructions (AGENTS.md hierarchy from {source_list})\n\n{docs}\n"
    )
}

pub(super) fn sub_agent_system_prompt(role: &str, title: &str, task: &str, project_path: &str, readonly: bool) -> String {
    let capability = if readonly {
        "You are read-only: you may read files, list directories and search, but never modify anything."
    } else {
        "You have full workspace tools: read, write, edit, move and search files; run commands and fetch URLs only when genuinely needed."
    };
    format!(
        "You are a {role} sub-agent (\"{title}\") spawned inside the Locagens orchestrator.\n\
Workspace root: {project_path}\n\n{capability}\n\n\
Your assigned task:\n{task}\n\n\
Work independently and completely; do not ask the user questions. When done, reply with a concise final summary of what you did and any important outcomes — it is returned to the coordinating agent as your task result. You may later receive follow-up tasks or messages; treat them as continuing this same job.\n{agents_md}",
        agents_md = agents_md_block(project_path),
    )
}

#[cfg(test)]
mod sub_agent_prompt_tests {
    use super::*;

    #[test]
    fn sub_agents_get_the_project_instructions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "Use tabs, never spaces.").unwrap();
        let path = dir.path().display().to_string();
        let prompt = sub_agent_system_prompt("explorer", "scan", "look around", &path, true);
        assert!(prompt.contains("Use tabs, never spaces."), "{prompt}");
        assert!(prompt.contains("## Project instructions"));
    }
}
