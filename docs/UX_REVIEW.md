# Locagens UX review (2026-11-14)

Scope: the `lcgn` TUI (`crates/core/src/cli/**`) and the desktop app
(`apps/desktop-rs`). This is a UX review, not a bug hunt; where an item touches
a finding of `docs/FABLE_REVIEW.md` it says so and does not repeat it.

Method: code and docs were read. Neither frontend was run, so nothing here was
observed on screen; rendering claims come from the draw code and its tests.
Line numbers are from the working tree on 2026-11-14 (other edits were in
progress, so they may have moved by a few lines). Paths are relative to the
repo root; `tui/` means `crates/core/src/cli/tui/`, `desktop/` means
`apps/desktop-rs/src/`.

## Executive summary

The two frontends are at different levels. The TUI has a solid core loop: a
good composer, a searchable model picker, a live status line with the running
tool and elapsed time, diffs in the transcript. The desktop looks restrained
and coherent (neutral surfaces, one blue accent, no glows) but is mouse-only
and reports most outcomes through a four-second, single-line toast.

The largest gaps, in order:

1. **A new user cannot get started without leaving the app.** The desktop has
   no API-key form at all and the TUI has no sign-in; both tell the user to run
   `lcgn auth login` in another terminal. Neither remembers the last model.
2. **The agent's work is hard to audit.** The desktop shows a failed tool call
   as "Done", shows a failed run only in a toast, and keeps the turn diff and
   undo out of the thread. The TUI truncates tool output and diffs with no way
   to expand them.
3. **Permission prompts ask for a decision without the facts.** Neither
   frontend says why it is asking or what "project" and "always" would cover,
   and the TUI shows an `apply_patch` request as raw JSON without the diff.
4. **The desktop has no keyboard layer**: no shortcuts, no keyboard path
   through the `/` and `@` menus, no focus move to a permission card.
5. **The frontends disagree** on commands, tool labels and status wording, so
   what a user learns in one does not carry to the other.

Most fixes are small. Three shared pieces would remove a lot of duplication
and inconsistency: tool labels, the `/` and `@` trigger detection, and a
permission "reason and scope" payload from the core.

## Top 10

| # | Item | Impact | Effort | Frontend |
|---|------|--------|--------|----------|
| 1 | UX-001 Connect a provider inside the desktop (API key form, first-run panel) | high | M | desktop |
| 2 | UX-002 `/login` and a first-run screen in the TUI | high | M | TUI |
| 3 | UX-014 Permission prompts that state reason and scope | high | M | both |
| 4 | UX-008 Show failures where they happen (failed run, failed tool) | high | M | desktop |
| 5 | UX-009 "What changed" card with Undo at the end of each turn | high | M | both |
| 6 | UX-019 Desktop keyboard layer and command palette | high | M | desktop |
| 7 | UX-005 `/resume` session picker in the TUI | high | M | TUI |
| 8 | UX-006 Expandable tool output and diffs in the TUI | high | S | TUI |
| 9 | UX-015 Readable `apply_patch` and network prompts in the TUI | high | S | TUI |
| 10 | UX-003 Remember the last model and effort | high | S | both |

---

## A. First run and onboarding

### UX-001 · high · M · desktop · Connect a provider inside the app

