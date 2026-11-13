//! Pure Rust workspace & orchestrator tools.
//!
//! Every path argument is resolved against the run's project folder and
//! canonicalized so nothing escapes the workspace (symlink-safe containment
//! via `canonicalize` + `starts_with`). Dangerous tools (`run_command`,
//! `fetch_url`) are permission-gated by the orchestrator; `build_preview`
//! produces the red/green diff payload the UI permission card renders.

use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Maximum bytes returned by any single tool response (keeps LLM context and
/// the UI bounded).
const MAX_OUTPUT_CHARS: usize = 24_000;
/// Maximum bytes read from a single file.
const MAX_FILE_READ_CHARS: usize = 120_000;
/// Directory names skipped by `search_files`.
const SEARCH_SKIP_DIRS: [&str; 8] =
    [".git", "node_modules", "target", "dist", ".next", "build", "vendor", ".venv"];
const MAX_SEARCH_DEPTH: usize = 14;

/// Tools that mutate the workspace (permission-gated in `ask_permissions`).
pub const MUTATION_TOOLS: [&str; 6] = ["write_file", "edit_file", "delete_file", "move_file", "create_directory", "apply_patch"];

/// Whether a tool call needs interactive permission approval under `mode`.
pub fn needs_permission(mode: &str, tool: &str) -> bool {
    if mode == "auto" {
        return false;
    }
    let dangerous = matches!(tool, "run_command" | "exec_command" | "write_stdin" | "fetch_url");
    let mutation = MUTATION_TOOLS.contains(&tool);
    let mcp = tool.starts_with("mcp__") && mode == "ask_permissions";
    (dangerous && mode != "plan" && mode != "chat") || (mutation && mode == "ask_permissions") || mcp
}

/// Whether a tool is flat-out forbidden in read-only (`plan` / `chat`) mode:
/// nothing that mutates the workspace or executes commands gets through.
pub fn is_forbidden_in_readonly_mode(mode: &str, tool: &str) -> bool {
    (mode == "plan" || mode == "chat")
        && (MUTATION_TOOLS.contains(&tool)
            || matches!(tool, "run_command" | "exec_command" | "write_stdin"))
}

// ----------------------------------------------------------------------
// Tool definitions (OpenAI function-tool shape)
// ----------------------------------------------------------------------

fn def(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            }
        }
    })
}

