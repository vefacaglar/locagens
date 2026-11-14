//! Pure Rust workspace & orchestrator tools.
//!
//! Every path argument is resolved against the run's project folder and
//! canonicalized so nothing escapes the workspace (symlink-safe containment
//! via `canonicalize` + `starts_with`), and every write, delete and move
//! goes through `workspace_fs`, which never follows a link swapped in after
//! that check (QA-017). Dangerous tools (`run_command`,
//! `fetch_url`) are permission-gated by the orchestrator; `build_preview`
//! produces the red/green diff payload the UI permission card renders.

use locagens_apply_patch::workspace_fs;
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Maximum bytes returned by any single tool response (keeps LLM context and
/// the UI bounded).
pub(super) const MAX_OUTPUT_CHARS: usize = 24_000;
/// Largest file `read_file` / `edit_file` will load (bytes).
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest file `search_files` scans (bytes).
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Directory names skipped by `search_files`.
const SEARCH_SKIP_DIRS: [&str; 8] =
    [".git", "node_modules", "target", "dist", ".next", "build", "vendor", ".venv"];
const MAX_SEARCH_DEPTH: usize = 14;

/// Tools that mutate the workspace (permission-gated in `ask_permissions`).
pub const MUTATION_TOOLS: [&str; 6] = ["write_file", "edit_file", "delete_file", "move_file", "create_directory", "apply_patch"];

/// Whether a tool call needs interactive permission approval under `mode`.
/// Only the exact `auto` mode skips prompts. `fetch_url` sends data out of
/// the machine, so it prompts in every other mode, Plan and Chat included
/// (commands are forbidden there outright, see
/// [`is_forbidden_in_readonly_mode`]).
pub fn needs_permission(mode: &str, tool: &str) -> bool {
    if mode == "auto" {
        return false;
    }
    let command = matches!(tool, "run_command" | "exec_command" | "write_stdin");
    let mutation = MUTATION_TOOLS.contains(&tool);
    // MCP servers are not sandboxed: their tools prompt in Build mode too.
    let mcp = tool.starts_with("mcp__");
    tool == "fetch_url" || command || (mutation && mode != "accept_edits") || mcp
}

/// Whether a `remember` call needs a prompt: global memory is injected into
/// every later run in every project, so it prompts outside Full Access;
/// project memory prompts in the modes that also prompt for file writes
/// (anything but Build and Full Access).
pub fn remember_needs_permission(mode: &str, scope: &str) -> bool {
    match mode {
        "auto" => false,
        "accept_edits" => scope == "global",
        _ => true,
    }
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
        def("read_file", "Read a text file from the workspace. Returns numbered lines (at most ~24k characters per call) and the file's total line count. For a large file read it in pieces with offset/limit.",
            json!({
                "path": path_prop("Relative path inside the workspace."),
                "offset": json!({ "type": "integer", "description": "1-based line to start at (default 1)." }),
                "limit": json!({ "type": "integer", "description": "Maximum number of lines to return." }),
                "start_line": json!({ "type": "integer", "description": "Alias for offset." }),
                "end_line": json!({ "type": "integer", "description": "1-based last line to return (inclusive); alternative to limit." })
            }), &["path"]),
        def("write_file", "Create a file (or overwrite an existing one) with the given content inside the workspace. Parent directories are created automatically.",
            json!({
                "path": path_prop("Relative path inside the workspace."),
                "content": json!({ "type": "string", "description": "Full file content to write." })
            }), &["path", "content"]),
        def("edit_file", "Replace old_text with new_text in a workspace file. old_text must match exactly and be unique, otherwise the edit is refused; set replace_all to replace every occurrence.",
            json!({
                "path": path_prop("Relative path inside the workspace."),
                "old_text": json!({ "type": "string", "description": "Exact existing text to replace." }),
                "new_text": json!({ "type": "string", "description": "Replacement text." }),
                "replace_all": json!({ "type": "boolean", "description": "Replace every occurrence of old_text (default false: old_text must be unique)." })
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
        def("search_files", "Search file contents across the workspace (skips .git/node_modules/target and .gitignore'd files). Returns path:line matches with context.",
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
                "max_output_tokens": json!({ "type": "integer", "description": "Rough bound on returned output (default and maximum 6000 tokens ~ 24k chars)." }),
                "shell": json!({ "type": "string", "description": "Optional shell program for the session (e.g. `bash`, `zsh`); defaults to the platform shell." }),
                "login": json!({ "type": "boolean", "description": "Run the shell as a login shell (-lc) so profile/rc files load." }),
                "tty": json!({ "type": "boolean", "description": "Allocate a pseudo-terminal for the session (enables TTY-aware programs like progress bars and editors)." }),
                "network_domains": json!({ "type": "array", "items": { "type": "string" }, "description": "Outbound hosts the command needs (e.g. registry.npmjs.org); the user approves them with the command." }),
                "justification": json!({ "type": "string", "description": "Short reason shown on the permission card." })
            }), &["cmd"]),
        def("write_stdin", "Send characters to a running exec session and collect its latest output. Omit `chars` to just poll. If the session is an interactive shell or REPL (exec_command started cmd, powershell/pwsh, bash/sh/zsh, python, node, a database or remote shell, ... without a script or command argument), every line you submit (text up to a newline) is treated as a command and needs the same approval as exec_command outside Full Access; text after the last newline is held and sent once a later write completes the line. Input to any other program (answering a prompt of a running build, data for a script) and Ctrl-C (\\u0003) / Ctrl-D (\\u0004) are sent as-is. Prefer exec_command for new commands.",
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
        def("spawn_agents", "Run parallel one-shot sub-agents with the current model: 'explorer' (read-only research) or 'worker' (full workspace tools). Waits until every agent finished and returns all results. Each agent takes one of the 4 live agent slots shared with spawn_agent while it runs.",
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
/// workspace (including via symlinks, dangling ones too). Non-existent
/// targets are validated by their deepest existing ancestor. One
/// implementation shared with `apply_patch`.
pub fn safe_path(project: &Path, relative: &str) -> Result<PathBuf, String> {
    locagens_apply_patch::resolve_safe_path(project, relative)
}

/// The first workspace path a mutation tool call would write that is
/// protected (`.git`, `.locagens`, `.codex`, `.agents`; see
/// `locagens_execpolicy::protected_paths`), as the relative path the model
/// gave. Such writes always need an explicit approval outside Full Access.
pub fn protected_mutation_target(project: &Path, name: &str, args: &Value) -> Option<String> {
    if !MUTATION_TOOLS.contains(&name) {
        return None;
    }
    let targets: Vec<String> = if name == "apply_patch" {
        let patch = str_arg(args, "input").or_else(|| str_arg(args, "patch")).unwrap_or("");
        match crate::orchestrator::apply_patch::parse_patch(patch) {
            Ok(parsed) => parsed
                .hunks
                .iter()
                .flat_map(|hunk| {
                    let mut paths = vec![hunk.path().to_string_lossy().to_string()];
                    if let crate::orchestrator::apply_patch::Hunk::UpdateFile { move_path: Some(to), .. } = hunk {
                        paths.push(to.to_string_lossy().to_string());
                    }
                    paths
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        ["path", "from", "to"].iter().filter_map(|key| str_arg(args, key)).map(str::to_string).collect()
    };
    let root = std::fs::canonicalize(project).map(crate::util::normalize_canonical).ok()?;
    targets.into_iter().find(|relative| match safe_path(project, relative) {
        Ok(resolved) => locagens_execpolicy::protected_paths::is_protected_in_workspace(&root, &resolved),
        // Unresolvable paths fail in the tool itself.
        Err(_) => false,
    })
}

fn truncate_output(mut text: String) -> String {
    if locagens_protocol::text::truncate_at_char_boundary(&mut text, MAX_OUTPUT_CHARS) {
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
        "search_files" => {
            // A directory walk blocks; keep it off the async runtime thread.
            let (project, args) = (project_path.to_path_buf(), args.clone());
            tokio::task::spawn_blocking(move || search_files(&project, &args))
                .await
                .map_err(|err| format!("search_files: {err}"))?
        }
        "run_command" => run_command(project_path, mode, args).await,
        "fetch_url" => fetch_url(args).await,
        "view_image" => view_image(project_path, args),
        _ => Err(format!("Unknown tool \"{name}\".")),
    }
}

/// The secret-stripped base environment for exec sessions
/// (`exec_command`); `[shell_environment_policy]` refines it at spawn time.
pub fn exec_session_env() -> std::collections::HashMap<String, String> {
    locagens_execpolicy::env_policy::safe_base_env()
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
        .unwrap_or((0, 0));
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

/// A required string argument; a missing one is an error instead of an
/// empty default (a lost `content` must not truncate a file, FR-025).
fn required_str<'a>(args: &'a Value, key: &str, tool: &str) -> Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("{tool}: missing required argument '{key}'."))
}

/// Reads a workspace file as text. Invalid UTF-8 is replaced lossily; the
/// flag says whether that happened.
fn read_text_lossy(path: &Path, tool: &str) -> Result<(String, bool), String> {
    let size = std::fs::metadata(path)
        .map_err(|err| format!("{tool}: cannot read {}: {err}", path.display()))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(format!("{tool}: {} is {size} bytes, over the {MAX_FILE_BYTES}-byte limit.", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|err| format!("{tool}: cannot read {}: {err}", path.display()))?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(err) => (String::from_utf8_lossy(err.as_bytes()).into_owned(), true),
    })
}

fn read_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("read_file: {err}"))?;
    let (content, lossy) = read_text_lossy(&path, "read_file")?;
    let line_arg = |key: &str| args.get(key).and_then(Value::as_u64).map(|v| v as usize);
    let start = line_arg("offset").or_else(|| line_arg("start_line")).unwrap_or(1).max(1);
    let end = match (line_arg("limit"), line_arg("end_line")) {
        (Some(limit), _) => start.saturating_add(limit).saturating_sub(1),
        (None, Some(end)) => end,
        (None, None) => usize::MAX,
    };
    let total = content.lines().count();
    if start > total && total > 0 {
        return Err(format!("read_file: offset {start} is past the end of the file ({total} lines)."));
    }
    let mut out = String::new();
    let mut last_shown = start - 1;
    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        if line_no < start {
            continue;
        }
        if line_no > end {
            break;
        }
        let line = locagens_protocol::text::prefix_at_char_boundary(line, MAX_OUTPUT_CHARS / 2);
        if !out.is_empty() && out.len() + line.len() + 8 > MAX_OUTPUT_CHARS {
            break;
        }
        out.push_str(&format!("{line_no:6}  {line}\n"));
        last_shown = line_no;
    }
    if last_shown < total && last_shown < end {
        out.push_str(&format!(
            "... (showing lines {start}-{last_shown} of {total}; call read_file again with offset={} to continue)\n",
            last_shown + 1
        ));
    } else if end == usize::MAX {
        out.push_str(&format!("(end of file, {total} lines)\n"));
    }
    if lossy {
        out.push_str("(note: the file is not valid UTF-8; invalid bytes are shown as U+FFFD and edit_file cannot edit it)\n");
    }
    Ok(out)
}

fn write_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("write_file: {err}"))?;
    let content = required_str(args, "content", "write_file")?;
    workspace_fs::write(project, &path, content.as_bytes()).map_err(|err| format!("write_file: {err}"))?;
    Ok(format!("Wrote {} ({} bytes).", path.display(), content.len()))
}

