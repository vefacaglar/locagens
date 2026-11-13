//! Memories: one Markdown file per fact (global and per project).

use super::*;

impl Db {
    // ------------------------------------------------------------------
    // Memories (one Markdown file per fact)
    // ------------------------------------------------------------------

    pub fn list_memories(&self) -> Vec<Memory> {
        self.sync_shared_files(false, false, true);
        let mut list = self.read().memories.clone();
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list
    }

    pub fn create_memory(
        &self,
        scope: &str,
        project_path: &str,
        category: &str,
        content: &str,
    ) -> anyhow::Result<Memory> {
        self.with_shared_mutation(|state| {
            let now = now_iso();
            let memory = Memory {
                id: state.next_memory_id,
                scope: scope.to_string(),
                project_path: project_path.to_string(),
                category: category.to_string(),
                content: content.to_string(),
                created_at: now.clone(),
                updated_at: now,
            };
            let path = memory_file_path(&self.root, &memory);
            write_text_atomic(&path, &render_memory(&memory))?;
            state.next_memory_id += 1;
            state.memory_files.insert(memory.id, path);
            state.memories.push(memory.clone());
            Ok(memory)
        })
    }

    pub fn update_memory(&self, id: i64, content: &str) -> Option<Memory> {
        self.with_shared_mutation(|state| {
            let memory = state.memories.iter_mut().find(|memory| memory.id == id)?;
            memory.content = content.to_string();
            memory.updated_at = now_iso();
            let updated = memory.clone();
            let path = state
                .memory_files
                .entry(id)
                .or_insert_with(|| memory_file_path(&self.root, &updated))
                .clone();
            if let Err(err) = write_text_atomic(&path, &render_memory(&updated)) {
                tracing::error!("[Database] Failed to write {}: {err}", path.display());
            }
            Some(updated)
        })
    }

    pub fn delete_memory(&self, id: i64) -> bool {
        self.with_shared_mutation(|state| {
            let before = state.memories.len();
            state.memories.retain(|memory| memory.id != id);
            if let Some(path) = state.memory_files.remove(&id) {
                let _ = std::fs::remove_file(path);
            }
            state.memories.len() != before
        })
    }

    pub fn clear_memories(&self) {
        self.with_shared_mutation(|state| {
            for (_, path) in state.memory_files.drain() {
                let _ = std::fs::remove_file(path);
            }
            state.memories.clear();
        })
    }
}

// ----------------------------------------------------------------------
// Memories as Markdown
// ----------------------------------------------------------------------

/// Directory name for a project path: every non-alphanumeric char becomes `-`.
pub(crate) fn sanitize_project_path(path: &str) -> String {
    let name: String = path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .take(120)
        .collect();
    if name.is_empty() { "project".to_string() } else { name }
}

pub(super) fn memory_dir(root: &Path, memory: &Memory) -> PathBuf {
    if memory.project_path.is_empty() {
        root.join("memories")
    } else {
        root.join("projects")
            .join(sanitize_project_path(&memory.project_path))
            .join("memories")
    }
}

/// `<first words of the fact>-<id>.md`.
pub(super) fn memory_file_path(root: &Path, memory: &Memory) -> PathBuf {
    let words: Vec<String> = memory
        .content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(6)
        .map(|word| word.to_lowercase())
        .collect();
    let mut slug: String = words.join("-").chars().take(40).collect();
    if slug.is_empty() {
        slug = "memory".to_string();
    }
    memory_dir(root, memory).join(format!("{slug}-{}.md", memory.id))
}

/// YAML frontmatter (string values JSON-quoted, which is valid YAML and
/// needs no further escaping) followed by the verbatim body.
pub(super) fn render_memory(memory: &Memory) -> String {
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let mut text = String::from("---\n");
    text.push_str(&format!("id: {}\n", memory.id));
    text.push_str(&format!("category: {}\n", quote(&memory.category)));
    text.push_str(&format!("scope: {}\n", quote(&memory.scope)));
    if !memory.project_path.is_empty() {
        text.push_str(&format!("project_path: {}\n", quote(&memory.project_path)));
    }
    text.push_str(&format!("created_at: {}\n", quote(&memory.created_at)));
    text.push_str(&format!("updated_at: {}\n", quote(&memory.updated_at)));
    text.push_str("---\n");
    text.push_str(&memory.content);
    text
}

pub(super) fn parse_frontmatter_value(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with('"') {
        if let Ok(value) = serde_json::from_str::<String>(raw) {
            return value;
        }
    }
    raw.trim_matches('\'').to_string()
}

pub(super) fn parse_memory(raw: &str) -> Option<Memory> {
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    let mut memory = Memory::default();
    let mut has_id = false;
    let mut offset = 0;
    let mut body = None;
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == "---" {
            body = Some(&rest[offset..]);
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = parse_frontmatter_value(value);
        match key.trim() {
            "id" => {
                memory.id = value.parse().ok()?;
                has_id = true;
            }
            "category" => memory.category = value,
            "scope" => memory.scope = value,
            "project_path" => memory.project_path = value,
            "created_at" => memory.created_at = value,
            "updated_at" => memory.updated_at = value,
            _ => {}
        }
    }
    if !has_id {
        return None;
    }
    memory.content = body?.to_string();
    if memory.scope.is_empty() {
        memory.scope = if memory.project_path.is_empty() { "global" } else { "project" }.to_string();
    }
    Some(memory)
}

/// The global memory directory plus one per project directory.
pub(super) fn memory_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("memories")];
    if let Ok(entries) = std::fs::read_dir(root.join("projects")) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                dirs.push(entry.path().join("memories"));
            }
        }
    }
    dirs
}

/// Cheap fingerprint of all memory files (path, mtime, size): changes when
/// another process adds, edits or removes a memory.
pub(super) fn memories_signature(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(PathBuf, FileSig)> = Vec::new();
    for dir in memory_dirs(root) {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let sig = file_sig(&path);
                entries.push((path, sig));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut hasher);
    hasher.finish()
}

/// Reads every memory `.md` (global and per-project) with its file path.
pub(super) fn load_memories(root: &Path) -> Vec<(Memory, PathBuf)> {
    let mut found = Vec::new();
    for dir in memory_dirs(root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(memory) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| parse_memory(&raw))
            {
                found.push((memory, path));
            }
        }
    }
    found.sort_by_key(|(memory, _)| memory.id);
    found
}