fn path_prop(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

/// The full toolset. Subsets are produced by filtering names via
/// `filter_tool_defs`.
pub fn all_tool_defs() -> Vec<Value> {
    let mut defs = base_tool_defs();
    defs.extend(module_tool_defs());
    defs
}

fn base_tool_defs() -> Vec<Value> {
    vec![
        def("read_file", "Read a text file from the workspace. Returns numbered lines. Use start_line/end_line for large files.",
            json!({
                "path": path_prop("Relative path inside the workspace."),
                "start_line": json!({ "type": "integer", "description": "1-based first line to return." }),
                "end_line": json!({ "type": "integer", "description": "1-based last line to return (inclusive)." })
            }), &["path"]),
        def("write_file", "Create a file (or overwrite an existing one) with the given content inside the workspace. Parent directories are created automatically.",
            json!({
                "path": path_prop("Relative path inside the workspace."),
                "content": json!({ "type": "string", "description": "Full file content to write." })
            }), &["path", "content"]),
        def("edit_file", "Replace the first occurrence of old_text with new_text in a workspace file. old_text must match exactly and be unique.",
            json!({
                "path": path_prop("Relative path inside the workspace."),
                "old_text": json!({ "type": "string", "description": "Exact existing text to replace." }),
                "new_text": json!({ "type": "string", "description": "Replacement text." })
            }), &["path", "old_text", "new_text"]),
        def("delete_file", "Delete a file from the workspace.",
            json!({ "path": path_prop("Relative path inside the workspace.") }), &["path"]),
        def("list_directory", "List the entries of a workspace directory. Directories are suffixed with '/'.",
            json!({ "path": json!({ "type": "string", "description": "Relative directory path; defaults to the workspace root." }) }), &[]),
        def("create_directory", "Create a directory (and parents) inside the workspace.",
            json!({ "path": path_prop("Relative directory path.") }), &["path"]),
        def("move_file", "Move or rename a file inside the workspace.",
            json!({
                "from": path_prop("Source relative path."),
                "to": path_prop("Destination relative path.")
            }), &["from", "to"]),
        def("search_files", "Search file contents across the workspace (skips .git/node_modules/target). Returns path:line matches with context.",
            json!({
                "query": json!({ "type": "string", "description": "Text to search for (regular expression allowed)." }),
                "path": json!({ "type": "string", "description": "Optional subdirectory to search within." }),
                "file_pattern": json!({ "type": "string", "description": "Optional filename filter, e.g. '*.rs' or 'README'." })
            }), &["query"]),
        def("run_command", "Execute a shell command in the project folder (120s timeout). Outbound network is denied unless the matching domains are approved. Secrets are stripped from the environment.",
            json!({
                "command": json!({ "type": "string", "description": "The shell command to execute." }),
                "network_domains": json!({ "type": "array", "items": { "type": "string" }, "description": "Explicitly required outbound hosts (e.g. npm registry)." })
            }), &["command"]),
        def("fetch_url", "Fetch a public http(s) URL and return the response body (30s timeout, private IPs blocked).",
            json!({ "url": json!({ "type": "string", "description": "Absolute http(s) URL." }) }), &["url"]),
        def("apply_patch", "Apply a Codex-format patch (`*** Begin Patch` … `*** End Patch`) to the workspace: add, delete, update and move files with fuzzy context matching. Preferred for multi-file edits.",
            json!({
                "input": json!({ "type": "string", "description": "Patch text in *** Begin Patch ... *** End Patch format." }),
                "patch": json!({ "type": "string", "description": "Alias for `input` (Codex compatibility)." })
            }), &["input"]),
        def("exec_command", "Run a shell command in the workspace (Codex unified exec). If it finishes within yield_time_ms the result is returned directly; otherwise it keeps running as a background session you can drive with write_stdin.",
            json!({
                "cmd": json!({ "type": "string", "description": "The command to execute." }),
                "workdir": json!({ "type": "string", "description": "Optional working directory inside the workspace." }),
                "yield_time_ms": json!({ "type": "integer", "description": "Wait up to this long for completion (default 10000, clamped 500..60000)." }),
                "max_output_tokens": json!({ "type": "integer", "description": "Rough bound on returned output (default 20000 tokens ~ 80k chars)." }),
                "shell": json!({ "type": "string", "description": "Optional shell program for the session (e.g. `bash`, `zsh`); defaults to the platform shell." }),
                "login": json!({ "type": "boolean", "description": "Run the shell as a login shell (-lc) so profile/rc files load." }),
                "tty": json!({ "type": "boolean", "description": "Allocate a pseudo-terminal for the session (enables TTY-aware programs like progress bars and editors)." }),
                "justification": json!({ "type": "string", "description": "Short reason shown on the permission card." })
            }), &["cmd"]),
        def("write_stdin", "Send characters to a running exec session and collect its latest output. Omit `chars` to just poll.",
            json!({
                "session_id": json!({ "type": "string", "description": "Session id returned by exec_command." }),
                "chars": json!({ "type": "string", "description": "Characters to write to the session's stdin." }),
                "yield_time_ms": json!({ "type": "integer", "description": "Wait up to this long for new output (default 2000)." }),
                "max_output_tokens": json!({ "type": "integer", "description": "Rough bound on returned output." })
            }), &["session_id"]),
        def("view_image", "Inspect an image file inside the workspace: returns its mime type, byte size and pixel dimensions.",
            json!({ "path": path_prop("Relative image path inside the workspace.") }), &["path"]),
        def("request_user_input", "Ask the user one or more multiple-choice questions (Codex schema). The run pauses until the user answers.",
            json!({
                "questions": json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "header": { "type": "string" },
                            "question": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label"]
                                }
                            }
                        },
                        "required": ["question"]
                    }
                })
            }), &["questions"]),
        def("compact_context", "Manually compact the conversation: summarize older messages into a context-summary block and drop them from the working history (also triggered automatically near the token threshold).",
            json!({}), &[]),
        def("set_chat_title", "Set the chat title shown in the sidebar (max ~60 characters). Call this once when the topic of the conversation is clear.",
            json!({ "title": json!({ "type": "string" }) }), &["title"]),
        def("update_plan", "Create or update the visible plan for this run. Tasks appear as a checklist in the plan panel.",
            json!({
                "title": json!({ "type": "string", "description": "Short plan title." }),
                "body": json!({ "type": "string", "description": "Optional markdown context / rationale." }),
                "tasks": json!({
                    "type": "array",
                    "description": "Checklist steps.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["text"]
                    }
                })
            }), &["title", "tasks"]),
        def("remember", "Persist a durable fact to long-term memory so it survives across sessions.",
            json!({
                "scope": json!({ "type": "string", "enum": ["global", "project"], "description": "global memories apply everywhere; project memories only to this project." }),
                "category": json!({ "type": "string", "enum": ["user", "feedback", "project", "reference"] }),
                "content": json!({ "type": "string", "description": "The fact to remember, one sentence." })
            }), &["scope", "category", "content"]),
        def("load_skill", "Load a specialized instruction pack (SKILL.md) by name for detailed guidance on a task.",
            json!({ "name": json!({ "type": "string", "description": "Skill name." }) }), &["name"]),
        def("ask_user_question", "Ask the user one or more multiple-choice questions. The run pauses until the user answers.",
            json!({
                "questions": json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "The question text." },
                            "header": { "type": "string", "description": "Very short chip label (1-3 words)." },
                            "multiSelect": { "type": "boolean" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label"]
                                }
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                })
            }), &["questions"]),
        def("spawn_agents", "Run parallel sub-agents with the current model: 'explorer' (read-only research) or 'worker' (full workspace tools).",
            json!({
                "agents": json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "role": { "type": "string", "enum": ["explorer", "worker"] },
                            "task": { "type": "string", "description": "Self-contained task description." }
                        },
                        "required": ["role", "task"]
                    }
                })
            }), &["agents"]),
        def("list_mcp_resources", "List resources exposed by connected MCP servers (cursors paginate). Returns pretty JSON with a 'resources' array.",
            json!({
                "server": json!({ "type": "string", "description": "Optional MCP server name filter; lists all servers when omitted." }),
                "cursor": json!({ "type": "string", "description": "Optional pagination cursor from a previous list." })
            }), &[]),
        def("read_mcp_resource", "Read a resource from an MCP server by URI and return its JSON contents.",
            json!({
                "server": json!({ "type": "string", "description": "MCP server name." }),
                "uri": json!({ "type": "string", "description": "Resource URI as returned by list_mcp_resources." })
            }), &["server", "uri"]),
    ]
}