fn edit_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("edit_file: {err}"))?;
    let old_text = str_arg(args, "old_text").unwrap_or_default();
    let new_text = required_str(args, "new_text", "edit_file")?;
    let replace_all = args.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
    if old_text.is_empty() {
        return Err("edit_file: old_text is empty.".to_string());
    }
    let (content, lossy) = read_text_lossy(&path, "edit_file")?;
    if lossy {
        return Err(format!(
            "edit_file: {} is not valid UTF-8, so it cannot be edited safely; use a shell command to change it.",
            path.display()
        ));
    }
    let matches = content.matches(old_text).count();
    if matches == 0 {
        return Err("edit_file: old_text was not found in the file.".to_string());
    }
    if matches > 1 && !replace_all {
        return Err(format!(
            "edit_file: old_text matches {matches} places; add surrounding context to make it unique or set replace_all."
        ));
    }
    let updated = content.replace(old_text, new_text);
    workspace_fs::write(project, &path, updated.as_bytes()).map_err(|err| format!("edit_file: {err}"))?;
    Ok(if matches > 1 {
        format!("Edited {} ({matches} replacements).", path.display())
    } else {
        format!("Edited {}.", path.display())
    })
}

fn delete_file(project: &Path, args: &Value) -> Result<String, String> {
    let path = safe_path(project, str_arg(args, "path").unwrap_or_default())
        .map_err(|err| format!("delete_file: {err}"))?;
    if path.is_dir() {
        return Err("delete_file: refusing to delete a directory; use a shell command instead.".to_string());
    }
    workspace_fs::remove_file(project, &path).map_err(|err| format!("delete_file: {err}"))?;
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
    workspace_fs::create_dir_all(project, &path).map_err(|err| format!("create_directory: {err}"))?;
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
    workspace_fs::rename(project, &from, &to).map_err(|err| format!("move_file: {err}"))?;
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
    // `ignore` honours .gitignore / .ignore / global excludes (also outside a
    // git repo); dot-directories are searched except the skip list.
    let walker = ignore::WalkBuilder::new(&base)
        .max_depth(Some(MAX_SEARCH_DEPTH))
        .follow_links(false)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !SEARCH_SKIP_DIRS.contains(&name.as_ref())
        })
        .build();

    for entry in walker.filter_map(|entry| entry.ok()) {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_pattern_matches(&file_name, pattern) {
            continue;
        }
        if entry.metadata().map(|meta| meta.len() > MAX_SEARCH_FILE_BYTES).unwrap_or(true) {
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
        return file_name.ends_with(&format!(".{extension}"));
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

/// The one place the sandbox of a command tool call (`run_command` and
/// `exec_command`) is decided, so both tools agree (QA-015).
///
/// Network rules:
/// - Full Access runs unsandboxed (the spec is then ignored).
/// - `[sandbox_workspace_write] network_access = true` opens the network in
///   every sandboxed mode and on every OS.
/// - Windows otherwise routes through the per-domain proxy (`network`).
/// - macOS/Linux have no per-domain proxy: a call that names
///   `network_domains` (approved by the user with the command) opens the
///   whole network for that command; without domains it stays offline.
pub fn command_sandbox_spec(
    mode: &str,
    project: &Path,
    network: locagens_execpolicy::network_proxy::NetworkPolicy,
    requested_domains: &[String],
) -> locagens_execpolicy::sandbox::SandboxSpec {
    let sandbox_mode = locagens_protocol::from_run_mode(mode).1;
    let allow_network = sandbox_mode == locagens_protocol::SandboxMode::DangerFullAccess
        || crate::config_runtime::sandbox_network_access()
        || (!cfg!(windows) && !requested_domains.is_empty());
    locagens_execpolicy::sandbox::SandboxSpec {
        mode: sandbox_mode,
        project_path: project.to_path_buf(),
        allow_network,
        network: Some(network),
        writable_roots: crate::config_runtime::sandbox_writable_roots(),
        secret_paths: sandbox_secret_paths(),
    }
}

/// Credential files of the user's home and the agent dirs that sandboxed
/// commands must not read (macOS/Linux) or write (QA-020).
pub fn sandbox_secret_paths() -> Vec<PathBuf> {
    locagens_execpolicy::secret_paths::credential_paths(&crate::paths::user_home(), &crate::paths::agent_dir_chain())
}

async fn run_command(project: &Path, mode: &str, args: &Value) -> Result<String, String> {
    run_command_with_network(project, mode, args, None).await
}

/// `run_command` with an explicit network policy for the sandbox proxy
/// (Windows). `None` allows exactly the call's `network_domains`.
pub async fn run_command_with_network(
    project: &Path,
    mode: &str,
    args: &Value,
    network: Option<locagens_execpolicy::network_proxy::NetworkPolicy>,
) -> Result<String, String> {
    let command = str_arg(args, "command").unwrap_or_default();
    if command.trim().is_empty() {
        return Err("run_command: command is empty.".to_string());
    }

    let domains = crate::orchestrator::network_access::requested_domains(args);
    let network = network.unwrap_or_else(|| {
        locagens_execpolicy::network_proxy::NetworkPolicy::allowlist_only(
            locagens_execpolicy::network_proxy::DomainAllowlist::new(&domains),
        )
    });
    let spec = command_sandbox_spec(mode, project, network, &domains);
    let (shell_prog, shell_args): (String, Vec<String>) = if cfg!(windows) {
        ("cmd".to_string(), vec!["/C".to_string(), command.to_string()])
    } else {
        ("sh".to_string(), vec!["-c".to_string(), command.to_string()])
    };

    // Secret-free environment, refined by `[shell_environment_policy]`.
    let env = locagens_execpolicy::env_policy::command_env();

    let output = locagens_execpolicy::sandbox::run_captured(
        &shell_prog,
        &shell_args,
        project,
        env,
        Some(&spec),
        std::time::Duration::from_secs(120),
    )
    .await
    .map_err(|err| match err.kind() {
        // The sandbox appends its violation note to the timeout message.
        std::io::ErrorKind::TimedOut => format!(
            "run_command: timed out after 120 seconds.{}",
            err.to_string().strip_prefix("command timed out").unwrap_or_default()
        ),
        _ => format!("run_command: {err}"),
    })?;

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
    if !output.success() {
        result.push_str(&format!("\nExit code: {}", output.exit_code.unwrap_or(-1)));
    }
    if !output.blocked_hosts.is_empty() {
        result.push_str("\n\n");
        result.push_str(&locagens_execpolicy::network_proxy::blocked_hosts_note(&output.blocked_hosts));
    }
    let mut result = truncate_output(result);
    if !output.sandbox_violations.is_empty() {
        // After truncation, so the warning is never cut off.
        result.push_str("\n\n");
        result.push_str(&locagens_execpolicy::sandbox::sandbox_violation_note(&output.sandbox_violations));
    }
    Ok(result)
}

/// Redirects `fetch_url` follows, each re-validated like the first URL.
const FETCH_MAX_REDIRECTS: usize = 5;
/// Bytes of response body `fetch_url` reads before it stops.
const FETCH_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Which resolved addresses `fetch_url` refuses; production uses
/// `network_proxy::is_non_public_ip` (tests swap in their own predicate).
type BlockedAddress = dyn Fn(&std::net::IpAddr) -> bool + Sync;

/// Validates one `fetch_url` target and resolves it to public addresses:
/// http(s) only, and no address the host resolves to may be `blocked`
/// (`network_proxy::is_non_public_ip`, which also knows IPv4-mapped IPv6,
/// link-local and reserved ranges).
async fn resolve_public_target(
    url: &url::Url,
    blocked: &BlockedAddress,
) -> Result<(String, Vec<std::net::SocketAddr>), String> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("fetch_url: only http(s) URLs are allowed.".to_string());
    }
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "fetch_url: URL has no host.".to_string())?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|err| format!("fetch_url: cannot resolve {host}: {err}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("fetch_url: cannot resolve {host}."));
    }
    if addrs.iter().any(|addr| blocked(&addr.ip())) {
        return Err("fetch_url: requests to private/internal addresses are blocked.".to_string());
    }
    Ok((host, addrs))
}

