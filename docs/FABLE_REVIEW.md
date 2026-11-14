# Locagens independent review (2026-11-14)

Second-pass review after `docs/QA_REVIEW.md` was worked through. It lists only
what is still wrong, was missed, or was introduced since. Nothing in the source
tree was changed.

## Summary

- `cargo test --workspace --no-fail-fast` (stable-x86_64-pc-windows-gnu): all
  suites pass (20 + 4 + 101 + 10 + 350 + 63 + 70 + 16 + 14 tests, 1 ignored).
- 78 findings: 0 critical, 19 high, 36 medium, 23 low.
- The biggest cluster is the run lifecycle around failed, cancelled and
  compacted turns: the history replayed to the provider can become invalid, and
  the conversation then fails on every following turn (FR-001 to FR-005).
- Five findings were reproduced with the built `lcgn` binary against a local
  mock provider in isolated home directories (marked **reproduced**). The rest
  come from reading the code; those that depend on behaviour I could not run
  are marked **needs-confirmation**.
- Findings marked *(sub-review)* come from two delegated read-only passes
  (desktop, CLI/config). I re-checked the load-bearing lines of the ones rated
  high; the others carry the delegate's verification only.
- The working tree was being edited by another session during the review
  (`api/runs.rs`, `orchestrator/drive.rs`, `cli/tui/*`, a new
  `orchestrator/auto_title.rs`). Line numbers are as read and may have drifted
  by a few lines.

Paths are relative to the repository root. `core/` means `crates/core/src/`,
`desktop/` means `apps/desktop-rs/src/`.

---

## 1. Run lifecycle, streaming, persistence

### FR-001 · high · a failed or cancelled round leaves an empty assistant message that breaks every later turn (reproduced)

**Status:** fixed. `orchestrator/history.rs` `sanitize` drops empty assistant messages in `seed_history` and before every request.

- **Location:** `core/orchestrator/agent_loop.rs:95-96` (placeholder persisted
  before the request), `core/orchestrator/mod.rs:304-316` (`seed_history`),
  `core/orchestrator/llm/openai_chat.rs:454-476`, `llm/anthropic.rs:355-357`.
- **Problem:** each round persists an empty assistant placeholder before it
  calls the provider. When the call fails or is cancelled, the placeholder
  stays. `seed_history` replays it on the next `continue_run`. The OpenAI body
  becomes `{"role":"assistant"}` with no content and no tool calls, and the
  Anthropic body becomes a `{"type":"text","text":""}` block. OpenAI and
  Anthropic both reject these with HTTP 400, which persists another empty
  placeholder.
- **Reproduction:** mock provider that answers the first 4 requests with HTTP
  500, then `lcgn -p "first question"` (exit 1) and `lcgn -c -p "second
  question"`. The fifth request body contains
  `user "first question"`, `{'role': 'assistant'}`, `user "second question"`.
- **Fix:** drop assistant messages with empty content and no tool calls in
  `seed_history` (and delete the placeholder when the round fails before any
  delta arrives).

### FR-002 · high · cancelling or failing between tool calls leaves tool calls without results in the replayed history

**Status:** fixed. Unanswered tool calls get an "interrupted" result; orphan and duplicate results are dropped.

- **Location:** `core/orchestrator/agent_loop.rs:180-190` (assistant message
  with `raw_response` tool calls persisted), `:239-279` (results persisted one
  by one, `check_cancel` between them), `core/orchestrator/mod.rs:291-327`.
- **Problem:** an assistant message with three tool calls is persisted, the
  run is cancelled (or a prompt is cancelled) after the first result, and the
  other two never get a `tool` message. `seed_history` replays the assistant
  message with all three calls. OpenAI and Anthropic both require a result for
  every call and return 400 on the next turn.
- **Scenario:** press Stop while the second of several parallel tool calls
  waits for permission, then send a new message.
- **Fix:** in `seed_history`, add a synthetic `"interrupted"` tool result for
  every call id that has none (or strip the unanswered calls).

### FR-003 · high · auto-compaction can cut between a tool call and its result

**Status:** fixed. The compaction cut never separates a call from its results.

- **Location:** `core/orchestrator/compact.rs:86-90` (`split_history`),
  `:147-157` (`apply_compaction`).
- **Problem:** the tail is always the last 8 messages. When message
  `len - 8` is a `tool` message, its assistant `tool_calls` message is folded
  into the summary and the tail starts with an orphan tool result. Providers
  reject that request, so the run fails right after it compacted.
- **Scenario:** any round with two or more tool calls shifts the alignment
  (`A,T,T,A,T,…`), so a long tool-heavy run hits this about half the time.
- **Fix:** move the split point back to the nearest message that is not a
  `tool` message (and not an assistant message whose results follow).

### FR-004 · high · compaction summarizes only the oldest 80 000 characters and silently drops the rest

**Status:** fixed. All older history is summarized in chunks; only one oversized message is shortened, with a marker.

- **Location:** `core/orchestrator/compact.rs:41`, `:99-123`.
- **Problem:** the transcript sent to the summarizer is cut at
  `COMPACT_USER_MESSAGE_MAX_TOKENS * 4` characters from the start. Compaction
  triggers above about 360 000 characters, so roughly three quarters of the
  history (the most recent part before the 8-message tail) is neither
  summarized nor kept.
- **Scenario:** after the first auto-compaction the agent remembers the start
  of the session and the last 8 messages, and nothing of the work in between.
- **Fix:** keep the newest part of the older history when truncating, or
  summarize in chunks.

### FR-005 · medium · the compaction threshold ignores the model's context window, and compaction is never persisted

**Status:** fixed. The threshold uses the catalog context window (minus prompt and tools); compaction is persisted as a marker that `seed_history` replays; sub-agents compact too.

- **Location:** `core/orchestrator/compact.rs:38`, `:74-78`,
  `crates/config/src/config_toml.rs:398-405`,
  `core/orchestrator/agent_loop.rs:77-79`, `core/orchestrator/mod.rs:291`.
- **Problem 1:** the threshold is 90 000 estimated tokens unless `config.toml`
  sets a limit. The window from the provider catalog (`ModelInfo.context_window`,
  already used by `context_usage`) is not consulted. A local 8k to 32k model
  overflows long before compaction runs; a 1M model compacts needlessly.
- **Problem 2:** the compacted history lives only in the `AgentLoop`. Every
  `continue_run` reseeds the full message list, so each new user turn of a long
  session pays one more summarization call and loses detail again.
- **Problem 3:** sub-agents never compact (`agent_role.is_none()` guard), so a
  long worker turn fails on context overflow.
- **Fix:** derive the threshold from the catalog window, persist a compaction
  marker in the rollout that `seed_history` honours, and compact sub-agents.

### FR-006 · high · the turn snapshot deadlocks on large repositories, so the run never ends (reproduced)

**Status:** fixed. `git_bytes` writes stdin from a separate task while reading stdout; regression test with a ~3 MB path list under a timeout.