/// Tools implemented in their own modules (agents, context, host lookup).
fn module_tool_defs() -> Vec<Value> {
    let mut defs = super::agents::tool_defs(&super::agent_roles::builtin_roles());
    defs.push(super::context_remaining::tool_def());
    defs.push(super::host_executable::tool_def());
    defs
}

/// Filters the full toolset down to the named tools (order preserved from
/// `all_tool_defs`).
pub fn filter_tool_defs(names: &[&str]) -> Vec<Value> {
    all_tool_defs()
        .into_iter()
        .filter(|tool| {
            tool.pointer("/function/name")
                .and_then(Value::as_str)
                .map(|name| names.contains(&name))
                .unwrap_or(false)
        })
        .collect()
}

/// Read-only research toolset (explorer / utility sub-agents).
pub fn readonly_tool_defs(include_move: bool) -> Vec<Value> {
    let mut names: Vec<&str> = vec!["read_file", "list_directory", "search_files"];
    if include_move {
        names.push("move_file");
    }
    filter_tool_defs(&names)
}

// ----------------------------------------------------------------------
// Workspace path containment
// ----------------------------------------------------------------------

/// Resolves `relative` against `project`, refusing any path that escapes the
/// workspace (including via symlinks). Non-existent targets are validated by
/// their deepest existing ancestor.
pub fn safe_path(project: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim();
    if relative.is_empty() {
        return Ok(project.to_path_buf());
    }
    if relative.contains('\0') {
        return Err("Path contains a null byte.".to_string());
    }
    let candidate = project.join(relative);

    // Walk up to the deepest existing ancestor, collecting the missing tail.
    let mut missing_tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe: PathBuf = candidate.clone();
    while !probe.exists() {
        match probe.file_name() {
            Some(file) => missing_tail.push(file.to_os_string()),
            None => return Err("Path could not be resolved.".to_string()),
        }
        if !probe.pop() {
            return Err("Path could not be resolved.".to_string());
        }
    }
    let deepest = std::fs::canonicalize(&probe)
        .map(crate::util::normalize_canonical)
        .map_err(|_| "Path could not be resolved.".to_string())?;
    let project_canonical = std::fs::canonicalize(project)
        .map(crate::util::normalize_canonical)
        .map_err(|_| "Project folder is not accessible.".to_string())?;
    if !deepest.starts_with(&project_canonical) {
        return Err("Path is outside the workspace.".to_string());
    }
    let mut resolved = deepest;
    for part in missing_tail.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn truncate_output(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_CHARS {
        text.truncate(MAX_OUTPUT_CHARS);
        text.push_str("\n... (output truncated)");
    }
    text
}

// ----------------------------------------------------------------------
// Tool execution
// ----------------------------------------------------------------------

/// Executes a workspace/dangerous tool, returning the tool-response content.
/// Interactive orchestrator tools (plan/title/memory/skills/questions) and
/// delegation tools are handled by the orchestrator itself.
pub async fn execute(project_path: &Path, name: &str, args: &Value) -> Result<String, String> {
    execute_with_mode(project_path, "accept_edits", name, args).await
}

/// Executes a workspace/dangerous tool under the given run `mode` (the mode
/// shapes the native sandbox used for `run_command`).
pub async fn execute_with_mode(
    project_path: &Path,
    mode: &str,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    match name {
        "read_file" => read_file(project_path, args),
        "write_file" => write_file(project_path, args),
        "edit_file" => edit_file(project_path, args),
        "delete_file" => delete_file(project_path, args),
        "list_directory" => list_directory(project_path, args),
        "create_directory" => create_directory(project_path, args),
        "move_file" => move_file(project_path, args),
        "search_files" => search_files(project_path, args),
        "run_command" => run_command(project_path, mode, args).await,
        "fetch_url" => fetch_url(args).await,
        "view_image" => view_image(project_path, args),
        _ => Err(format!("Unknown tool \"{name}\".")),
    }
}

/// Builds the secret-stripped environment for exec sessions (`exec_command`).
pub fn exec_session_env() -> std::collections::HashMap<String, String> {
    const SAFE: [&str; 18] = [
        "PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "TEMP", "TMP", "HOME", "USERPROFILE",
        "APPDATA", "LOCALAPPDATA", "PROGRAMFILES", "NUMBER_OF_PROCESSORS", "OS", "PROCESSOR_ARCHITECTURE",
        "LANG", "TERM", "SHELL",
    ];
    let mut env = std::collections::HashMap::new();
    for (name, value) in std::env::vars() {
        if is_secret_env(&name) {
            continue;
        }
        if SAFE.contains(&name.as_str()) || name.starts_with("SYSTEMROOT") {
            env.insert(name, value);
        }
    }
    // Long build/tool output never blocks on a pager; keep ANSI color noise
    // out of model-visible output.
    env.insert("PAGER".to_string(), "cat".to_string());
    env.insert("GIT_PAGER".to_string(), "cat".to_string());
    env.insert("GH_PAGER".to_string(), "cat".to_string());
    env.insert("NO_COLOR".to_string(), "1".to_string());
    env
}

/// Maximum image size `view_image` will inline as base64 (10 MiB).
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// `view_image`: reports mime type, byte size, pixel dimensions and the
/// base64-encoded image data (for real multimodal viewing by the model).
fn view_image(project: &Path, args: &Value) -> Result<String, String> {
    let relative = str_arg(args, "path").unwrap_or_default();
    let path = safe_path(project, relative).map_err(|err| format!("view_image: {err}"))?;
    let metadata = std::fs::metadata(&path)
        .map_err(|err| format!("view_image: cannot read {}: {err}", path.display()))?;
    if metadata.is_dir() {
        return Err("view_image: path is a directory.".to_string());
    }
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let mime_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        _ => return Err(format!("view_image: unsupported image extension '{extension}'.")),
    };
    let bytes = metadata.len();
    if bytes > MAX_IMAGE_BYTES {
        return Err(format!(
            "view_image: image is {} bytes, which exceeds the {} MiB limit.",
            bytes,
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    let raw_bytes = std::fs::read(&path)
        .map_err(|err| format!("view_image: cannot read {}: {err}", path.display()))?;
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&raw_bytes);
    let (width, height) = image_dimensions(&path, &extension)
        .unwrap_or_else(|| (0, 0));
    Ok(serde_json::json!({
        "ok": true,
        "path": relative,
        "mime_type": mime_type,
        "bytes": bytes,
        "width": width,
        "height": height,
        "data_base64": b64,
        "data_url": format!("data:{mime_type};base64,{b64}"),
    })
    .to_string())
}

/// Reads pixel dimensions from PNG/GIF/BMP headers and JPEG SOF markers.
fn image_dimensions(path: &Path, extension: &str) -> Option<(u64, u64)> {
    let mut data = vec![0u8; 64 * 1024];
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let read = file.read(&mut data).ok()?;
    data.truncate(read);
    match extension {
        "png" if data.len() >= 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" => {
            let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]) as u64;
            let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]) as u64;
            Some((width, height))
        }
        "gif" if data.len() >= 10 && &data[0..3] == b"GIF" => {
            let width = u16::from_le_bytes([data[6], data[7]]) as u64;
            let height = u16::from_le_bytes([data[8], data[9]]) as u64;
            Some((width, height))
        }
        "bmp" if data.len() >= 26 && &data[0..2] == b"BM" => {
            let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]).unsigned_abs() as u64;
            let height = i32::from_le_bytes([data[22], data[23], data[24], data[25]]).unsigned_abs() as u64;
            Some((width, height))
        }
        "jpg" | "jpeg" => {
            // Walk JPEG markers to the first SOFn segment.
            let mut index = 2usize;
            while index + 9 < data.len() {
                if data[index] != 0xFF {
                    break;
                }
                let marker = data[index + 1];
                if marker == 0xD8 || (0xD0..=0xD9).contains(&marker) {
                    index += 2;
                    continue;
                }
                let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
                let is_sof = (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker);
                if is_sof {
                    let height = u16::from_be_bytes([data[index + 5], data[index + 6]]) as u64;
                    let width = u16::from_be_bytes([data[index + 7], data[index + 8]]) as u64;
                    return Some((width, height));
                }
                index += 2 + length;
            }
            None
        }
        _ => None,
    }
}

