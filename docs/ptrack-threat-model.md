# p-track threat model

## Executive summary

p-track is a single-user, local-first application: one Rust binary provides the
CLI, the terminal dashboard, and a Tauri desktop app that share a per-project
redb database. Its dominant risks are a hostile WebView script reaching
privileged IPC, a hostile project or agent smuggling options or control bytes
into subprocesses and terminals, secrets leaking into persistent project data,
agent-facing text being read as instructions, and a compromised release
channel installing a malicious binary.

Plan 4's capability broker, which let agents make brokered HTTP, Git, SSH, and
scp calls, has been retired. Capability brokering moved to the companion
project pam. p-track no longer starts a broker, injects capability tokens into
terminals, exposes capability IPC commands, or serves `ptrack capability call`
or `ptrack capability mcp`; `ptrack capability` only prints a pointer to pam.
The `ptrack-capability` and `ptrack-capability-policy` crates are deleted.
Capability grant and audit records that earlier releases wrote stay in the
project store so those databases still open, but nothing reads them as
authority, and Reset Application State revokes leftover grants in the open
project. The broker-specific threats are kept below as retired entries so
their IDs stay traceable.

The residual boundary is explicit: p-track is not a sandbox. Agent and shell
processes launched in its terminals are ordinary user processes and can use
the network, Git, and SSH directly.

## Scope and assumptions

- In scope: the desktop IPC surface and WebView (`src-tauri`,
  `crates/ptrack-app/src/desktop_runtime`), detached terminal windows, the
  terminal host and paste/clipboard handling (`crates/ptrack-terminal`,
  `frontend/src/terminal`), Git subprocesses (`crates/ptrack-git`,
  `crates/ptrack-app/src/service.rs`), the cwd-bound project MCP server
  (`crates/ptrack-app/src/project_mcp.rs`, `mcp_transport.rs`), the agent
  integration loopback server (`crates/ptrack-agent`), agent-facing digests
  (`crates/ptrack-core/src/report.rs`, `secrets.rs`), and the updater
  (`crates/ptrack-updater`).
- Runtime model: one local user runs p-track; agents run as child processes of
  the terminal host or as independent processes in the project directory.
- Data sensitivity: project records, terminal output, clipboard captures, and
  repository metadata can contain credentials or private code.
- Out of scope: OS-level network or filesystem sandboxing of agent processes,
  multi-user or remote hosting, and release-workflow permission design beyond
  the signing step.

## System model

### Primary components

- The desktop runtime parses every IPC request once into a typed command and
  checks it against a fixed allowlist, the calling window, and the current
  workspace generation (`crates/ptrack-app/src/desktop_runtime/command.rs`,
  `wire.rs`).
- The project store holds plans, tasks, notes, issues, commits, the scratchpad,
  and inert capability records (`crates/ptrack-store/src/project.rs`).
- The terminal host runs PTY sessions for shells and agent profiles and injects
  only the agent-event endpoint and token (`crates/ptrack-app/src/terminal_runtime.rs`).
- Git inspection and `commit show` run Git through one hardened command
  builder (`crates/ptrack-git/src/runner.rs`).
- The updater discovers, verifies, stages, and hands off releases
  (`crates/ptrack-updater`).

### Data flows and trust boundaries

- WebView → IPC: typed commands cross the Tauri bridge. The CSP allows scripts
  only from the app itself; the main window and detached terminal windows have
  different command allowlists.
- Host → agent child: `PTRACK_AGENT_EVENT_ENDPOINT_V1`,
  `PTRACK_AGENT_EVENT_TOKEN_V1`, and for linked launches `PTRACK_LAUNCH_CONTEXT_V1`
  cross the process environment. Launch context is redacted with the shared
  credential detector. No capability variable is set.
- Agent → integration server: structured events cross authenticated loopback
  HTTP with a run-bound token; bodies are closed schemas without prompts,
  output, or credentials.
- Agent → project database: `ptrack mcp` and the CLI read and write the project
  selected by the working directory. Closing gates apply to both.
- Project data → agent: `ptrack context` and MCP `get_context` return a capped,
  redacted digest that begins with an untrusted-data notice.
- Project data → Git: stored commit SHAs and hook arguments reach Git
  subprocesses.
- Clipboard and paste: terminal copies can be saved to the scratchpad; pasted
  text is written into a PTY.
- Release → updater: `checksums.txt`, its signature, and the package cross from
  GitHub into a private stage.