- **Location:** `core/services/git.rs:58-62` (`git_bytes` writes all of stdin,
  then reads stdout), `core/services/snapshots.rs:217-232` (`hash_files`),
  called from `core/orchestrator/drive.rs:461-466` and `run_handle.rs:31-36`.
- **Problem:** `git hash-object --stdin-paths` prints one hash per path while
  it reads. `git_bytes` only starts reading stdout after the whole path list
  has been written, so both sides block once the pipes are full. Neither call
  has a timeout or a cancellation check, and the git child is not killed.
- **Reproduction (Windows):** a repository with 13 200 files whose path list
  is 2.5 MB. `lcgn -p "say OK"` prints the answer and then never exits; the
  process tree shows `git hash-object -w --no-filters --stdin-paths` blocked
  for the full 4-minute test timeout. A std-only program with the same
  write-then-read order deadlocks at 4 000 files (166 KB).
- **Scope:** on Windows tokio buffers up to about 2 MB of stdin, so only very
  large repositories hang. On Linux and macOS the pipe holds 64 KB, so the
  threshold should be a few thousand files (**needs-confirmation**, not run).
- **Fix:** write stdin from a spawned task while reading stdout (or pass the
  list through a temp file), add a timeout, and set `kill_on_drop(true)`.

### FR-007 · medium · the snapshot hashes the whole worktree twice per turn

**Status:** fixed. Snapshots keep a `hashcache.json` (size + mtime → blob) and only re-hash changed files.

- **Location:** `core/services/snapshots.rs:145-197`, `:273-290`.
- **Problem:** every Build or Full Access turn runs `hash-object -w` over every
  non-ignored file before and after the turn. Measured on a 4 000-file
  repository: `lcgn -p "say OK"` takes 10.5 s, against 1.2 s on a one-file
  repository. Untracked large files are also copied into `.git/objects` as
  loose objects each time they change.
- **Fix:** hash only files whose stat changed since the last snapshot (git's
  own index does this), or build the tree with `git add -A` into a throwaway
  index, which reuses the stat cache.

### FR-008 · medium · undo reverts edits the user made while the turn was running

**Status:** fixed. Undo only reverts paths the agent's file tools changed this turn (a shell or MCP call falls back to the whole-tree diff); other changes are listed as skipped.

- **Location:** `core/services/snapshots.rs:379-390`, `:407-431`.
- **Problem:** "paths the turn changed" is the difference between the pre-turn
  and post-turn worktree trees. A file the user saved in their editor during
  the turn, or one changed by another run in the same project, is in that
  difference and still equals the post-turn bytes, so undo restores the old
  content.
- **Scenario:** fix a typo in `README.md` while the agent works on `src/`,
  then `/undo`: the typo fix is gone (the `#undo` backup snapshot can bring it
  back, but nothing tells the user).
- **Fix:** intersect with the paths the turn's tools reported
  (`TurnDiffTracker`) where no command ran, or list the files and ask before
  restoring ones the tools never touched.

### FR-009 · low · undo and snapshot gaps

**Status:** partly fixed. `snapshots.json` updates are serialized in-process, symlinks are excluded, and undo refuses while the run is live. A project in a subfolder of a repo still gets no snapshot.

- **Location:** `core/services/snapshots.rs:149`, `:208`, `:193-195`,
  `core/api/projects.rs:570-578`.
- `project_path.join(".git").exists()` means a project that is a subfolder of
  a repository gets no snapshot and no undo.
- `is_file()` follows symlinks, so a symlink is stored as a regular file and
  undo writes a regular file back.
- `snapshots.json` is read, changed and written without a lock; two runs in
  one project can lose each other's entries.
- `undo_last_turn` does not refuse while the run is still generating.

### FR-010 · high · a message queued during the last round is silently lost on the desktop, and sub-agents can consume queued messages

**Status:** fixed in core. Only the main agent drains the inbox; a message queued during the final answer starts another round; undelivered messages are emitted as `queued_messages_undelivered`.

- **Location:** `core/orchestrator/agent_loop.rs:223-229` (returns before the
  inbox is drained), `:281-284` (drain runs in every `AgentLoop`, sub-agents
  included), `desktop/components/composer.rs:813-822`.
- **Problem 1:** the inbox is only folded in after a round that had tool
  calls. A message queued while the final answer streams is never read. The
  desktop has already cleared the composer and shows nothing. The TUI resends
  leftovers (`take_leftover_queued`); the desktop has no equivalent.
- **Problem 2:** sub-agents share the parent's `RunHandle`, and their loop
  drains `handle.inbox` too. A message the user queues for the main agent can
  be appended to a worker's conversation instead.
- **Fix:** drain the inbox only when `agent_role.is_none()`; on a final answer
  with a non-empty inbox, continue the loop with the queued messages; return
  the undelivered messages on completion so every frontend can resend them.

### FR-011 · medium · streamed text is not persisted when a round is cancelled or fails

**Status:** fixed. Streamed text is saved when a round is cancelled or fails.

- **Location:** `core/orchestrator/agent_loop.rs:157-169`,
  `core/db/messages.rs:74-135`, `core/db/sync.rs:19-28`.
- **Problem:** `update_message_full` runs only on success. On cancel or error
  the rollout keeps the last 5-second checkpoint, while memory and the UI hold
  more. Once the run is terminal its owner claim is released, the next
  `refresh_runs` replays the rollout from disk, and the visible answer shrinks.
  After a restart the tail is gone for good.
- **Fix:** persist the partial content on the cancel and error paths.

### FR-012 · medium · a provider stream that ends early or reports an error counts as a finished answer

**Status:** fixed. A stream without a terminal event, or an Anthropic `error` event, is an error; truncation is reported.

- **Location:** `core/orchestrator/llm/mod.rs:229-240`, `:253`,
  `llm/stream_acc.rs:109-111`, `:166-175`.
- **Problem 1:** Anthropic `{"type":"error"}` events (for example
  `overloaded_error` mid-stream) are only checked on the OpenAI branch. The
  Anthropic branch ignores them and returns what arrived so far as a success.
- **Problem 2:** a connection that closes without `[DONE]` or `message_stop`
  also ends in `finish(acc)`. The run turns `done` with a truncated answer.
- **Problem 3:** `finish_reason: "length"` and `stop_reason: "max_tokens"` are
  ignored. A tool call cut off at the token limit has invalid JSON arguments,
  which `agent_loop.rs:244` replaces with `{}`; the tool then fails with a
  missing-argument error and the model is never told its output was truncated.
  Anthropic defaults to `max_tokens` 8192 when the catalog has no value
  (`anthropic.rs:415`), so a large `write_file` hits this.
