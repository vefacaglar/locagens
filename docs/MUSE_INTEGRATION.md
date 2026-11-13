# Muse Code integration - research and design

Status: research, nothing implemented. Researched 2026-11-14. Most sources are
third-party; items marked (unverified) must be checked against a real `muse`
install before coding.

## 1. Findings

### Muse Code
- Meta's terminal coding agent. Beta announced 2026-08-05, out of beta
  2026-08-31 (inter-session messaging, workflow orchestration, TypeScript SDK in
  developer preview). Proprietary CLI, Muse Spark models; plan tiers or Model API
  billing (`META_API_KEY` from dev.meta.ai).
  [Terminal Trove](https://terminaltrove.com/ai-coding-agents/muse-code/),
  [AI Weekly](https://aiweekly.co/alerts/meta-graduates-muse-code-out-of-beta-with-inter-session-messaging-workflow),
  [TNW](https://thenextweb.com/news/meta-muse-code-ai-coding-agent-launch).
  (One source says Spark 2.1, most say 1.2 - ambiguous.)
- Name clash: "Muse Code" here is Meta's. Unrelated: PyPI
  [`code-muse`](https://pypi.org/project/code-muse/).
- Auth: `muse login` (OAuth) writes `~/.config/muse/auth.json`; or `META_API_KEY`
  env var. Meta ships no GUI itself.
- Headless: `muse exec [--yolo] [--json] [--max-model-steps N] "<prompt>"`;
  `--json` emits JSONL which is "literally an MSP session event stream"
  ([E2B docs](https://docs.e2b.dev/agents/muse), [issue #1721](https://github.com/developing-today/code/issues/1721)).
  `--yolo` = no approvals, no OS sandbox, trusts workspace.
- Server mode: `muse serve` = long-lived host speaking the **Muse Session Protocol
  (MSP)**: newline-delimited JSON-RPC 2.0 over stdio. Flags seen: `--disable-sandbox`,
  `--sandbox-network enabled|proxy-only|restricted`.
- Official MIT TypeScript client: [`@muse-code/sdk`](https://github.com/meta-models/muse-code-sdk)
  (docs: https://meta-models.github.io/muse-code-sdk). Repo has generated wire
  types (`clients/msp-ts`) and protocol declarations + conformance transcripts
  (`schema/msp`); `muse schema generate-ts` emits types. SDK version tracks the CLI
  version; "experimental" surfaces may change. No Rust SDK known - we would
  implement the JSON-RPC client ourselves from `schema/msp`.
- MSP methods seen (via [bb-plugin-provider-muse](https://github.com/braedonsaunders/bb-plugin-provider-muse)):
  `session/start|resume|fork|list|read|compact`, `turn/start|steer|interrupt`;
  notifications `item/started`, `item/delta`, `turn/completed`,
  `session/tokenUsage`, `usage/changed`, `session/todoListChanged`,
  `view/gap|page`, `session/replaced`; approvals `approval/requested` ->
  `approval/decide` (`approval/updated`, `approval/listPending`); questions
  `userInput/requested` -> answer/clarify/cancel; `subagent` items. Approval
  modes `allowAll` / `onRequest`; OS sandbox on/off.
- MCP: Muse supports MCP servers and skills via `~/.config/muse`; others inject
  their own tools by writing a per-thread config dir with an MCP server entry.
- Session storage (third-party, conflicting, unverified):
  `~/.local/share/muse/sessions/<date>/<session>/session.jsonl`
  (another project says `sessions/*.jsonl`). Format undocumented outside MSP
  docs. Caveat: a session's route belongs to the `muse serve` process that
  opened it; resuming on a second process can fail with a provider-private
  history error. Resume after killing an idle host works.
- ACP: Muse does **not** speak Zed's Agent Client Protocol natively; community
  builds adapters ([issue #1721](https://github.com/developing-today/code/issues/1721),
  an AFK-Pilot-style VS Code extension includes "a small adapter").
- License/ToS: SDK is MIT; the CLI is proprietary and no wrapper-specific ToS
  was found. One integrator states the OAuth login is plan-only (never PAYG).
  Read Meta's terms before shipping. Do **not** copy tokens out of `auth.json`
  to call the model API directly (same stance as our Anthropic rule).

### Helicon
- [HarjjotSinghh/helicon](https://github.com/HarjjotSinghh/helicon) (MIT,
  unofficial, by Harjot Singh Rana; [overview](https://www.everydev.ai/tools/helicon)).
- Architecture: React UI shared by a Tauri 2 desktop app, a web app, and a VS Code
  extension; a Node **daemon** (`packages/daemon`) spawns one `muse serve` per
  workspace and talks MSP via `@muse-code/sdk`; state in SQLite
  `~/.helicon/helicon.db` (projects -> sessions -> turns). It does not scrape the
  TUI. Auth is the user's own `muse login`; Helicon stores no credentials.
- Session discovery: terminal-started sessions are "found automatically" and
  grouped by project/worktree. The README does not say how; most likely
  `session/list` over MSP and/or reading Muse's session dir (unverified - read
  `packages/daemon` source).
- Answer: Helicon = spawn the official CLI as an MSP server over stdio (SDK),
  not file parsing and not ACP.
- Similar integrations all use MSP: [open-muse](https://github.com/navycrow/open-muse)
  (OpenCode plugin), [gray-muse-sub](https://github.com/vstaln/gray-muse-sub)
  (Muse as a "model provider"), [donmai harness](https://github.com/RenseiAI/donmai/pull/899),
  [muse-code-mcp](https://github.com/shippopotamuslabs/muse-code-mcp),
  [VS Code ext](https://marketplace.visualstudio.com/items?itemName=corona10.muse-code-unofficial).

### Comparable patterns
- Claude Code: `-p --output-format stream-json`, `--resume`; Agent SDK wraps it
  (opcode/claudia, crystal, conductor all spawn the CLI and parse stream-json or
  read `~/.claude/projects/*.jsonl`).
- Codex: `codex exec --json`, and `codex app-server` (JSON-RPC over stdio; the
  closest analogue of MSP, with approval requests as server->client requests).
- Zed: ACP (JSON-RPC over stdio) with adapters per agent.
- Common lesson: a JSON-RPC stdio server beats parsing one-shot output (live
  approvals, interrupts, multi-turn) and file reading is only for history.

## 2. Design for Locagens

Guiding rule: **Muse is an external agent backend, not a model provider.** It
brings its own loop, tools, sandbox and auth; Locagens cannot call the Muse
model itself without copying plan tokens. gray-muse-sub-style "provider" wrapping
loses tool events and approvals, and conflicts with the "single model, our tools"
design.

### (a) Read-only session import/listing (phase 1, low risk)
- Source preference: `muse serve` `session/list` + `session/read` (stable,
  documented) over parsing `session.jsonl` (undocumented). Fallback: scan
  `~/.local/share/muse/sessions/**/session.jsonl` and read only the header and
  last lines for title/date/cwd; tolerate unknown lines like our rollout loader.
- Map to a lightweight `ExternalSessionSummary { source: "muse", id, cwd, title,
  updated_at, path }` and, on open, convert items to `ChatMessage`s read-only
  (text, tool call/result, plan). Never write to Muse's directories.
- Surfaces: `lcgn sessions --external` (or `--source muse`) and a desktop sidebar
  group "Muse" filtered by current project cwd. Opening shows a read-only
  transcript with a "Continue with Muse" button (phase 2).
- Do not import into our rollout files (avoids duplication/drift); render live.

### (b) Muse as external agent backend (phase 2)
- New `MuseHost` in `crates/core/src/services/muse/` (`msp.rs` JSON-RPC client over
  child stdio, `host.rs` lifecycle, `map.rs` event mapping). One `muse serve` per
  run/project, spawned via `ProcessManager`-style child handling; binary from
  `$LOCAGENS_MUSE_BIN` or PATH (`host_executable`). Keep each session on the same
  host process (resume caveat above).
- Generate Rust types from `schema/msp` (hand-written serde structs for the
  ~15 methods used; do not depend on Node/SDK).
- Run integration: a `backend: muse` option on `run()`/`continue_run()` bypassing
  the Locagens drive loop but reusing `RunHandle`, persistence and events:
  `item/started|delta` -> assistant/tool messages (+`message_update` rollout
  lines), `turn/completed` -> run `done`, `session/tokenUsage` -> `usage` line,
  `session/todoListChanged` -> `plan`, `subagent` items -> `agentRole/agentName`
  messages, `approval/requested` -> `awaiting_permission` and our answer flow
  (`allow_once|allow_run` -> approve, `deny` -> deny; `allow_project/always` stay
  Muse-side unless MSP offers a grant), `userInput/requested` -> `awaiting_input`,
  cancel -> `turn/interrupt`, steer text -> `turn/steer`.
- Modes: Plan -> `onRequest` + read-only prompt instruction (MSP has no known
  read-only mode - open question); Build -> `onRequest`, sandbox on; Full Access
  -> `allowAll`. Never pass `--yolo`/`--disable-sandbox` unless Full Access.
- Cheaper fallback: `muse exec --json` one-shot (no approvals, no interrupt
  except kill); useful as a spike to capture the event schema.
- Auth: never touch `auth.json`; surface "run `muse login`" if the host reports
  unauthenticated. Reuse the "Providers & Secrets" rule: no logging of keys.

### (c) Where it fits
- `crates/core/src/services/muse/` (new): protocol client, discovery, mapping.
- `crates/core/src/api/muse.rs` (new facade): `muse_status()`,
  `list_muse_sessions(cwd)`, `read_muse_session(id)`; `runs.rs` gains a backend
  selector. Facade holds no logic.
- `crates/protocol`: add `ExternalSessionSummary` and `RunBackend::{Locagens,
  Muse}`; document in `docs/CORE_API.md`, `docs/CLI.md`, `docs/database-schema.md`.
- CLI: `lcgn sessions --source muse`, `lcgn --backend muse -p "..."`.
- Desktop (`apps/desktop-rs/src/components`): sidebar "External sessions" section,
  backend dropdown next to the model picker (hides provider/model since Muse owns
  them), reuse the existing transcript, permission card and plan components.
- Tests: recorded MSP transcripts as fixtures (the SDK repo ships conformance
  transcripts) driving a fake stdio host; no real Muse needed in CI.

### (d) Risks / decisions for the user
1. Provider vs backend: recommend backend (above). Confirm.
2. Licensing/ToS: no explicit rules found on third-party wrappers; the plan login
   is described as plan-only. Check Meta's terms before shipping publicly.
3. MSP stability: SDK is "developer preview"; schema may change. Pin a tested
   `muse` version range and show a clear error on mismatch.
4. Muse's own sandbox vs Locagens `SandboxMode`: two sandboxes stack; Windows
   behaviour of `muse serve` sandbox is unclear (one source mentions a Windows
   sandbox note). Test on this machine.
5. Session dir layout is unverified; install Muse and inspect before phase 1.
6. Plan/read-only mode has no verified MSP equivalent.
7. Permission grant persistence: Locagens `permissions.json` vs Muse's own.
8. Scope: phase 1 (listing) alone is cheap and independently useful; phase 2 is
   several days. "Not Implemented" list in Claude.md does not forbid this, but it
   is a new feature, so needs explicit go-ahead.

## 3. Recommended next steps
1. Install `muse`, run `muse exec --json` and `muse serve` by hand, record the
   event stream and session directory (resolves items 5 and parts of 3, 6).
2. Read Helicon's `packages/daemon` to confirm how it discovers terminal sessions.
3. Implement phase 1 read-only listing, then phase 2 behind a `--backend muse` flag.