fn read_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("read_file: {err}"))?;
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("read_file: cannot read {}: {err}", path.display()))?;
    let truncated = if content.len() > MAX_FILE_READ_CHARS {
        let mut cut = content[..MAX_FILE_READ_CHARS].to_string();
        cut.push_str("\n... (file truncated)");
        cut
    } else {
        content
    };
    let start = args.get("start_line").and_then(Value::as_u64).map(|v| v.max(1) as usize);
    let end = args.get("end_line").and_then(Value::as_u64).map(|v| v as usize);
    let mut out = String::new();
    for (index, line) in truncated.lines().enumerate() {
        let line_no = index + 1;
        if let Some(start) = start {
            if line_no < start {
                continue;
            }
        }
        if let Some(end) = end {
            if line_no > end {
                break;
            }
        }
        out.push_str(&format!("{line_no:6}  {line}\n"));
    }
    Ok(truncate_output(out))
}

fn write_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("write_file: {err}"))?;
    let content = args.get("content").and_then(Value::as_str).unwrap_or("");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("write_file: {err}"))?;
    }
    std::fs::write(&path, content).map_err(|err| format!("write_file: {err}"))?;
    Ok(format!("Wrote {} ({} bytes).", path.display(), content.len()))
}

fn edit_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("edit_file: {err}"))?;
    let old_text = str_arg(args, "old_text").unwrap_or_default();
    let new_text = str_arg(args, "new_text").unwrap_or("");
    if old_text.is_empty() {
        return Err("edit_file: old_text is empty.".to_string());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("edit_file: cannot read {}: {err}", path.display()))?;
    match content.find(old_text) {
        Some(_) => {
            let updated = content.replacen(old_text, new_text, 1);
            std::fs::write(&path, &updated).map_err(|err| format!("edit_file: {err}"))?;
            Ok(format!("Edited {}.", path.display()))
        }
        None => Err("edit_file: old_text was not found in the file.".to_string()),
    }
}

fn delete_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("delete_file: {err}"))?;
    if path.is_dir() {
        return Err("delete_file: refusing to delete a directory; use a shell command instead.".to_string());
    }
    std::fs::remove_file(&path).map_err(|err| format!("delete_file: {err}"))?;
    Ok(format!("Deleted {}.", path.display()))
}

fn list_directory(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(
        project,
        str_arg(args, "path").filter(|p| !p.trim().is_empty()).unwrap_or("."),
    )
    .map_err(|err| format!("list_directory: {err}"))?;
    let entries = std::fs::read_dir(&path).map_err(|err| format!("list_directory: {err}"))?;
    let mut lines: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let suffix = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
        lines.push(format!("{name}{suffix}"));
    }
    lines.sort();
    if lines.is_empty() {
        return Ok(format!("{} is empty.", path.display()));
    }
    Ok(truncate_output(lines.join("\n")))
}