**Current.** With no credentials, Settings opens on the Providers tab, which
shows one line of text: "No providers configured. Add one via `lcgn auth
login` or the provider config file (~/.locagens/auth.json …)"
(`desktop/components/settings/providers.rs:14-17`, `desktop/i18n.rs:312`).
The Accounts tab offers OAuth sign-ins only
(`desktop/components/settings/auth.rs:22-33`). There is no API-key input
anywhere in the desktop, although the facade already has
`api::providers::set_pi_auth` and `test_provider`
(`crates/core/src/api/providers.rs:166,192`); nothing in `apps/desktop-rs`
calls them. In the chat view the composer just shows "Provider" / "Model"
placeholders and sending produces the toast "Select a provider and model first
(Settings → Providers)" (`desktop/components/composer.rs:792-794`,
`desktop/i18n.rs:413`), which points at the tab that cannot help.

**Proposal.**
- Merge Providers and Accounts into one tab, **Models & accounts**. Top
  section "Connect a provider": one row per provider with a single action on
  the right, either `Sign in` (OAuth providers) or `Add API key`.
- `Add API key` expands the row in place: a password input (placeholder
  `Paste your API key`), buttons `Save` and `Cancel`, and the hint "Stored in
  ~/.locagens/auth.json on this computer. Never sent anywhere except the
  provider." `Save` calls `set_pi_auth`, then `test_provider`; show an inline
  result under the row: "Connected · 14 models" or "The provider rejected this
  key (401)." Never a toast.
- Connected providers show `Connected · API key ending …4f2a` (or
  `Signed in · renews automatically`) with `Remove`.
- First-run empty state in the chat view, shown while the provider catalog is
  empty, replacing the starter cards: headline "Connect a model to start",
  body "Locagens runs on your own provider account.", primary button `Connect
  a provider` (opens the tab above), secondary link "Already use Codex or pi?
  Their credentials are picked up automatically."
- The composer's send button is disabled with the tooltip "Connect a provider
  first" while the catalog is empty.

**Reference.** Zed's agent panel (provider list with inline key fields),
Cursor's model settings.

### UX-002 · high · M · TUI · `/login` and a first-run screen

**Current.** Without a model the TUI opens with the notice "No model selected.
Type /model and pick one from the list." (`tui/mod.rs:142-144`). With no
credentials `/model` then answers "No models available. Sign in with `lcgn auth
login <provider>`." (`tui/driver.rs:374`), and an unauthenticated provider
gives "provider "x" is not authenticated. Run `lcgn auth login x`."
(`cli/commands.rs:83-91`). Both require quitting the TUI. The welcome text is
static (`tui/ui.rs:61-71`).

**Proposal.**
- Add `/login [provider]` and `/logout <provider>` to the shared table
  (`crates/core/src/commands.rs:48-66`). `/login` opens a picker overlay built
  like `ModelPicker` (`tui/overlay.rs:19-80`): rows `OpenAI  Sign in with
  ChatGPT`, `Anthropic  API key`, with `✔ connected` on the right.
- API-key provider: the overlay switches to one masked input, title
  ` Anthropic API key `, footer `Enter save · Esc back`. On save, run the same
  store call as `lcgn auth login`, reload `model_choices`, and open the model
  picker filtered to that provider.
- Device-code provider: the overlay shows the code in bold, the URL below it,
  and `Waiting for approval…  Enter open browser · c copy code · Esc cancel`.
- When there are no callable providers at start, replace the welcome text
  with:

  ```txt
    Locagens

    No model provider is connected yet.
    Press Enter to connect one, or type /login.

    Credentials from Codex (~/.codex) and pi (~/.pi/agent) are used
    automatically when present.
  ```

  Enter on the empty composer opens `/login` in that state.
- Change the two error texts to end with "Type /login to connect it."

**Reference.** Claude Code `/login`, OpenCode `/connect`, pi `/login`.

### UX-003 · high · S · both · Remember the last model and effort

**Current.** The TUI takes its model only from `--model` or `config.toml`
(`cli/mod.rs:147-170`); `/model` changes the session only
(`cli/commands.rs:236-249`). A user who picks a model with `/model` has to pick
it again on every launch. The desktop starts on the first provider's first
model (`desktop/state_ui.rs:301-309`); FABLE FR-057 covers the desktop restore
bug, this item is about the shared behaviour.

**Proposal.** One small file, `~/.locagens/ui_state.json`
(`{"model":"provider/id","effort":"high"}`), written whenever the user picks a
model or effort in either frontend and read at start when no flag or
`config.toml` default applies. Precedence: flag, then the opened session's
model, then `config.toml`, then `ui_state.json`. In the TUI model picker, list
the three most recently used models first under a dim `Recent` row.

**Reference.** pi and Codex both reopen on the last model.

### UX-004 · med · S · desktop · Opening a folder is the first step, make it one click

**Current.** With no project selected the empty state still reads "What should
we build?" and a prompt runs in the core's default project directory without
saying so (`desktop/components/message_thread.rs:123-138`,
`crates/core/src/api/runs.rs:317-321`). Adding a project goes through a modal
with a text field whose placeholder is `C:\path\to\project` on every OS, a
`Browse` button and a second confirm button
(`desktop/components/sidebar.rs:517-572`).

**Proposal.**
- The sidebar `Add project` button and the empty state's primary button open
  the native folder picker directly and register the folder on confirm. Drop
  the modal (keep path typing for the CLI).
- With no projects, the empty state reads: headline "Open a folder to start",
  body "The agent reads and edits files inside the folder you choose.",
  button `Open folder…`. The composer is disabled with the placeholder "Open
  a folder first".
- With a project, make the starters contextual: if the project has
  uncommitted changes, the first starter is "Review my uncommitted changes"
  and runs `/review` (today it pre-fills a prompt telling the model to run
  `git status`, `desktop/i18n.rs:214-215`); below the starters list up to
  three recent chats of this project as "Continue: <title> · 2h".

**Reference.** Zed and Cursor open a folder from the empty window.

---

## B. Core loop

### UX-005 · high · M · TUI · `/resume` session picker

**Current.** Inside the TUI, `/sessions` prints up to 15 lines of `id  status
title` as a dim notice and nothing can be done with them
(`cli/commands.rs:199-215`). Switching means quitting and running `lcgn -r`,
which is a numbered stderr prompt before the UI starts
(`cli/auth.rs:449-478`), or `--session <id>`.

**Proposal.** `/resume` (keep `/sessions` as an alias) opens an overlay built
on the model-picker pattern: one row per session, `title` left, `2h ago ·
model · status` dim on the right, newest first, type to filter on title and
first prompt, `Enter` loads it through the existing `Driver::load_session`
(`tui/driver.rs:340-358`), `Esc` closes. Footer: `23 sessions in this folder ·
type to search · Enter open · Esc close`. `lcgn -r` starts the TUI with this
overlay open. Refuse while a run is generating, with the existing busy notice.

**Reference.** Codex `/resume`, Claude Code `/resume`.

### UX-006 · high · S · TUI · Expand truncated output and diffs

**Current.** A tool cell shows 5 output lines and a diff cell 40, followed by
"… +N lines" / "… +N more diff lines" (`tui/transcript.rs:11-13,236-245,
262-274`). There is no key to see the rest. `Ctrl+O` only toggles reasoning
(`tui/app/input.rs:53-56`). A failing test run is exactly the case where line
6 onward matters.

**Proposal.** Make `Ctrl+O` a "details" toggle for the whole transcript:
reasoning, full tool output (cap 200 lines per cell) and full diffs. The cache
key already carries `expanded` (`tui/transcript.rs:86-98`), so pass it to
`render_tool` and `render_diff`. Change the hints to "… +37 lines · Ctrl+O to
expand" and the welcome/help text to "Ctrl+O  show details". Failed tool cells
show 15 lines instead of 5 by default.

**Reference.** Claude Code `Ctrl+O`, Codex `Ctrl+T` transcript view.

### UX-007 · high · S · TUI · Copying text out

**Current.** The TUI enables mouse capture (`tui/mod.rs:54`), so a plain
click-drag no longer selects text in most terminals, and there is no copy
command. A user who wants the answer or a command has no obvious way to get
it.

**Proposal.**
- `/copy` copies the last assistant message as Markdown; `/copy code` copies
  its last fenced block. Use OSC 52 with a clipboard-crate fallback, and answer
  in the status line: "Copied 42 lines."
- On the first mouse drag event of a session, show once in the status line:
  "Hold Shift while dragging to select text · /copy copies the last answer".
- Add both lines to `/help`.

**Reference.** Claude Code `/copy`, OpenCode `/copy`.

### UX-008 · high · M · desktop · Show failures where they happen

**Current.**
- A failed run is reported only as a toast, "Run failed: {0}"
  (`desktop/state_ui.rs:599-601`), which is one line with an ellipsis and
  disappears after four seconds (`desktop/state_ui.rs:627-633`,
  `desktop/theme.rs:980-984`). `Run.error_message`
  (`crates/core/src/db/mod.rs:129`) is never rendered, so reopening the chat
  shows no reason at all.
- A tool call is either "Running" or "Done"
  (`desktop/components/message_thread.rs:361-362,408`). A call that failed or
  was denied shows the same green dot and "Done". The TUI already tells these
  apart (`tui/app/events.rs:207-210`, `tui/transcript.rs:209-213`).
- Tool rows show the raw tool name, e.g. `run_command`
  (`desktop/components/message_thread.rs:404`), where the TUI shows "Ran cargo
  test" (`tui/transcript.rs:286-323`).

**Proposal.**
- Move `describe_tool` and the ok/failed/denied test into a shared core module
  (for example `locagens_core::tool_labels`) and use it in both frontends.
- Desktop tool row: status glyph, then "Ran `cargo test`" / "Edited
  src/a.rs", then a right-aligned badge only when not ok: `Failed` (error
  colour) or `Denied` (muted). Failed rows start expanded. The activity
  summary becomes "Ran 3 commands, edited 2 files · 1 failed".
- A failed run ends the thread with a persistent block (not a toast), also
  rendered from `error_message` when the chat is reopened:

  ```txt
  ✖ The run stopped
  <error text, wrapped, selectable>
  [Copy error]   [Open Models & accounts]   (second button only for auth errors)
  Send a message to continue from here.
  ```
- Reserve toasts for confirmations of something the user just did ("Folder
  trusted"). Errors of an action go next to the control that caused them.
- TUI: after the red error cell add one dim line: "Send a message to continue
  · /undo reverts this turn's file changes".

**Reference.** Zed's agent panel (inline error with retry), Claude Code (red
tool result inline).

### UX-009 · high · M · both · "What changed" at the end of a turn, with Undo

**Current.**
- Desktop: the turn diff is only in the inspector under Changes
  (`desktop/components/right_panel.rs:476-489`), signalled by a small count on
  the inspector icon (`desktop/components/chat_header.rs:111-113`). Nothing in
  the thread says which files a turn touched. Undo exists only as the typed
  command `/undo`, and its result is the toast "Restored snapshot {id}"
  (`desktop/components/composer.rs:749-764`, `desktop/i18n.rs:393`).
- TUI: the turn ends with a "Changes: N files" diff cell cut at 40 lines and
  "── done · 1m 12s ──" (`tui/app/turn.rs:88-97`). `/undo` answers "Restored
  snapshot {id} (HEAD {sha}, N file(s))." (`cli/commands.rs:156-161`), which is
  internal vocabulary.

**Proposal.**
- Desktop: after a turn that changed files, render a card in the thread:

  ```txt
  3 files changed  +42 −7
    src/parser.rs        +30 −5
    src/lexer.rs         +10 −2
    tests/parser.rs       +2 −0
  [Review changes]   [Undo this turn]
  ```

  `Review changes` opens the inspector's Changes section. `Undo this turn`
  swaps the buttons for an inline confirm: "Revert these 3 files to how they
  were before this turn? Files you changed since are left alone." `Undo` /
  `Cancel`. Afterwards the card reads "Reverted · 3 files restored" (plus "·
  1 left alone: src/lexer.rs" when the API reports skipped paths). Only the
  latest turn's card has the Undo button.
- TUI: end the turn with one summary line instead of two cells:
  `✔ 1m 12s · 3 files changed +42 −7 · /diff to view · /undo to revert`,
  followed by the per-file stat list. The full diff appears with `Ctrl+O`
  (UX-006) or `/diff`.
- `/undo` text in both: "Reverted 3 files to before the last turn." and, when
  needed, "Left alone because they changed since: src/lexer.rs".

**Reference.** Cursor checkpoints ("Restore checkpoint"), Claude Code
`/rewind`, Zed "Review Changes".

### UX-010 · med · S · desktop · A status row that says what is happening

**Current.** While a run is active the thread shows a pulsing dot and
"Working…" (`desktop/components/message_thread.rs:143-148`), with no elapsed
time and no current action, and the same text while the run waits for a
permission answer. The TUI status line shows the running tool, elapsed time
and the interrupt key (`tui/ui.rs:169-179`) and has distinct waiting states
(`tui/ui.rs:143-153`).

**Proposal.** Reuse the TUI wording through the shared labels of UX-008:
"Thinking…", "Running `cargo test` · 1m 12s", "Writing changes · src/a.rs +3
−1", "Waiting for your approval", "Waiting for your answer". Append "· Esc to
stop" once UX-019 lands. Stop the pulse in the two waiting states.

### UX-011 · med · M · both · Queued messages are visible and recoverable

**Current.**
- Desktop: sending while a run is generating clears the composer and shows
  nothing until the model picks the message up
  (`desktop/components/composer.rs:812-825`).
- TUI: the message appears with "(queued)" (`tui/transcript.rs:161-165`) but
  cannot be edited or withdrawn; when the run is interrupted or fails, queued
  messages are dropped with "N queued message(s) were not delivered."
  (`tui/app/turn.rs:118-132`).

**Proposal.**
- Both: show queued messages in a strip directly above the composer, not in
  the transcript: "Queued · sent after the current step", one line per
  message.
- Desktop: each line has a remove button. TUI: `Up` on an empty composer while
  busy pulls the newest queued message back into the composer for editing.
  Both need one small facade function to withdraw a queued message.
- On interrupt or failure, put the undelivered text back into the composer
  (joined by blank lines) instead of discarding it, with the status line
  "Queued message restored to the composer."

**Reference.** Codex (queued messages listed above the composer, `Alt+Up` to
edit), Claude Code.

### UX-012 · med · S · both · Make the mode visible, and Full Access unmistakable

**Current.** TUI: the mode is one cyan word in the footer (`tui/ui.rs:248`);
`Shift+Tab` only toggles Plan and Build (`tui/app/input.rs:69-73`); Full
Access looks the same as Build apart from that word. Desktop: the mode button
looks identical in all three modes
(`desktop/components/composer.rs:617-628`), and switching by command toasts
the raw id, e.g. "Mode set to accept_edits" (FABLE FR-065).

**Proposal.**
- TUI composer border and title by mode: Build keeps cyan and ` Message `;
  Plan uses blue and ` Plan · read-only `; Full Access uses red and ` Full
  Access · no permission prompts `. Running keeps the current yellow border
  only in Build.
- `Shift+Tab` cycles Build → Plan → Build, and includes Full Access in the
  cycle only when the session was started with `--approve` or `/auto` was used
  once in it.
- Desktop: in Full Access the mode button text and shield use `--warn`; in
  Plan they use `--accent`. The menu gets one-line descriptions: "Build ·
  edits files, asks before commands", "Plan · reads only, proposes a plan",
  "Full Access · no permission prompts". `Ctrl+.` cycles the mode
  (`Shift+Tab` must stay reverse focus traversal in a GUI).
- Mode changes do not add a transcript notice or toast; the control itself
  changes.

**Reference.** Claude Code's coloured mode line and `Shift+Tab` cycle.

### UX-013 · med · M · desktop · One searchable model picker

**Current.** Provider and model are two separate dropdowns
(`desktop/components/composer.rs:454-506,508-611`), neither searchable.
Changing provider silently resets the model to the provider's first one
(`:491-494`). Reasoning effort is only reachable through a hover flyout on a
model row (`:567-578`), which keyboard and touch users cannot open. The TUI
has one searchable `provider/model` list (`tui/overlay.rs:19-80`).

**Proposal.** One button showing `model · effort`. It opens a popover with a
search field focused, models grouped under provider headings, the current one
checked, `↑↓` to move, `Enter` to pick, `Esc` to close. Below the list, when
the highlighted model supports it, a segmented row `Effort  Low | Medium |
High | …`. Last row: "Manage models…" (opens Settings). Shortcut `Ctrl+M`
(`Cmd+M` is taken on macOS, use `Cmd+Shift+M`).

**Reference.** Zed's model selector, Cursor's model menu.

---

## C. Trust and safety

### UX-014 · high · M · both · Permission prompts that state reason and scope

**Current.**
- Desktop: the card headline is assembled as "The agent wants to run
  **run_command** on **path**." (`desktop/components/cards.rs:160-169`,
  `desktop/i18n.rs:236-237`), so a command request reads "wants to run
  run_command". Five buttons of equal weight follow: Allow Once, Allow for
  Run, Allow in Project, Always Allow, Deny
  (`desktop/components/cards.rs:37-43,176-196`). No keyboard shortcuts.
- TUI: a one-line legend, `o once  r this run  p project  a always  d deny`
  (`tui/ui.rs:401-417`).
- Neither says why the prompt appeared, nor what the standing choices would
  grant. The event carries only `toolCall`, `preview` and the agent
  (`crates/core/src/orchestrator/run_handle.rs:111-118`). Dangerous commands
  and protected paths always prompt and no grant covers them (CLAUDE.md), yet
  the standing options are still offered. FABLE FR-032 notes that grants are
  broader than the card suggests; this item is the presentation side.
- A denial cannot carry a message, so the model gets no guidance.

**Proposal.**
- Core: add two fields to the `preview` object. `reason`: `command`,
  `dangerous`, `protected_path`, `network` or `fetch`. `grantLabel`: the exact
  text of what a standing grant would match (for example `cargo test *`), or
  null when no grant can apply.
- Titles by action in both frontends (the TUI already has them,
  `tui/overlay.rs:145-153`): "Run this command?", "Edit src/a.rs?", "Allow
  network access to api.github.com?".
- Reason line under the title:
  - `command`: "Commands need your approval in Build mode."
  - `dangerous`: "This command can delete or overwrite data. It is asked every
    time."
  - `protected_path`: "This changes .git. It is asked every time."
  - `network`: "The command wants to reach this host."
- Desktop buttons: primary `Allow once`; a secondary menu button `Always
  allow ▾` with "For this run · any command", "In this project · `cargo
  test *`", "Everywhere · `cargo test *`"; then `Deny`. Below them a text
  link "Deny and tell the agent what to do instead" that opens a one-line
  input; its text is sent with the denial. Hide the menu when `grantLabel` is
  null. Keys while the card has focus: `Enter` allow once, `Esc` deny.
- TUI legend, three rows, same rules:

  ```txt
  y allow once     r allow commands for this run
  p always in this project: cargo test *     a always, everywhere: cargo test *
  n deny     e deny and reply…     ↑↓ scroll · 12 more lines
  ```

  The "N more lines" cue also answers the missing scroll hint noted in FABLE
  FR-047.
- Show the asking sub-agent in the title when present ("explorer docs-scan
  wants to …"); FABLE FR-055 covers the missing data on the desktop.

**Reference.** Claude Code ("Yes, and don't ask again for `cargo test`
commands in <project>" and "No, and tell Claude what to do differently"),
Codex approval overlay.

### UX-015 · high · S · TUI · Readable `apply_patch` and network prompts

**Current.** `Approval::from_event` knows the actions command, fetch, move,
create, edit, delete and mkdir (`tui/overlay.rs:145-175`). The core also sends
`action: "patch"` with `files` and a ready `diff`
(`crates/core/src/orchestrator/tools.rs:1053-1060`) and `action: "network"`
(`crates/core/src/orchestrator/network_access.rs:124`). Both fall through to
the generic branch: the title is "Allow apply_patch?" and the body is the
first 400 characters of the raw JSON arguments, with no diff, because
`diff_for` only builds one from old and new content
(`tui/overlay.rs:171-174,186-196`). The desktop card already uses
`preview.diff` (`desktop/components/cards.rs:131-136`).

**Proposal.** In `from_event`: for `patch`, title "Apply this patch? (N
files)", details are the file list, and the diff is `preview.diff`. For
`network`, title "Allow network access to {host}?" with the detail `{host}:
{port}`. Keep the raw-JSON fallback only for unknown tools and label it
"Arguments:".

### UX-016 · med · S · desktop · Standing permissions readable as sentences

**Current.** Settings → Permissions lists `{tool} {status}` and `{command} ·
{project_path}` in monospace with a red Delete button
(`desktop/components/settings/permissions.rs:35-58`). The TUI has no way to
see or revoke grants.

**Proposal.** One sentence per grant, grouped under "Everywhere" and one
heading per project: "Always allow commands matching `cargo test *`". The
button reads `Revoke`. After revoking, the row stays for five seconds as
"Revoked · Undo". Empty state: "Nothing is allowed permanently. Choosing
'Always allow' on a permission prompt adds an entry here." In the TUI add
`/permissions`, which lists the same sentences numbered, and `/permissions
revoke <n>`.

---

## D. Information design

### UX-017 · med · S · TUI · Quieter footer, more useful header

**Current.** The footer packs `provider/model · think:high · Build ·
#abcdef12 · 12.3k tok · $0.0123 · 72% context left` on the left and key hints
on the right, which disappear first when the terminal is narrow
(`tui/ui.rs:238-276`). The session hash is not actionable. The header shows
the full project path and the title, no git branch (`tui/ui.rs:50-58`).
Confirmations such as "Mode set to build." and "Thinking set to high." are
appended to the transcript as notices (`cli/commands.rs:140,233`,
`tui/driver.rs:410`) although the footer already shows the new value.

