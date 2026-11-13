# Command sandbox

Commands started by the agent (`run_command`, `exec_command`) run inside a
native sandbox chosen from the run's mode (`locagens-protocol`
`from_run_mode`):

| Mode | Sandbox mode | Effect |
| --- | --- | --- |
| `chat`, `plan` | `read-only` | (commands are not offered in these modes) |
| `accept_edits` (Build), `ask_permissions` | `workspace-write` | writes only inside the workspace + a temp dir; network: Windows uses a per-domain allowlist proxy, macOS/Linux are offline (or fully open once `network_domains` is approved), see "Network by platform" |
| `auto` (Full Access) | `danger-full-access` | no sandbox, no proxy, unrestricted network |

### Network by platform

`run_command` and `exec_command` build their sandbox with one function
(`tools::command_sandbox_spec`), so they follow the same rules:

| Case | Windows | macOS / Linux |
| --- | --- | --- |
| Full Access | unrestricted | unrestricted |
| `[sandbox_workspace_write] network_access = true` | unrestricted (no proxy) | unrestricted |
| Build, call without `network_domains` | per-domain allowlist proxy (config + grants; unknown hosts prompt) | offline |
| Build, call with approved `network_domains` | per-domain allowlist proxy | **whole network opens for that command** (no per-domain proxy exists on these platforms) |

The per-domain allowlist is therefore a Windows-only feature. On macOS/Linux
approving `network_domains` is an all-or-nothing grant for the command.

Backends (`locagens_execpolicy::sandbox::detect_backend`):

| Platform | Backend | Mechanism |
| --- | --- | --- |
| macOS | `macos-seatbelt` | `sandbox-exec` profile (argv wrapper) |
| Linux | `linux-bwrap` | Bubblewrap bind mounts (argv wrapper) |
| Windows | `windows-restricted-token` | write-restricted token + capability-SID ACLs + Job Object + network proxy (in-process spawn) |

### Fail closed on macOS / Linux

The backend binary is looked up in its well-known paths (`/usr/bin/sandbox-exec`;
`/usr/bin/bwrap`, `/bin/bwrap`) and then on `PATH` (NixOS, `/usr/local/bin`,
Homebrew). If it is not found, **sandboxed modes refuse to run commands**
(the tool result / session log says `Locagens sandbox unavailable: …` with
how to install bubblewrap or to switch to Full Access); nothing runs
unsandboxed outside Full Access. The desktop security tab shows the status
`unavailable` with the same message.

The rest of this document describes the Windows backend (`crates/execpolicy/src/windows_sandbox/`) and the per-domain
network proxy (`crates/execpolicy/src/network_proxy.rs`).

## Windows backend

It is a port of the non-elevated ("legacy") path of Codex
`windows-sandbox-rs`. It needs no admin rights, no service and no extra user
accounts.

### How a command is started

1. **Plan** (`windows_sandbox/plan.rs`, pure data, unit-tested): from the
   sandbox mode and workspace root compute the capability SID, the writable
   roots, the protected paths and the private temp dir.
2. **Directories**: create the private temp dir and `<workspace>\.locagens`
   (so a sandboxed command cannot create `.locagens` itself first).
3. **ACLs** (`windows_sandbox/acl.rs`): grant / deny ACEs for the capability
   SID (exact list below). Each edit is skipped when the ACE already exists.
4. **Token** (`windows_sandbox/token.rs`): `CreateRestrictedToken` on the
   user's own token with `DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED`
   and the restricting SIDs `[capability SID, logon-session SID, Everyone]`.
   Reads are checked as usual; every *write* must also be granted to one of
   the restricting SIDs. The token's default DACL grants the logon session
   full access so the child (and Locagens) can use the objects it creates.