fn create_directory(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("create_directory: {err}"))?;
    std::fs::create_dir_all(&path).map_err(|err| format!("create_directory: {err}"))?;
    Ok(format!("Created {}.", path.display()))
}

fn move_file(project: &Path, args: &Value) -> Result<String, String> {
    let from = safe_path(project, str_arg(args, "from").unwrap_or_default())
        .map_err(|err| format!("move_file: {err}"))?;
    let to = safe_path(project, str_arg(args, "to").unwrap_or_default())
        .map_err(|err| format!("move_file: {err}"))?;
    if !from.exists() {
        return Err("move_file: source does not exist.".to_string());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("move_file: {err}"))?;
    }
    std::fs::rename(&from, &to).map_err(|err| format!("move_file: {err}"))?;
    Ok(format!("Moved {} → {}.", from.display(), to.display()))
}

fn search_files(project: &Path, args: &Value) -> Result<String, String> {
    let query = str_arg(args, "query").unwrap_or_default();
    if query.is_empty() {
        return Err("search_files: query is empty.".to_string());
    }
    let base = safe_path(
        project,
        str_arg(args, "path").filter(|p| !p.trim().is_empty()).unwrap_or("."),
    )
    .map_err(|err| format!("search_files: {err}"))?;
    let pattern = str_arg(args, "file_pattern").unwrap_or_default();
    // A valid regular expression wins; otherwise a case-insensitive literal.
    let regex = Regex::new(query).ok();

    let mut matches: Vec<String> = Vec::new();
    let mut size = 0usize;
    let walker = walkdir::WalkDir::new(&base)
        .max_depth(MAX_SEARCH_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !name.starts_with('.') && !SEARCH_SKIP_DIRS.contains(&name.as_ref())
        });

    for entry in walker.filter_map(|entry| entry.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_pattern_matches(&file_name, pattern) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for (line_no, line) in content.lines().enumerate() {
            let hit = match regex.as_ref() {
                Some(regex) => regex.is_match(line),
                None => line.to_lowercase().contains(&query.to_lowercase()),
            };
            if hit {
                let rel = entry
                    .path()
                    .strip_prefix(project)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .to_string();
                let snippet = line.trim_start().chars().take(200).collect::<String>();
                let line = format!("{rel}:{}: {snippet}", line_no + 1);
                if size + line.len() > MAX_OUTPUT_CHARS {
                    matches.push("... (more matches truncated)".to_string());
                    return Ok(matches.join("\n"));
                }
                size += line.len();
                matches.push(line);
                if matches.len() >= 200 {
                    matches.push("... (result limit reached)".to_string());
                    return Ok(matches.join("\n"));
                }
            }
        }
    }
    if matches.is_empty() {
        Ok(format!("No matches for \"{query}\"."))
    } else {
        Ok(matches.join("\n"))
    }
}

fn file_pattern_matches(file_name: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    if let Some(extension) = pattern.strip_prefix("*.") {
        return file_name.ends_with(extension);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return file_name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return file_name.ends_with(suffix);
    }
    file_name.contains(pattern)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

// ----------------------------------------------------------------------
// Dangerous tools
// ----------------------------------------------------------------------

/// Environment variable names that must never leak into child processes.
fn is_secret_env(name: &str) -> bool {
    let upper = name.to_uppercase();
    upper.contains("API_KEY")
        || upper.contains("APIKEY")
        || upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("PASSWD")
        || upper == "LOCAGENS_API_TOKEN"
        || upper.contains("PRIVATE_KEY")
}

async fn run_command(project: &Path, mode: &str, args: &Value) -> Result<String, String> {
    let command = str_arg(args, "command").unwrap_or_default();
    if command.trim().is_empty() {
        return Err("run_command: command is empty.".to_string());
    }

    let domains: Vec<String> = args
        .get("network_domains")
        .or_else(|| args.get("networkDomains"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let spec = locagens_execpolicy::sandbox::SandboxSpec {
        mode: locagens_protocol::from_run_mode(mode).1,
        project_path: project.to_path_buf(),
        allow_network: !domains.is_empty() || mode == "auto",
    };
    let (shell_prog, shell_args): (String, Vec<String>) = if cfg!(windows) {
        ("cmd".to_string(), vec!["/C".to_string(), command.to_string()])
    } else {
        ("sh".to_string(), vec!["-c".to_string(), command.to_string()])
    };
    let (program, prog_args) = locagens_execpolicy::sandbox::wrap_shell_command(
        &shell_prog,
        &shell_args,
        Some(&spec),
    );
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&prog_args);

    // Deny-by-default network + secret-free environment.
    cmd.current_dir(project)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd.env_clear();
    for (name, value) in std::env::vars() {
        if is_secret_env(&name) {
            continue;
        }
        // Only pass a small safe allowlist plus the bare minimum.
        const SAFE: [&str; 18] = [
            "PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "TEMP", "TMP", "HOME", "USERPROFILE",
            "APPDATA", "LOCALAPPDATA", "PROGRAMFILES", "NUMBER_OF_PROCESSORS", "OS", "PROCESSOR_ARCHITECTURE",
            "LANG", "TERM", "SHELL",
        ];
        if SAFE.contains(&name.as_str()) || name.starts_with("SYSTEMROOT") {
            cmd.env(&name, &value);
        }
    }

    let future = cmd.output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), future)
        .await
        .map_err(|_| "run_command: timed out after 120 seconds.".to_string())?
        .map_err(|err| format!("run_command: {err}"))?;

    let mut result = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        result.push_str(stdout.trim_end());
        result.push('\n');
    }
    if !stderr.trim().is_empty() {
        result.push_str("--- stderr ---\n");
        result.push_str(stderr.trim_end());
        result.push('\n');
    }
    if result.trim().is_empty() {
        result.push_str("(no output)");
    }
    if !output.status.success() {
        result.push_str(&format!("\nExit code: {}", output.status.code().unwrap_or(-1)));
    }
    Ok(truncate_output(result))
}