**Proposal.**
- Footer left: `model · high · Build`. Footer right: `72% context · $0.01`.
  Drop the session hash and token total (both stay in `/usage`). Below 15 %
  context the right side reads "12% context left · /new starts fresh" in red.
- Header: ` lcgn  ~/repos/app (main)  Title`, with the home directory
  shortened to `~` and the branch from the existing read-only git helper.
- Key hints move to the composer placeholder, which is already there
  (`tui/ui.rs:202`): "Ask anything…  / commands · @ files · ? help".
- Setting changes flash in the status line for two seconds ("Thinking: high")
  and add nothing to the transcript.
- Reword the compaction notice from "context compacted (42 -> 9 messages)"
  (`tui/app/events.rs:36-40`, desktop `i18n.rs:382`) to "Older messages were
  summarized to free up context."

### UX-018 · med · S · TUI · `/help` as an overlay, `?` to open it

**Current.** `/help` prints the command names on one line and three dense
lines of keys as a dim transcript notice (`tui/app.rs:286-294`,
`tui/driver.rs:367-369`). It scrolls away and has no descriptions, although
the table has them (`crates/core/src/commands.rs:48-66`).

**Proposal.** A centered overlay with two columns, "Commands" (usage and
description from the table) and "Keys" (grouped: Send and edit, Navigate, Run
control). `?` on an empty composer opens it, `Esc` closes it. The welcome text
ends with "? for help".