## Assets and security objectives

| Asset | Why it matters | Security objective (C/I/A) |
| --- | --- | --- |
| Project records and scratchpad | Durable plans, notes, and snippets; may hold sensitive text | C, I, A |
| User's shell startup files and home directory | Overwriting them yields code execution at next login | I |
| Installed `ptrack` binary | Replacing it runs attacker code with the user's authority | I |
| Terminal sessions | Keystrokes written to a PTY execute as the user | I |
| Desktop IPC | Every privileged action flows through it | I, A |
| Agent event token | Lets a process report events for one run | I |
| Agents' reading context | Text that agents treat as instructions steers their actions | I |

## Attacker model

### Capabilities

- A hostile repository can contain crafted commit subjects, paths, author
  names, remote URLs, Git configuration, and hook files.
- A launched agent or another same-user process can write arbitrary text into
  project records through the CLI or MCP, and can race lifecycle transitions.
- Script injected into the WebView (for example through a rendering bug) can
  call any IPC command that window is allowed to call.
- A compromised release account or Actions run can publish arbitrary release
  assets.
- Text copied from a web page or a hostile file can reach the paste path.

### Non-capabilities

- There is no internet-facing listener; loopback servers bind to 127.0.0.1.
- A remote attacker cannot read process environments or the private runtime
  directory without another local compromise.

## Entry points and attack surfaces

| Surface | How reached | Trust boundary | Notes | Evidence (repo path / symbol) |
| --- | --- | --- | --- | --- |
| Main-window IPC | Tauri invoke | WebView → host | Typed allowlist, exact generation on mutations | `crates/ptrack-app/src/desktop_runtime/command.rs` |
| Terminal-window IPC | Tauri invoke from `terminal-*` windows | WebView → host | Nine terminal commands only; window label taken from the caller | `crates/ptrack-app/src/desktop_runtime/wire.rs`, `scope_request_to_window` |
| Content security policy | WebView load | Page → script execution | `script-src 'self'`; inline theme script hashed at build time | `src-tauri/tauri.conf.json` |
| OS notifications | Rust-only notification plugin | Host → OS shell | Opt-in, background-only, identifier-only copy | `src-tauri/src/notification_runtime.rs` |
| Project MCP stdio | `ptrack mcp` | Provider → project database | Cwd-bound; four closed, bounded tools; closing gates preserved | `crates/ptrack-app/src/project_mcp.rs` |
| Agent integration HTTP | `/v1/runs/<id>/events` | Local process → host | Run-bound bearer token, closed event schemas | `crates/ptrack-agent/src/integration.rs` |
| Context digest | `ptrack context`, MCP `get_context` | Project data → agent | Byte caps, redaction, untrusted-data notice | `crates/ptrack-core/src/report.rs` |
| Commit show | `ptrack commit show`, desktop | Stored SHA → Git | SHA validated as hex; `--end-of-options` | `crates/ptrack-app/src/service.rs`, `check_commit_sha` |
| Git snapshot | Opening a project | Repository → host | Lossy decoding, credential-free remote URLs | `crates/ptrack-git/src/snapshot.rs` |
| Hook install | `ptrack hook install` | CLI → hooks directory | Honors `core.hooksPath`; refuses foreign interpreters | `crates/ptrack-app/src/service.rs`, `hook` |
| Paste | Terminal paste | Clipboard → PTY | Control bytes stripped; multi-line review | `frontend/src/terminal/paste.ts` |
| Clipboard capture | Terminal copy | PTY output → project store | Secret screening before save | `frontend/src/terminal/secrets.ts` |
| Updater | Check, download, install | GitHub → installed binary | Ed25519-signed manifest, pinned key | `crates/ptrack-updater/src/signature.rs` |

## Top abuse paths

1. An agent stores `--output=$HOME/.zshrc` as a commit SHA, and a later
   `commit show` passes it to `git show`, which overwrites the file → SHAs are
   validated as 4 to 64 hex digits on write and again before show, and Git
   receives `--end-of-options`, `--no-ext-diff`, and `--no-textconv` through the
   hardened runner.
2. A compromised release run publishes a trojaned Linux tarball with a matching
   `checksums.txt` → the updater refuses any manifest whose `checksums.txt.sig`
   does not verify against the Ed25519 public key compiled into the binary.
3. Injected script in a detached terminal window calls project mutations → the
   window may call only its terminal commands, and tab state is keyed by the
   caller's own window label.
