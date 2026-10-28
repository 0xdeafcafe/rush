# rush in Rust — design

The Go rush (`../cmd`, `../internal`) rewritten in Rust, module by module.
The Go version keeps shipping until this one does everything it does. What
matters is taken from decline (`0xdeafcafe/terminal-decline`): idle costs
nothing, memory is small and measured, floods never block typing, and every
speed or memory claim has a number behind it.

## Goals

- **Idle costs nothing.** No ticks, redraws or wakeups while nothing
  changes. The main loop blocks on one channel; background threads (file
  watching, process table, spend scanning) send it only real changes. The
  one timer is the spinner of a live agent, and only while one is on screen
  and the terminal has focus.
- **Small memory.** Measured as macOS *physical footprint*
  (`/usr/bin/footprint -p <pid>`), not RSS. Go rush sits at about 40 MB with
  a session open; opening one large transcript retains about 22 MB. Target:
  < 20 MB with a large session open. Conversations are where the memory is:
  historic turns keep their text, not their rendered lines (decline's
  scrollback lesson: store text plus style runs, expand only what's shown).
- **Floods never block typing.** A transcript growing by megabytes is read
  and parsed on a background thread in slices; the UI thread takes each
  result in one short step. Input is handled before drawing.
- **Draw only what's visible.** Screens render only the rows in view;
  history is indexed so a frame's cost doesn't grow with the conversation
  (the Go version's remaining hot spot, see
  `../docs/performance-review-2026-10-01.md`).
- **Same look.** Screens copy the Go TUI exactly (`../docs/screenshots`,
  `internal/ui/style.go`, `internal/convo/style.go`). Same keys, same words.

## Modules (Cargo workspace)

| crate         | does                                                          | Go source                                  |
|---------------|---------------------------------------------------------------|--------------------------------------------|
| `rush-core`   | agents, jobs, spend, messages, styled text, palette, time     | `internal/agent`, `ui/style.go` colours     |
| `rush-hl`     | single-pass syntax highlighter                                | `photon/hl`                                |
| `rush-convo`  | transcript tail → session model → styled lines                | `internal/convo`                           |
| `rush-claude` | Claude Code adapter: discovery, transcripts, spend, preview   | `internal/adapters/claude`                 |
| `rush-draw`   | the thin drawing layer; the only crate that knows ratatui     | (bubbletea / ultraviolet)                  |
| `rush-ui`     | screens: agent list, session view; event loop                 | `internal/ui`                              |
| `rush`        | the binary: same commands and flags as Go rush                | `cmd/rush`                                 |

`rush-core`, `rush-hl`, `rush-convo` and `rush-claude`'s parsing have no
terminal code and are tested and benchmarked on their own. `rush-draw`
exposes rush's own types (`rush_core::text::{Line, Span, Style}`, rects, key
events); no ratatui or crossterm type crosses it, so the backend can be
swapped for our own cell grid if measurements ever say the screen matters
(ratatui's cell is 48 bytes, two screens kept: ~1 MB at 200×50).

Later modules, in order: host (running Claude headless, PTYs), the other
adapters (codex, kimi, copilot, …), overview/efficiency/machine screens,
plugins, pr drive/watch, remote, menubar, update.

## Threads

1. **UI thread**: owns all screen state. Blocks on a channel of events:
   keys/mouse/resize from the terminal reader thread, and results from
   workers. After handling everything queued it draws once.
2. **Terminal reader thread**: crossterm events into the channel.
3. **Workers**: discovery (sessions/jobs files, process liveness), spend
   scanning (incremental from the last offset), previews, the open
   transcript's tail. Each watches files with kqueue/FSEvents where it can
   and sends only changes. Polling, where unavoidable, is slow (≥ 2 s) and
   stops when nothing is live. Discovery also samples what each live
   agent's processes use (memory, CPU, how many), walking only those
   process trees (`proc_listchildpids`, `proc_pid_rusage`), not the whole
   process table as Go does.

## Parity oracles

- Agent list: Go `rush --dump` prints each agent as
  `ID State Name CPU Mem Cost Elapsed Age`; Rust `rush --dump` prints the
  same, so the two diff.
- Screens: Go `rush --render WxH [keys]` prints one frame; so does Rust.
- Conversation: Go `RUSH_DUMP=<file> go test ./internal/convo -run TestDump`
  writes every rendered line at widths 20/60/120/250; the Rust tests render
  the same transcripts and compare text.

## Performance testing

- `cargo bench -p rush-convo`: apply events, open a large transcript cold,
  warm frame of a 3,000-turn conversation, incremental tail.
- `cargo bench -p rush-claude`: spend scan, preview read.
- Footprint: `/usr/bin/footprint -p <pid>` idle on the list, and with the
  largest transcript under `~/.claude/projects` open; side by side with Go.
- Idle: `top -l` CPU and wakeups over 60 s with nothing changing.

## Hosted sessions' memory

rush's own footprint is small next to the agents it hosts. Measured
2026-10-28 (physical footprint, macOS, Apple silicon), 13 Claude Code
sessions and one Codex session open:

| process                                      | each          | 13 sessions |
|----------------------------------------------|---------------|-------------|
| Claude Code 2.1.306 (Bun + bundled JS)       | ~150 MB       | 2.0 GB      |
| LangWatch MCP server via `npx` (user config) | ~110 MB       | 1.4 GB      |
| Codex 0.163.0 `app-server`, busy turn        | 100–340 MB    | —           |
| Codex `codex-code-mode-host`                 | ~20–27 MB     | —           |
| rush (Go)                                    | ~40 MB, once  | —           |

Every agent already runs headless (Claude `-p` stream-json, Codex
`app-server`, the ACP agents, Pi `--mode rpc`), so there is no terminal UI
left to strip. Where the memory goes, and what the host module does about it:

- **Idle sessions are most of it.** Most hosted processes sit at 0% CPU
  waiting for the user. The host stops an agent process after it has been
  idle a while and starts it again with its session id when the user comes
  back (`claude --resume <id>`, Codex `thread/resume`): about a second's wait,
  ~150 MB back per session. Same goal as "idle costs nothing", applied to
  what rush hosts.
- **Codex fragments its heap.** Idle, 55 MB of its 123 MB heap is in use.
  `MallocSpaceEfficient=1` in the app-server's environment: idle 80 → 36 MB,
  busy-turn peak 146 → 77 MB, after the turn 106 → 45 MB, CPU the same
  (5 turns without, 10 with, each ~670 shell commands; one of 12 with it
  stalled in `codex-code-mode-host`, not seen again). The commands Codex
  runs are kept from it (`shell_environment_policy.exclude`). No effect on
  Claude Code (Bun has its own allocator).
- **One Codex app-server can hold many threads.** rush starts one per
  session today (`internal/adapters/codex/conn.go`); one per Codex account
  saves a process per extra session, at the cost of one crash ending them
  all and per-session environment (TMPDIR) needing care.
- **MCP servers started per session multiply.** A stdio MCP server in the
  user's config runs once per session (LangWatch: an `npm exec` wrapper plus
  node, ~110 MB, reported upstream as langwatch/langwatch#9161). Where rush
  can, it runs one and lets every session share it.
- **Not worth it: rewriting Claude Code's insides.** `BUN_OPTIONS=--preload`
  runs our code inside Claude Code, and native code loads from there
  (`bun:ffi`, N-API), but its CPU is ~1–2% on average, its time per turn is
  the model's, and nothing loaded beside it shrinks its JS heap. Bun's
  `--smol` saves ~1–9%. A network proxy (`ANTHROPIC_BASE_URL`) gains
  visibility, not speed.

## Milestones (full plan: PLAN.md)

1. **Read-only Claude** (this): the agent list (header, rows, summaries,
   columns) for Claude sessions — live, background jobs and past — and
   opening one session read-only (turns, folded steps, markdown, code
   highlighting, diffs). `rush --dump`, `rush --render`.
2. Host Claude: start, send, interrupt, permissions, the message box.
3. Other adapters.
4. Remaining screens, plugins, pr, remote, menubar, update; then the Go
   version retires.