**Reference.** Claude Code and Codex both open shortcuts on `?`.

---

## E. Keyboard and consistency

### UX-019 · high · M · desktop · Keyboard layer and command palette

**Current.** The only key handlers in the desktop are Enter in the composer,
Enter in the terminal tab and Enter in the memory field
(`desktop/components/composer.rs:267`, `right_panel.rs:909`,
`settings/memory.rs:33`), plus a script that closes modals on Esc
(`desktop/state_ui.rs:646-650`). There is no way to stop a run, start a chat,
switch chats or open settings from the keyboard, and no place that lists
commands. `/help` puts the whole command list into one toast
(`desktop/components/composer.rs:775-778`), which is cut to one line.

**Proposal.** One `onkeydown` on `.app-shell` (`Ctrl` is `Cmd` on macOS):

| Key | Action |
|-----|--------|
| `Esc` | Close the open popover; otherwise stop the active run |
| `Ctrl+N` | New chat in the current project |
| `Ctrl+K` | Command palette |
| `Ctrl+L` | Focus the composer |
| `Ctrl+B` / `Ctrl+Alt+B` | Toggle sidebar / inspector |
| `Ctrl+.` | Cycle mode (UX-012) |
| `Ctrl+,` | Settings |
| `Ctrl+/` | Shortcuts and commands sheet (also what `/help` opens) |
| `Up` (empty composer) | Recall the previous prompt |