4. An inline `<script>` injected into the page runs → the CSP no longer allows
   inline scripts.
5. A pasted line carries `ESC[201~` followed by control characters to escape
   bracketed paste and execute before review → C0 controls and bracketed-paste
   markers are stripped, and multi-line text always goes through the review
   dialog unless rich shell integration reports a running command.
6. A user copies terminal output containing an API key and it is saved to the
   scratchpad in the project database → captures that look like credentials
   are refused with "Not saved: looks like a secret".
7. A note or title contains "ignore previous instructions" or a forged
   heading and an agent follows it → titles are flattened to one line, fields
   are capped and redacted, and the digest begins with a notice that its
   contents are untrusted data.
8. A crafted commit subject containing separator bytes blanks the Git panel →
   the log format is NUL-delimited and malformed records are skipped.
9. A stale frontend issues a mutation against the previous project →
   mutations require the exact workspace generation.

## Threat model table

| Threat ID | Threat source | Prerequisites | Threat action | Impact | Impacted assets | Existing controls (evidence) | Gaps | Recommended mitigations | Detection ideas | Likelihood | Impact severity | Priority |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TM-001 to TM-008 | — | — | Retired with the capability broker (token replay, profile spoofing, HTTP/Git/SSH scope escapes, grant races, audit secret leakage, broker exhaustion) | — | — | No broker, token, or executor exists; stored grants authorize nothing and can be revoked by Reset Application State | None | Keep the capability tables read-only until a schema change can drop them; never reintroduce a reader that treats them as authority | Tests assert no `PTRACK_CAPABILITY_*` variable is injected and no capability IPC command exists | — | — | Retired |
| TM-009 | User expectation mismatch | Agent terminal launched normally | Agent invokes network tools, Git, or SSH directly | Actions happen without p-track oversight | Remote and project data | Documentation states p-track is not a sandbox; pam owns scoped capabilities | No OS-level enforcement | Do not describe p-track as a sandbox; point users to pam | None reliable in p-track | High | High | High |
| TM-010 | Malicious agent or project | Can write commit records | Store option-shaped SHAs that `git show` later interprets | Overwrite arbitrary user files; escape project-local mode | Home directory, shell startup files | Hex validation on add and show; hardened Git runner with scrubbed environment, `--end-of-options`, `--no-ext-diff`, `--no-textconv` (`check_commit_sha`, `hardened_git_command`) | Records written before this build are validated only at show time | Keep every Git call on the hardened runner | Count refused SHAs | Low | High | Medium |
| TM-011 | Compromised release channel | Release token or Actions run | Publish a malicious package with a matching checksum | Malicious binary installed by the updater | Installed binary | Ed25519 signature over `checksums.txt` verified against a compiled-in key on every platform before any digest is trusted; macOS adds Developer ID team pin and Gatekeeper (`signature.rs`, `staging.rs`) | Key compromise requires an app release to rotate | Keep the private key only in the release secret; document rotation | Release job verifies its own signature before publishing | Low | Critical | High |
| TM-012 | Script in a detached terminal window | Code execution in that WebView | Call project, plan, settings, or update commands | Unauthorized mutation from a less trusted surface | Project records, IPC | Terminal windows reach only nine terminal commands; window label replaced by the caller's; main window cannot call terminal-window tab commands (`scope_request_to_window`, `allowed_terminal_window_commands`) | None known | Keep new commands off the terminal-window allowlist by default | Security contract test enumerates the allowlist | Low | Medium | Medium |
| TM-013 | Injected markup | HTML injection in the WebView | Run inline script | Full IPC access for that window | Desktop IPC | `script-src 'self'` without `'unsafe-inline'`; build-time hash for the one inline theme script; `ws://localhost:*` dropped from `connect-src` (`tauri.conf.json`, `security_contract.rs`) | Styles still allow `'unsafe-inline'` | Move remaining inline styles to stylesheets, then drop `'unsafe-inline'` from `style-src` | Contract test pins the CSP string | Low | High | Medium |
| TM-014 | Hostile clipboard content | User pastes into a terminal | Smuggle control bytes or end bracketed paste early to run commands | Command execution without review | Terminal sessions | C0 controls and bracketed-paste markers stripped; multi-line review forced, and output-entered alternate screen alone no longer bypasses it (`paste.ts`) | Single-line pastes without controls are sent directly by design | Keep the strip list in tests | Paste guard tests | Medium | High | Medium |
| TM-015 | Secrets in terminal output | User copies text holding a credential | Capture is persisted in the project database | Credential stored in project data and backups | Scratchpad | Captures matching the credential patterns are refused (`secrets.ts`, `looksLikeSecret`) | Pattern-based; novel formats pass | Extend patterns with the shared Rust detector's cases | Tests for each pattern family | Medium | Medium | Medium |
| TM-016 | Malicious agent or collaborator | Can write records the next agent reads | Plant instructions or forged sections in titles and notes | Next agent is steered off task or into harmful actions | Agents' reading context | One-line titles on write; per-field and 32 KiB total caps; shared credential redaction; untrusted-data notice in Markdown, JSON, and MCP (`report.rs`, `secrets.rs`, `validation.rs`) | Agents may still follow instructions in bodies | Keep the notice first in every agent-facing digest | Truncation flags in output | Medium | Medium | Medium |
| TM-017 | Hostile repository | Crafted subjects, paths, or remotes | Break or spoof the Git panel, or leak remote credentials into the UI | Availability loss or credential display | Desktop, credentials | NUL-delimited log, lossy decoding, malformed records skipped, userinfo stripped from remote URLs, process group killed on timeout (`snapshot.rs`, `status.rs`, `runner.rs`) | None known | Keep Git output parsing total | Snapshot tests with hostile fixtures | Low | Medium | Low |
| TM-018 | Hook managers or foreign hooks | `core.hooksPath` set, or a non-shell hook | Append shell to a Python/Node hook or install where Git never runs it | Broken hooks or silent audit gaps | Git hooks | Hooks directory resolved through Git and refused outside the project; non-shell interpreters refused; block inserted before a final `exec` (`service.rs`, `hook`) | None known | Keep refusals explicit | `hook status` reports the effective path | Low | Low | Low |
| TM-019 | Stale frontend or racing window | Project switch in progress | Apply a mutation to the wrong workspace | Writes land in another project | Project records | Mutations require the exact generation; legacy generation-free commands removed; initialization re-checks under the transition lock | None known | Keep new mutations on the exact-generation check | Desktop runtime tests | Low | Medium | Low |