/// Resolves a host and rejects private/loopback/link-local targets (SSRF-safe).
fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 64
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00 || v6.is_unique_local()
        }
    }
}

async fn fetch_url(args: &Value) -> Result<String, String> {
    let url = str_arg(args, "url").unwrap_or_default();
    let parsed = url::Url::parse(url.trim()).map_err(|err| format!("fetch_url: invalid URL: {err}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("fetch_url: only http(s) URLs are allowed.".to_string());
    }
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "fetch_url: URL has no host.".to_string())?;
    let port = parsed.port_or_known_default().unwrap_or(80);

    // SSRF protection: resolve the host and refuse private addresses.
    let resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| format!("fetch_url: cannot resolve {host}: {err}"))?;
    for addr in resolved {
        if is_private_ip(&addr.ip()) {
            return Err("fetch_url: requests to private/internal addresses are blocked.".to_string());
        }
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Locagens/1.0")
        .build()
        .map_err(|err| format!("fetch_url: {err}"))?;
    let response = http
        .get(parsed)
        .send()
        .await
        .map_err(|err| format!("fetch_url: {err}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().await.unwrap_or_default();
    let mut out = format!("HTTP {status}\ncontent-type: {content_type}\n\n");
    if content_type.starts_with("text/html") {
        // Rough HTML→text strip so the model sees content, not markup soup.
        let without_scripts = Regex::new(r"(?is)<(script|style)[^>]*>[\s\S]*?</(script|style)>")
            .ok()
            .and_then(|regex| Some(regex.replace_all(&body, "").to_string()))
            .unwrap_or_else(|| body.clone());
        let text = Regex::new(r"<[^>]+>")
            .ok()
            .map(|regex| regex.replace_all(&without_scripts, " ").to_string())
            .unwrap_or(without_scripts);
        out.push_str(&text.split_whitespace().collect::<Vec<_>>().join(" "));
    } else {
        out.push_str(&body);
    }
    Ok(truncate_output(out))
}

// ----------------------------------------------------------------------
// Permission previews
// ----------------------------------------------------------------------

/// Builds the `PermissionPreview` payload for a tool call, or `None` when the
/// tool has no meaningful preview (never-gated read tools).
pub fn build_preview(project: &Path, name: &str, args: &Value) -> Option<Value> {
    let relative = str_arg(args, "path").or_else(|| str_arg(args, "from")).unwrap_or("");
    let absolute = safe_path(project, relative).ok();
    let json_string = |value: Option<String>| value.map(|content| content.chars().take(4000).collect::<String>());

    let preview = match name {
        "write_file" => {
            let old = std::fs::read_to_string(absolute.as_deref().unwrap_or(Path::new(""))).ok();
            json!({
                "tool": name,
                "action": if old.is_some() { "edit" } else { "create" },
                "path": relative,
                "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                "oldContent": json_string(old),
                "newContent": json_string(Some(str_arg(args, "content").unwrap_or("").to_string())),
            })
        }
        "edit_file" => {
            let old = std::fs::read_to_string(absolute.as_deref().unwrap_or(Path::new(""))).ok();
            let new = old.clone().and_then(|content| {
                let old_text = str_arg(args, "old_text")?;
                let new_text = str_arg(args, "new_text").unwrap_or("");
                Some(content.replacen(old_text, new_text, 1))
            });
            json!({
                "tool": name,
                "action": "edit",
                "path": relative,
                "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                "oldContent": json_string(old),
                "newContent": json_string(new),
            })
        }
        "delete_file" => json!({
            "tool": name,
            "action": "delete",
            "path": relative,
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "oldContent": json_string(std::fs::read_to_string(absolute.as_deref().unwrap_or(Path::new(""))).ok()),
            "newContent": Value::Null,
        }),
        "move_file" => json!({
            "tool": name,
            "action": "move",
            "path": relative,
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "oldContent": Value::Null,
            "newContent": Value::Null,
            "destPath": str_arg(args, "to").unwrap_or(""),
        }),
        "create_directory" => json!({
            "tool": name,
            "action": "mkdir",
            "path": relative,
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "oldContent": Value::Null,
            "newContent": Value::Null,
        }),
        "run_command" => json!({
            "tool": name,
            "action": "command",
            "path": "",
            "absolutePath": project.display().to_string(),
            "oldContent": Value::Null,
            "newContent": Value::Null,
            "command": str_arg(args, "command").unwrap_or(""),
            "networkDomains": args.get("network_domains").or_else(|| args.get("networkDomains")).cloned().unwrap_or(json!([])),
        }),
        "fetch_url" => json!({
            "tool": name,
            "action": "fetch",
            "path": "",
            "absolutePath": "",
            "oldContent": Value::Null,
            "newContent": Value::Null,
            "url": str_arg(args, "url").unwrap_or(""),
        }),
        "exec_command" => json!({
            "tool": name,
            "action": "command",
            "path": "",
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| project.display().to_string()),
            "oldContent": Value::Null,
            "newContent": Value::Null,
            "command": str_arg(args, "cmd").unwrap_or(""),
            "networkDomains": json!([]),
            "justification": str_arg(args, "justification").unwrap_or(""),
        }),
        "apply_patch" => {
            let patch_text = str_arg(args, "input")
                .or_else(|| str_arg(args, "patch"))
                .unwrap_or("");
            let (diff, affected) = crate::orchestrator::apply_patch::preview_patch(project, patch_text)
                .unwrap_or_default();
            let files: Vec<String> = affected.all();
            let path_summary = files.join(", ");
            json!({
                "tool": name,
                "action": "patch",
                "path": path_summary,
                "absolutePath": project.display().to_string(),
                "oldContent": Value::Null,
                "newContent": Value::Null,
                "patch": patch_text.chars().take(12_000).collect::<String>(),
                "diff": diff.chars().take(12_000).collect::<String>(),
                "files": files,
            })
        }
        "read_file" => json!({
            "tool": name,
            "action": "read",
            "path": relative,
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "oldContent": Value::Null,
            "newContent": Value::Null,
        }),
        "list_directory" => json!({
            "tool": name,
            "action": "list",
            "path": relative,
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "oldContent": Value::Null,
            "newContent": Value::Null,
        }),
        "search_files" => json!({
            "tool": name,
            "action": "search",
            "path": relative,
            "absolutePath": absolute.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "oldContent": Value::Null,
            "newContent": Value::Null,
            "query": str_arg(args, "query").unwrap_or(""),
        }),
        _ => return None,
    };
    Some(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locagens-tools-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn safe_path_containment() {
        let ws = temp_ws("contain");
        let ws_canonical = crate::util::normalize_canonical(std::fs::canonicalize(&ws).unwrap());
        // Existing files canonicalize to the normalized absolute path.
        std::fs::write(ws.join("main.rs"), "x").unwrap();
        assert_eq!(safe_path(&ws, "main.rs").unwrap(), ws_canonical.join("main.rs"));
        assert!(safe_path(&ws, "../escape.txt").is_err());
        assert!(safe_path(&ws, "a/../../escape.txt").is_err());
        // Absolute paths outside the workspace are rejected. (On non-Windows
        // hosts `C:/...` is just a folder name and resolves inside the
        // workspace, so the rejection only holds on Windows itself.)
        #[cfg(windows)]
        assert!(safe_path(&ws, "C:/Windows/system32").is_err());
        #[cfg(not(windows))]
        assert!(safe_path(&ws, "C:/Windows/system32")
            .unwrap()
            .starts_with(&ws_canonical));
        // Nested non-existent path resolves inside the workspace.
        let nested = safe_path(&ws, "new/dir/file.txt").unwrap();
        assert!(nested.starts_with(&ws_canonical));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn file_tool_roundtrip() {
        let ws = temp_ws("roundtrip");
        let ws_value = ws.to_string_lossy().replace('\\', "/");
        let project = PathBuf::from(&ws_value);

        let out = execute(&project, "write_file", &json!({
            "path": "docs/readme.md",
            "content": "hello\nworld\n"
        })).await.unwrap();
        assert!(out.contains("Wrote"));
        assert_eq!(std::fs::read_to_string(ws.join("docs/readme.md")).unwrap(), "hello\nworld\n");

        let read = execute(&project, "read_file", &json!({ "path": "docs/readme.md", "start_line": 2 }))
            .await.unwrap();
        assert!(read.contains("world"));
        assert!(!read.contains("hello"));

        let edited = execute(&project, "edit_file", &json!({
            "path": "docs/readme.md", "old_text": "world", "new_text": "locagens"
        })).await;
        assert!(edited.is_ok());
        assert!(std::fs::read_to_string(ws.join("docs/readme.md")).unwrap().contains("locagens"));

        let miss = execute(&project, "edit_file", &json!({
            "path": "docs/readme.md", "old_text": "nope", "new_text": "x"
        })).await;
        assert!(miss.is_err());

        let listed = execute(&project, "list_directory", &json!({ "path": "docs" })).await.unwrap();
        assert!(listed.contains("readme.md"));

        let found = execute(&project, "search_files", &json!({ "query": "locagens", "path": "docs" })).await.unwrap();
        assert!(found.contains("readme.md:2:"), "search output was: {found}");

        execute(&project, "move_file", &json!({ "from": "docs/readme.md", "to": "out/guide.md" }))
            .await.unwrap();
        assert!(ws.join("out/guide.md").exists());
        assert!(!ws.join("docs/readme.md").exists());

        execute(&project, "delete_file", &json!({ "path": "out/guide.md" })).await.unwrap();
        assert!(!ws.join("out/guide.md").exists());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn permission_gating_matrix() {
        assert!(!needs_permission("auto", "run_command"));
        assert!(!needs_permission("auto", "write_file"));
        assert!(needs_permission("accept_edits", "run_command"));
        assert!(needs_permission("accept_edits", "exec_command"));
        assert!(needs_permission("accept_edits", "write_stdin"));
        assert!(!needs_permission("accept_edits", "write_file"));
        assert!(!needs_permission("accept_edits", "edit_file"));
        assert!(needs_permission("ask_permissions", "write_file"));
        assert!(needs_permission("ask_permissions", "move_file"));
        assert!(needs_permission("ask_permissions", "fetch_url"));
        assert!(needs_permission("ask_permissions", "create_directory"));
        assert!(needs_permission("ask_permissions", "apply_patch"));
        assert!(!needs_permission("ask_permissions", "read_file"));
        assert!(!needs_permission("chat", "read_file"));
        assert!(needs_permission("ask_permissions", "mcp__github__list_issues"));
        assert!(!needs_permission("auto", "mcp__github__list_issues"));
        assert!(!needs_permission("accept_edits", "mcp__github__list_issues"));
        // Plan/chat never mutate.
        assert!(!needs_permission("plan", "run_command"));
        assert!(!needs_permission("plan", "exec_command"));
        assert!(!needs_permission("chat", "exec_command"));
    }

    #[test]
    fn readonly_mode_forbids_mutations_and_exec() {
        for mode in ["plan", "chat"] {
            for tool in [
                "write_file",
                "edit_file",
                "delete_file",
                "move_file",
                "create_directory",
                "apply_patch",
                "run_command",
                "exec_command",
                "write_stdin",
            ] {
                assert!(
                    is_forbidden_in_readonly_mode(mode, tool),
                    "{tool} must be forbidden in {mode} mode"
                );
            }
        }
        for tool in ["read_file", "list_directory", "search_files", "view_image", "fetch_url"] {
            assert!(!is_forbidden_in_readonly_mode("plan", tool), "{tool} stays allowed in plan");
            assert!(!is_forbidden_in_readonly_mode("chat", tool), "{tool} stays allowed in chat");
        }
        // Other modes never forbid outright (they gate instead).
        assert!(!is_forbidden_in_readonly_mode("accept_edits", "write_file"));
        assert!(!is_forbidden_in_readonly_mode("auto", "exec_command"));
    }

    #[tokio::test]
    async fn view_image_returns_base64_data() {
        let ws = temp_ws("viewimage");
        // 1x1 transparent PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(ws.join("dot.png"), png).unwrap();
        let out = execute(&ws, "view_image", &json!({ "path": "dot.png" })).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["mime_type"], "image/png");
        assert_eq!(parsed["width"], 1);
        assert_eq!(parsed["height"], 1);
        let b64 = parsed["data_base64"].as_str().unwrap();
        assert!(!b64.is_empty());
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        assert_eq!(decoded, png);
        assert!(parsed["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[tokio::test]
    async fn view_image_rejects_oversized_files() {
        let ws = temp_ws("viewimage-big");
        let big = vec![0u8; 10 * 1024 * 1024 + 1];
        std::fs::write(ws.join("big.png"), &big).unwrap();
        let err = execute(&ws, "view_image", &json!({ "path": "big.png" })).await;
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("10 MiB"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn apply_patch_preview_lists_affected_files() {
        let ws = temp_ws("patchpreview");
        std::fs::write(ws.join("a.txt"), "one\ntwo\n").unwrap();
        let preview = build_preview(&ws, "apply_patch", &json!({
            "input": "*** Begin Patch\n*** Update File: a.txt\n@@\n-two\n+TWO\n*** Add File: new.txt\n+hello\n*** End Patch"
        })).unwrap();
        assert_eq!(preview["action"], "patch");
        let files = preview["files"].as_array().unwrap();
        assert!(files.contains(&json!("a.txt")));
        assert!(files.contains(&json!("new.txt")));
        let path_summary = preview["path"].as_str().unwrap();
        // Added files list first, then modified ones.
        assert_eq!(path_summary, "new.txt, a.txt");
        assert!(!preview["diff"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn previews_carry_diff_content() {
        let ws = temp_ws("preview");
        std::fs::write(ws.join("a.txt"), "one\ntwo\n").unwrap();
        let preview = build_preview(&ws, "edit_file", &json!({
            "path": "a.txt", "old_text": "two", "new_text": "TWO"
        })).unwrap();
        assert_eq!(preview["action"], "edit");
        assert_eq!(preview["oldContent"], "one\ntwo\n");
        assert_eq!(preview["newContent"], "one\nTWO\n");

        let preview = build_preview(&ws, "run_command", &json!({
            "command": "npm test", "network_domains": ["registry.npmjs.org"]
        })).unwrap();
        assert_eq!(preview["action"], "command");
        assert_eq!(preview["command"], "npm test");
        assert_eq!(preview["networkDomains"][0], "registry.npmjs.org");

        let preview = build_preview(&ws, "write_file", &json!({ "path": "b.txt", "content": "x" })).unwrap();
        assert_eq!(preview["action"], "create");
        assert_eq!(preview["oldContent"], Value::Null);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn tool_def_filtering() {
        let readonly = readonly_tool_defs(false);
        assert_eq!(readonly.len(), 3);
        assert!(readonly.iter().any(|tool| tool.pointer("/function/name") == Some(&json!("read_file"))));
        let readonly_move = readonly_tool_defs(true);
        assert_eq!(readonly_move.len(), 4);
        let full = all_tool_defs();
        assert_eq!(full.len(), filter_tool_defs(&[
            "read_file", "write_file", "edit_file", "delete_file", "list_directory", "create_directory",
            "move_file", "search_files", "run_command", "fetch_url", "apply_patch", "exec_command",
            "write_stdin", "view_image", "request_user_input", "compact_context", "spawn_agent",
            "send_message", "followup_task", "wait_agent", "interrupt_agent", "close_agent", "list_agents", "get_context_remaining", "host_executable", "set_chat_title", "update_plan",
            "remember", "load_skill", "ask_user_question", "spawn_agents",
            "list_mcp_resources", "read_mcp_resource",
        ]).len());
    }
}