The palette is one search field over three groups: chats (title and first
prompt, all projects), commands (the shared table) and settings tabs. Focus
the composer after opening a chat, starting a new one, and answering a card.
Put the shortcut in each button's tooltip, e.g. "New chat (Ctrl+N)".

**Reference.** Zed and Cursor command palettes; Codex desktop `Cmd+K`.

### UX-020 · med · M · desktop · `/` and `@` work from the keyboard

**Current.** The placeholder says "Ask anything, @ to mention, / for commands"
(`desktop/i18n.rs:245`). Typing `/` shows suggestions that can only be
clicked; Enter sends the half-typed text instead of accepting the highlighted
command (`desktop/components/composer.rs:229-256,267-271`). Typing `@` does
nothing; mentions are reached through `+` → Mentions, and the chosen path is
appended to the end of the prompt, not at the caret (`:335-343,364-408`).

**Proposal.** Move `detect_trigger` and the popup item builders
(`tui/popup.rs:126`) next to the command table in `locagens_core` and use them
in the desktop composer. Same keys as the TUI: `↑↓` move, `Tab` or `Enter`
accept, `Esc` dismiss; insert at the caret. Keep `+` for files outside the
project.

### UX-021 · med · S · desktop · No "CLI only" dead ends

**Current.** `/model`, `/thinking`, `/diff`, `/mcp`, `/skills`, `/sessions`
and `/usage` are CLI-only in the table (`crates/core/src/commands.rs:49-63`);
on the desktop they answer "{0} is only available in the lcgn CLI."
(`desktop/components/composer.rs:780`, `desktop/i18n.rs:398`), although each
has a desktop equivalent.