5. **Process** (`windows_sandbox/process.rs`): `CreateProcessAsUserW` with
   piped stdin/stdout/stderr, `CREATE_NO_WINDOW`, desktop `Winsta0\Default`,
   an explicit inheritable-handle list (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`)
   and the Job Object attached at creation (`PROC_THREAD_ATTRIBUTE_JOB_LIST`).
   `cmd /C <script>` is passed as `cmd.exe /s /c "<script>"` so the script's
   own quotes survive. `TEMP`/`TMP` point at the private temp dir; the
   network proxy variables are added (see below).
6. **Job Object** (`windows_sandbox/job.rs`): `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
   Killing a session (`ProcessManager::kill`, run cancellation, timeouts)
   terminates the whole job; if Locagens exits, closing the job handle kills
   the tree. When the root command exits on its own, kill-on-close is cleared
   so background children it started keep running (Codex
   `preserve_descendants`).

Interactive `exec_command` sessions keep working: stdin is a pipe fed by a
writer thread, output is read by blocking reader threads. Windows has no PTY
mode (as before); `tty` is ignored there.

### Fail closed

If any step fails (workspace missing, ACL cannot be changed, token or process
creation fails, proxy cannot bind) the command is **not** run and the tool
result / session log says `Windows sandbox: …` with the reason. Only
Full Access (`danger-full-access`) runs unsandboxed. Callers that still try
the argv-wrapper API (`wrap_shell_command`) for a sandboxed Windows command
get a stand-in command that prints an error and exits with code 126.

### What changes on disk

All changes only name the sandbox capability SID. Existing ACEs are kept.

| Path | Change |
| --- | --- |
| `<workspace>` | explicit, inheritable (`OI`/`CI`) **allow** ACE: read, write, execute, delete. `SetNamedSecurityInfoW` propagates it to existing files as inherited ACEs (one-time cost on large trees). |
| `%TEMP%\locagens-sandbox\tmp-<hash>` | created; same allow ACE. Used as the command's `TEMP`/`TMP`. |
| `<workspace>\.locagens` | created if missing; explicit inheritable **deny** ACE: write data, append, write EA/attributes, delete, delete child. |
| `<workspace>\.git\hooks` | same deny ACE (only if it exists). |
| `<workspace>\.git\config` | same deny ACE, not inheritable (only if it exists). |

The capability SID has the shape `S-1-5-21-a-b-c-d` and is derived from the
lowercased workspace path (`plan::capability_sid_for`), so every workspace
gets its own SID and one workspace's sandbox cannot write into another. A
read-only sandbox uses a fixed SID with no grants anywhere. Nothing else is
written: no state file, no registry change, no change to the `NUL` device.

Unsandboxed processes are unaffected: the ACEs only matter for tokens that
carry the capability SID as a restricting SID.

**Reverting**: `locagens_execpolicy::windows_sandbox::revoke_workspace_access(path)`
removes every ACE the sandbox added for that workspace (root, protected paths,
temp dir). Manually: `icacls "<workspace>"` lists the entry as a raw
`S-1-5-21-…` SID; `icacls "<path>" /remove *S-1-5-21-a-b-c-d` removes it
(repeat for the paths in the table above). Deleting the temp dir is safe.

### Not covered on Windows

- **Reads** are not restricted (same as Codex's non-elevated backend and the
  macOS profile's `file-read*`): a sandboxed command can read anything the
  user can.
- **Network approval is best-effort on Windows** (the permission card and
  the security tab say so). `NO_PROXY` also exposes local services. Deny-read
  ACEs for `~/.locagens` or `.ssh` are not possible with this token model:
  `WRITE_RESTRICTED` applies the restricting-SID check to writes only, so an
  ACE naming the capability SID never affects reads. Only dedicated sandbox
  user accounts (the elevated Codex backend) would close this.
- **Network isolation is proxy-based only.** Programs that ignore the proxy
  variables (raw sockets, some native tools) can still connect directly.
  Codex blocks that with dedicated sandbox user accounts plus Windows
  Filtering Platform / firewall rules, which need admin rights and a setup
  service — left out.
- Codex's elevated backend features are left out for the same reason:
  sandbox user accounts, deny-read ACLs, firewall/WFP rules, the setup
  service, and the private desktop.
- A `.git` directory created *by* the sandboxed command (`git init`) is not
  protected; only metadata that exists before the command starts is.
- Objects writable by `Everyone` or the logon session stay writable (inherent
  to the restricting-SID model; Codex has the same property).
- Tools that write caches or config outside the workspace (`%APPDATA%`,
  `%LOCALAPPDATA%`, `%USERPROFILE%\.cargo`, the registry under `HKCU`) get
  "access denied" in Build mode, as on macOS/Linux. Point them at the
  workspace (e.g. `npm_config_cache`) or use Full Access.
- The first sandboxed command in a large workspace pays a one-time cost while
  the inheritable ACE propagates to existing files.

## Network proxy

`NetworkProxy` is a loopback HTTP proxy (`127.0.0.1:<ephemeral>`) started per
sandboxed command on Windows and dropped with it. It handles `CONNECT
host:port` tunnels (HTTPS, git, websockets) and absolute-form plain HTTP
requests (rewritten to origin form, `Connection: close`). Full Access mode
starts no proxy.

### Allowlist

Patterns follow Codex: `example.com` matches exactly that host,
`*.example.com` any subdomain, `**.example.com` the apex and any subdomain;
the global `*` is ignored. A command may reach:

1. `[sandbox.network] allowed_domains` in config.toml (`~/.locagens`, then the `~/.codex` / `~/.pi/agent` fallbacks; empty by
   default, like Codex — package registries are **not** allowed implicitly):

   ```toml
   [sandbox.network]
   allowed_domains = ["registry.npmjs.org", "**.crates.io"]
   ```

2. the `network_domains` of the tool call (`run_command` and
   `exec_command`), which the user approves together with the command. Such
   calls never use the known-safe "no prompt" shortcut;
3. hosts with a standing `network_access` grant in `permissions.json`
   (scope `project` or `global`);
4. hosts approved with "Allow for Run" earlier in the same run.

Targets that resolve to loopback, private, link-local or CGNAT addresses are
refused unless the allowlist has an exact entry that is `localhost` or an IP
literal. An approved ordinary hostname that resolves to such an address
(DNS rebinding, `169.254.169.254`) stays refused. Request hosts must be plain
hostnames (`[A-Za-z0-9.-]`) or IP literals; anything else (`*.com`, spaces)
gets `400` before any prompt, and approved hosts are stored as literal
entries, never wildcards.

### Approval of new hosts

Like Codex, a request for an unknown host is held while the user is asked:
the run goes to `awaiting_permission` with a regular permission request whose
tool is `network_access` and whose preview carries `url: "host:port"` and
`networkDomains: [host]` — the existing permission card and `lcgn` prompt
render it. Answers:

- `allow_once`: allowed for the rest of this command;
- `allow_run`: this host is allowed for every command in this run (it does
  not cover other hosts or tools);
- `allow_project` / `allow_always`: persisted as a `network_access` grant;
- `deny`: refused.

Prompts are only raised while the tool call that started the command is
still running (and no other permission request is pending). Requests from a
backgrounded `exec_command` session after its call returned are refused
without a prompt.

A refused request gets `403 Forbidden` with the body
`Locagens sandbox: network access to "<host>" is blocked … re-run the command
with network_domains: ["<host>"]`, which tools like curl/npm print. In
addition the tool result (`run_command`) or the session log (`exec_command`)
ends with `Sandbox blocked network access to: <hosts>. To request access,
re-run the command with network_domains: [...]`.

### Environment of a command

Every command path (`run_command`, `exec_command` via pipes, PTY or the
Windows sandbox spawn) builds its environment with
`locagens_execpolicy::env_policy`: a secret-free safe allowlist (`PATH`,
`HOME`, `TEMP`, …, no `*API_KEY*`/`*TOKEN*`/`*SECRET*`/…) refined by
`[shell_environment_policy]` (`inherit`, `exclude`, `set`, `include_only`).

### Environment of a proxied command

All of these point at `http://127.0.0.1:<port>`: `HTTP_PROXY`, `HTTPS_PROXY`,
`ALL_PROXY`, `FTP_PROXY`, `WS_PROXY`, `WSS_PROXY` (+ lowercase),
`YARN_HTTP(S)_PROXY`, `npm_config_http(s)_proxy`, `npm_config_proxy`,
`NPM_CONFIG_*`, `BUNDLE_HTTP(S)_PROXY`, `PIP_PROXY`, `DOCKER_HTTP(S)_PROXY`.
`NO_PROXY` (and `no_proxy`, `npm_config_noproxy`, `NPM_CONFIG_NOPROXY`,
`BUNDLE_NO_PROXY`) is `localhost,127.0.0.1,::1`. `NODE_USE_ENV_PROXY=1`,
`ELECTRON_GET_USE_PROXY=true` and `LOCAGENS_NETWORK_PROXY_ACTIVE=1` are set.

## Protected paths and command approval

`execpolicy::protected_paths` is the one definition used by every sandbox
backend (`.locagens`, `.git/hooks`, `.git/config`) and by the file tools
(`.git`, `.locagens`, `.codex`, `.agents`, matched case-insensitively and
ignoring `:stream` suffixes and trailing dots/spaces). Outside Full Access a
file-tool mutation of a protected path always prompts, and no standing grant
or Allow-for-Run covers it.

Known-safe commands (no prompt in Build) are a short read-only list (`git
status/diff/log/show/branch`, `rg`, `grep`, `find`, `ls`, `cat`, ...) and only
while their arguments stay inside the workspace and avoid writing or
executing options (`find -exec/-delete`, `rg --pre`, `git diff --output`,
...). Build and test tools (`cargo check`, ...) prompt. On Windows commands
are split with cmd.exe quoting, and any `%`, `!`, `^`, redirection or
grouping makes the command non-safe. Dangerous markers (`sudo`, `rm -rf`,
`git push --force`, `reg delete`, ...) are matched on tokens and always
prompt outside Full Access. Approving a command never amends project
`.rules`; standing grants go to `permissions.json` only. Commands on Windows
run with `NoDefaultCurrentDirectoryInExePath=1`, so a repo-shipped `git.bat`
is not picked up from the working directory.

## Deviations from Codex

| Codex | Locagens | Why |
| --- | --- | --- |
| Random capability SIDs persisted in `cap_sid` | Derived from the workspace path | No state file; a restricting SID can only reduce access, so knowing it grants nothing |
| Per-writable-root SIDs, `TEMP`/`TMP` dirs granted | One SID per workspace; a private temp dir is created and granted instead of the user's whole `%TEMP%` | Minimal ACL footprint (no propagation through `%TEMP%`) |
| Protects `.git`, `.codex`, `.agents` | Protects `.git\hooks`, `.git\config`, `.locagens` | Same carve-outs as the macOS/Linux profiles; git keeps working |
| Grants the capability SID on `\\.\NUL` | No change | `> nul` works without it (verified) |
| Job with `BREAKAWAY_OK` | No breakaway | The whole tree must die with the run |
| Private desktop | `Winsta0\Default` | Elevated-backend hardening, not needed for write isolation |
| WFP/firewall network block, SOCKS5, MITM, credential broker | HTTP/CONNECT proxy only | Firewall rules need admin; the rest is out of scope |
| `NO_PROXY` empty unless local binding is allowed | `NO_PROXY=localhost,127.0.0.1,::1` | Local dev servers and test fixtures keep working |
| ConPTY for `tty` sessions | Pipes only | Locagens never had a Windows PTY mode |

## Tests

- `windows_sandbox::plan` — policy → plan (roots, carve-outs, SIDs, fail
  closed), all platforms.
- `windows_sandbox::acl` — grant / deny / revoke round trip (Windows).
- `network_proxy` — allowlist semantics, env injection, request parsing,
  allow/deny against a local test server, decider prompts once per host.
- `sandbox` — fail-closed setup errors and argv wrapping (Windows); missing bwrap / sandbox-exec refuses, resolved bwrap path, PATH lookup (all platforms).
- `crates/execpolicy/tests/windows_sandbox.rs` (Windows) — writes inside the
  workspace and temp dir succeed, writes to a sibling dir and the home dir
  are denied (with a full-access control), `.git`/`.locagens` carve-outs,
  read-only mode, a sandboxed `curl` cannot reach a non-allowlisted host
  through the proxy but reaches an allowed one, interactive stdin sessions,
  job kill, fail-closed sessions.
