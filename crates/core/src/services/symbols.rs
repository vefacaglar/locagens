//! Lightweight code symbol extraction — port of `SymbolExtractor.ts` +
//! `SymbolIndexer.ts` onto regex + file walking (no mtime cache needed for
//! Settings-scale projects).

use crate::util::canonical_project_path;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CodeSymbol {
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line: usize,
    pub preview: String,
}


const SUPPORTED_EXTENSIONS: [&str; 11] =
    ["ts", "js", "tsx", "jsx", "vue", "py", "go", "rs", "mjs", "cjs", "json"];

pub const VALID_KINDS: [&str; 6] = ["function", "class", "interface", "type", "variable", "component"];

struct SymbolRegexes {
    ts_function: Regex,
    ts_arrow: Regex,
    ts_class: Regex,
    ts_interface: Regex,
    ts_type: Regex,
    py_function: Regex,
    py_class: Regex,
    go_function: Regex,
    go_type: Regex,
    rs_function: Regex,
    rs_struct: Regex,
}

fn regexes() -> &'static SymbolRegexes {
    static RE: OnceLock<SymbolRegexes> = OnceLock::new();
    RE.get_or_init(|| SymbolRegexes {
        ts_function: Regex::new(r"^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([a-zA-Z0-9_$]+)").unwrap(),
        ts_arrow: Regex::new(r"^(?:export\s+)?(?:const|let|var)\s+([a-zA-Z0-9_$]+)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[a-zA-Z0-9_$]+)\s*=>").unwrap(),
        ts_class: Regex::new(r"^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([a-zA-Z0-9_$]+)").unwrap(),
        ts_interface: Regex::new(r"^(?:export\s+)?interface\s+([a-zA-Z0-9_$]+)").unwrap(),
        ts_type: Regex::new(r"^(?:export\s+)?type\s+([a-zA-Z0-9_$]+)\s*=").unwrap(),
        py_function: Regex::new(r"^(?:async\s+)?def\s+([a-zA-Z0-9_]+)\s*\(").unwrap(),
        py_class: Regex::new(r"^class\s+([a-zA-Z0-9_]+)").unwrap(),
        go_function: Regex::new(r"^func\s+(?:\([^)]+\)\s+)?([a-zA-Z0-9_]+)\s*\(").unwrap(),
        go_type: Regex::new(r"^type\s+([a-zA-Z0-9_]+)\s+(struct|interface)").unwrap(),
        rs_function: Regex::new(r"^(?:pub\s+)?(?:async\s+)?fn\s+([a-zA-Z0-9_]+)").unwrap(),
        rs_struct: Regex::new(r"^(?:pub\s+)?(?:struct|enum|trait)\s+([a-zA-Z0-9_]+)").unwrap(),
    })
}