**Proposal.** Map them: `/model` opens the model picker, `/thinking <level>`
sets the effort, `/diff` opens the inspector's Changes, `/usage` opens the
Usage page, `/sessions` opens the palette on chats, `/mcp` and `/skills` open
their settings tabs. Mark all of them `ALL` in the table.

### UX-022 · med · S · TUI · Prompt history survives restarts

**Current.** Composer history is in memory only (`tui/composer.rs:18,29,
222-230`), so `Up` recalls nothing after a restart, although every prompt is
already written to `history.jsonl` (`crates/core/src/db/rollout.rs:223-224`).

**Proposal.** Seed the composer history at start with the last 100 prompts of
this project's sessions from `history.jsonl`. Add `Ctrl+R`: the composer title
becomes ` History search `, typing filters, `Enter` puts the match in the
composer, `Esc` cancels.

**Reference.** Codex and Claude Code keep prompt history across sessions.

---

## F. Visual polish

### UX-023 · med · S · TUI · A small theme instead of fixed ANSI colours

**Current.** Colours are literals: cyan accents, a black-on-cyan ` lcgn `
badge, yellow for busy and waiting states, magenta agent tags, and the `DIM`
modifier for all secondary text (`tui/ui.rs:19-21,53,146,187`,
`tui/transcript.rs:107,148-150,153`). `DIM` is ignored or nearly invisible in
some terminals, and yellow text is hard to read on a light background. There
is no `NO_COLOR` handling. User prompts are bold with a `›` marker only
(`tui/transcript.rs:152-160`), so turns are hard to find when scrolling.

