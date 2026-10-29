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
| `rush-fleet`  | the list's rows from every adapter in the registry; what a row's state means | `internal/fleet`                           |
| `rush-host`   | the host: the process a rush-mode session lives in (`rush host run <id>`), its socket protocol, the UI's client to it, its per-agent watching (sleep, away, stalls, hung shells, long tasks, answers spawned agents owe), and spawn_agent's agents | `internal/host`, cmd/rush's host commands |
| `rush-draw`   | the thin drawing layer; the only crate that knows ratatui     | (bubbletea / ultraviolet)                  |
| `rush-ui`     | screens: agent list, session view; event loop                 | `internal/ui`                              |
| `rush`        | the binary: same commands and flags as Go rush; registers the adapters | `cmd/rush`                                 |
| `rush-sys`, `rush-state`, `rush-events`, `rush-gh`, `rush-net`, `rush-keymap`, `rush-update`, `rush-harnessup` | wave A: the machine (processes, file watching), state, the events log, GitHub, the network, keys, updates, agent program updates | `internal/proc`, `fswait`, `state`, … |

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
   transcript's tail. Each watches files with `rush_sys::fswait::Watcher`
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

## Panics

Release builds unwind (`panic = "unwind"`), so one thread's panic ends
that thread, not rush: the host contains a panic in one session's thread
and the rest carry on. Threads whose work must outlive a panic catch it
(`catch_unwind` in rush-state's write-behind saves and rush-gh's PR
reads). The terminal is given back only when the UI thread panics; a
worker's panic is kept and printed once the terminal is given back
(rush-draw's term.rs), so it never scribbles over the screen.

## Open files

The binary raises its soft limit on open files to the most allowed
(`kern.maxfilesperproc` on macOS) at startup, as Go's runtime does:
launchd starts programs at 256, and each watched folder and transcript
is a descriptor.

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
