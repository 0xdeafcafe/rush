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
| `rush-core`   | agents, jobs, spend, styled text, palette, time; the adapter traits and registry, events and their wire, tool calls and their words, failures, quotas | `internal/agent` (and its event, tool, failure, usage), `ui/style.go` colours |
| `rush-hl`     | single-pass syntax highlighter                                | `photon/hl`                                |
| `rush-convo`  | events → session model → styled lines; the transcript tail    | `internal/convo`                           |
| `rush-claude` | Claude Code adapter (`Claude`, rush_core's Adapter): discovery, transcripts, spend, preview, history; its tools and messages as rush's | `internal/adapters/claude`                 |
| `rush-codex`  | Codex adapter (`Codex`): rollouts read back and followed as events, threads found, a live thread in its own `codex app-server` (`Conn`), limits, resets, models, sign-ins | `internal/adapters/codex`                  |
| `rush-pi`     | Pi adapter (`Pi`): `pi --mode rpc` run headless (a Driver), its session files listed (live: written in the last 2 min), read back as the branch of the tree they're on — sliced, and followed with a reset when the branch changes | `internal/adapters/pi` |
| `rush-antigravity` | Antigravity adapter (`Antigravity`): agy in its persistent stream-json mode, the history rush keeps of it under `antigravity-history/` (read, tailed, followed), its model catalog snapshot | `internal/adapters/antigravity` |
| `rush-fleet`  | the list's rows from every adapter in the registry; what a row's state means | `internal/fleet`                           |
| `rush-host`   | the host: the process a rush-mode session lives in (`rush host run <id>`), its socket protocol, the UI's client to it, its per-agent watching (sleep, away, stalls, hung shells, long tasks, answers spawned agents owe), spawn_agent's agents, and rush's own tools served to each session (agtools.rs: in process, or `rush mcp-tools` over stdio); `rush spawn` (a headless session standing in for claude -p or codex exec) is the binary's | `internal/host`, cmd/rush's host commands |
| `rush-draw`   | the thin drawing layer; the only crate that knows ratatui     | (bubbletea / ultraviolet)                  |
| `rush-ui`     | screens: the frame (places, routing, sheets, editing; "Screens"), agent list, session view, the screens under `screens/` | `internal/ui`                              |
| `rush`        | the binary: same commands and flags as Go rush; registers the adapters | `cmd/rush`                                 |
| `rush-ollama` | Ollama's models in Claude Code, Codex, Pi and Vibe: one `Local` per server (what each model reads, models found without loading one, context sizes), tuned per model, a local server started only when needed | `internal/adapters/ollama` |
| `rush-copilot` | Copilot: the CLI run over ACP (rush-acp's `Agent`), its profile, gh's accounts, premium requests, the coding agent's sessions on GitHub and their logs | `internal/adapters/copilot` |
| `rush-providers` | DeepSeek's dsh (run over ACP by rush-acp's `Agent`; its balance), and one provider's models in another's harness (DeepSeek and GLM in Claude Code; Anthropic, OpenAI, DeepSeek and GLM in Pi) | `internal/adapters/deepseek`, `cross` |
| `rush-sys`, `rush-state`, `rush-events`, `rush-gh`, `rush-net`, `rush-keymap`, `rush-update`, `rush-harnessup` | wave A: the machine (processes, file watching), state, the events log, GitHub, the network, keys, updates, agent program updates | `internal/proc`, `fswait`, `state`, … |
| `rush-watch`  | `rush watch pr`: waiting on a PR's checks for an agent (exit 0/1/2/3 as Go), the gh answers every watcher shares (ETag, flock, `watch/gh`), a watch's live record, the hook that turns polling into it | `internal/watch`, `cmd/rush/watch.go` |
| `rush-prdrive`| `rush pr`: what's left before merge in order, trusted reviewers, reply/resolve guards, the detached follower that wakes a session only with news | `internal/prdrive`, `cmd/rush/pr.go`, `prfollow.go` |
| `rush-remind` | reminders agents set: a file each under rush's folder, a detached waiter (`rush remind run`) that sleeps until due and sends it through the session's host, waking it; `when` read as a wait, a time of day or a date | `internal/remind`, `cmd/rush/remind.go` |
| `rush-memory` | rush's side of an agent's memory notes: a note drafted from a few words by a cheap model the caller hands in, which notes are worth a look, those kept as they are (`memory-kept.json`), `rush memory add`'s flags, and `Keeper`, the agent's side of writing and tidying them: rush-claude's `keeper::NoteKeeper`, which the binary registers for the UI (`memreview::register_keeper`) and `rush memory` (in the binary) uses through the adapter's MemoryKeeper | `internal/memory`, `cmd/rush/memory.go` |
| `rush-remote` | `rush serve`: the HTTP server in front of this machine's session hosts (web app, devices, pairing, push, a hub's peers, attach), what puts it in reach (Tailscale, Cloudflare), and its SSH (rush itself per paired key, kept 30 min after a drop): the transport is its own (sshwire.rs: curve25519, ed25519 host key, chacha20-poly1305 and AES-GCM, strict kex, re-keying) on ring, tested against this Mac's OpenSSH on loopback | `internal/remote`, `cmd/rush/serve.go` |
| `rush-testenv` | tests only: every test process in a throwaway home (see "Tests never reach the real home") | — |
| `rush-efficiency` | where tokens go: transcripts read into hourly figures on threads of its own (a Store, its cache Go's), the savers and what they changed, findings, and the advisor's record, digest, briefs and passes (a model's call behind `advisor::Ask`) | `internal/efficiency`, `internal/advisor`, `adapters/claude/efficiency.go` |

`rush pr` and `rush watch` are library entry points (`pr_cmd`,
`watch_cmd`; `*_in` with an `Env` for tests). gh is one trait
(`rush_watch::gh::Gh`, a fake per test thread with `set_thread_gh`), so
nothing in a test asks GitHub. `rush-prdrive/tests/parity` holds 44
commands with fake gh answers and what Go's rush said for each
(`gofixtures/gen.sh` writes them again); the Rust commands must say the
same, end the same, and ask gh the same things in the same order. A
follower tells its session through `rush_prdrive::Host::deliver` (an
`event::Exchange`, as Go's host.Deliver); the binary gives its host once
that's ported. Waits are a `poll()` on a pipe ^C and SIGTERM write to
(`rush_watch::stop`), so a stopped watch ends with 3 as Go's does.

`rush-core`, `rush-hl`, `rush-convo` and `rush-claude`'s parsing have no
terminal code and are tested and benchmarked on their own. `rush-draw`
exposes rush's own types (`rush_core::text::{Line, Span, Style}`, rects, key
events); no ratatui or crossterm type crosses it, so the backend can be
swapped for our own cell grid if measurements ever say the screen matters
(ratatui's cell is 48 bytes, two screens kept: ~1 MB at 200×50).

Later modules, in order: host (running Claude headless, PTYs), the other
adapters (codex, kimi, copilot, …), overview/efficiency/machine screens,
plugins, pr drive/watch, remote, menubar, update.

## Contracts

What flows between the crates, and who owns each type. Wave B (host, live
Claude, fleet, the other adapters) builds on these.

```
 agent's words                      rush's own                      drawn
 ─────────────                      ──────────                      ─────
 Claude transcript line ─┐
   (convo Tail frames it;│ MessageDecoder
    rush_claude::messages)├────────────────→ event::Event ──→ convo::Session ──→ Renderer
 headless stream / other ┘  (adapter)        tool::Call          ::apply(&Event)
 adapters (wave B)                           tool::Output
```

- **Events.** `rush_core::event::Event` is the one stream a session's
  conversation is built from, live or read back. `convo::Session::apply(&Event,
  now)` folds one in (Go's applyNeutral). Only two things reach a Session
  that aren't events, both the host's: `Session::sent(text, images, now)`
  (you sent a message; the agent's own copy of it is matched, not shown
  twice) and `Session::settle(request)` (an approval answered); a
  message rush passed between agents is `Session::sent_exchange(e, now)`,
  and `restore_exchanges(records)` merges rush's archived ones into a
  transcript read back. An `Exchange` is an `Item::Exchange` card in its
  turn (`exchange_text(ref)`, `exchange_peer(ref)` for its ref
  `exchange:dir:id:peer`; the peer's mark is Go's default ◇ until the UI
  hands over agents' logos). `TaskStarted/Updated/Progress/Done` and
  `Background` keep `Session::jobs()` (`Job`, Go's jobs.go: running,
  backgrounded, ended, a shell task with no call of its own given a
  stand-in step); a task that ends or fires puts the turn the agent wakes
  for down to it (`Turn::from`/`cause`/`command`), and a turn's end ends
  the foreground ones. `MessageStart`, `Quota`, `Billing`, `Commands` and
  `Other` are the host's, as in Go. Not yet: the background-work view
  that lists jobs, and linking a subagent's run to the agents it ran.
- **Reading a transcript back.** `rush_core::adapter::MessageDecoder` turns a
  transcript's message lines into events, one per transcript (it remembers
  the calls whose results are still to come). `convo::Tail` takes a
  `Decode = Arc<dyn Fn() -> Box<dyn MessageDecoder> + Send + Sync>`
  (`convo::decode(rush_claude::messages)` in tests; rush-ui builds one from
  the registry's native agent, `Native::messages`) and frames Claude
  Code's transcript format (prompts, notices, compactions, hooks) itself,
  as Go's convo does.
- **Following a transcript.** `convo::Tail::fetch_most(most)` reads at
  most `most` bytes of what's new and parses it off the UI thread into a
  `Fresh` (`more`: there's more written, fetch again now; a line cut by
  the slice's end waits for the next). rush-ui's tail worker fetches
  `TAIL_SLICE` (512 KiB) at a time and sends each on a lane of one, so
  the UI applies a slice per step and a 20 MB flood or a rewritten file
  never holds up typing. `fetch()` still takes everything, for one-off
  reads. A rewrite is noticed by the file's identity as well as its
  size (`rush_core::adapter::rewritten`, device and inode): a transcript
  renamed over by an equal or longer one starts over too. A line longer
  than a slice is kept as read, not copied per slice.
- **Drawing a window.** `convo::Renderer` draws a Session a window at a
  time (Go's RenderWindow, convo/window.go): `count(s, o)` is the rows
  the whole session has as counted now (each turn as last drawn, else at
  an estimate, Go's `estimate`), `window(s, o, from, to)` draws only the
  turns holding those rows and returns `(base, total)`, `rows()` gives
  those turns' rows from `base`; `turn_row(i)` / `turn_at(row)` go
  through a Fenwick index in log time. Drawn turns are counted as drawn
  from then on, so a caller whose rows moved draws again (up to three
  passes, as Go's ui does). At most `KEEP_ROWS` (20,000) rows of drawn
  turns are kept, the longest unused let go first, never those of the
  last window. A Session has a `serial()`, so a renderer never mistakes a
  session started over for the one it drew. `render` (every row, for the
  dumps) and `total_height` (lay out every turn) are still there; `tail`
  is a following view's last rows.
- **The open conversation's view.** rush-ui's `SessionView` keeps its
  `Options` between frames (folds in `o.open`, never cloned per frame)
  and where it is as `Pos::End` (following) or `Pos::At { n, off }`
  (scrolled up: `off` rows into turn number n), so the view stays put
  while rows are added below or counted again above. A frame lays it out
  once (`body`); home is the real first row; the keys read the window
  last drawn. `SessionView::follow(agent, now)` is Go's followTail: on
  each list reading and each slice, a live turn is ended (`TurnEnd`
  "done", its running steps lost) when the agent isn't live, and
  `Session::info` takes the list's cwd, cost and model.
- **Tool calls.** `rush_core::tool::{Call, Kind, Input, Output}` are the only
  tool model. Each adapter reads its own tool names and inputs into them
  (`rush_claude::tools::{kind_of, call, output}` for Claude Code, reading an
  input field by field until one of the wrong type, as Go's decoder does);
  a Call keeps the input as sent in `raw`, an Output the result in `raw`.
  The conversation keeps each step's Call and Output as made, and keeps
  the step in Claude Code's words too (`Step::tool`, `input_json`,
  `result_json`; another agent's call put as the Claude tool of its kind,
  convo's words.rs) because the renderer still reads those, as Go's does.
- **Doing words.** What a call is doing ("running pnpm test") is
  `rush_core::tool::doing`, the one copy. An adapter with words of its own
  for some tools is a `Describer` (Claude's: `rush_claude::tools::doing` /
  `describe`); `Registry::doing(kind, call)` says the adapter's words, else
  tool::doing's. List rows say them that way (Claude's preview through its
  Describer); the conversation has no doing line yet, and says it the same
  way when it does, so the two never differ.
- **Images.** Events carry an image's bytes (`event::ImageData`); the
  conversation keeps only `convo::Picture { media_type, bytes, path }`.
  Prompt images in a transcript are measured from their base64 without
  decoding it.
- **Claude's message types** (`Message`, `Block`, its base64 `Image`) are
  private to rush-claude; `rush_core::message` is gone.
- **Event size.** An `Event` is at most 200 bytes (a test holds it there):
  the rare or big variants (`Exchange`, `Init`, `Approval`, `CallUpdated`,
  each holding a call with its input) are boxed, since events queue per
  session and the host replays them in streams.
- **Running sessions.** `Driver::start(options, sink: Arc<dyn Sink>)`.
  The adapter's own reader thread calls `sink.event(ev)` for each thing
  the session says, in order, and `sink.ended(err)` once, last (Go's
  closed Events channel and its Ender). The caller keeps no thread or
  channel per session: `Tagged { key, tx }` is a Sink that sends
  `Said::Event(key, ev)` / `Said::Ended(key, err)` into one channel every
  session shares, which is the host's queue. `event` and `ended` never
  block: they run on the agent's only reader thread, so a sink that waits
  stalls the agent. A sink feeding the UI uses
  `rush_draw::Sender::try_send` (never waits; a full lane hands the
  message back as `TrySend::Full`) and keeps what it couldn't send on its
  own side, coalesced or buffered; never `send`, which waits while the
  lane is full. `Conn` is `Send + Sync`, so
  the host answers an approval from one thread while another sends; it
  has no `events()` and no `err()` any more.
- **Reading history.** `HistoryReader::history(session, before, most,
  each)` hands events to `each` a slice at a time, each slice read from
  at most `most` bytes of transcript (`SLICE` = 1 MiB by default); `each`
  returning false stops it. `read_history` joins the slices for callers
  that want the whole. `HistoryFollower::follow_history(session)` gives a
  `Follow`: `next(most)` returns a `Slice { events, reset, more }` of
  what's new since the last call (`more`: call again at once; `reset`:
  the file was rewritten, start over), and `wait_on()` names what to wait
  on before calling again: paths (for `fswait::Watcher`), a descriptor,
  or, for a remote log, how often to ask. Nothing in it polls or blocks;
  the caller owns the waiting and stops by not calling. The order is
  fixed, so a write between the last read and the wait is never missed:
  watch what `wait_on()` names, *then* call `next` until `more` is false,
  then wait, and round again (a path replaced is watched anew).
  `rush_sys::fswait::follow(follow, most, stop, each)` is that loop
  (`each` gets each non-empty slice or a failed read; false stops it);
  use it rather than one of your own. A Follow notices a rewrite by
  file identity as well as size (`adapter::rewritten(&mut seen, off,
  &meta)`), so a file renamed over by a longer one resets.
- **Adapter parts.** An adapter implements the part traits it can and
  names them once in its `impl Adapter` with `rush_core::has!(previewer:
  Previewer, discoverer: Discoverer, …)`; the rest stay None. A part
  that is only half there gets a default rather than a stub:
  `RunFollower::subagent_runs` is `Option`, None while an adapter only
  counts its subagents.
- **Who reads an agent.** Only its adapter. The binary registers each
  adapter with `rush_core::adapter::global()` as it starts
  (`rush_claude::Claude::new()`); everything after asks the registry.
  rush-fleet's `Loader::new(registry)` makes the list's rows from each
  adapter's parts: `Discoverer::live/past` (a background job is a Session
  with `job` set and its own state word in `state`), `JobKeeper::workers`
  and `watched`, `TranscriptLocator`/`Transcripts` (where a session's
  transcript is, and where it moved), `SpendReader` (one scanner per
  adapter, kept for the process), `RunFollower::count_subagents`. Each row
  says which adapter found it (`Agent::kind`). `Loader::watched()` is what
  to watch for a new reading (a transcripts folder's subfolders listed
  again only once its mtime moves), with `Loader::rush_watched(&hosted)`
  for rush's own (its state folder, the sessions folder and each
  session's, the shared usage readings: a rush session starting a turn
  wakes the list), and `Loader::changed(paths)` passes
  what the watcher saw change to each adapter's `Discoverer::changed`, so
  what the adapter keeps (Claude's projects listing, otherwise read again
  every 15 s) is read again at once. A pid the daemon's roster names
  counts only if it is the agent's program by name
  (`rush_sys::proc::comm`, Go's isProgramPID); a session file's pid only
  if that process started by the time the session did, so a reused pid
  isn't taken for it. Rows, previews and Spend are `PartialEq`, so a
  reading can be compared with the last. rush-ui reads previews, context windows and
  names (`registry.harness_label(kind)`, never "Claude Code" written in)
  through the registry, and opens any agent's conversation through it
  (`session::Source::of(reg, agent)`): an adapter whose sessions write
  Claude Code's transcripts is read by `convo::Tail` with the native
  agent's decoder (`convo::Decode`, `Arc<dyn Fn() -> Box<dyn
  MessageDecoder>>`), framing prompts, notices and compactions as Go's
  does; any other by its own `HistoryFollower` (`worker::spawn_history`,
  through `fswait::follow`; `Note::History { events, reset }`), else read
  once by its `HistoryReader`. History events carry no times yet, so
  they are applied as of time 0.
  rush-ui and rush-fleet don't depend on rush-claude (only their tests
  do); the binary does, to register it and for `--dump`'s daemon column.
- **Riders start through their harness.** An adapter that runs a
  provider in another's program (Ollama in Claude Code, Codex, Pi,
  Vibe; the cross pairs) holds the registry and starts a session by
  readying StartOptions (env, flags, its own config folder) and handing
  them to the harness adapter's `Driver` from the registry ("rush can't
  run <harness>" while none is registered); its sessions and history
  are the harness's, read the same way and given its own kind.
- **Copilot and dsh run over ACP** as Go's do: each holds a rush-acp
  `Agent` (Go's `cli`) and its Driver hands it the start its
  `acp_options` ready (gh's token and COPILOT_HOME; no mode and
  DSH_HOME).
- **Native** is the transcript half of Go's Native (`messages`,
  `kind_of`, `call`, `output`); the host's half, reading a live session's
  lines (`line`, `message`, `neutral`), is `NativeStream`; rush-claude
  has it (a live line's message is read by the same `neutral::Neutral`
  as a transcript's).
- **Claude Code live** (rush-claude's `headless`, `conn`): `claude -p`
  speaking stream-json. A session has three threads, each blocked until
  there's something to do: the reader (decodes each line and calls the
  conn's handler, then its end), the stdin writer (writes queue, so no
  caller waits on Claude Code) and a stderr tail. A `Reader` dropped
  unread (or with no thread to read on) ends Claude Code and tells the
  session's end, so `stop` and `err` never wait for good. The conn answers what
  is its own (rush's tools' MCP messages and approvals, control replies
  someone waits on) and pushes the rest, as rush's events, into the
  caller's Sink. A control request waited on (Asker, ContextReader) is
  bounded by a minute, as Go's callers bound it. A ~/.claude session
  runs in the home of the login in use (`Claude::run_as`). Not yet:
  sharing the plan usage Claude Code reports with other rushes (its Quota events reach the host, which
  keeps them: keeps_quota is false).
- **Errors that are Gone or Elsewhere** in an agent's own words have it
  as their source: `adapter::caused_by::<Gone>(&*err)` asks.
- **What rush-claude's adapter has today:** Homer, Discoverer, JobKeeper
  (pins and PR links empty for now), Transcripts, TranscriptLocator,
  Previewer, SpendReader, RunFollower (its runs read by subruns.rs),
  Pricer, FastPricer, ContextWindower, Describer, Native, NativeStream,
  HistoryReader, HistoryFollower, Driver, Orphans, Rewinder, SpawnFinder,
  Querier, Joiner, Pinner, SessionsViewer, Dispatcher, Stopper, Remover,
  Replier, Attacher, Loginer, Screener, Mover, Programmer,
  ProjectFolderer, LoginHinter, Made, Keyed, Speaker, Worded; and
  Accounts, QuotaSource, SignInReader, PlanReader (logins kept in rush's
  vault, each with a home linked to ~/.claude: accounts.rs, login.rs,
  home.rs, oauth.rs, usage.rs), Chooser, DefaultModeler, Commander,
  SkillRooter, Brancher, ModelNamer, Attributor, MediaReader, Compacter,
  Publisher, LastQuotaReader, Summarizer, StatsReader, SettingsFiler,
  StatusLiner, StatusLineProfiler, SettingsPager, BuiltinDefNamer,
  Definer, Plugger, MemoryReader, Timeliner, Scratcher.
  `Claude::find_logins`, `refresh_login`, `renew_login`, `use_login`,
  `adopt_login`, `forget_login` are Go's state.LoginKeeper, inherent
  until rush-state has the trait. efficiency.rs reads transcripts into a
  `Figures` trait, which the efficiency module's File will be. Its
  features say Yes to what running it live does and to Commands,
  Plugins, StatusLine, Settings, Stats, Switch, SignIn, Quota and Memory,
  and Planned to the rest Go's says Yes to (Compact, SideQuestion, Dirs,
  HandoffIn).
- **What rush-codex's adapter has:** everything Go's has: Discoverer,
  Driver (its Conn is an Answerer and a TaskStopper; `set_effort` is
  its own, the Conn trait has none), HistoryReader, HistoryFollower,
  TailReader, HistoryTailBeforeReader, QuotaSource, ResetSpender,
  Accounts, Chooser, ModelLister, MediaReader, ChildFinder, Spawnable,
  Programmer, Publisher, LoginHinter, ProjectFolderer, Made, Keyed,
  Speaker; no Previewer, SpendReader or Pricer, as Go's has none. A
  rollout tells a turn twice (response_items and item_completed items);
  Go reads a turn whole to choose, so its follower gives the whole
  history every time. Here a turn's events are given as soon as which
  telling they come from is settled (its first item says), so history
  slices and a Follow give each event once and never take one back. The
  live Conn's reader thread pushes into the sink as it reads; the sink is
  told `ended` once the app-server's output ends. Until `start`
  returns, what is said waits in the Conn, then goes on in order; a
  start that fails tells the sink nothing, not even `ended`.
- **Shared types.** `SubagentStats` is `rush_core::agent`'s (the adapter
  module re-exports it). Processes are read by `rush_sys::proc` (a table
  with `snapshot`, filled only for the trees asked; `Sampler` for one
  tree), files watched by `rush_sys::fswait::Watcher`; no crate keeps its
  own.
- **The fleet** (rush-fleet, Go's internal/fleet whole). A row is
  `rush_core::agent::Agent`, which carries what Go's fleet.Agent does:
  the profile it runs in (`acct`), pinned, put away (`done`), group,
  seen, the job's worker, the main checkout (`root`), the pull requests
  its agent linked (`prs`) and the one GitHub has for its branch (`pr`,
  a `PrStatus`), a rush session's `workspace`, `left`, `decides`,
  `stalled`, its running subagents as `SubagentTile`s, temp work and
  `compaction`. `Loader::load(now)` is Go's Load(true): a `Snapshot` of
  rows, accounts, logins, the machine's processes (`Machine`: the daemon,
  workers, spares, native views, other runs in a terminal, ended
  sessions' leftovers, each with its tree's memory and CPU) and the hosts'
  infos; `load_unsampled` is Load(false), `load_quick` LoadQuick. It reads
  the process table once a load (`proc::snapshot`, one sysctl) and fills
  memory and CPU only for the rows' and machine rows' trees.
  `Loader::new(registry)` reads no state of rush's, lists no rush
  sessions and never asks GitHub (what tests want); `Loader::from_state`
  is rush's: config and overlay from rush-state (names, groups, done,
  seen, hidden), rush's sessions from their hosts' info files
  (`InfoFiles`, read only, until `set_hosts` gives it the host
  module's), the spawn links in the cache folder, GitHub through
  rush-gh's shared `Gh`. Rush's own sessions come through the `Hosts`
  trait (`list`, `config`, `revive`), which the host module implements
  (rush-host's `hosts::Sessions`, which the binary hands the loader);
  `HostInfo` is the part of Go's host.Info the fleet reads. What the UI
  hands in (spend, nudges, plan readings, spawn links, settle, skip past)
  goes through a `Handle`, so it never waits for a load. `watch(every)`
  has the loader watch what it read and reuse its last reading when
  nothing changed (Go's watch.go); `wait` blocks on that watch. Beside
  the loader: `find_worktrees` and each `Worktree`'s `check`, `verdict`
  (the one clean-up rule), `landing`, `settle`, `remove_worktree`,
  `tidy_worktree`, `landed_branches`; `check_folder`; temp work
  (`temp_dirs`, `TempSizes`, `disk_usage`, paced in the background,
  getattrlistbulk on macOS); `look_left`/`remove_left`; the top of /tmp
  (`find_scratch`, `clear_scratch`); `Reaper` for what agents leave
  running; `next_login` and `chain`. Claude's subagent runs are read by
  rush-claude's `subruns::SubagentRuns`, its `RunFollower::subagent_runs`.

Measured (`RUSH_REAL=1 cargo bench -p rush-fleet --bench load` against
`RUSH_REAL=1 go test -bench Real ./internal/fleet`, 2026-10-29, M1 Max,
~760 rows and ~1,200 rush sessions, the machine busy with other agents):
a full load 8.9 ms (Go 9.2 ms); pricing every row's transcripts, none
grown, 6.0 ms (Go 5.7); a load after one watched file changed 8–14 ms (Go
6.3: Go trusts more of its watch, mostly the hosts' folders); a sweep
11.8 ms (Go 10.1). `rush --dump` lists the same rows, accounts and
processes as Go's (other agents' rows come with their adapters).

Claude's spend cache is kept between runs in `costs-rs.json` in rush's
cache folder (version `spend::CACHE_VERSION`), never in Go's `costs.json`,
which Go still writes in its own format. Measured 2026-10-29, ~860 rows:
`rush --dump`'s scan 5.1 s with no cache (every transcript read), 0.14–0.19
s with it (Go about 0.2 s), every row's cost the same either way.

Where the Rust rows' spend differs from Go's on purpose (checked against
the transcripts, 2026-10-29):

- A session's workflow agents (`subagents/workflows/<run>/*.jsonl`) are
  priced with it; Go prices only `subagents/*.jsonl`. Their cost, tokens
  and today's share only, not their folders or pull requests. A session
  that ran 13 workflows: Go $94, Rust $847, its transcripts priced by hand
  $839, Claude's own figure (the host's `costUsd`) $912. The first scan
  after reads them whole (~200 MB there, a few seconds); the cache keeps
  them after.
- Go's `rush --render` prices nothing: Frame asks for the agents'
  transcripts before the first reading of the list, so it scans none,
  and every row of a rush session shows the host's saved `costUsd`
  (an older host summed Claude's running totals: one row reads $506,066
  where its transcript says $0). Its section totals and the background
  card's `out 0 cache 0` come from that. Go's TUI scans after the first
  reading, as `rush --dump` does; compare spend with `--dump`, not
  `--render`.
- Go's `costs.json` keeps totals priced under an older price list (not
  re-priced when the list changed): a transcript reads $3.30 from it,
  $3.66 priced afresh, which Rust's own cache and the host agree on.

Measured (cargo bench -p rush-convo --bench open, a 19.5 MiB transcript,
603 steps): open 35 ms either way; retained heap 2.8 → 3.9 MiB, from keeping
each step's Call and Output once rather than reading Claude's JSON again
on every frame. Go retains about 22 MiB for the same.

## Threads

1. **UI thread**: owns all screen state. Blocks until there's something
   to do: keys/mouse/resize from the terminal reader thread, or results
   from workers. Input waits in a queue of its own and is always taken
   first, all of it, before each worker message, so typing never queues
   behind a flood. Workers' messages are taken for up to 16 ms, then it
   draws once.
2. **Terminal reader thread**: crossterm events into the channel.
3. **Workers**: discovery (sessions/jobs files, process liveness), spend
   scanning (incremental from the last offset), previews, the open
   transcript's tail (its last 2 MB first; a longer one is then read
   whole on the same worker and takes the end's place, its turns'
   refs renumbered so nothing on screen moves: Go's twopass.go). Each
   watches files with `rush_sys::fswait::Watcher`
   (kqueue on macOS; the one file watcher in rush: `watch` before reading,
   `wait` blocks until a change, a `Stop` or a timeout; a path not there
   yet, such as a jobs folder not made, waits on the nearest folder above
   it and is watched once it appears, with no timer) and sends only
   changes. Each worker sends on a lane of its own (`rush_draw::Sender::
   lane(cap)`) holding at most `cap` messages the UI hasn't handled yet; a
   send waits while its lane is full, so a worker can't run ahead of the
   UI (the transcript tail and discovery send on lanes of one: the next
   slice is read while the UI applies the last). A send fails once the
   loop has ended. A thread that must never wait (an agent's reader,
   through its Sink) uses `try_send`, which hands the message back when
   the lane is full. Never send from the UI thread itself. Discovery sends a reading only when it differs from the
   last one sent, and polls (every 2 s) only while an agent is working;
   an idle terminal's own session file, watched, says when it starts
   again. Discovery also reads what each live agent's processes use
   (memory, CPU, how many): the loader lists the process table as Go does
   (one sysctl), which spawn folding and the machine's rows need, and
   fills memory and CPU only for the trees it shows.

4. **netwatch** (rush-net): whether the API can be reached. It checks
   once at start, when the network changes, and when a job fails on what
   looks like the network; on Go's timings (2 min steady, 30 s shaky, 5–30
   s down) only while something needs it: a holder of
   `netwatch::want_checks()` (wave C's screens that show the network hold
   one while drawn), the API down, or a job held for it. Rates are read
   only while a `watch_rates()` guard lives. Idle, it doesn't wake.

## The host

`rush host run <id>` serves one session (rush-host). Its files
(`sessions/<id>/config.json`, `info.json`, `events.jsonl`, `answer.json`,
`report`, `context-start.json`, the views and the inbox) and its socket's
lines are Go's byte for byte (`tests/fixtures`), so a Go UI drives a Rust
host and back. The agent runs through rush_core's Driver; every start's
events go into one queue one thread takes. What Go watches with a ticker
for the host's whole life (idle sleep, away check-ins, a stalled turn,
hung shells, long background tasks) is a timer here, armed only while
there's something to watch: an idle host wakes for nothing. A spawned
agent's wait for its answer waits on its folder rather than looking every
half second. rush-fleet reads hosted sessions through its `Hosts` trait, which
rush-host's `hosts::Sessions` implements over its `Lister` (the fleet
can't depend on the host, which depends on it); it also revives a host
lost mid-turn (Go's Revive).

The TUI reaches hosts through rush-ui's `session_core::host::Hosts`,
which the binary sets before the loop runs (`rush_ui::run_with`, the
binary's `hosted.rs` over rush-host's `Client`; `--render` and tests set
none, or a `Fake`). Setting hosts that can start sessions also hands
Accounts its starters (`session_core::start::install`), so a task typed
in the Agents box, `#new`, `#use`, the sheets and `#ask` start a session
through `Hosts::start` off the UI thread; once the list has it, it's
picked and opened. A session rush hosts with nothing on disk yet opens
as `Source::Hosted` and its host's link fills it. A host that sleeps
lets its socket go, the conversation stays, and a message wakes it
(`Hosts::wake`); a stopped one resumes. A send without a view
(`Hosts::send_to`: a spawned agent from its parent's view) waits for its
echo before letting the connection go. The host keeps a gone client's
unread ops either way: a writer whose write fails shuts its own side only,
and a hello whose read wait can't be set (macOS, on a connection already
closed) is read regardless, as Go's is. The binary's tests
run against rush-host's fake host (its `fake` feature) in the test's own
process. Not yet: the cleanup agent's start (`CareStarter`), switching
a session's setup in place (`Accounts::switcher`), and carrying on sessions a limit
stopped (`limit_stopped`/`relogin`). Watching a spawned agent rush hosts in
the subagents view, the box goes to it (`subview::watched_host`, then
`session_core::spawned::send`).

## A session's runs

The Session follows the runs its session started (session_core's
`subs`, `running`, `workflow`, `subview`): its subagents listed beside its
transcript, a workflow's agents, the agents rush hosted for its shell or
`spawn_agent` calls, and those only a task says. One reader per session
reads them off the UI thread, a read at a time (`Model::later`), and the
frame takes in whole what it brought: the list, each run's numbers
(`RunStat`: what it's doing, its steps, tokens, cost and last words; the
transcripts stay with the reader), each task's output tail, a workflow's
files, and the run shown whole, as a slice of fresh lines its
`SessionView` takes. It reads once a second while something runs, at
once when asked (a run opened, the subagents view shown), and not at all
when nothing does. The dock shows the running ones (or the column beside
a wide conversation, or the top of the views panel while that's open);
the subagents view lists every run, its numbers followed only while it
shows, and opens one under a banner; ↑ from the box picks the dock's runs
and space watches one. The agents rush hosted for a session come from the
binary's `Hosts::spawned` (rush/hosted.rs `hosted_runs`, spawns.go
hostedRuns): the hosts whose meta says `spawnedBy` it, their prompt and
start from the request exchange, their transcript where their agent keeps
it.

## Screens

rush-ui's frame (`src/frame/`) is what every screen hangs off: the
header and its place tabs, the pages row, key/mouse/paste routing, boxes
drawn over the screen, the Agents box and text editing. Screens are added
without editing it: each group of screens owns one module under
`src/screens/` (`session_core`, `session_cards`, `list_extras`, `cmdbar`,
`projects_machine`, `settings`, `accounts`, `plugin_ui`, `social`; turn
yours into a folder with a `mod.rs` if it grows) whose
`pub fn register(x: &mut frame::Ext)` the frame calls once as each Model
is made. Everything below is in `rush_ui::frame`.

- **Places.** `PlaceId::{Agents, Projects, Harnesses, Settings}`, in the
  bar's order. A top-level screen is a `Place` (`frame/places.rs`):
  `name`, `body(&mut self, m: &Model, w, h) -> Vec<Line>` (the rows
  under the header, `w` wide, indented two cells by the frame, which
  shows `h` of them scrolled so a row on `SEL_BG` stays in view: draw
  only those), `key(&mut self, m: &mut Model, k: &Press) -> Out`, and
  optional `mouse` (x, y within the body), `paste`, `hint` (the bottom
  row's keys; a flash replaces it), `pages`/`page`/`set_page` (the pages
  row and `[ ]`/tab), `pages_line` (a pages row drawn your own way, as
  Settings' tabs), `enter`/`leave`, `asking` (a question being typed:
  `, . < >` are text then) and `wake_at`. Register with
  `x.place(PlaceId::Projects, Box::new(MyPlace::default()))`; until then
  a placeholder says the place isn't here yet. Harnesses and Settings
  are both the settings group's. A page of Agents past its list (the
  Wall) is a `Place` too: `x.agents_page(Box::new(Wall))` replaces the
  placeholder of the same name. Navigation is the frame's: `, . < >`
  with nothing typed (and never in an open conversation, where they're
  text), `ctrl+\` always, esc back to Agents (Go's `setView`,
  `placeStep`, `landOnNeeds`), clicks on the tabs. `m.set_place(id)`,
  `m.step_page(d)`, `m.set_page(i)` do it from code.
- **Keys.** `Press { code, mods, s }` (`s` is Go's key name, cmd as
  `super`; `Press::of("ctrl+k")`, `k.text()` the char typed). Handlers
  return `Out`: `Pass` (not mine: the next in line has it), `Idle`,
  `Draw`, `Close` (a sheet closes; a place goes back to Agents), `Quit`.
  The order is Go's `key()`: `x.keys` hooks (every key, first: the
  command bar's ctrl+k, a remap), the key help, ctrl+q, a question, a
  sheet, place moves, then the screen showing: a Place's `key`, else
  with a conversation open `x.session_keys` then its own, else ctrl+c,
  `x.list_keys` then the list's own and the Agents box. To add a key to
  an existing screen push a `fn(&mut Model, &Press) -> Out` onto
  `x.list_keys` / `x.session_keys` / `x.keys`, returning `Pass` for keys
  that aren't yours. `m.press("enter")` presses a key through all of it.
- **Sheets.** A panel over the screen with every key until it closes:
  `trait Sheet: Any` (`frame/sheet.rs`) with `body(&mut self, m: &Model,
  w, h)`, `key(&mut self, m: &mut Model, k) -> Out` (`Close` closes;
  opening another sheet replaces it), optional `width` (0 is 112, never
  past the screen less 6), `mouse` (`Pass` leaves clicks to the frame,
  which presses the keys a click stands for: a row with `▍` is ↑/↓ then
  enter, a tab drawn by `sheet_tabs` is `]`, a key in a `keys(...)` row
  is that key, outside the box esc), `paste`, `wake_at`. Open with
  `m.open_sheet(Box::new(s))`; reach the open one with
  `m.sheet_as::<MySheet>()`. Pieces: `sheet_title`, `sheet_tabs`,
  `sheet_row`, `window`, `text_field`, `center`; `dialog::edged_box`,
  `paste_at`, `fade`, `Model::modal_over` for boxes of your own. Ready
  made: `Picker` (rows under headings, typing that narrows them,
  `Pick`, `on_pick`, `on_query` to look for more off the UI thread and
  set them through `m.picker()`), `ChoiceSheet` (/model's list, the one
  running marked). A question is `m.ask(Confirm::new(q, |m| …))` with
  `detail`, `on_no`/`no_text`, `on_bang`, `more` keys and `again`.
- **Text.** `Editor` (text, cursor, selection, undo; `key(&Press)` runs
  Go's editing keys, chips whole) and `InputBox` (the bordered box with
  labels, wrapped text, cursor, `at`/`near` for clicks). `m.input` is
  the Agents box's Editor; set `x.agents_box = true` and draw it with a
  `x.list_dock` hook, and typing in the list goes into it (the list's
  own j k g G h l q are text then, as in Go); enter runs a `#command`
  (`x.command(name, about, fn(&mut Model, &str) -> Out)`) or hands the
  text to `x.submit` hooks.
- **Rows and cards on existing screens.** `x.card_cells` (one more cell
  in the Agents dock card's footer, `fn(&Model, &Agent) -> Option<Line>`),
  `x.top_bar` (the header's right, rows 0–3: a `TopBar` of fns, the
  lines given each row's room, the gauges while the layout is the
  default, whether usage shows, what had no room, the orphans count;
  `Model::head_rows(w)` draws the header's own rows from `&Model`, for
  /statusline's preview), `x.list_dock` and
  `x.session_dock` (rows under the list or the conversation; the body
  gives them up: the Agents box, a Session's message box and cards),
  `x.session_body` (the open Session's body in place of the
  conversation, `w` wide and `rows` tall: its other views, and the
  conversation with its map; `SessionView::side` narrows the
  conversation for what's beside it), `x.session_head` (over the
  Session's five header rows once drawn: its tabs and the rule under
  them),
  `x.list_foot` (rows at the list's own foot, each with a key for the
  pointer and clicks; the first that has any is drawn: Getting started
  while the list keeps 8 rows, else the feed's dock), `x.start_tag` (the
  header's second row, what new sessions start on),
  `x.overlays` (drawn over the finished frame, under sheets and
  questions: the quick ask, the command bar), `x.on_fleet` (after each
  reading of the list), `x.mouse`, `x.paste`.
- **What rush says** (`frame/messages.rs`, Go's messages.go). An
  answer to a key is `m.flash(s)`, a refusal `m.flash_err(s)` (plain
  too, the way forward in its words), both on the line under the box
  for six seconds; `m.fail(s)` is for what broke: red, until the next
  key. What rush did on its own is `m.news(s, ref)`: never that line,
  but a count in the header, rows under the feed at the foot of the
  list, and `m.side_news` for the column beside the conversation, each
  a click to `ref` (`projects:<folder>` opens Projects there), until
  seen; `m.noted(s)` only logs it. Everything said is kept, the last
  50, and `#log` opens it newest first.
- **What the list reads besides the fleet** (`screens/list_extras`).
  With a loop running, after a fleet reading and off the UI thread: git
  about the projects it shows, every 15 s (`landing.rs`: what's left
  before a worktree's work lands, the To land section, alt+p); and
  turns.json, then one finished turn at a time read by the cheap model
  (`session_core/turnwants.rs`: what a turn wants of you puts it under
  Needs you, and ends its session's last answer with a Needs-you
  section).
  A model drawn once is handed both through `m.kept`.
- **State.** A group keeps its own state as a type of its own:
  `m.kept::<MyState>()` (made on first use) — no field on Model.
- **Work off the UI thread.** `m.later(work, apply)` runs `work` on a
  thread of its own and `apply(&mut Model, result) -> Out` back on the
  UI thread (Go's `later`/`sheetDo`); with no loop (tests, `--render`)
  both run at once, so tests never sleep. A worker of your own takes
  `m.sender()`, gives itself a lane (`tx.lane(1)`) and sends
  `Note::Apply(Box::new(|m| …))`; never send from the UI thread. Work
  from the background draws at most every 250 ms (`framehold.rs`); keys
  and the mouse draw at once. Ask for a wake-up with
  `m.ask_wake(instant)` or a Place's/Sheet's `wake_at`, only while
  something on screen moves.
- **Keys and the command bar** (`screens/cmdbar`). Its `x.keys` hooks
  run first, in Go's order: every key goes through the keymap in force
  (`keybind::remap`: rush-keymap's Map, keybindings.json read off the UI
  thread once a loop runs; a moved key is pressed again as the default it
  stands for, a chord waits two seconds, a bound # command runs), then
  the keys guide (F1 anywhere, ? in the list) and the bar (ctrl+k, cmd+k,
  ctrl+f). Other screens read the map with `keybind::key_map(m)` and
  `bound_key(m, id)`; Settings, Keys takes the action the guide's enter
  picked from `m.kept::<keyhelp::ShowKey>()`. The bar searches
  transcripts after a 200 ms pause on threads of its own, in batches on
  a lane of one; with no loop it searches only on ctrl+j, as one Go
  frame never reaches the pause. Each list reading indexes the
  conversations in the background (`config::dir()/search`).

- **Settings.** `screens/settings` is both settings places: Harnesses
  (Providers, Harnesses, Config) and Settings (General, Keys, Plugins,
  Updates, Remote), one dialog between them as in Go, its tabs drawn
  through `Place::pages_line`; its `register` makes the bundled plugins
  known (`rush_bundled::register`, Go's init). Pages read and write rush's config through
  a rush-state `Store` kept in the group's state (`settings::use_store`
  puts one in place, for tests), saved after each change; what's slow
  (models, definitions, an agent's settings file, plugins, versions,
  serve's devices) is read through `m.later` as a page opens or keys
  come, never while drawing. What other modules own reaches Settings
  through a `provide(…)` each, called once from their `register`:
  `settings::acct::provide(Accounts)` (account rows, who's signed in,
  limits' problems, switching, adding, reading limits),
  `settings::plugins::provide(Broker)` (the plugins running, changing
  one's setting) and `settings::statusline::provide(Header)` (the top
  bar's and agent header's segments, as bars.go draws them; the bars
  group's). Until then each has a stand-in (kept logins with no
  readings; nothing running). The sheets that change a session's
  settings open from code: `sheets::open_permissions`, `open_hooks`,
  `open_skills`, `statusline::open_status_line`; `#network` and
  `#statusline` are registered commands; `sheets::start_view` is the
  first-run layout question.

- **Plugins on screen** (`screens/plugin_ui`). Everything plugins add
  goes through `rush_plugind::hooks::Client`, kept in `m.plugins()`
  (`Plugins`) with a `Disk` for what's read and changed on disk
  (approvals, bundled plugins, offers, onboarding, the layout); nothing
  touches the broker or the disk until the binary calls
  `plugin_ui::start(m, Client::new())`, so a test or `--render` model
  never does, and tests swap in a fake Disk. A sheet that hands work off
  the UI thread uses `plugin_ui::sheet_do(m, work, done)` and, right
  after, `settled(self, m, done)`: done runs with the sheet itself,
  whether it's back in the model (a loop) or still running its key (no
  loop). `#plugin.command` goes through `x.hash` (frame/ext.rs), the
  frame's one addition for this group. The file editor
  (`plugin_ui::doc::DocEditor`) is a component for the memory and
  settings screens to embed. The views panel (`plugin_ui::views`) sits
  between the conversation's text and the map (`beside::views_beside`,
  `beside::paste`, called from session_core's conversation and side
  column), and has the pointer first (`beside::mouse`, pushed before the
  Session's own mouse hooks). Nothing reads view files on the UI thread:
  `read::follow` keeps one thread waiting (kqueue) on the open session's
  views folder, reading only when it changes, and the list's summaries
  are read after each fleet reading, only for folders whose time
  changed; host calls a click makes (drop, bring back, change a row)
  go through `Model::later`. rush's rows over the view
  (`ViewsState::live`) are the session's shells, monitors and tasks
  (`subs::panel_subs`; the subagents wait on runs being read), and
  `quiet` (`quiet_why`) is worked out only when the view or the turns
  change; folded
  beside the column, the views are its last part (`side_views`). Its sheets, the editor, the panel and
  the artifacts view are checked against Go row by row, words and
  colours, at the widths 80, 120 and 200 column screens give them
  (`testdata/wide`), and the editor's JSON problems against jsontext's.
  Not yet: a link's text clickable in the terminal (Go's OSC 8 `link`),
  as a rush-core Span has no link.
- **The bars** (`screens/bars`, bars.go, bars_quiet.go, usagebar.go,
  header_context.go). The top bar is the frame's `top_bar`: its lines
  down the header's right a row each (`top::top_rights`; the plan alone
  on a line takes the spare rows for each provider's queue of
  accounts), or the gauges while it's the default. The agent header
  goes over the Session's rows one and two through `session_head`,
  registered after session_core's: it draws the name's row again from
  `session::header` (the name cut to leave the low note room), then the
  bar's two lines, what the agent runs as and how it's reached, and
  keeps where the context and the permission mode landed; a click on
  the mode is `session_cards::permissions::permission_command`. The
  layout is the one being built in /statusline while its tab shows,
  else bars.json, read off the UI thread with the first list reading;
  what a line left out is kept by `which*10+line` for /statusline.
  Account readings: logins' through the logins agent's `PlanReader`
  (`accounts::state::readings`), others' from quotas.json; `fetch.rs`
  reads both again every `usage::EVERY` with a loop running. Not yet:
  the other logins' own refresh (fetchLoginUsage), a kept sign-in's of
  an agent reading any account's, a session's billing and its host's
  rush version (no Info for them), the name's @handle, headerGit, the
  hover on the label and its click to switch.
- **Gaps.** Colours are still the dark ground's constants: `ground.rs`
  (photon's theme), `headerwash.rs` and `scheme.rs` (the terminal's
  colour and mode 2031 reports, parsed) are ported but the reports
  aren't asked for, as rush-draw hands the UI keys, not raw replies.
  Handing the terminal to `$EDITOR` (Go's `editorCmd`) waits on
  rush-draw pausing its input reader. The UI stall watch times each
  message and frame after the fact (no watchdog thread, no stack).
- **The list and the Session side by side.** `list.rs` groups each
  section by project (Settings' split by project) and draws the rail look
  or the classic; `list_extras/split.rs` splits a screen 170 columns or
  wider between the list and the picked agent's Session, which is read
  into `Model::pane` (its own `SessionView`, notes routed by generation)
  so the list keeps the keys; enter opens the one already read. The
  frame draws the list, its box and keys at the list's width, then joins
  the pane's rows beside them. Reading the pane starts only when the
  pick moves to another agent.
- **Pictures.** `screens/session_cards/pictures.rs` draws an image fitted
  to a box of cells in quarter blocks (Go's convo/picture.go and thumb.go
  where Kitty isn't there), each image and size made once on a thread of
  its own, two at a time, the frame after it's made having it; the image
  crate decodes PNG, JPEG, GIF and WebP, as Go does. Kitty pictures and
  asking the terminal its cell size (Go's pictures.go) wait on rush-draw
  writing raw output; until then the cell is taken as Go's 10×20.

## Panics

Release builds unwind (`panic = "unwind"`), so one thread's panic ends
that thread, not rush: the host contains a panic in one session's thread
and the rest carry on. Threads whose work must outlive a panic catch it
(`catch_unwind` in rush-state's write-behind saves and rush-gh's PR
reads). The terminal is given back only when the UI thread panics; a
worker's panic is kept and printed once the terminal is given back
(rush-draw's term.rs), so it never scribbles over the screen.

## Menu bar

`rush-menubar` is Go's internal/menubar. The app is rush itself drawn
with AppKit through objc2 (Go's is Swift, compiled on the Mac): the
bundle at `<rush folder>/menubar/rush.app` holds a copy of the rush binary
as its program, so building it needs no compiler. The binary's main asks
`rush_menubar::bundled()` first and, when true, runs `app_main()`;
`rush menubar [off|feed]` is `rush_menubar::cli`. The app and its feed
speak Go's lines (`wire`), and the feed answers sessions through their
host's socket as Go's does, so either rush's app runs either rush's feed.
The feed waits on files (fswait), the app's ops and limits' readings
instead of Go's two-second tick: it reads again every two seconds only
while an agent works, asks for limits once a minute only while one is
live, and reads at most every two seconds however many files change.
The app's only timer is the icon's, while an agent works. It reads what
it can of the fleet today (`source::FleetSource`); hosted sessions, the
logins' own refresh and the fleet's accounts come with those modules.

## Open files

The binary raises its soft limit on open files to the most allowed
(`kern.maxfilesperproc` on macOS) at startup, as Go's runtime does:
launchd starts programs at 256, and each watched folder and transcript
is a descriptor.

## Tests never reach the real home

Every crate's tests link `rush-testenv` (`rush_testenv::sandboxed!()` at
the crate root and atop each integration test). Its constructor runs
before any test and moves the whole process, every thread and child
included, into `/tmp/rush-test-<pid>-N`: HOME, RUSH_HOME, RUSH_CACHE,
XDG_*, CLAUDE_CONFIG_DIR, CODEX_HOME and TMPDIR inside it; PATH an empty
bin folder and `/usr/bin:/bin:/usr/sbin:/sbin`; every variable naming a
live session, host, socket or plugin dropped. `rush_core::sandbox` arms
the tripwires: in a test, rush's folder, its cache, the events log or a
host's socket resolving under the real home panics; agent programs
aren't looked for in `/opt/homebrew/bin`; `rush_sys::proc::kill` and
`kill_group` refuse anything the test didn't start. pid 1 and below
(init, -1 = every process, 0 = this group) are refused always, test or
not. A thread's own rush folder (`set_thread_dir`) still keeps tests
apart from each other; the sandbox keeps them all from the user's.

To check, run the tests under `sandbox-exec` with a profile that denies
writes to the real `~/.config`, `~/.claude`, `~/.codex`,
`~/Library/Caches/{agtop,rush}`, unix-socket connects there, exec of the
user's claude, codex and rush, and signals outside the sandbox: any
denial shows in `log show --predicate 'sender == "Sandbox"'`. Under it,
four tests fail for the sandbox's own reasons (it won't run setuid `ps`
nor set a setuid bit): rush-sys `script::tests::breakpoint` and
`signal_tree_pauses_and_resumes`, rush-gate `watch_pauses_the_newest`,
rush-plugin `digest_is_gos`. Run rush-sys's process tests and rush-gate's
watch test outside it. `end_tree_returns_once_the_tree_has_gone` passes
in it.

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
- **Claude Code: its engine's options, not its code.** A
  `BUN_OPTIONS=--preload` runs inside Claude Code; with it, JSC's sampling
  profiler and a heap snapshot show two thirds of its heap is its own code
  (25 MB of modules, 32 MB of bytecode), the conversation a few MB. CPU is
  a few seconds a busy turn, a fifth of it the file-read safety check
  (symlinks followed with lstat/realpath), so nothing to win there.
  `BUN_JSC_useDFGJIT=0` plus a `Bun.gc(true)` a minute from the preload:
  idle after a busy turn 103 MB every time, against 108–126, same CPU
  (Go: `internal/adapters/claude/lean.go`). The preload deletes both
  variables, so what Claude Code starts doesn't inherit them. No effect:
  `--smol`, `MallocSpaceEfficient`, `forceRAMSize`, FTL alone off;
  discarding old bytecode saves ~5 MB more for twice the CPU; the JIT
  entirely off won't start. A network proxy (`ANTHROPIC_BASE_URL`) gains
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