**Proposal.** One `Theme` struct (accent, muted, warn, ok, err, user
background) with a dark and a light preset, chosen by `tui.theme` in
`config.toml` and defaulting from `COLORFGBG` when set. Use an explicit grey
for muted text instead of `DIM`. Honour `NO_COLOR` by keeping only bold,
reverse and the glyphs. Give user prompts a full-width subtle background band.
Keep one accent; do not add more hues.

**Reference.** Codex and Claude Code both mark user turns with a background
band.

### UX-024 · med · S · desktop · Secondary text contrast and minimum size

**Current.** In the dark theme `--text-3` is `#6f6f79` on `#0e0e10`
(`desktop/theme.rs:32,48`), a contrast of about 3.9:1, below the 4.5:1 that
WCAG AA asks for body text. It is used in 54 rules, including 11 px monospace
tool arguments (`desktop/theme.rs:690`); 18 rules set 11 px or smaller.

**Proposal.** Raise dark `--text-3` to `#85858f` (about 5.3:1). Use 12 px as
the minimum for anything the user reads (tool arguments, paths, timestamps)
and keep 11 px for badges and counters only. The light theme's `#6e6e76` on
`#f6f6f4` is about 4.7:1 and can stay.

### UX-025 · low · S · desktop · Less repetition in the thread