/// One request pinned to the addresses that were just validated, so a
/// second DNS answer (rebinding) can never redirect the connection. No
/// proxy (it would resolve the host itself) and no automatic redirects.
async fn fetch_pinned(url: &url::Url, blocked: &BlockedAddress) -> Result<reqwest::Response, String> {
    let (host, addrs) = resolve_public_target(url, blocked).await?;
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Locagens/1.0")
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if url.host().is_some_and(|h| matches!(h, url::Host::Domain(_))) {
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    let http = builder.build().map_err(|err| format!("fetch_url: {err}"))?;
    http.get(url.clone()).send().await.map_err(|err| format!("fetch_url: {err}"))
}

/// Reads at most `limit` bytes of the body; `true` when it was cut.
async fn read_capped_body(mut response: reqwest::Response, limit: usize) -> (Vec<u8>, bool) {
    let mut body: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        let room = limit.saturating_sub(body.len());
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            return (body, true);
        }
        body.extend_from_slice(&chunk);
    }
    (body, false)
}

async fn fetch_url(args: &Value) -> Result<String, String> {
    fetch_url_guarded(args, &|ip| locagens_execpolicy::network_proxy::is_non_public_ip(ip)).await
}

/// `fetch_url` with the address guard applied to the first URL and to every
/// redirect hop.
async fn fetch_url_guarded(args: &Value, blocked: &BlockedAddress) -> Result<String, String> {
    let url = str_arg(args, "url").unwrap_or_default();
    let mut current = url::Url::parse(url.trim()).map_err(|err| format!("fetch_url: invalid URL: {err}"))?;
    // Follow redirects by hand: every hop is resolved, checked and pinned.
    let mut redirects = 0usize;
    let response = loop {
        let response = fetch_pinned(&current, blocked).await?;
        if !response.status().is_redirection() {
            break response;
        }
        let Some(location) = response.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()) else {
            break response;
        };
        if redirects >= FETCH_MAX_REDIRECTS {
            return Err(format!("fetch_url: more than {FETCH_MAX_REDIRECTS} redirects."));
        }
        current = current.join(location).map_err(|err| format!("fetch_url: invalid redirect: {err}"))?;
        redirects += 1;
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let (bytes, cut) = read_capped_body(response, FETCH_MAX_BODY_BYTES).await;
    let mut body = String::from_utf8_lossy(&bytes).to_string();
    if cut {
        body.push_str("\n... (response body truncated)");
    }
    let mut out = format!("HTTP {status}\ncontent-type: {content_type}\n\n");
    if content_type.starts_with("text/html") {
        // Rough HTML→text strip so the model sees content, not markup soup.
        let without_scripts = Regex::new(r"(?is)<(script|style)[^>]*>[\s\S]*?</(script|style)>")
            .ok().map(|regex| regex.replace_all(&body, "").to_string())
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

/// Characters of file content a permission preview shows.
const PREVIEW_CHARS: usize = 4_000;
/// Characters of a patch or diff a permission preview shows.
const PREVIEW_PATCH_CHARS: usize = 12_000;

/// The first `max` characters of `text`; a longer text ends in a visible
/// "(N more characters)" marker so the user knows the tail is unseen.
fn clip_with_marker(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(max).collect();
    clipped.push_str(&format!("
… ({} more characters)", total - max));
    clipped
}

/// Builds the `PermissionPreview` payload for a tool call, or `None` when the
/// tool has no meaningful preview (never-gated read tools).
pub fn build_preview(project: &Path, name: &str, args: &Value) -> Option<Value> {
    let relative = str_arg(args, "path").or_else(|| str_arg(args, "from")).unwrap_or("");
    let absolute = safe_path(project, relative).ok();
    let json_string = |value: Option<String>| value.map(|content| clip_with_marker(&content, PREVIEW_CHARS));

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
                let replace_all = args.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
                Some(if replace_all { content.replace(old_text, new_text) } else { content.replacen(old_text, new_text, 1) })
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
            "networkDomains": args.get("network_domains").or_else(|| args.get("networkDomains")).cloned().unwrap_or(json!([])),
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
                "patch": clip_with_marker(patch_text, PREVIEW_PATCH_CHARS),
                "diff": clip_with_marker(&diff, PREVIEW_PATCH_CHARS),
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

    /// QA-021: cutting tool output / big files at a byte budget that lands
    /// inside a multibyte character must not panic.
    #[test]
    fn truncation_never_splits_multibyte_characters() {
        // `→` is 3 bytes: 23_999 + 3 straddles the 24_000 cut.
        let text = format!("{}→ tail", "a".repeat(MAX_OUTPUT_CHARS - 1));
        let cut = truncate_output(text);
        assert!(cut.ends_with("\n... (output truncated)"));
        assert!(cut.starts_with(&"a".repeat(MAX_OUTPUT_CHARS - 1)));

        let ws = temp_ws("big-utf8");
        let body = format!("{}\n", "ğ".repeat(MAX_OUTPUT_CHARS));
        std::fs::write(ws.join("big.txt"), &body).unwrap();
        let read = read_file(&ws, &serde_json::json!({ "path": "big.txt" })).unwrap();
        assert!(read.starts_with("     1  ğ"));
        let _ = std::fs::remove_dir_all(ws);
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

    /// Creates `link -> target`; `false` when the OS refuses (Windows
    /// without the symlink privilege).
    fn make_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        return std::os::windows::fs::symlink_file(target, link).is_ok();
    }

    #[tokio::test]
    async fn writes_never_follow_a_dangling_symlink_out_of_the_workspace() {
        let ws = temp_ws("dangling");
        let outside = temp_ws("dangling-target");
        let target = outside.join("autostart.desktop");
        if !make_symlink(&target, &ws.join("notes.md")) {
            eprintln!("skipping: cannot create symlinks here");
            return;
        }
        let err = safe_path(&ws, "notes.md").unwrap_err();
        assert!(err.contains("dangling"), "{err}");
        assert!(execute(&ws, "write_file", &json!({ "path": "notes.md", "content": "x" })).await.is_err());
        assert!(!target.exists(), "the write must not create the link target");
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// Windows: a junction (no privilege needed) to a missing directory
    /// outside the workspace must not let a write create files there.
    #[cfg(windows)]
    #[tokio::test]
    async fn writes_never_follow_a_dangling_junction_out_of_the_workspace() {
        let ws = temp_ws("junction");
        let outside = temp_ws("junction-target").join("missing");
        let created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(ws.join("notes"))
            .arg(&outside)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !created {
            eprintln!("skipping: cannot create junctions here");
            return;
        }
        assert!(safe_path(&ws, "notes/x.txt").is_err());
        assert!(execute(&ws, "write_file", &json!({ "path": "notes/x.txt", "content": "x" })).await.is_err());
        assert!(!outside.join("x.txt").exists());
        let _ = std::fs::remove_dir(ws.join("notes"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn protected_mutation_targets_cover_every_mutation_tool() {
        let ws = temp_ws("protected");
        std::fs::create_dir_all(ws.join(".git").join("hooks")).unwrap();
        let cases = [
            ("write_file", json!({ "path": ".locagens/hooks.json", "content": "{}" }), Some(".locagens/hooks.json")),
            ("edit_file", json!({ "path": ".git/config", "old_text": "a", "new_text": "b" }), Some(".git/config")),
            ("create_directory", json!({ "path": ".locagens/rules" }), Some(".locagens/rules")),
            ("delete_file", json!({ "path": ".git/hooks/pre-commit" }), Some(".git/hooks/pre-commit")),
            ("move_file", json!({ "from": "evil.sh", "to": ".git/hooks/pre-commit" }), Some(".git/hooks/pre-commit")),
            ("write_file", json!({ "path": ".GIT/hooks/post-checkout", "content": "x" }), Some(".GIT/hooks/post-checkout")),
            ("write_file", json!({ "path": "src/main.rs", "content": "x" }), None),
            ("read_file", json!({ "path": ".git/config" }), None),
            (
                "apply_patch",
                json!({ "input": "*** Begin Patch\n*** Update File: a.txt\n*** Move to: .locagens/agents/explorer.toml\n@@\n-a\n+b\n*** End Patch" }),
                Some(".locagens/agents/explorer.toml"),
            ),
        ];
        for (tool, args, expected) in cases {
            assert_eq!(protected_mutation_target(&ws, tool, &args).as_deref(), expected, "{tool} {args}");
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn fetch_url_refuses_internal_targets_in_every_spelling() {
        for url in [
            "http://127.0.0.1/",
            "http://localhost:8080/admin",
            "http://169.254.169.254/latest/meta-data/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[fe80::1]/",
            "http://10.0.0.5/",
            "http://240.0.0.1/",
            "file:///etc/passwd",
        ] {
            let err = fetch_url(&json!({ "url": url })).await.unwrap_err();
            assert!(err.contains("blocked") || err.contains("only http"), "{url}: {err}");
        }
        use locagens_execpolicy::network_proxy::is_non_public_ip;
        for ip in ["64:ff9b::7f00:1", "ff02::1", "::10.0.0.1", "224.0.0.1", "198.18.0.1", "192.0.0.8"] {
            assert!(is_non_public_ip(&ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "2606:4700:4700::1111", "1.1.1.1"] {
            assert!(!is_non_public_ip(&ip.parse().unwrap()), "{ip}");
        }
    }

    /// Serves `GET /ok` (200 "fetched-ok"), `GET /hop` (302 to `/ok`) and
    /// anything else as a 302 to the cloud metadata address.
    async fn spawn_redirect_server() -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let redirect = |location: &str| {
                    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                };
                let reply = if request.starts_with("GET /ok ") {
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\nConnection: close\r\n\r\nfetched-ok".to_string()
                } else if request.starts_with("GET /hop ") {
                    redirect(&format!("http://127.0.0.1:{port}/ok"))
                } else {
                    redirect("http://169.254.169.254/latest/meta-data/")
                };
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        port
    }

    /// QA-009: every redirect hop is resolved and checked like the first URL,
    /// so a public page cannot bounce `fetch_url` to an internal address.
    #[tokio::test]
    async fn fetch_url_revalidates_every_redirect_hop() {
        use locagens_execpolicy::network_proxy::is_non_public_ip;
        let port = spawn_redirect_server().await;
        // The test server stands in for a public host; every other address
        // keeps the production check.
        let blocked = |ip: &std::net::IpAddr| ip.to_string() != "127.0.0.1" && is_non_public_ip(ip);
        let followed = fetch_url_guarded(&json!({ "url": format!("http://127.0.0.1:{port}/hop") }), &blocked)
            .await
            .unwrap();
        assert!(followed.contains("fetched-ok"), "{followed}");
        let err = fetch_url_guarded(&json!({ "url": format!("http://127.0.0.1:{port}/evil") }), &blocked)
            .await
            .unwrap_err();
        assert!(err.contains("blocked"), "{err}");
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
        assert!(needs_permission("accept_edits", "mcp__github__list_issues"));
        // fetch_url prompts in Plan and Chat too (QA-005); commands there
        // are forbidden before gating (is_forbidden_in_readonly_mode).
        assert!(needs_permission("plan", "fetch_url"));
        assert!(needs_permission("chat", "fetch_url"));
        assert!(needs_permission("accept_edits", "fetch_url"));
        assert!(!needs_permission("auto", "fetch_url"));
        assert!(!needs_permission("plan", "read_file"));
        // Unknown / misspelled modes prompt for everything (QA-011).
        for tool in ["write_file", "run_command", "fetch_url", "mcp__x__y", "apply_patch"] {
            assert!(needs_permission("Auto", tool), "{tool}");
            assert!(needs_permission("build", tool), "{tool}");
        }
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

    /// QA-015: both command tools build the same spec, so network rules agree.
    #[test]
    fn command_sandbox_spec_applies_one_network_rule() {
        use locagens_execpolicy::network_proxy::NetworkPolicy;
        let ws = std::env::temp_dir();
        let domains = vec!["registry.npmjs.org".to_string()];
        let build = |mode: &str, domains: &[String]| {
            command_sandbox_spec(mode, &ws, NetworkPolicy::default(), domains)
        };
        // Full Access: unrestricted.
        assert!(build("auto", &[]).allow_network);
        // Build without domains: offline everywhere.
        assert!(!build("accept_edits", &[]).allow_network);
        assert!(!build("ask_permissions", &[]).allow_network);
        // Build with domains: Windows keeps the per-domain proxy, Unix has
        // no proxy and opens the network for that command.
        assert_eq!(build("accept_edits", &domains).allow_network, !cfg!(windows));
        assert_eq!(
            build("accept_edits", &domains).mode,
            locagens_protocol::SandboxMode::WorkspaceWrite
        );
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

    #[test]
    fn read_file_reaches_late_lines_with_offset_and_limit() {
        let ws = temp_ws("read-offset");
        let body: String = (1..=20_000).map(|n| format!("line number {n}\n")).collect();
        std::fs::write(ws.join("big.txt"), &body).unwrap();
        let first = read_file(&ws, &json!({ "path": "big.txt" })).unwrap();
        assert!(first.len() < MAX_OUTPUT_CHARS + 300);
        assert!(first.contains("of 20000"), "{first}");
        let next = first.trim_end().rsplit("offset=").next().unwrap().split(' ').next().unwrap().to_string();
        let page = read_file(&ws, &json!({ "path": "big.txt", "offset": next.parse::<u64>().unwrap() })).unwrap();
        assert!(page.contains(&format!("line number {next}\n")));
        let tail = read_file(&ws, &json!({ "path": "big.txt", "offset": 19_999, "limit": 2 })).unwrap();
        assert!(tail.contains("line number 20000") && !tail.contains("showing lines"), "{tail}");
        assert!(read_file(&ws, &json!({ "path": "big.txt", "offset": 30_000 })).is_err());
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn edit_file_refuses_ambiguous_matches_unless_replace_all() {
        let ws = temp_ws("edit-unique");
        std::fs::write(ws.join("a.txt"), "x = 1\ny = x\nx = 2\n").unwrap();
        let args = json!({ "path": "a.txt", "old_text": "x", "new_text": "z" });
        let err = edit_file(&ws, &args).unwrap_err();
        assert!(err.contains("matches 3 places"), "{err}");
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "x = 1\ny = x\nx = 2\n");
        let all = json!({ "path": "a.txt", "old_text": "x", "new_text": "z", "replace_all": true });
        assert!(edit_file(&ws, &all).unwrap().contains("3 replacements"));
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "z = 1\ny = z\nz = 2\n");
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn write_and_edit_reject_missing_required_arguments() {
        let ws = temp_ws("required-args");
        std::fs::write(ws.join("keep.txt"), "data").unwrap();
        assert!(write_file(&ws, &json!({ "path": "keep.txt" })).unwrap_err().contains("'content'"));
        assert!(edit_file(&ws, &json!({ "path": "keep.txt", "old_text": "data" })).unwrap_err().contains("'new_text'"));
        assert_eq!(std::fs::read_to_string(ws.join("keep.txt")).unwrap(), "data");
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn non_utf8_files_read_lossily_and_refuse_edits() {
        let ws = temp_ws("non-utf8");
        std::fs::write(ws.join("latin.txt"), b"caf\xe9 au lait\n").unwrap();
        let read = read_file(&ws, &json!({ "path": "latin.txt" })).unwrap();
        assert!(read.contains("caf") && read.contains("not valid UTF-8"), "{read}");
        let err = edit_file(&ws, &json!({ "path": "latin.txt", "old_text": "au", "new_text": "du" })).unwrap_err();
        assert!(err.contains("not valid UTF-8"), "{err}");
        assert_eq!(std::fs::read(ws.join("latin.txt")).unwrap(), b"caf\xe9 au lait\n");
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn search_pattern_needs_the_dot_and_hidden_dirs_are_searched() {
        assert!(file_pattern_matches("main.rs", "*.rs"));
        assert!(!file_pattern_matches("users", "*.rs"));
        assert!(!file_pattern_matches("foo.tsrs", "*.rs"));
        let ws = temp_ws("search-hidden");
        std::fs::create_dir_all(ws.join(".github")).unwrap();
        std::fs::write(ws.join(".github").join("ci.yml"), "needle here\n").unwrap();
        assert!(search_files(&ws, &json!({ "query": "needle" })).unwrap().contains("ci.yml"));
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn previews_mark_clipped_content() {
        let text = "a".repeat(PREVIEW_CHARS + 25);
        assert!(clip_with_marker(&text, PREVIEW_CHARS).ends_with("(25 more characters)"));
        assert_eq!(clip_with_marker("short", PREVIEW_CHARS), "short");
    }

    #[test]
    fn remember_prompts_follow_the_mode() {
        assert!(!remember_needs_permission("auto", "global"));
        assert!(remember_needs_permission("accept_edits", "global"));
        assert!(!remember_needs_permission("accept_edits", "project"));
        for mode in ["chat", "plan", "ask_permissions"] {
            assert!(remember_needs_permission(mode, "global"), "{mode}");
            assert!(remember_needs_permission(mode, "project"), "{mode}");
        }
    }
}