## Criticality calibration

- Critical: installing attacker code through the updater, or remote code
  execution in the host without a prior local compromise.
- High: writing outside the project from stored data (TM-010), IPC escalation
  from injected script, or command execution through paste without review.
- Medium: persistence of secrets in project data, steering an agent through
  planted text, or mutations from a less trusted window.
- Low: availability loss in read-only panels, or misleading diagnostics.

## Focus paths for security review

| Path | Why it matters | Related Threat IDs |
| --- | --- | --- |
| `crates/ptrack-app/src/desktop_runtime/command.rs` | Typed IPC parsing and allowlist | TM-012, TM-019 |
| `crates/ptrack-app/src/desktop_runtime/wire.rs` | Window scoping of IPC requests | TM-012 |
| `src-tauri/tauri.conf.json`, `src-tauri/tests/security_contract.rs` | CSP and Tauri permissions | TM-013 |
| `crates/ptrack-app/src/service.rs` | Commit SHA validation, `git show`, hook install | TM-010, TM-018 |
| `crates/ptrack-git/src/runner.rs`, `snapshot.rs`, `status.rs` | Hardened Git subprocesses and parsing | TM-010, TM-017 |
| `crates/ptrack-updater/src/signature.rs`, `staging.rs`, `discovery.rs` | Release authenticity | TM-011 |
| `frontend/src/terminal/paste.ts` | Paste guard | TM-014 |
| `frontend/src/terminal/secrets.ts` | Clipboard capture screening | TM-015 |
| `crates/ptrack-core/src/report.rs`, `secrets.rs` | Agent-facing digest bounds and redaction | TM-016 |
| `crates/ptrack-app/src/terminal_runtime.rs` | Agent environment injection | TM-001 to TM-008 (retired), TM-009 |
| `src-tauri/src/notification_runtime.rs` | OS-facing copy and opt-in | — |

## Quality check

- Covered main-window and terminal-window IPC, the CSP, notifications, project
  MCP, the agent integration server, agent-facing digests, Git subprocesses,
  hooks, paste, clipboard capture, and the updater.
- Recorded the capability broker's retirement and kept its threat IDs as
  retired entries.
- Stated the not-a-sandbox boundary as a residual risk rather than claiming
  isolation.