/// Extracts code symbols (functions, classes, interfaces, types) from a file.
pub fn extract(file_path: &str, content: &str) -> Vec<CodeSymbol> {
    let re = regexes();
    let ext = file_path.rsplit('.').next().unwrap_or("").to_lowercase();
    let is_web = matches!(ext.as_str(), "ts" | "js" | "tsx" | "jsx" | "vue" | "mjs" | "cjs");

    let mut symbols = Vec::new();
    for (idx, raw_line) in content.split('\n').enumerate() {
        let line = raw_line.trim();
        if line.is_empty()
            || line.starts_with("//")
            || line.starts_with('#')
            || line.starts_with("/*")
            || line.starts_with('*')
        {
            continue;
        }
        let line_num = idx + 1;
        let preview: String = line.chars().take(100).collect();

        let push = |symbols: &mut Vec<CodeSymbol>, name: String, kind: &str| {
            symbols.push(CodeSymbol {
                name,
                kind: kind.to_string(),
                file_path: file_path.to_string(),
                line: line_num,
                preview: preview.clone(),
            });
        };

        if is_web {
            if let Some(name) = re.ts_function.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "function");
                continue;
            }
            if let Some(name) = re.ts_arrow.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "function");
                continue;
            }
            if let Some(name) = re.ts_class.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "class");
                continue;
            }
            if let Some(name) = re.ts_interface.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "interface");
                continue;
            }
            if let Some(name) = re.ts_type.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "type");
                continue;
            }
        }

        if ext == "py" {
            if let Some(name) = re.py_function.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "function");
                continue;
            }
            if let Some(name) = re.py_class.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "class");
                continue;
            }
        }

        if ext == "go" {
            if let Some(name) = re.go_function.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "function");
                continue;
            }
            if let Some(caps) = re.go_type.captures(line) {
                if let (Some(name), Some(gotype)) = (caps.get(1), caps.get(2)) {
                    let kind = if gotype.as_str() == "interface" { "interface" } else { "class" };
                    push(&mut symbols, name.as_str().to_string(), kind);
                    continue;
                }
            }
        }

        if ext == "rs" {
            if let Some(name) = re.rs_function.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "function");
                continue;
            }
            if let Some(name) = re.rs_struct.captures(line).and_then(|c| c.get(1)) {
                push(&mut symbols, name.as_str().to_string(), "class");
                continue;
            }
        }
    }
    symbols
}

fn list_files(project_path: &Path) -> Vec<String> {
    // Prefer git's file list (respects .gitignore via -co --exclude-standard).
    // Metadata a sandboxed command swapped is repaired first, so git never
    // reads a planted config (the repair itself is reported by the tools).
    let _ = crate::services::git::check_sandbox_metadata(project_path);
    if let Ok(output) = crate::services::git::git_command(project_path, &["ls-files", "-co", "--exclude-standard"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return stdout
                .split('\n')
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
                .collect();
        }
    }

    // Fallback: bounded directory traversal.
    let mut files = Vec::new();
    walk(project_path, "", &mut files, 0);
    files
}

fn walk(dir: &Path, base: &str, files: &mut Vec<String>, depth: usize) {
    if depth > 6 || files.len() > 500 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "dist" || name == "build" {
            continue;
        }
        let rel = if base.is_empty() { name.clone() } else { format!("{base}/{name}") };
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            walk(&entry.path(), &rel, files, depth + 1);
        } else if file_type.is_file() {
            files.push(rel);
        }
    }
}

/// Searches symbols across the project matching the query and optional kind filter.
pub fn search(
    project_path: &Path,
    query: &str,
    kind: Option<&str>,
    limit: usize,
) -> Vec<CodeSymbol> {
    let q = query.trim().to_lowercase();
    let mut all_symbols = Vec::new();
    for rel_path in list_files(project_path) {
        let ext = rel_path.rsplit('.').next().unwrap_or("").to_lowercase();
        if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let full_path: PathBuf = project_path.join(&rel_path);
        let Ok(content) = std::fs::read_to_string(&full_path) else {
            continue;
        };
        all_symbols.extend(extract(&rel_path, &content));
    }

    all_symbols
        .into_iter()
        .filter(|sym| {
            if let Some(kind) = kind {
                if sym.kind != kind {
                    return false;
                }
            }
            if q.is_empty() {
                return true;
            }
            sym.name.to_lowercase().contains(&q)
                || sym.file_path.to_lowercase().contains(&q)
                || sym.preview.to_lowercase().contains(&q)
        })
        .take(limit)
        .collect()
}

/// Validates the optional `kind` query parameter.
pub fn valid_kind(kind: Option<&str>) -> Option<&'static str> {
    kind.and_then(|k| VALID_KINDS.iter().find(|valid| **valid == k).copied())
}

/// Public entry point used by the facade: canonicalizes and searches.
pub fn search_canonical(raw_path: &str, query: &str, kind: Option<&str>) -> Result<Vec<CodeSymbol>, String> {
    let canonical = canonical_project_path(raw_path)?;
    Ok(search(&canonical, query, valid_kind(kind), 60))
}