**Current.** Every assistant message carries a label with the raw model id
(`desktop/components/message_thread.rs:179-182`), so a turn with ten tool
rounds repeats it ten times. User messages have no actions at all
(`:167-173`); assistant messages get age and Copy on hover (`:262-287`).

**Proposal.** Show the model label only on the first assistant message of a
turn and when it differs from the previous one. Give user bubbles the same
hover footer with `Copy` and `Reuse` (puts the text in the composer).

### UX-026 · low · S · desktop · Motion that explains, under a reduced-motion guard

**Current.** The only animation is the status-dot pulse
(`desktop/theme.rs:440-445`); cards and messages appear abruptly, and there is
no reduced-motion rule (FABLE FR-066).

**Proposal.** Inside `@media (prefers-reduced-motion: no-preference)`: a 120
ms fade and 4 px rise for a new permission or question card and for the
"files changed" card; nothing on streamed text. Outside it, no animation and a
static status dot.

---

## G. Accessibility

### UX-027 · med · M · desktop · Focus and announcements for cards that block the run

**Current.** A permission or question card is inserted between the thread and
the composer (`desktop/main.rs:121-124`). Focus stays where it was, nothing is
announced, and a keyboard user has to tab through the thread to reach the
buttons. The toast has `role="status"` (`desktop/components/mod.rs:120`); the
cards have no role.

**Proposal.** When a card mounts, focus its primary button. Give the card
`role="group"` with `aria-labelledby` its title, and write the title into a
visually hidden `aria-live="assertive"` region ("Permission needed: run
command cargo test"). After the answer, return focus to the composer. Give
the status row of UX-010 `aria-live="polite"`; do not put a live region on
streamed message text. In the question card, options are radio or checkbox
inputs so the arrow keys work (FABLE FR-066 notes the missing state).

### UX-028 · low · S · TUI · Reduced motion and a screen-reader path

**Current.** The spinner and the shimmer redraw the status line about ten
times a second while a run is active (`tui/ui.rs:122-139,169-176`,
`tui/app.rs:32`), which screen readers on a terminal read as constant change.
There is no switch.

**Proposal.** `tui.animations = false` in `config.toml` (and
`LCGN_REDUCED_MOTION=1`): a static `…` instead of the spinner, no shimmer, the
elapsed time updated every five seconds. In `docs/CLI.md` and `/help`, name
`--no-tui` as the mode to use with a screen reader.

---

## Not proposed

Cloud sync, accounts, multi-user features and further git operations are out
of scope per CLAUDE.md. No item adds an approval checkpoint beyond the
existing permission flow; UX-009's undo confirm is a confirmation of a user
action, not a new gate on the agent.