- **Fix:** treat a stream without a terminal event as an error, handle
  Anthropic `error` events, and return a clear tool result ("arguments were
  truncated at the output limit") instead of running with `{}`.

### FR-013 · high · Anthropic with a reasoning effort breaks on the first tool call (needs-confirmation)

**Status:** fixed. Thinking blocks are captured with signatures, persisted and replayed; Claude 4.6+ models get adaptive thinking. Not run against the live API.

- **Location:** `core/orchestrator/llm/anthropic.rs:336-359`, `:415-424`,
  `llm/stream_acc.rs:147-154`.
- **Problem:** with an effort set, the request enables extended thinking. The
  stream parser keeps the thinking text but not the block signature, and the
  replayed assistant message contains only text and `tool_use` blocks. The
  Messages API requires the thinking block to be passed back in a tool-use
  loop, so the second round should fail with HTTP 400.
- **Not run:** I did not call the live API. Newer models may also reject the
  fixed `budget_tokens` form.
- **Fix:** capture `signature_delta` and replay thinking blocks unchanged for
  the current tool loop, or do not enable thinking when tools are present.

### FR-014 · medium · no prompt caching on the Anthropic path, and cached tokens are double-counted on the OpenAI path

**Status:** fixed. Anthropic cache breakpoints; input tokens normalized so cached tokens are not double-counted.

- **Location:** `core/orchestrator/llm/anthropic.rs:325-427`,
  `core/orchestrator/drive.rs:259-263`, `llm/stream_acc.rs:44-56`.
- The Anthropic body never sets `cache_control`, so every round of a long tool
  loop is billed at the full input price.
- `cost_of` adds `cache_read * rate` on top of `input * rate`. OpenAI-style
  `prompt_tokens` already includes the cached tokens, so those are charged
  twice in the displayed cost.
- **Fix:** mark the system prompt and tool list as cacheable; subtract cached
  tokens from the input count for OpenAI-shaped usage.

### FR-015 · low · the non-streaming fallback zeroes the output token count (reproduced)

**Status:** fixed. The non-streaming fallback keeps output tokens and `tool_use` input.

- **Location:** `core/orchestrator/llm/mod.rs:180-182`.
- **Problem:** after `feed_openai` has read `completion_tokens`, the same
  `usage` object is fed to `feed_anthropic` as a `message_delta`, which sets
  `output_tokens` from the missing `output_tokens` key, so to 0.
- **Reproduction:** the mock provider returns `completion_tokens: 4`; `lcgn`
  prints `tokens: 10 in / 0 out`.
- **Fix:** only replay the Anthropic usage event when the body is an Anthropic
  response.

### FR-016 · medium · model-supplied question items can panic the drive task

**Status:** fixed. Question items are normalized; non-objects are skipped.

- **Location:** `core/orchestrator/interactive.rs:194-195`
  (`question.as_object_mut().unwrap()`).
- **Problem:** `request_user_input` with `questions: ["Proceed?"]` (strings
  instead of objects) panics. The supervisor marks the run failed with
  "agent loop crashed".
- **Fix:** skip or wrap non-object items.

### FR-017 · medium · `continue_run` uses a 120-second staleness guess instead of the run's owner lock

**Status:** fixed. `continue_run` uses the owner lock instead of a staleness guess.

- **Location:** `core/api/runs.rs:160-170`, `:423-430`; the exact probe exists
  at `core/db/locks.rs:79-91` (`run_owned_by_live_process`).
- **Scenario:** a run waits on a permission card in the desktop app for more
  than two minutes. `lcgn -c "…"` in a terminal sees an active status that is
  "stale", takes the run and drives it. Two processes now append to one
  rollout.
- **Fix:** refuse while another live process holds the owner lock.

### FR-018 · low · `continue_run` can leave a claimed handle that never finishes

**Status:** fixed. The mode is validated before a drive is prepared.

- **Location:** `core/api/runs.rs:432-438`.
- **Problem:** `prepare_drive` inserts a claimed handle, then
  `effective_mode(...)?` can still return an error. No drive is spawned, so
  `is_driving` stays true and every later `continue_run` returns "still
  generating" until the run is cancelled (which takes the 7-second grace).
- **Fix:** validate the mode before `prepare_drive`.

### FR-019 · low · `ask_user_question` announces the question before it can be answered

**Status:** fixed. The question slot is filled before `question_requested` is emitted.

- **Location:** `core/orchestrator/interactive.rs:63-75`.
- **Problem:** `question_requested` is emitted before the slot holds the
  sender. A frontend that answers at once (`lcgn -p` auto-answers) can get "No
  pending question", after which the run waits forever. `ask_permission` does
  it in the right order. The window is microseconds (**needs-confirmation**).
- **Fix:** fill the slot first, then emit.

### FR-020 · low · round budget and argument handling details

**Status:** fixed. The round limit fails the run with a notice; invalid JSON arguments get an explanatory result; hook-rewritten input is what is stored.

- `core/orchestrator/agent_loop.rs:302-304`: after 40 rounds the run ends as
  `done` with the last intermediate text and no notice that it was cut off.
- `agent_loop.rs:244`: unparseable tool arguments become `{}` without telling
  the model.
- `agent_loop.rs:259-268`: a PreToolUse hook's `updated_input` is executed,
  but the PostToolUse hook and the stored message see the original arguments.

### FR-021 · low · shared JSON stores reset to empty on a parse error

**Status:** fixed. An unparseable store file is backed up as `<name>.corrupt` before it is rewritten.

- **Location:** `core/db/sync.rs:113-118`, `:150-151`, `:88-97`.
- **Problem:** `projects.json` and `permissions.json` that fail to parse (a
  hand edit with a typo) load as empty lists, and the next mutation writes the
  empty list back. `write_text_atomic` uses a fixed `.tmp` name and no fsync.
- **Fix:** keep the broken file aside (`.corrupt`) and refuse to overwrite it;
  use `write_private_atomic`.

### FR-022 · low · exec sessions of a finished run are never cleaned up at exit (needs-confirmation)

**Status:** fixed in core (`api::runs::shutdown`); frontends call it at exit.

- **Location:** `core/orchestrator/drive.rs:133-136`,
  `crates/execpolicy/src/processes.rs:585` (`kill_on_drop(false)`).
- **Problem:** sessions are kept after a `done` turn. Nothing kills them when
  the app or `lcgn` exits, so a dev server started by the agent can outlive it.
  I did not check whether the Windows Job Object closes them.

---

## 2. Tools

### FR-023 · medium · `read_file` cannot reach anything past the first 120 KB

**Status:** fixed. `read_file` takes `offset`/`limit`, with a 32 MiB size guard and a continuation hint.

- **Location:** `core/orchestrator/tools.rs:498-522`.
- **Problem:** the file is cut at 120 000 bytes before `start_line` /
  `end_line` are applied, and the result is cut again at 24 000 characters.
  The tool description says to use the line range for large files, but lines
  beyond the first 120 KB can never be returned.
- **Fix:** apply the line range first, then the size cap, and say how many
  lines the file has.

### FR-024 · medium · `edit_file` does not check that `old_text` is unique

**Status:** fixed. A non-unique `old_text` is refused with the match count; `replace_all` replaces every match.

- **Location:** `core/orchestrator/tools.rs:543-549`.
- **Problem:** the description says "must match exactly and be unique". The
  code replaces the first match, so an ambiguous snippet edits the wrong place
  without any warning.
- **Fix:** fail when `content.matches(old_text).count() > 1`.

### FR-025 · low · `write_file` and friends accept missing required arguments

**Status:** fixed. A missing `content` / `new_text` is rejected.

- **Location:** `core/orchestrator/tools.rs:528` (`unwrap_or("")`).
- **Problem:** a call with a `path` but no `content` truncates the file to
  zero bytes. Together with FR-012 (arguments cut at the token limit) this can
  empty a file.
- **Fix:** reject calls that lack a required argument.

### FR-026 · low · `search_files` details

**Status:** fixed except `.gitignore` support.

- **Location:** `core/orchestrator/tools.rs:606-694`.
- `*.rs` matches with `ends_with("rs")`, so `users` and `foo.tsrs` match.
- Every directory starting with a dot is skipped (`.github`, `.config`); the
  description names only `.git`, `node_modules` and `target`.
- Files are read whole with no size cap, and the walk blocks a runtime thread.
- `.gitignore` is not honoured.

### FR-027 · medium · `view_image` stores the image twice as base64 in the rollout and the event stream

**Status:** fixed. Image bytes are not stored in the rollout or events; the model can call `view_image` again.

- **Location:** `core/orchestrator/tools.rs:426-440`,
  `core/orchestrator/compact.rs:48-65`.
- **Problem:** the tool result carries `data_base64` and `data_url`, up to
  10 MiB each. The request builders strip them, but the persisted `tool`
  message, the `message_created` event and the token estimate use the full
  text. One 1 MB image adds about 2.7 MB to the rollout and about 700 000
  estimated tokens, which forces a compaction on every following round while
  the image is in the 8-message tail.
- **Fix:** keep the bytes out of the message (store a path reference) and
  exclude them from the estimate.

### FR-028 · medium · `remember` writes global memory with no prompt, in every mode

**Status:** fixed. `remember` prompts for global memory outside Full Access and for project memory in chat/plan/ask modes.

- **Location:** `core/orchestrator/agent_loop.rs:399-411`,
  `core/orchestrator/mod.rs:96-101` (offered in chat and plan),
  `core/orchestrator/prompts.rs:9-23`.
- **Problem:** the model can store `scope: "global"` text that is injected
  into the system prompt of every later run in every project. Content from an
  untrusted repository or a fetched page can use this to persist an
  instruction. There is no cap on count or size.
- **Fix:** prompt for global memories (or all memories outside Full Access),
  cap the block, and show new memories in the UI.

---

## 3. Security

### FR-029 · high · Locagens' own git calls run repository-controlled config before any trust decision (needs-confirmation)

**Status:** fixed. Every internal git call goes through one hardened builder: fsmonitor, hooks, external diff, textconv, pager and askpass are off, and system config is ignored (except for `push`, which needs the credential helper).

- **Location:** `core/services/git.rs:20-37`, `:40-69`; callers
  `core/api/projects.rs:307-326` (`git_status`, polled by the desktop Git tab),
  `core/services/snapshots.rs`, `core/orchestrator/review.rs:55`.
- **Problem:** `git status`, `git diff`, `git ls-files` run unsandboxed in the
  project folder with the repository's own `.git/config`. A project that
  arrives with a `.git` directory (an archive, a copied folder) can set
  `core.fsmonitor`, `diff.external` or a `textconv` driver, which git executes.
  The trust gate added for QA-001 covers hooks, MCP, plugins and rules but not
  this. Opening the project in the desktop app is enough.
- **Not run:** I did not plant a config to demonstrate it.
- **Fix:** for untrusted projects pass
  `-c core.fsmonitor=false -c core.hooksPath=<empty dir> -c diff.external=`
  and `--no-ext-diff --no-textconv`, or skip git until the folder is trusted.

### FR-030 · high · on macOS and Linux the `exec_command` `shell` argument runs any program without a prompt (needs-confirmation)

**Status:** fixed. Only known shells are accepted as `shell`; anything else is an error.

- **Location:** `crates/execpolicy/src/processes.rs:50-62`
  (`shell_for_options`), `core/orchestrator/exec_session.rs:121-124`,
  `core/orchestrator/gating.rs:64-70`, `:321-350`.
- **Problem:** the known-safe check, standing grants and "Allow for Run" look
  only at `cmd`. The `shell` value is used as the program. In Build mode the
  model can write `run.sh` (no prompt) and call
  `exec_command {cmd: "ls", shell: "./run.sh"}`: `ls` is known-safe, so
  `./run.sh -c ls` runs unprompted. The workspace sandbox still applies.
  Windows ignores `shell`.
- **Not run:** no Unix machine was available.
- **Fix:** allow only a fixed list of shells (`sh`, `bash`, `zsh`, `fish`),
  and include `shell`, `login` and `workdir` in what a grant matches.

### FR-031 · medium · MCP tools run without a prompt in Build mode

**Status:** fixed. MCP tools prompt in Build mode; standing grants work per tool.

- **Location:** `core/orchestrator/tools.rs:40`
  (`tool.starts_with("mcp__") && mode != "accept_edits"`).
- **Problem:** Build mode prompts for every command and every `fetch_url`,
  but any MCP tool (a database server's `execute_sql`, a GitHub server's
  delete call) runs at once. MCP servers are not sandboxed.
- **Fix:** prompt for MCP tools in Build mode too, with per-tool standing
  grants.

### FR-032 · medium · standing grants are broader than the card suggests

**Status:** partly fixed. Command grants need an equal domain list and `fetch_url` grants are per host; file-tool grants are still tool-wide.

- **Location:** `core/orchestrator/gating.rs:321-350`.
- A command grant saved without domains matches the same command with any
  `network_domains` (`if !rule.network_domains.is_empty()`), so a later call
  can add hosts with no new prompt.
- "Allow always" on `fetch_url` covers every URL, and on `write_file` (ask
  mode) every path.
- **Fix:** require equal domain lists; scope `fetch_url` grants to the host.

### FR-033 · high · an environment API key of one vendor is sent to any other provider that has no key *(sub-review, lines re-checked)*

**Status:** fixed. A vendor env key is only used for that vendor's built-in, or when it is named explicitly (`$VAR`, `envKey`).

- **Location:** `crates/config/src/providers/auth_store.rs:248-264`,
  `core/orchestrator/drive.rs:207-212`.
- **Problem:** `resolve_api_key` falls back to every variable in
  `ENV_FALLBACKS` for every provider id. With `ANTHROPIC_API_KEY` set and no
  key for `groq` or a custom `models.json` provider, the Anthropic key is sent
  as the bearer token to that provider's `baseUrl`.
- **Fix:** resolve only the provider's own variable; add an explicit
  no-auth flag for keyless local providers.

### FR-034 · high · model output can open arbitrary URI schemes and load remote images in the desktop app *(sub-review, lines re-checked)*

- **Location:** `desktop/markdown.rs:87-119` (links and images pass through
  unchanged).
- **Problem:** raw HTML is escaped, but `[x](file:///…)`, `ms-…:` and UNC
  links are handed to the OS opener on click, and `![](https://host/?d=…)` is
  fetched by the webview with no click, which is an exfiltration channel for a
  prompt-injected model.
- **Needs-confirmation:** which schemes the Windows opener accepts.
- **Fix:** keep only `http`, `https` and `mailto` links; replace images with
  their alt text or a link.

### FR-035 · medium · credential helper and redirect handling *(sub-review)*

**Status:** fixed. The credential helper runs in the Locagens home with a 10-minute cache; neither the provider client nor the LLM client follows redirects.
- `crates/config/src/providers/auth_store.rs:78`: the `!command` helper runs
  through `cmd /C` in the inherited working directory (the project folder), so
  a repository's `op.bat` can shadow the real helper. Its output is cached for
  the life of the process, including short-lived tokens.
- `crates/config/src/providers/mod.rs`, `core/orchestrator/llm/http.rs:138-146`:
  the HTTP clients follow redirects, and `x-api-key` is not stripped on a
  cross-origin redirect.
- **Fix:** set the helper's `current_dir` to the Locagens home and
  `NoDefaultCurrentDirectoryInExePath=1`; add a TTL; use
  `redirect(Policy::none())` for provider calls.

### FR-036 · medium · `auth.json` lock is held across the network refresh *(sub-review)*

**Status:** fixed. The OAuth refresh runs unlocked, then re-reads and compares; refreshes are serialized per provider; a lock broken by mistake is put back.

- **Location:** `crates/config/src/providers/auth_store.rs:188-194`,
  `:400-404`; `crates/config/src/oauth/auth_lock.rs:13`, `:76-83`.
- **Problem:** a refresh holds the lock for up to the HTTP timeout (30 to
  60 s) while other waiters give up after 10 s and report "No API key". The
  stale-lock break is check-then-remove, so two waiters can both take it.
  `acquire_blocking` sleeps on async threads.
- **Fix:** refresh outside the lock, then lock, re-read, compare and write.

### FR-037 · low · other credential and config hygiene *(sub-review)*

**Status:** fixed. Atomic `config.toml` trust writes; `Debug` hides env values; `save_configs` only removes custom providers it owns, never panics and never overwrites an unparseable `models.json`; `[model_providers]` is read from the Locagens home too.

- `crates/config/src/config_toml.rs:222`: `write_project_trust_level` rewrites
  the user's `config.toml` with a plain `std::fs::write` (not atomic, no lock).
- `config_toml.rs:43`, `:56`: `Debug` derives on structs that hold MCP env and
  header secrets.
- `crates/config/src/providers/model_catalog.rs:841-888`: `save_configs`
  deletes every provider missing from the submitted map (credentials too);
  `as_object_mut().unwrap()` panics on a hand-edited `models.json`; an
  unparseable `models.json` is replaced by `{}`.
- `crates/config/src/oauth/kimi.rs:164`: no expiry skew.
- `crates/config/src/oauth/xai.rs:59-62`, `device_code.rs`: one transient
  network error aborts a device-code login.
- `crates/config/src/oauth/callback_server.rs:116`: an accept error loops
  without a pause.
- `model_catalog.rs:284`: `[model_providers.*]` in the primary
  `~/.locagens/config.toml` is ignored (only fallback dirs are read), against
  `docs/CONFIG.md:50`.

---

## 4. CLI and TUI

### FR-038 · high · `--no-session` with `-c` or `--session` deletes the existing session (reproduced)

**Status:** fixed. `--no-session` with `-c`, `-r` or `--session` is rejected.

- **Location:** `core/cli/runner.rs:192-195`, `core/cli/mod.rs:204-220`,
  `core/cli/tui/driver.rs:138`.
- **Reproduction:** two sessions listed by `lcgn sessions`; run
  `lcgn -c --no-session -p "quick q"`; `lcgn sessions` now lists one. The most
  recent session's rollout is gone.
- **Fix:** reject the combination, or delete only runs this process created.

### FR-039 · high · typed-ahead keys answer a TUI permission prompt, including "always" *(sub-review)*

**Status:** fixed. Grant keys are ignored for 600 ms after the prompt opens; project/always need a double press.

- **Location:** `core/cli/tui/overlay.rs:100-104`,
  `core/cli/tui/app/input.rs:46`, `core/cli/tui/app/events.rs:77`.
- **Problem:** the overlay takes the very next key. While the user types a
  queued follow-up, an `a` becomes `allow_always` (persisted) and a `p`
  becomes `allow_project`.
- **Fix:** ignore keys for about 500 ms after the overlay opens, drain pending
  key events, and ask for a second key for the standing grants.

### FR-040 · high · AltGr characters cannot be typed in the TUI on Windows *(sub-review)*

**Status:** fixed. Ctrl+Alt+char is typed as text on Windows (AltGr).

- **Location:** `core/cli/tui/app/input.rs:223`
  (`KeyCode::Char(ch) if !ctrl && !alt`), `core/cli/tui/overlay.rs:307`.
- **Problem:** crossterm reports AltGr as Ctrl+Alt on Windows. On a Turkish Q
  layout `@`, `\`, `{`, `[`, `]`, `}`, `|`, `~` and `€` are dropped, so the
  `@file` popup and the `\` line continuation cannot be used.
- **Fix:** treat Ctrl+Alt with a printable character as text on Windows.

### FR-041 · high · environment-only authentication is rejected by the CLI pre-check *(sub-review)*

**Status:** fixed. The pre-check uses the same key resolution as the run; keyless providers work on loopback / `*.localhost` base URLs.
- **Location:** `core/cli/commands.rs:84`, `core/cli/auth.rs:325`,
  `crates/config/src/providers/auth_store.rs:526`.
- **Problem:** `has_api_key` looks only at `auth.json`. With just
  `ANTHROPIC_API_KEY` set, `lcgn --model anthropic/… -p hi` exits 2 with
  "not authenticated", although `docs/CLI.md:266` says the variable counts and
  `resolve_api_key` would use it. Keyless custom providers are blocked too.
- **Fix:** use one predicate for listing, the pre-check and resolution.

### FR-042 · medium · model ids that contain `/` conflict with `--provider` and config defaults *(sub-review)*

**Status:** fixed. With `--provider`, the whole string is tried as that provider's model id first.

- **Location:** `core/cli/models.rs:38`.
- **Scenario:** `lcgn --provider openrouter --model openai/gpt-4o`, or
  `model = "openai/gpt-4o"` with `model_provider = "openrouter"` in
  `config.toml`, fails with a provider conflict on every run.
- **Fix:** when a provider is given, first try the whole string as its model
  id.

### FR-043 · medium · the TUI panic hook restores the terminal for any thread while the UI keeps running *(sub-review)*

**Status:** fixed. The panic hook restores the terminal only for the UI thread.

- **Location:** `core/cli/tui/mod.rs:80-83`.
- **Problem:** a panic in a tokio task leaves raw mode and the alternate
  screen while the event loop continues, so the screen is garbled.
- **Fix:** restore only on the UI thread; log other panics.

### FR-044 · medium · quitting the TUI while busy skips cleanup *(sub-review)*

**Status:** fixed. Quitting waits up to 2 s for the run's cleanup.

- **Location:** `core/cli/tui/driver.rs:171-176`.
- **Problem:** `quit()` cancels but does not wait. A `--no-session` run keeps
  its rollout, and other runs stay `generating` until the next startup marks
  them failed.
- **Fix:** await the terminal event (about 2 s) before leaving.

### FR-045 · medium · `-p` can block forever on an inherited stdin pipe *(sub-review)*

**Status:** fixed. Piped stdin is read only without a message or with `--stdin`.

- **Location:** `core/cli/args.rs:339-349`.
- **Problem:** stdin is read to the end whenever it is not a terminal. A CI
  step or parent process that leaves the pipe open hangs `lcgn -p "…"`.
  Non-UTF-8 input is dropped silently.
- **Fix:** read stdin only when no message was given, or behind `--stdin`.

### FR-046 · medium · no `tracing` subscriber is installed *(sub-review)*

**Status:** fixed. `lcgn` logs to `<home>/logs/lcgn.log` (`RUST_LOG`).

- **Location:** `core/cli/mod.rs` (`main_entry`); none in the workspace.
- **Problem:** OAuth refresh failures, ignored config files, provider retries
  and `!command` errors are logged with `tracing` and never shown. The user
  sees only "No API key available".
- **Fix:** stderr subscriber behind `LCGN_LOG` for the CLI, a log file for the
  TUI and desktop.

### FR-047 · low · further CLI and TUI details *(sub-review)*

**Status:** fixed (every item).

- `core/cli/auth.rs:23-25`: `lcgn auth login 3` stores a credential under the
  provider id `3` (the documented number form is only resolved in the picker).
  `--api-key` puts the secret in the process list.
- `core/cli/repl.rs:52-69`, `:151`: Ctrl+C at an idle REPL prompt does not
  exit (docs say it does); during a run two handlers print two cancel notices;
  the watcher can call `process::exit(0)` and skip MCP shutdown.
- `core/cli/repl.rs:89`, `core/cli/tui/app.rs:313`: any line starting with `/`
  is a command, so `/usr/bin/foo fails` is "Unknown command" and the text is
  lost.
- `core/cli/tui/composer.rs:92-124`: deleting text does not reset the history
  position, so Down replaces an edited recalled entry.
- `core/cli/prompt.rs:54-56`: `@notes.txt` follows a symlink out of the
  project; `@./src/a.rs` is not attached.
- `core/cli/tui/ui.rs:355-390`: a long command or diff in the approval box is
  cut with no "more below" cue.
- `core/cli/tui/app/events.rs:23`: a question overlay replaces an open
  approval overlay (**needs-confirmation** that both can be pending).
- `core/cli/tui/app/events.rs:236-239`: any `user` event pops a queued entry.
- `core/cli/review.rs:150-158`: a review with no structured result prints
  nothing and exits 0.
- `core/cli/models.rs:155`: the `images` column is always `-`.
- `core/cli/args.rs:92-103`: an unquoted prompt whose first word is a
  subcommand name (`lcgn -p review the auth module`) runs the subcommand
  (**needs-confirmation**).

---

## 5. Desktop

### FR-048 · high · answering a permission or question card can wipe the next queued prompt *(sub-review, lines re-checked)*

- **Location:** `desktop/components/cards.rs:11-19` (`decide`), about `:399`
  (`answer_question`); core side `core/api/runs.rs:604-623` (no request id).
- **Problem:** `decide` sets `pending_permission` to `None` after the await.
  The core has already emitted the next queued prompt (sub-agents and parallel
  tool calls share one queue), so the new card is cleared. The run stays
  `awaiting_permission` with no card and a disabled composer until the run is
  reselected. The card is also cleared when the API call fails. Because
  decisions carry no request id, a double click on "Allow" answers the next
  prompt too.
- **Fix:** include the prompt id in the event and in `permission_decision`;
  clear the card only if it still shows the answered id.

### FR-049 · high · the Git and Terminal tabs start a new poll loop on every project switch and never stop the old one *(sub-review, lines re-checked)*

- **Location:** `desktop/components/right_panel.rs:415-447`, `:849-867`.
- **Problem:** the effect re-runs when the project changes and spawns another
  endless loop; only a `version` bump ends a loop. Old loops keep writing the
  previous project's status and diffs, so the lists flicker between projects,
  and every loop runs `git_diff_details` every few seconds.
- **Fix:** end the loop when the captured path differs from the current one,
  or use a keyed `use_resource`.

### FR-050 · high · opening a conversation does not switch the selected project *(sub-review, lines re-checked)*

- **Location:** `desktop/state_ui.rs` `select_run` (about `:356`); the only
  writers of `selected_project` are `state_ui.rs:320`, `sidebar.rs:300`,
  `:333`, `:621` and `right_panel.rs:709`.
- **Problem:** the header and the continue path use the run's project. The
  Git tab, review button, `@` mentions, `$` skills and `/undo` use
  `selected_project`.
- **Scenario:** project A is selected, the user opens an old conversation of
  project B. The header says B, the Git tab shows and commits A's changes, and
  `/undo` fails with "No git snapshot available".
- **Fix:** set `selected_project` in `select_run`; use one `current_project()`
  helper everywhere.

### FR-051 · high · the skill editor cannot save an existing skill *(sub-review)*

- **Location:** `desktop/components/settings/skills.rs:157`, `:181`;
  `core/services/skills.rs:68-71`.
- **Problem:** the editor is filled with the parsed body (no frontmatter,
  possibly with the "[skill body truncated]" marker). Saving calls `install`,
  which requires frontmatter, so it fails with "Invalid SKILL.md".
- **Fix:** load the raw `SKILL.md` for editing.

### FR-052 · medium · the Windows release build opens a console window

- **Location:** `desktop/main.rs:1-22` (no
  `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` anywhere
  in the workspace).
- **Problem:** `locagens.exe` is a console-subsystem program, so starting it
  from Explorer or the Start menu opens a console window next to the app.
  When the attribute is added, the `git`, hook and MCP child processes need
  `CREATE_NO_WINDOW` (only the sandbox sets it today), or each poll flashes a
  window. `bootstrap().expect(...)` at `:24` would then fail with no visible
  message; show a dialog instead.

### FR-053 · low · `default-run` and `build` are inside a metadata table

- **Location:** `apps/desktop-rs/Cargo.toml:7-11`.
- **Problem:** both keys follow the `[package.metadata.notes]` header, so
  Cargo reads them as metadata. The build script still runs (auto-detected);
  `default-run` has no effect.

### FR-054 · medium · a background run that waits for the user gives no signal *(sub-review)*

- **Location:** `desktop/components/sidebar.rs:442`
  (`status == "generating" || status == "created"`).
- **Problem:** runs in `awaiting_permission` or `awaiting_input` get no dot
  (the warn style exists in the CSS but is never applied), and
  `permission_requested` for a non-active run is dropped with no toast. A run
  started and left in the background blocks silently.

### FR-055 · medium · the permission card does not say which agent is asking *(sub-review)*

- **Location:** `desktop/components/cards.rs`; the fields exist in
  `core/orchestrator/run_handle.rs:115-116`, and are dropped by
  `pending_permission()` at `:156-164`.
- **Fix:** show role and name on the card and keep them in the replay payload.

### FR-056 · medium · `select_run` subscribes after loading, and `run_completed` can overwrite the active run *(sub-review)*

- **Location:** `desktop/state_ui.rs:371-400`, `:582-593`.
- Events between `list_messages` and `subscribe_run_events` are lost, so a
  tool card can stay "running".
- The `run_completed` handler sets `active_run` after an await without
  re-checking the active id; switching runs in that window leaves the header
  and composer on the wrong run.

### FR-057 · medium · provider, model, effort and mode are neither restored on start nor taken from the opened run *(sub-review)*

- **Location:** `desktop/state_ui.rs` (`use_ui_store` defaults),
  `desktop/components/composer.rs:823-835`.
- **Problem:** after a restart the first provider and Build mode are selected
  again. Continuing an old conversation sends the composer's selection, so the
  model changes silently.

### FR-058 · medium · settings and inspector wiring gaps *(sub-review)*

- `desktop/components/settings/mod.rs:29`: `use_signal(|| initial_tab)` is
  set once, so "Customizations" in the sidebar does nothing while Settings is
  open on another tab.
- `desktop/components/settings/mcp.rs:91`: a stdio server is saved with
  `args: None` and the whole input as the program name, so
  `npx -y @scope/server` cannot be added.
- `desktop/components/settings/skills.rs:24`: the list does not reload when
  the selected project changes.
- `desktop/components/cards.rs:31`: `QuestionCard` has no key, so a second
  question reuses the previous selections and step.
- `desktop/state_ui.rs` about `:540`: agents are stored as `running` and no
  event ever changes that; `right_panel.rs:370-382` prints raw role and status
  ids.
- `desktop/components/sidebar.rs:134`: the "Toggle sidebar" button toggles the
  search box.
- `desktop/components/sidebar.rs:572`: adding a project does not select it.

### FR-059 · medium · the Git tab shows a changed file as fully removed and fully added *(sub-review)*

- **Location:** `desktop/components/right_panel.rs:460-466` (`pseudo_diff`).
- **Problem:** a one-line change in a 2 000-line file renders 4 000 coloured
  lines, with no line cap.
- **Fix:** use `git diff` output, which the core already produces.

### FR-060 · medium · copy buttons may do nothing on Windows (needs-confirmation) *(sub-review)*

- **Location:** `desktop/markdown.rs` (`COPY_SCRIPT`),
  `desktop/components/message_thread.rs` about `:277`.
- **Problem:** the page origin on Windows is `http://dioxus.index.html/`, not
  a secure context, so `navigator.clipboard` is undefined. The check mark is
  shown regardless.
- **Fix:** copy from Rust and show the check mark only on success.

### FR-061 · medium · layout at small window sizes *(sub-review)*

- **Location:** `desktop/main.rs:28-32` (no `with_min_inner_size`),
  `desktop/theme.rs:714`.
- The sidebar (264 px) and inspector (380 px) are fixed, so a narrow window
  squeezes the chat column to nothing.
- Permission and question cards have no `max-height`; with a long command and
  a diff the Allow and Deny buttons are clipped below the window.

### FR-062 · medium · toasts overwrite each other and cut long text *(sub-review)*

- **Location:** `desktop/state_ui.rs:627`, `.toast` in `desktop/theme.rs`.
- Each toast starts its own 4-second timer that clears whatever is shown, so
  a second toast can vanish after one second.
- `white-space: nowrap` with an ellipsis hides most of an error message and
  the `/help` list.

### FR-063 · medium · streaming re-renders the whole thread on every event *(sub-review, needs-confirmation on cost)*

- **Location:** `desktop/components/message_thread.rs:113`,
  `desktop/components/review.rs` (`AssistantBody`).
- `store.messages.read().clone()` plus regrouping runs per `message_updated`;
  Markdown and syntax highlighting are redone for every assistant message when
  `active_run` changes.

### FR-064 · low · preferences file handling *(sub-review)*

- **Location:** `desktop/prefs.rs:108-113`, `desktop/components/appearance.rs`.
- A `settings.json` that fails to parse is replaced; one unknown value resets
  every preference; the write is not atomic; the font inputs save and rebuild
  the stylesheet on every keystroke.

### FR-065 · low · i18n and raw-id leftovers *(sub-review)*

- `desktop/i18n.rs`: `header.tokens` is "tok" in Turkish too ("tok" means
  "full").
- `desktop/components/composer.rs:176-180`: `/chat` mode shows as "Build";
  the mode toast prints `accept_edits`.
- `desktop/components/message_thread.rs:434-450`: every role except
  `explorer` is labelled "Worker".
- `desktop/components/sidebar.rs:532`: the path placeholder is a Windows path
  on every OS.
- Raw values in the permissions tab, sandbox status, MCP transport badge and
  terminal status; `"main"` at `right_panel.rs:716`.
- `desktop/components/settings/security.rs:63`: bind address and
  "authenticated" from the removed HTTP server are still shown.
- `desktop/components/review.rs:85`: `!verdict.contains("incorrect")` shows
  "patch is not correct" as passing.

### FR-066 · low · accessibility and small polish *(sub-review)*

- Row actions are `display:none` or `opacity:0` until hover, so keyboard users
  cannot reach or see them.
- Modals have no initial focus, focus trap or focus return.
- Segmented controls and question options have no pressed or radio state.
- No `prefers-reduced-motion` rule for the status-dot pulse.
- `with_disable_context_menu(true)` also removes paste and copy from text
  inputs.
- Task-list checkboxes in Markdown are invisible (the global `input` reset);
  no styles for `img`, `hr`, `h4` to `h6`.
- "Working…" is shown while the run waits on a permission card
  (`message_thread.rs:116`).
- Delete conversation, remove workspace, delete memory, skill, plugin, MCP
  server and grant all act on one click; a failed conversation delete shows
  nothing (`sidebar.rs:649`).
- The Terminal tab starts a command twice on a double Enter
  (`right_panel.rs:910`), and its log has no tail-follow.
- Classes used with no CSS: `badge accent`, `review-chip p3`,
  `patch-progress`, `toolbar-btn`. CSS never used: `.ccm-divider`, `.pill`,
  `.btn`, `.row-focusable`.
- Hooks called inside event handlers (`components/mod.rs:65`,
  `settings/mod.rs:43`).
- The Usage page sends one full scan per keystroke with no debounce, and a
  slow response can overwrite a newer one.

---

## 6. Code quality and tests

### FR-067 · low · missing regression tests for the lifecycle failures above

**Status:** fixed. Regression tests for the lifecycle, history, compaction, streaming and store fixes.

- No test replays a history that contains a failed round (FR-001), unanswered
  tool calls (FR-002) or a compaction split inside a tool group (FR-003).
- No test runs the snapshot on more than a handful of files (FR-006).
- The Anthropic request builder has no test with thinking plus tools (FR-013).
- `--no-session` is only tested for a fresh run (FR-038).

### FR-068 · low · rollout growth

**Status:** fixed. `run_update` lines repeat the task and prompts only when they change.

- **Location:** `core/db/runs.rs:48-65`.
- Every `patch_run` appends the whole `Run` (task text and system prompts
  included). A run with many permission prompts and an `@file`-heavy first
  prompt repeats that text on every status change.

### FR-069 · low · documentation drift

**Status:** fixed. CLI.md, CONFIG.md, database-schema.md and the LLM / orchestrator comments match the code.

- `docs/QA_REVIEW.md` QA-035 says streaming writes are one checkpoint every
  5 s; `core/orchestrator/mod.rs:66` still names a 250 ms
  `STREAM_PERSIST_INTERVAL` (the 5 s interval is enforced inside the store),
  which is confusing to read.
- `docs/CLI.md:188` (Ctrl+C exits an idle REPL), `:266` (environment keys) and
  `docs/CONFIG.md:50` (`[model_providers]` in the primary file) do not match
  the code (FR-047, FR-041, FR-037).
- `core/orchestrator/llm/mod.rs:1-22` still describes credentials as
  `~/.pi/agent/auth.json`.

---

## Additional findings folded into the count

The following are counted individually in the index and are short enough to
state here.

- **FR-070 · medium** · `core/orchestrator/llm/http.rs:108-117`: `Retry-After`
  is capped at 10 s and there are 4 attempts, so a 60-second rate limit always
  fails the run after about 30 s.
  **Status:** fixed. `Retry-After` / `retry-after-ms` honoured up to 90 s.
- **FR-071 · low** · `core/orchestrator/interactive.rs:63-84`: the question
  slot is single. It is only reachable from the main agent today, but the
  cleanup at `:80-84` clears whatever is in the slot, and
  `ask_user_question` does not react to a sub-agent interrupt
  (`stop_signal`), only to run cancel.
  **Status:** fixed. The question slot is owned by an id; a sub-agent interrupt ends the wait.
- **FR-072 · medium** · `core/orchestrator/tools.rs:958-970`: permission
  previews cut old and new content at 4 000 characters with no marker, so the
  user approves a write whose tail they cannot see.
  **Status:** fixed. Previews end with "… (N more characters)".
- **FR-073 · medium** · `core/orchestrator/exec_session.rs:114-118`:
  `max_output_tokens` allows up to 800 000 characters in one tool result, far
  above the 24 000-character cap of every other tool and enough to overflow
  the context in one call.
  **Status:** fixed. `exec_command` / `write_stdin` output is capped like the other tools.
- **FR-074 · medium** · `core/orchestrator/tools.rs:496`, `:541`: `read_file`
  and `edit_file` fail on any file that is not valid UTF-8 (Windows-1254 or
  Latin-1 sources), with a raw "stream did not contain valid UTF-8" error and
  no way to edit the file.
  **Status:** fixed. Non-UTF-8 files are read lossily with a note; editing them is refused.
- **FR-075 · low** · `core/orchestrator/mod.rs:96-101`,
  `core/orchestrator/agent_loop.rs:440-464`: chat and plan modes block MCP
  tools, but still offer `list_mcp_resources` and `read_mcp_resource`, which
  start the configured MCP servers and read from them with no prompt.
  **Status:** fixed. Chat and plan modes no longer offer the MCP resource tools.
- **FR-076 · low** · `core/orchestrator/drive.rs:190-192`:
  `reasoning_supported` uses `contains` on short markers (`o1`, `o3`, `o4`),
  so any model id that merely contains one of them gets a `reasoning_effort`
  parameter, which some providers reject.
  **Status:** fixed. `o1`/`o3`/`o4` only match as the model name's first segment.
- **FR-077 · low** · `core/api/runs.rs:562-574`: `delete_run` on a run driven
  by another process cancels nothing there, deletes the rollout, and the other
  process recreates nothing (tombstones are per process), so its later appends
  are lost without an error on either side (**needs-confirmation**, traced in
  code only).
  **Status:** fixed. `delete_run` refuses a run another process owns.
- **FR-078 · low** · `core/orchestrator/agent_loop.rs:205-209`,
  `core/api/runs.rs:793-800`: the cache hit rate is `cache_read / input`.
  Anthropic's `input_tokens` excludes cached tokens, so the rate can exceed
  100 %, and `context_usage` under-counts the context for Anthropic.
  **Status:** fixed. The cache hit rate stays ≤ 100 % and `context_usage` counts Anthropic correctly.

---

## Severity index

| Severity | Count | Findings |
| --- | --- | --- |
| critical | 0 | |
| high | 19 | FR-001, FR-002, FR-003, FR-004, FR-006, FR-010, FR-013, FR-029, FR-030, FR-033, FR-034, FR-038, FR-039, FR-040, FR-041, FR-048, FR-049, FR-050, FR-051 |
| medium | 36 | FR-005, FR-007, FR-008, FR-011, FR-012, FR-014, FR-016, FR-017, FR-023, FR-024, FR-027, FR-028, FR-031, FR-032, FR-035, FR-036, FR-042, FR-043, FR-044, FR-045, FR-046, FR-052, FR-054, FR-055, FR-056, FR-057, FR-058, FR-059, FR-060, FR-061, FR-062, FR-063, FR-070, FR-072, FR-073, FR-074 |
| low | 23 | FR-009, FR-015, FR-018, FR-019, FR-020, FR-021, FR-022, FR-025, FR-026, FR-037, FR-047, FR-053, FR-064, FR-065, FR-066, FR-067, FR-068, FR-069, FR-071, FR-075, FR-076, FR-077, FR-078 |

Reproduced with the built binary: FR-001, FR-006, FR-007 (timing), FR-015,
FR-038. Needs-confirmation: FR-013, FR-019, FR-022, FR-029, FR-030, FR-034
(scheme list), FR-060, FR-063 (cost), FR-077, the Unix threshold of FR-006,
and the two marked items in FR-047.
