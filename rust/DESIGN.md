# rush in Rust — design

The Go rush (`cmd/`, `internal/` at tag `go-final`) rewritten in Rust, module by module.
The Go version keeps shipping until this one does everything it does. What
matters is taken from decline (`0xdeafcafe/terminal-decline`): idle costs
nothing, memory is small and measured, floods never block typing, and every
speed or memory claim has a number behind it.

## Goals

- **Idle costs nothing.** No ticks, redraws or wakeups while nothing
  changes. The main loop blocks on one channel; background threads (file
  watching, process table, spend scanning) send it only real changes. The
  one drawing timer is the spinner of a live agent, and only while one is
  on screen and the terminal has focus. Go's chores on its one-second tick
  (watchBinary every 30 s; in the rush that leads, settleLanded every
  minute and tidyDone every 10) wake the loop only at the second one is
  due, hand their work to a thread, and draw nothing (rush-ui chores.rs).
  What Go redraws every second for the clock's sake is drawn when its
  text changes: a label that moves with the clock (`style::age_since`,
  `dur_since`, `dur_left`, plan's `roughly_left` and `note_resets`) notes
  when it next turns, and the frame asks to be woken then; the list is
  laid out again when an agent's group runs out of time (rebuild's
  `group_turn`). Screens past the list (a Session, a place, a sheet) are
  drawn again with the chores' wake, every 30 seconds, as before. The
  battery and disk are read at the same wake while shown (IOKit, not
  pmset) and drawn only when they'd show differently; the hosts' sweep
  goes over the list's own listing, which its watcher keeps fresh.
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
  `../docs/performance-review-2026-10-01.md`). A frame whose rows are
  those on screen is neither painted nor written (rush-draw
  `Canvas::keep`), and what several parts of one frame each ask (the
  accounts, which read folders on disk) is asked once a frame
  (rush-ui `frame/framememo.rs`).
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
| `rush-ui-base` | what every screen builds on: the model, the frame (places, routing, sheets, editing, the `Ext` hooks and slots the screens fill; "Screens"), the agent list, config, the workers, and the data the screens share (a session's connection as every screen reads it, `sess::ConnCore`; the hosts; the accounts' state and rush's store; the keymap in force; where new sessions start) | `internal/ui` |
| `rush-ui-social` | the social group's screens: the feed and its graph, rooms, broadcasts, updates and reloading; depends on rush-ui-base only (what it asks of other screens goes through `Ext::calls` and `Act`); its tests that need the whole UI are rush-ui's `screens_tests/social` | `internal/ui` |
| `rush-ui`     | the screens under `screens/`, registered into a model (`new_model`); the entry points (`run`, `render`, `soak`); re-exports rush-ui-base's modules at their old paths | `internal/ui`                              |
| `rush-ui-projects-machine` | the Projects place and the machine: projects, worktrees, folders, processes, clean-up, memory review and the machine's own care; depends on rush-ui-base only (slots and `Act` for the rest); its tests that need the whole UI are rush-ui's `screens_tests/projects_machine` | `internal/ui/projects*.go`, `machinecare.go`, `cleanup.go` |
| `rush-ui-settings` | the Harnesses and Settings places: every settings page, the form they share and the sheets opened from them; reaches no other screen (slots and `Act` in rush-ui-base) | `internal/ui/settings*.go` |
| `rush-ui-plugin-ui` | the plugin screens: plugin sheets, approvals, picks, the layout question, an agent's views beside its conversation, artifacts and the file editor; depends on rush-ui-base only (what it asks of session_core goes through `Ext::calls`, `Act` and its `PanelHost`, which session_core fills); its tests that need the whole UI are rush-ui's `screens_tests/plugin_ui` | `internal/ui` |
| `rush-ui-accounts` | the account screens: each provider's look, accounts and their limits, what the next session starts as (#use, the New session and Start sheets), signing in, Getting started, #ask, the info sheet's tabs; depends on rush-ui-base and, by rank (settings < accounts, debt), rush-ui-settings; what it asks of other screens goes through `Ext::calls` and `Act`; its tests are rush-ui's `screens_tests/accounts` | `internal/ui` |
| `rush-ui-session-cards` | the cards and sheets on a session: approvals, Claude's questions, limit and sign-in cards, /rewind, /fork, /compact, #slim, the message sheet, the quick ask, /btw, rooms on a session, handing over, pastes and pictures; depends on rush-ui-base and, by rank, plugin_ui, settings, accounts, projects_machine and social; reaches session_core only through `Ext::calls`, `Act` and its HOST fn table (`link`); its tests are rush-ui's `screens_tests/session_cards` | `internal/ui` |
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
(Go rush at `go-final` wrote them); the Rust commands must say the
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
  `Other` are the host's, as in Go. The background view lists the jobs
  (see "A session's runs"). A step that ran an agent is drawn as that
  agent's row: a spawn_agent call from its input, a shell command once
  the UI hands the renderer the agent it found (`Options::ran`, Go's
  SetChildren with one child). `Options::kids` are the agents a step
  ran with their own sessions (`render::Child`): one found draws its
  steps under its row while it works or once opened, and what it said
  back as its body; several are a row each under the step (Go's fan). An
  opened spawn_agent row names the agent alone, its task under it
  (spawnName, brief). `Options::kind` is the session's own harness
  (session.rs, Go's kindOf). `Session::watch_shells` and `running_part`
  (chainrun.go) say which command of a running chain runs now;
  `Session::writes` the files a command sends its output to.
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
  is a following view's last rows. A link's lookup landing
  (`linkinfo::lookups_gen`) makes the next window draw again; counting
  keeps the rows last drawn, so what a frame drew and what a click
  reads are the same rows. A Bash call rush ran as a script
  (`Options::scripts`, a `ScriptView` by call id) is drawn as the file
  it ran from: the line it's on and for how long, each line's time, its
  breakpoints and the held marker (render/scriptdoc.rs).
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
- **Links.** A run of text that links somewhere carries it in its
  style: `Style::link` is a `text::Link`, a number for its URL kept once
  per process, so a Style stays Copy and 16 bytes and cutting, wrapping
  or recolouring a row keeps its links (Go keeps OSC 8 escapes in its
  rows). The conversation links markdown links, bare URLs, image chips
  and the files steps name (`Style::linked`); `text::link_at(line, col)`
  reads one back for a click. rush-draw keeps each cell's link beside
  it and writes OSC 8 around a linked run (term.rs diffs and writes the
  frames itself, so a cell whose link alone changed is written again),
  so the terminal opens it on cmd+click; as Go's renderer, not where its
  colour profile would be NoTTY (off a terminal, TERM unset or dumb,
  unless CLICOLOR_FORCE). `Headless::ansi` (`--render`) writes them too,
  as Go prints its view's escapes.
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
- **A hosted session's conversation.** Once rush-ui's link reaches a
  host, the host's info and config are read again (session_core's
  `live::pre_for`, Go's preFor) and the conversation starts from the
  transcript up to where the host's replay begins: the last
  `PRE_TAIL` (2 MiB) of it first, Partial, then the whole of it off the
  UI thread with the host's lines that came meanwhile (`Conn::since`)
  put on top, taking the end's place through `SessionView::take_whole`.
  The host's info gives the conversation how it began
  (`convo::Info::{id, kind, effort, started_at, started_by, spawned_by,
  started_from}`) and its commands event `Session::commands`, which the
  first turn's opening card draws (render/opening.rs, in the turn's key).
- **A host's lines in a burst.** A session streaming big tool results
  makes tens of thousands of lines a second. Each end of a host's socket
  asks for 4 MiB (rush-host `sys::widen`; macOS gives 8 KiB, so the
  host's writer and the client took turns every 8 KiB), the client reads
  256 KiB at a time, and the host lets a client go only once 32 MiB
  (`CLIENT_BEHIND`, four rings) wait for it, never waiting on it itself
  (Go: 4096 lines, passed in a tenth of a second, so the list's preview
  was let go, replayed and let go again through a burst). The UI's
  follower (session_core `live::follow`) never waits for the UI: what it
  reads while the UI is busy gathers (`Gathered`), and one message takes
  all of it, at most `TAKE_MAX` lines a turn. A host's writer passes its
  own lines (info above all, sent on every step) on without taking them
  apart (`lines::is_host_line`). Live bench, a session flooding (`flood
  15000`, ~90,000 lines in 2-4 s) with the list's preview on it: let go
  and dialled again 4-5 times a burst before, so 7-66 thousand of the
  lines came and the rest as replays; none after, all 90,000 taken.
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
  through `fswait::follow`; `Note::History { events, reset, more }`),
  else read once by its `HistoryReader`, each slice sent once the next
  is read so the last says it's the last. History events carry no times
  yet, so they are applied as of time 0. `SessionView::caught_up` is
  whether the reading has reached what's written: a transcript's turn
  is ended for a stopped agent only then (between two slices it's still
  being read), and an adapter's history's never by the list (Go's
  followTail needs a tail). A reset keeps the info the host saved.
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
  `adopt_login`, `forget_login` are Go's state.LoginKeeper, and
  `Claude` is rush-state's `vault::LoginKeeper` through them (the binary
  registers it with rush-ui's `accounts::logins::register_keeper`).
  Unlike Go, a refresh token Anthropic called revoked isn't sent again
  by that process (only a new sign-in is tried); a reading whose
  sign-in can't be refreshed says why (revoked, or in use by a running
  Claude Code) until the next try, not "sign-in expired"; and a failed
  reading's reason is kept beside the last one in usage.json, so the
  rows say it rather than only "not read since" (oauth.rs, usage.rs).
  efficiency.rs reads transcripts into a
  `Figures` trait, which the efficiency module's File will be. Its
  features are Go's (claude.go:62): Compact and btw through the host's
  control requests, cd and add-dir through its cd op, a hand-off in
  through a history carried or told.
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
  priced with it, as Go's main has since 2c6ffb47 (before, Go priced only
  `subagents/*.jsonl`). Their cost, tokens and today's share only, not
  their folders or pull requests. A session that ran 13 workflows: Go
  before $94, Rust $847, its transcripts priced by hand $839, Claude's own
  figure (the host's `costUsd`) $912. The first scan after reads them
  whole (~200 MB there, a few seconds); the cache keeps them after.
  Checked again 2026-10-29 against Go built from main: `--dump` with each
  row's today added, all 768 rows' cost and today the same (but the two
  live sessions', which grew between the runs, and one of Go's stale
  `costs.json` totals, below); account today $1,489.65 Go, $1,489.81 Rust.
  Go's Frame patched to scan after its first reading draws clanker's coin,
  `Working 2 · $1,643`, `Earlier 825 · $18,357` and the card's
  `spent $1,062` as Rust does, at 80, 120 and 200 columns.
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
2. **Terminal reader thread**: crossterm events into the channel. It
   waits in one `poll()` on the terminal, a pipe SIGWINCH writes to and
   a wake pipe, and asks crossterm only once there's something to read,
   so it can stop reading while another program has the terminal
   (`rush_draw::App::outside`, Go's tea.ExecProcess): the loop pauses
   it, gives the terminal back (alternate screen, raw mode, mouse,
   paste, focus and kitty keyboard modes left; ^C and ^\ the program's),
   runs the program on the UI thread, takes it all back, reads the size
   again and draws whole. rush-ui's side is `Model::exec(cmd, done)`
   (`frame/outside.rs`): $EDITOR through `memedit::edit_then` (the
   memory view's and review's notes, settings' files, ctrl+g's draft)
   and a sign-in sent to the terminal. With no loop (tests) a program
   waits for `Model::run_outside`.
   The kitty keyboard is asked for flags 1 and 2 (`\x1b[>3u`): keys
   disambiguated, and their event types, so a held key's repeats come as
   `Event::KeyRepeat` (handled as a press everywhere) and its letting go
   as `Event::KeyUp` (ignored everywhere but push-to-talk,
   rush-ui-social's `dictation.rs`, which the model reaches through
   `Calls::dictation_released`). `Term::reports_releases` says whether
   the terminal answered the kitty query; without it ctrl+space toggles.
   Both dictations are told the words on screen (session, repo, branch
   and file names, identifiers in the visible conversation, the open
   session's first, at most 200: rush-ui-social's `screenwords.rs`): yap as
   a `{"context":{"app":…,"words":[…]}}` line on its socket, re-read after
   a frame at most once a second and only while yap is followed; rush's
   helper in `Command::Start.context`, the first 100 set as the speech
   request's contextualStrings.
3. **Workers**: discovery (sessions/jobs files, process liveness), spend
   scanning (incremental from the last offset), previews, and the open
   conversation (`worker::spawn_read`, one flow for every agent, Rust-only
   in sharing it: Go's twopass.go and warmed, rushmode.go:2064). It reads
   through `rush_convo::reading::Reading`: its last 2 MB first (`end`),
   shown at once with "reading earlier…" above it; then the whole from its
   start on the same worker, taking the end's place with its turns' refs
   renumbered so nothing on screen moves, its last 400 rows laid out as
   the view last drew and its edits worked out, so the frame that swaps
   it in draws little; then what's added as it grows
   (`fswait::follow_grows`). A transcript in the form Claude Code writes
   (`reading::Transcript`, every adapter whose `claude_transcripts` says
   so) and another agent's history as its adapter reads it back
   (`reading::History`: its `TailReader` for the end, its follower or
   reader for the whole) are the two forms read; an adapter that can't
   read its end alone (Gemini, whose log rewinds; Copilot's remote log)
   has its whole read first, off the UI thread, the same way. The
   read-ahead (predict.rs) reads the same end and keeps only it. Every
   end lands on a line's start found from the file's end backwards
   (`rush_core::tailread`: whole lines only, a last line still being
   written left out, the last line kept however long); every adapter's
   `TailReader` is held to its own whole by `adapter::conform`. A 50 MB
   Codex rollout (ctrl+k, enter): its end on screen 287-626 ms after
   enter -> already there (its end read ahead in ~40 ms), memory with it
   open 133-138 MB -> 77-81 MB, read ahead 85-115 MB -> 35-53
   MB. Each
   watches files with `rush_sys::fswait::Watcher`
   (kqueue on macOS; the one file watcher in rush: `watch` before reading,
   `wait` blocks until a change, a `Stop` or a timeout; a path not there
   yet, such as a jobs folder not made, waits on the nearest folder above
   it and is watched once it appears, with no timer) and sends only
   changes. Discovery's (and the menu bar's) is a `Watcher::many`: where
   eight or more watched paths share a folder (rush's sessions folder,
   Claude's projects), one FSEvents stream on those entries serves the
   quiet ones, instead of a kqueue descriptor each. One that appeared
   since (a session just made) or was written in deep inside (a live
   session's events, a transcript) within the hour keeps or gets a
   descriptor and is told at once; writes inside it then cost nothing,
   as the stream is made again without it. The stream's news is checked
   against each entry's status, so it tells what kqueue would (entries
   added, removed, renamed; a file written), 10-40 ms after the change
   (a few hundred under heavy load; Go's list shows it at its next
   one-second tick). A path moving onto the stream keeps its descriptor
   until the next ask has read it, so nothing between is missed. Each
   worker sends on a lane of its own (`rush_draw::Sender::lane(cap)`)
   holding at most `cap` messages the UI hasn't handled yet; a send
   waits while its lane is full, so a worker can't run ahead of the UI
   (the transcript tail and discovery send on lanes of one: the next
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
   `netwatch::want_checks()` (the TUI holds one for its whole life, as
   Go's watchNet starts at Init: the top bar's "net steady / net shaky /
   offline" needs it; a session's host and `--render` hold none), the API
   down, or a job held for it. Rates are read only while a
   `watch_rates()` guard lives (#network). A host's netwatch, unwanted,
   doesn't wake.

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
lost mid-turn (Go's Revive), unless the rush follows (below). Beyond Go,
a host lost between turns, awake (it never said it stopped and didn't go
to sleep: killed, or taken down with the app rush ran in), is revived
too, told nothing, as a reload's restart leaves it; Go leaves it stopped
until someone notices. Not into a folder that's gone, and at most once
every ten minutes, as a mid-turn one.

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
process. Every stop of an agent (#stop, #rm, #kill, #done, ctrl+x, alt+d,
the close question, hibernate) goes through `cmdbar::outside::stop_outside`
(`cmdbar/stops.rs`): a rush session stops through `Hosts::stop_all`, with
the agents it spawned; the cleanup agent's host through `Hosts::stop`.
The cleanup agent starts through `CareStarter`, which `start::install`
sets (`care::start_with_hosts`: StartedBy and Meta's why go into its
config). `start::install` also sets `Accounts::switcher` and
`Accounts::hand_over` (`session_core/switch.rs`): a switch on the same agent
paid the same way goes in place as ops (`Op::Model`, `Op::Effort`,
`Op::Permission`, then your message), else the conversation is told (or
carried whole) and `Hosts::hand_over` starts the new host off the UI thread,
waits for it to take the history, and stops the source only if it stayed
quiet; the source is then put away in Earlier. A usage limit's hand-on
(`accounts/handoff.rs`, only where rush leads) uses `hand_over` directly, as
Go's handOff calls handOverMessage. Not yet: carrying on sessions a limit
stopped (`limit_stopped`/`relogin`). Watching a spawned agent rush hosts in
the subagents view, the box goes to it (`subview::watched_host`, then
`session_core::spawned::send`).

#reload restarts the view, not the sessions: `rush_ui::run_with` returns
what the next rush starts with (`Model::reload_env`) once the terminal is
given back, and the binary runs the installed rush in its place
(`rush_sys::selfexe::path`, exec), as Go's main.go:290-300. While a view
runs it's listed for `rush reload` (`rush_state::instances`), whose
SIGUSR1 is a #reload; a hosted view isn't listed.

## The list kept between runs (Rust-only)

rush opens on the list as it last read it (rush-ui `listcache.rs`): each
priced reading is saved to rush's cache (`list-kept.json`, at most once a
minute and on quitting), read on a thread as rush starts, and drawn in the
first frame (it waits up to 50 ms for it) until the first live reading
replaces it whole. Until then its rows are drawn as they were but for an
agent's mark that says what it's doing now (working, waiting on you, just
finished, halted), drawn neutral and still so a row that was working last
time doesn't claim to be now; nothing acts on it: no host is dialled and
no reading's hooks run. One from another version or older than a week
isn't shown.

The start is one frame that then only gains details: your top bar is read
before the first frame (Go reads it with the first reading, so it opened
on the default gauges), and the kept file has the top bar's segments as
last drawn with their readings in (and the plan's rows, and the account
beside the folder). Each stands in until its own reading is in, while
the kept list shows and for at most 3 s after the first live reading: the machine's processes for cpu and ram, the
battery and disk, the network, a priced reading for today's spend, the
accounts read (and their limits fetched, for the plan). "costing
transcripts…" isn't said over the kept list's costs.

Measured on the bench copy (870 agents): the list on screen 31-36 ms
after start, against 1.35 s for the first reading.

## Following (Rust-only)

A rush leads or follows (rush-ui's `lead.rs`, after Go's hosted.go:70-77
`Follow`, `following`, `leads`). The one that leads does what rush does
for every session on its own; one that follows shows the same machine and
does what you ask of it, and leaves the rest to the one that leads. Go
follows when the app that embeds its view says so; the Rust rush follows
when `RUSH_FOLLOW` is set to anything, so it can run beside the Go rush on
the same home while the Go one leads. Over SSH (`RUSH_VIA=ssh`) it takes
no lock and follows while a view on the Mac holds it, read every 2
seconds, as Go's main.go:252-260 does (`lead::follow_while_led`). A hosted view (`rush open <id>`,
rush-ui's `hosted.rs`, Go's NewHosted: one session at the whole width,
ctrl+6 for every agent beside it, no command bar, only ctrl+q quits;
`RUSH_RENDER_HOSTED=<id>` draws it for `--render`) never leads either,
so `leads()` is neither. What only the rush that leads does:

- ends what agents left running and orphans past their grace
  (`projects_machine/tidy.rs` `to_end`, `due_orphans`; cleanup.go:79,
  snapload.go:84 reap);
- watches disk and memory and offers the cleanup agent, which tidies
  worktrees (`projects_machine/care.rs` `watch_machine`; model.go:1100);
- switches logins and other agents' accounts (`accounts/autoswitch.rs`
  `auto_switch`, `check_limits`; model.go:1100 autoSwitch, agentquota.go:90
  checkLimits) and looks for logins to keep, which can switch ~/.claude
  back (`accounts/logins.rs` `find_logins`; logins.go:63);
- compresses idle transcripts (`social/mod.rs`; model.go:1100);
- asks about optional plugins and notes it in config.json
  (`plugin_ui/picks.rs`; model.go:1100 checkOptional);
- indexes conversations for search (`cmdbar/mod.rs`; snapload.go:84);
- asks a model what a finished turn wants (`session_core/turnwants.rs`
  `turn_to_read`; turnwants.go:298); a follower reads the cache;
- notes sign-ins found and Getting started's progress in config.json, and
  shows its tips (`accounts/keys.rs` `took_sign_ins`, `accounts/onboard.rs`);
- starts the plugin broker (`rush_plugind::hooks::dial_running` only
  dials one already running);
- starts a lost session's host again (`rush_host::hosts::Sessions::
  following`, set by the binary).

- stops agents finished for longer than Hibernate after
  (`cmdbar/stops.rs` `to_hibernate`; model.go:2036);
- puts a finished cleanup agent in Earlier and stops its host
  (`projects_machine/care.rs` `finished_care`; machinecare.go:80);
- tidies done work and settles work that landed, through rush-fleet's
  verdict and settle (`projects_machine/tidydone.rs` `tidy_plan`,
  `settle_looks`; cleanup.go:236, :741), every 10 minutes and every
  minute at most, with readings of the list;
  Beyond Go: a session whose branch has landed is put away at once, but
  while its host still runs it (process alive, not stopped or asleep in
  info.json; `tidydone.rs` `host_running`) its worktree and branch wait,
  as Go would have removed them 49 s after an idle turn and left a
  session sent more work running with its folder gone. A later minute's
  look removes them, by the same rule, once it has stopped or gone to
  sleep; a message before then (it brings the session back from put away)
  keeps them. One `worktree-kept` event ("its session is still running")
  is logged per session, and the 10-minute clean-up of done work passes
  over a running session's worktree the same way;
- restarts sessions' hosts on a new install with Restart on install, or
  while a reload has sessions to move (`settings/updates.rs`
  `watch_hosts`; settings_updates.go:143); enter on Sessions restarts
  them in either;
- moves new sessions to the next provider while the first's accounts
  are all nearly out (`accounts/profiles.rs` `auto_spill`;
  agentquota.go:113);
- tells a session at work its subagent run has written nothing for 5
  minutes, moving the call it waits on to the background first
  (`list_extras/stuck.rs` `nudge_stuck_subs`, a chore every 30 s;
  stucksubs.go). Beyond Go: a run another run started isn't the
  session's (its meta's depth, or its call missing from the session's
  transcript), so it isn't told; a run waiting on a background command
  of its own the session still runs (its result names the task, the
  host's `Info.background` has it) waits 30 minutes, as one in a tool;
  and "rush moved it to the background" is said only when the host did.

Go's other leading jobs (moving pending sessions, putting idle agents to
sleep) aren't ported, so nothing more is gated. A key or command you
press does what it says either way. Discovery reads rush's state.json
and config.json again when they change, so what's put away or hidden,
here or by another rush, stays so.

## Dry run (Rust-only)

With `RUSH_DRYRUN=1` (anything but "" or "0"), every action rush does by
itself, in the rush that leads or in a host, is written down instead of
done, once its own safety checks have passed: one line per action, tab
separated, `time pid action target why`, appended to `dryrun.log` in
rush's folder (`$RUSH_HOME/dryrun.log`, else `~/.config/rush/dryrun.log`;
`RUSH_DRYRUN_LOG` names another file). The same action on the same target
is written once an hour. rush-core's `dryrun::would` is the one call: it
says true in a dry run, and the action skips itself. A key you press
that removes a worktree is written down too, not done.

The actions, by the name the log gives them:

- `end-leftover`, `end-orphan`: processes agents left running, orphans
  past their 2 minutes (`projects_machine/tidy.rs`);
- `remove-worktree`, `delete-branch`, `fast-forward`, `put-away-landed`:
  done work tidied and landed work settled (rush-fleet's `worktree.rs`
  `remove`, `verdict.rs` `settle`, `fast_forward`; `tidydone.rs`);
- `remove-temp`, `empty-stale-temp`: done agents' temp work
  (rush-fleet's `temp.rs`); `trim-tmp`: a running session's tmp with the
  disk low (rush-host's `trim.rs`);
- `compress-transcript` (rush-sys's `squeeze.rs`);
- `hibernate` (`cmdbar/stops.rs`), `stop-cleanup-agent`
  (`projects_machine/care.rs`), `rest-agent` (a host's idle sleep);
- `switch-login`, `relogin-stopped`, `switch-account`, `hand-off`
  (`accounts/autoswitch.rs`, `handoff.rs`); `restore-login`,
  `import-folder`, `keep-signin`, `adopt-folder` (rush-claude);
- `restart-host`, `revive-host` (rush-host's `client.rs`, `hosts.rs`),
  `reload` (Restart on install, `settings/updates.rs`);
- `end-stray` (a host ending another process still running its
  session), `end-stuck-shell` (rush-host), `guard-stop` (the gate's
  memory guard);
- `ask-turn-model` (`session_core/turnwants.rs`), `send-continue` (the
  API back, `social/services.rs`), `set-attribution`
  (`frame/everywhere.rs`);
- `guard-hold`, `guard-move`, `guard-refuse-spawns`, and its
  `switch-login`/`switch-account` (the runaway guard,
  `accounts/guard.rs`).

Rush's own files (its state, caches, the search index, the events log)
are still written. To try a rush that leads without it touching anything,
run it under a sandbox that denies writes outside a copy of rush's folder
and signals to other processes as well (the sweep's `lead-trial.sb`).

## The runaway guard (Rust-only)

Three checks that keep a busy few hours from spending a week's limit,
each off until set (config.json `guard`; Settings › General › Runaway
guard). rush-core's `guard` decides from what it's handed; the accounts
module's `guard.rs` runs it in the rush that leads, every 5 s, off the
UI thread, and does what it says.

- **Pace**, per account, any provider: the fair share is the room left
  in its longest window (`Quota::weekly`: Claude's week, Codex's
  secondary) over the time to its reset; the burn is what that window
  gained over the last 2 hours. Each reading of each account's week is
  kept in `weekuse.json` (3 hours of them, every rush's readings
  merged), whether or not the guard is on, so turning it on has the
  hours behind it. A reading of a new week counts from 0; one lower
  than the most seen (an older cache) adds nothing. Under 30 minutes
  read says nothing. Past `warn` (1.5×) it's said once; past `trip`
  (2×) it trips, and stays tripped until under `release` (warn − 0.3,
  1.2×). When it frees up is worked out from its use ageing out of
  the 2 hours and its fair share growing as the reset nears. A provider
  without readings has no pace: only the cap and away cover it, as its
  row says. A month-long window (Copilot's premium requests) is paced
  the same way, over the days to its reset: its fair share is small
  (88% left over 29 days is 0.13%/h), so a day's real use there reads
  as tens of times its pace, and it is past its pace. Nothing of
  another agent's runs on it, so it holds only its own sessions.
  Accounts past their pace are kept in `guard-held.json` too, so a
  reloaded rush doesn't say they tripped again.
- **The cap**: agents working at once. A rush session's turn counts 1,
  each subagent in it 1 (its host's tasks not in the background), each
  background agent or workflow 1; background shells 0. Agents rush
  doesn't run count (1 + their running subagents) but are never held.
- **Away**: no key in rush (`Model::last_input`) nor anywhere on the
  Mac (rush-sys `idle`: HIDIdleTime) for `awayMinutes` (20), or the
  screen locked. A lower cap (2), no new subagents or workflows, and
  each session's running turn finishes, then holds; one on #away
  (keep working while I'm away) carries on.

What trips never cuts a turn, and never holds what you send yourself.
A message you send goes: the guard holds only work that starts without
you. Its hold is the host's guard hold (`Op::GuardQueue`, `queue_guard`,
`Info::queue_guarded`), apart from yours (alt+h, `Op::HoldQueue`,
`queue_held`): while it holds a session, a message with no exchange
(yours, typed or answered) goes as it would, at once while idle, at the
turn's end while working; another agent's message (an exchange) or
rush's own waits in the queue (`wire::queue_ready`). The queue card
says so ("yours go when this turn ends · agents' wait: past your pace
on alex · 3.1× · alt+h lets them through"). A Claude Code turn its own
background work starts isn't a message, and isn't held.

`s` on a queued message, and the send-now key, always send now,
whatever holds the queue, yours or the guard's: idle, it goes at once;
mid-turn, the turn is stopped and it goes when it ends. Only what you
sent now goes then (`go_now` in the host): the hold stays for the
rest. A message the agent doesn't take (an image it can't read) is a
`deliver-failed` event and the session's error, and a turn it was to
start isn't left "working" with nothing running (which had a queue
wait for a turn's end that never came, and a send now stop nothing). A
queued image is sent from the copy made when it was queued, else from
where you put it; one gone from both is left out (`image-gone`), its
marker saying so, rather than keeping the queue from going. Joined,
each queued message's images are numbered on, so each marker still
names its own image.

What the guard holds, has moved and has past its pace is kept in
`guard-held.json`, so the next rush to lead lets go of its holds and
not of yours; it looks only once the accounts have been read. A host
that keeps a guard hold the guard no longer has (held before it slept)
is held again or let go at the next look; one held the old way, as
alt+h, is held the guard's way. alt+h on a session the guard holds lets
it through: it isn't held again while you keep it busy, until it has
sat idle for 10 minutes (`LET_THROUGH_FOR`). Held for the cap,
sessions go again in the order held as room frees.

Moves: when the account in use trips, new sessions switch (new sessions
only, as `n` does) to the account under `release` that the work it'd
take on (the tripped one's burn) wouldn't trip, of those the one whose
room would go unused soonest ((fair − burn) ÷ (1 + hours to reset ÷
24)), so room that would be lost at a reset days away counts for less;
the event says why ("bob resets in 4h with 38% left"). Work only ever
moves to an account of its own agent's kind: a Claude session never to
a Copilot account. With none of its kind, work waits. Then each session on a tripped account: a new one (no turns)
moves; one whose prompt cache is warm (`Info::cache_warm`) waits, as a
move writes its whole context again at full price; a cold one moves
(`relogin`), at most once in `moveHours` (6); one that moved already
waits. rush's own switching (auto-switch, other agents' accounts) never
moves to an account the guard has past its pace.

A session on an account past its pace is also refused new subagents and
workflows (`Plan::spawns_for`: the sessions on a tripped account, minus
any you let through with alt+h, whose spawns go while you keep it busy;
released, under 1.2×, lifts it). The refusal says plainly that the
user's runaway guard is pausing new agents because the account is past
its pace, and to finish the current work without starting new ones. It
goes through the same `guard.json` / `rush gate hook` path as away.

Said: the list row and queue card ("pauses after this turn: past your
pace on alex · 2.2× · frees at 20:52" while the session's turn runs, then
"held: past your pace on alex · 2.1× · frees at 14:20" once the turn has
finished and the next is waiting; `Hold::say_running`), the header's counts row, the queue card, the provider's
`pace` line and a chip on each account's row; news when an account
trips, warns, or you go away or come back; a desktop notification
(unless Notify is off) when one trips or every account is past its
pace. Events: `guard-tripped` (rush's own), and per session
`guard-held`, `guard-moved` (with the context size and whether its cache
was cold), `guard-released`, `guard-refused` (a spawn), so `rush why`
tells them. New subagents and workflows are refused through
`guard.json` in rush's folder, which `rush gate hook` reads (`refuse`:
for every session, away and the cap; `sessions`: by rush id, for an
account past its pace): Claude
Code's PreToolUse hook matches Bash|Agent|Task|Workflow; it lapses 2
minutes after the last look. A harness without that hook isn't
refused spawns.

Not yet: a session you start yourself isn't held (its first turn goes
on the account picked); the menu bar shows a held session only as
idle.

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

A restart kills a session's runs: they work in its agent's process, and
none goes on in the next. So a run whose call the transcripts leave open
but which last wrote before the session's agent last started (its host's
`started_at`, or its agent's process's start if later: a 'revived' and
'host-started' leave both) is stopped, not quiet: `SubagentRuns::
set_since` and `cut` (rush-claude's subruns.rs), set by the fleet's
loader (no running tile, so nothing nudges it) and by the subagents view
(its row says stopped).

The subagents view (`subview::list`) is a line a run: its mark, what it
was asked, a short status and how long it went, under Running and
Finished headings. Its model, worktree, steps, cost and effort show only
for the run picked: under its row when the pane is narrow (a click there
opens it), else in the column beside the list (`subview::beside`, from
`WIDE_AT` = 110 cells). That column follows clicks only, never the
pointer, and a click never opens a run across the whole view; enter
does, and the views panel's run rows pick a run there too
(`subview::pick_run`). A workflow run is named with when it started;
finished, it's dimmed and folded to its row. A workflow agent's earlier
tries (resumed after a restart) fold under the try that replaced them
(`tries:<id>`), never rows of their own. Where a run works is the
worktree its own calls reached into most (`Session::worktree_most`): a
subagent's transcript only says the folder its parent was in when it
started it, so it's read for that only for agents rush hosts. Under the
run in the column is a box to message it (`subview::box_key`, `→` or a
click gives it the keys): an agent rush hosts gets it from its own host
(`spawned::send_text`); a running subagent whose host has an inbox, into
that; any other Claude Code subagent has no line of its own, so the main
session is asked to pass it on with SendMessage (which resumes one that
finished), and the box says so; a workflow's agent without an inbox, or a
session rush doesn't run, can't be reached, and the box says why
(`relay::route`).

The background view (`running::background_lines`, drawn by
list_extras::jobs `job_lines`, Go's jobLines) lists every task the
session ran in the background but subagents, from what the reader last
brought (each task's output tail): nothing is read to draw it. A
workflow's row has its phases under it; space or a click on it shows its
agents in the subagents view, and one watched from there goes back to
the background view on esc. x (ctrl+x) stops the picked task, b and
ctrl+b move what the turn waits on to the background, x on a run stops it
alone (session_core::jobkeys, the host link's `Op::StopTask` and
`Op::Background`). While a running task is opened there, the reader also
reads the machine's process table (session_core::jobprocs: one sysctl and
the agent's tree filled, the last table kept for CPU and let go once
nothing's open): the task's process, its CPU and memory, the processes
under it, and the shells its Bash calls run in, for the chain's command
running now. A shell whose own output file is empty is read from the
files its command writes (jobTailFrom), every other one listed once it's
opened; a plugin's card stands for a task it knows; a finished task's
rows are kept until what they're drawn from changes. The agents rush
hosted for a session are read whole (those in another format by their
own adapter, `read_history`), again only once their file grows: their
steps and replies go under the step that ran them, and Changes takes
their edits.

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
- **The cards and the session rush runs.** `session_cards` reads and
  acts on a hosted session through a `Host` of fns; the binary links
  `session_core::cards_host::host()` once at start (`register`), tests
  set their own on `Cards::host`. Its `info` is built from `&Model`
  alone (what x and k would stop included: `jobkeys::stops`), so the
  pickers it uses (`picked_job`, `picked_shell`, `picked_sub`,
  `views::view_name`, `subview::watching`) take `&Model`. Answers, a
  question's answers (`Op::Answer`), a permission mode
  (`Op::Permission`), a message, a decisions card set aside, #slim's
  without and a control request (`Op::Ask`, its `Said::Reply` handed to
  whoever asked by id) go to the host. A stop with why is one job: the
  work `session_core::later` is handed inside `in_order` is kept and run
  in order on one thread (tea.Sequence), so the message never overtakes
  the stop. Rewinds, compactions, forks and hand-offs are `Hosts` calls
  that wait off the UI thread (`carry.rs`): a rewind or compaction dials
  its own client and waits for the host to hang up, then the link is let
  go and dialled again, its message put back in the box. The host's
  count of the context (`Said::Context`) is kept on the session's
  `Conn`, with what it never used worked out once per count, not per
  frame. A room set up from a chat (#discuss, #intervene) opens the
  room screens' set-up sheet (`social::roomsetup`, as #room does): its
  members' configs and the room kept and started are `Hosts` calls
  (`room_member`, `open_room`) off the UI thread. Renaming from enter's
  first answer is the list's `list_extras::rename::start_rename` (ctrl+r,
  #rename, and enter once you chose it renames). #compact's model list
  has Ollama's decision models to prune with (`Hosts::decision_models`,
  rush-ollama's in the binary) and what each summariser can't use
  (`Summarizer::summary_exclusions`). A rewind's message put back in a
  session beside the list opens it, as Go's paneFocus gives its box the
  keys.
  A decisions card stays on a session rush doesn't run now when its
  answers would wake or resume it (`HostInfo::wakes`: asleep, as Go, or
  stopped with no host running, beyond Go, whose card vanished there and
  whose d said "can't take answers here"); `session_cards::card_info` is
  what its card, keys and clicks read. Its answers are `Act::Answer`:
  shown as sending, sent over the link, or waking or resuming it as a
  typed message does (`keys::send_offline_as`); answers that don't go
  bring the card back on its send page with them kept
  (`question::answer_failed`) rather than leaving a block that reads as
  answered. Set aside with no host, it's noted beside its info
  (`Hosts::dismiss_asleep`). The block the card is up for is pinned only
  once the last turn ends with one (a view reading from the start has
  an earlier turn last for a moment), and follows its turn when the
  view's turns are numbered afresh (`take_whole`).
- **Moving agents** (`session_core::rushmove`, `session_core::moveto`;
  rushmode.go moveToRush, picker.go). /rush, #rush and a message to a
  past session move it to rush mode through `Hosts::adopt` (the new
  session back; one mid-turn waits for its turn to end, in the rush that
  leads; the old row put away, its queue and name carried). ctrl+l picks
  where new sessions start (`list_extras::prompt::StartPick`), or with a
  # command typed where the agent moves; #cd moves it by name. A rush
  session is told mid-turn, then `Hosts::cd` has its host list it there
  and name it afresh. What's on disk for the pickers is read off the UI
  thread as they open and as what's typed changes.
- **A session's / commands and picker** (`session_core::rushcmd`,
  `session_core::slash`; sessionviews.go). A / command sent from the box
  goes through `rushcmd::run`: claudescreen's head (aliases, what's left
  out, claude:), rush's own, then Claude Code's screens; Pass sends it
  on. The picker over the box (`slash::slash_lines`, drawn before the
  task row) offers # commands, @ mentions, a command's values, else the
  / commands; its keys run before the box's (`slash::key`). The commands
  on disk are read off the UI thread at most every 30 seconds while a
  /word is typed (`Conn::local`, `local_at`). /model and /effort keep
  what was picked on the `Conn` (`picked`), put back when the host
  refuses it. The session's own commands come whole on
  `Session::command_list`.
- **Pausing and killing a chain's command** (`session_core::jobkeys`,
  shells.go). k and p act on the command of a running Bash call's chain
  that runs now (`Session::running_part`): its processes are matched
  again by pid and start (the conversation keeps the start to the
  millisecond) and ended or signalled off the UI thread. What's paused
  lives in `projects_machine::shells`'s one map; the conversation marks
  it (`Options::paused`) and rush lets them all go on as it quits.
- **Scripts** (`session_core::scripts`, scripts.go). A session's
  latest calls run as scripts are read off the UI thread every 300 ms
  while they run; each read's view goes to the open conversation's
  `Options::scripts`, worked out again only when a read or the call's
  state changes. A click on a line's number sets or takes away a
  breakpoint (the file written off the UI thread); p on a held one lets
  it go on.
- **The preview's live mirror** (`list_extras::live`, live.go). For an
  agent outside rush mode whose service runs it with a process, the
  Session beside the list attaches to its own screen
  (`Joiner::mirror`) and draws it from a terminal in memory
  (`list_extras::vt`, the visible screen only) under Go's strip
  (screen, summary; v swaps them). One mirror, for the agent the pane
  drew this frame, sized to it; let go as the pane closes or moves on,
  while `cmdbar::attach::attached` says a session's own screen has the
  terminal, and by attach itself (`Attached::close_live`). Attaching,
  asking whether the service runs (at most once a second) and closing
  are off the UI thread; the screen is read on a thread of its own that
  pokes the UI on a lane of one, so an idle session costs nothing.
  Typing into it (Go's embed.go): tab, → or a click on it gives it the
  keys, each sent as the bytes a terminal sends (a paste bracketed) by a
  thread of the mirror's own, in order; ctrl+] or a click on Agents takes
  them back. With the summary picked (v) the strip stays over it.
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
  from the background draws at most every 250 ms (`framehold.rs`),
  counted from the last frame background work drew, so a key's frame
  doesn't hold back what the key asked for; keys and the mouse draw at
  once. When what a key asked for lands (a placeholder such as "reading
  the conversation…" filled), call `m.answered()` and it's drawn at
  once. Show what's known at once and fill in the rest as it lands
  (Projects shows git's word on the worktrees before their sizes). Ask for a wake-up with
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

- **Settings.** `screens/settings` (the crate `rush-ui-settings`, re-exported there; its full-UI tests are in rush-ui's `settings_tests/`) is both settings places: Harnesses
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
  Keys differ from Go's on purpose (rush-only): a value changes only by
  a pick, a typed value or space. `form::form_key` opens a row's choices
  in a `frame::picker::Picker` on enter or → (a typed row with choices
  takes a value typed there; one without asks in the input line), space
  switches a row of two values or on/off (and steps a row with a
  preview), and ← and h are never a row's: an open provider, harness or
  plugin backs out on them as on esc, Config moves a column left, a flat
  form does nothing. Go's ← stepped the value back, except on an open
  provider. The key lines say these (`form::change_keys`), and keymap's
  `page.in`, `page.back`, `page.switch` describe them in the guide.
  A harness's own settings go through rush-core's `HarnessSettings`
  (rush-only): `own_settings(profile, env)` says each setting's key,
  label, kind (`OwnKind`: choice, open choice, on/off, text, number,
  tokens, a variable, a new variable, a plugin), value, where it comes
  from (`ValueFrom`: env, the file's env block, the file, a state file,
  default) and its choices; `check_setting` refuses a value before it's
  written; `set_own_settings` writes edits by key. `settings::ownsettings`
  draws whatever a harness offers as sections of its Harnesses page
  (`Setting::source` after the value) and writes off the UI, reading
  back before the row shows it; a harness with none shows nothing.
  Claude Code's (`rush_claude::ownsettings` over `usersettings`) is
  settings.json, its env block and enabledPlugins, written to every
  distinct settings.json the accounts read (a home's link and its target
  are one), the rest of each file and its links kept, mode 0600.
  Codex's (`rush_codex::settings`) is config.toml's model, reasoning
  effort, approvals and sandbox, each key's line rewritten alone and the
  result read as TOML before it's written. files.rs's generic
  settings-file section is left for an agent that offers only a
  SettingsPage.

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
  A link's text is clickable in the terminal through rush-draw's OSC 8
  (see Links).
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
  the mode is `session_cards::permissions::permission_command`. Not in
  Go: `bar_line` keeps where each segment it drew landed (`Ctx::at`), and
  the model, effort, account and billing segments take clicks too
  (`agent::HeadAct`: /model's and /effort's lists, the switch sheet on
  its Account row), lit under the pointer (`style::light_cells`); a click
  on the agent label opens the switch sheet on the part clicked
  (harness, account, model). The same for the boxes' edges:
  `InputBox::edge_at` says where an edge's labels landed, cut or
  dropped for room, `EdgeAt::place` puts a label's parts there and
  `InputBox::lit` lights the one under the pointer. The Prompt's folder
  opens ctrl+l's picker, its chip's name, model and effort the Start
  sheet on that row (the other setups are picked, as in Go); a
  Session's box takes clicks on its permission mode (as #perm) and its
  send mode (as ctrl+t). The
  layout is the one being built in /statusline while its tab shows,
  else bars.json, read off the UI thread with the first list reading;
  what a line left out is kept by `which*10+line` for /statusline.
  Account readings: logins' through the logins agent's `PlanReader`
  (`accounts::state::readings`), others' from quotas.json; `fetch.rs`
  reads both again every `usage::EVERY` with a loop running. A Claude
  plan without a 5-hour limit (Anthropic's top plan, since Go): its
  5-hour window, absent, null or sent with no reset time, is read as
  none (`rush_claude::usage::Window::five_hour`, also on usage.json
  Go wrote, and on a session's live report), so the switching, the
  order and the meters go by the weekly alone; a place drawn for the
  5-hour meter (Settings' account table, the provider's rows, the
  Usage tab, the limit card) says `usage::NO_FIVE_HOUR`
  (`Quota::no_five_hour`). The plan's name is the sign-in's
  subscriptionType. Not yet:
  the other logins' own refresh (fetchLoginUsage), a kept sign-in's of
  an agent reading any account's, a session's billing and its host's
  rush version (no Info for them), the name's @handle, headerGit, the
  hover on the label and its click to switch.
- **clanker's reactions** (`clanker.rs`). What `session_core::finish`
  keeps (`Finish.fx`) is drawn on him as Go's clkFX: drawing the header
  moves the reaction on a frame for every 70 ms gone and asks for a wake
  at the next, only while one runs and he's shown; his beat (a shimmer
  at second 8 of 15 working, 25 of 45 idle) rides the tick and never
  wakes rush itself. His mix uses `mul_add`, as Go's compiler fuses it on
  arm64, so every cell matches Go's (a hashed dump of every frame).
- **Gaps.** Colours are written as the dark ground's constants and
  moved onto the ground picked as rush-draw paints (`ground.rs`,
  photon's theme). For Theme "match terminal" rush-draw asks the
  terminal's colours (OSC 10, 11, then DA1) as it takes the terminal,
  before the input thread reads, waiting at most 50 ms for it to start
  answering (Rust-only: Go's bubbletea applies them whenever they come).
  Replies are kept in rush's cache by terminal (`term-colours.json`);
  one that hasn't answered in time starts on what it said last time, and
  the input thread takes its late replies in as colours (rush-draw
  `reader.rs` LateOsc), which are applied then. They aren't asked again on focus or
  a mode 2031 report, as rush-draw hands the UI keys, not raw replies.
  Handing the terminal to `$EDITOR` (Go's `editorCmd`) waits on
  rush-draw pausing its input reader: every $EDITOR use goes through
  `session_core::memedit::edit_then(m, path, done)`, which runs the
  frame's `set_editor` runner once there is one (done after the key
  that asked, so the memory review counts "edited" then) and until then
  says where the file is. The UI stall watch times each
  message and frame after the fact (no watchdog thread, no stack).
- **Links of a kind rush knows.** `rush_convo::linkinfo` asks each
  link's kind off the UI; the TUI turns lookups on as it starts
  (`cmdbar::linkkinds::start`: `look_links(true)`, and `on_lookup`
  sending a frame on a lane of one, never waiting), tests and `--render`
  leave them off. `cmdbar/linkkinds.rs` keys what the UI does by kind:
  a pull request's link opens its sheet (`prsheet::open_pr_link`, asked
  of gh by its url, read from the link's lookup) from a click in a
  message, and its menu offers Open in rush and Merge… first. Merging
  only ever asks; nothing goes to GitHub without a key.
- **The list and the Session side by side.** `list.rs` groups each
  section by project (Settings' split by project) and draws the rail look
  or the classic; `list_extras/split.rs` decides the layout as Go's
  `widths` does: the view setting (`split`, `agent`, `list`: View,
  ListOnly, ChatFull), `Model::full` (Go's m.full: ctrl+6 from the split,
  shift+←, #view agent), a Session on screen (`split::preview`: the
  opened one, or one beside the list for now), 170 columns splitting by
  itself, and room for an 84-column pane beside a list of at least 30
  columns. Not as Go, on purpose: the list goes down to 30 columns on
  any screen (Go's least is a quarter of it), by shift+← or dragging its
  edge, and a wide screen doesn't widen it to hold the Session under 128
  columns (Go's maxPane); rush's own width is still 28% up to 64, a
  share you set still kept (30 columns to three quarters). A Session
  wider than its text needs keeps the text at 124 columns; the column
  beside the conversation (`session_core/side.rs`: tasks, changes, what
  runs, folded views) takes the rest, its card's ground filling it and
  its rows laid out 79 wide at most (`side::text_width`). An opened Session (`Model::session`, with the
  keys) stands beside the list wherever there's room, as Go's focusPane
  does; it has the screen only when there isn't, or when the Session
  alone is your layout. esc, ←, { } and tab give the list the keys back
  and leave it beside the list as `Model::pane` (leavePane); esc in the
  list lets it go (leaveChat). The pane is drawn as an opened Session is
  (`Model::pane_frame`): the model narrowed to the pane's width while
  it's drawn and while it takes a key or the mouse (`split::in_pane`,
  `split::sw` the screen's width meanwhile), the pane's own swapped in
  with `pane_idle` when the list has the keys. The side without the keys
  fades 30% toward the ground (`split::fade_row`, Go's fadeRow), the
  pane's gutters kept; the divider is in the harness's grey. The
  Session's header carries Go's wash (`frame/headerwash.rs wash_head`)
  and its rule the harness's grey, lit orange only with the keys. A
  click on either side gives it the keys and is then that side's (in a
  box, the cursor goes where it landed), and the pointer crossing onto
  a side does too, once as it crosses, not on the way back from another
  window (`split::focus_at`, `split::pointer_at`; Go's focusAt). The
  Session's box is found on the finished frame past the list and its
  divider, the picked agent's too while the list has the keys
  (`dock::locate`), so one click both moves the keys and the cursor.
- **Pictures.** `screens/session_cards/pictures.rs` draws an image fitted
  to a box of cells in quarter blocks (Go's convo/picture.go and thumb.go
  where Kitty isn't there), each image and size made once on a thread of
  its own, two at a time, the frame after it's made having it; the image
  crate decodes PNG, JPEG, GIF and WebP, as Go does. Kitty pictures and
  asking the terminal its cell size (Go's pictures.go) wait on rush-draw
  writing raw output; until then the cell is taken as Go's 10×20.

## Conversation views (Rust-only)

The Conversation tab has five views, one for every session, kept in
config.json as `convoView` ("" is Normal): Normal (as always), Chat (what
you and the agent said, and what it asked; each stretch of steps one quiet
"n steps hidden" row that opens in place), Code (each edit open on its
diff, each check or failed command one line, the agent's words cut to
three lines), Catch-up (a card a turn: asked, said, changed, checks, open,
time and cost) and Trail (a line a step). ⌥s (session.preset, also
ctrl+] g) goes through them; the tab row names one other than Normal and a
click on it opens a picker; enter or a click on a catch-up card or a
trail's step opens it in Normal. Changing view keeps the picked row, else
the turn at the top.

They are `Options.preset` (rush_convo::Preset), part of the renderer's
signature: the same parsed turns, drawn a turn at a time
(render/draw/presets.rs), so only the turns in view are drawn whichever is
picked. Checks are read from the command's words (cargo, go, npm/pnpm/
yarn/bun scripts, make, pytest, eslint, ruff, tsc, …) and its exit code,
never from one harness's own result shape.

`cargo bench -p rush-convo --bench render`, 3000 turns at 120 wide
(µs per frame, before → each view):

| frame | Normal (main) | Normal | Chat | Code | Catch-up | Trail |
|---|---|---|---|---|---|---|
| cold, at the end | 6354* | 1813 | 546 | 528 | 184 | 186 |
| warm, at the end | 4.4 | 9.6 | 4.8 | 46.5 | 44.8 | 3.7 |
| warm, mid-way | 4.1 | 4.2 | 4.5 | 4.8 | 34.0 | 3.4 |

*Cold frames swing with the machine's load: the same Normal frame measured
790–1813 µs in the branch's own runs. Warm frames are the steady cost.
Code and Catch-up redraw the bench's live last turn on every tick (no
unit memo for them); still far under a millisecond.

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
is a descriptor. Discovery watches folders of many through FSEvents
(Threads), so the list on a home of 1,200 sessions holds 42 open files,
not 1,391.

## Each rush home keeps its own sign-ins (Rust-only)

rush keeps the sign-ins of logins not in use, and providers' API keys,
in its vault (rush-state `vault.rs`): the login keychain on macOS, files
under `<home>/logins` elsewhere. Every provider goes through it
(`rush_state::vault()`), so the rule below holds for all of them.

- The normal rush home, `~/.config/rush` (or `~/.config/agtop` while
  that one is in use) of the user this runs as, keeps the keychain
  service `rush-login`, as Go and every earlier rush did: nothing is
  signed in again. A RUSH_HOME naming that same folder (a plugin, the
  remote service, a link to it) is the normal home too: homes are
  compared by canonical path. "The user" is the password file's home,
  not HOME, as the keychain is the account's whatever HOME says.
- Any other home has its own service, `rush-login-` and 8 hex of a
  SHA-256 of its canonical path (`vault::service_for`), which starts
  empty. Off macOS the files were per home already.
- So a copy of the user's rush home (a test, a trial run) never reads,
  renews or hands out the normal home's sign-ins: a login's home it
  makes is filled only from its own store, and with nothing there it
  says "rush has no sign-in for X; sign in to it again". Each login
  home still has the keychain item Claude Code names for it
  (`Claude Code-credentials-<hash>`), made by Claude Code's own rule.
- Who sets RUSH_HOME for their real rush sees, there, the logins in its
  config.json with no sign-ins behind them, and API keys rush kept
  (keys in their variables still work): sign in again in Settings ›
  Providers (`r` on a login signs in to it again, `l` adds another), and
  put API keys in again there. There is no import from the normal home on purpose: a
  sign-in copied to two stores is renewed by both, and the first renewal
  ends the other copy, which is what this keeps from happening.
- In a test (`rush_core::sandbox` active) `vault::keychain` is a map in
  the process, so tests exercise the keychain path without the real
  keychain.

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

To check, run the tests under `sandbox-exec` with `rust/tools/rush-tests.sb`
(through `rust/tools/sbtest.sh cargo test -p <crate>`), a profile that denies
writes to the real `~/.config`, `~/.claude`, `~/.codex`,
`~/Library/Caches/{agtop,rush}`, unix-socket connects there and to Claude
Code's daemon for the real `~/.claude`, exec of the
user's claude, codex and rush, the keychain, and signals outside the sandbox: any
denial shows in `log show --predicate 'sender == "Sandbox"'`. The profile
needs `-D CLAUDE_DAEMON` and `-D CLAUDE_DAEMON_REAL` (sbtest.sh passes them). Under it,
these tests fail for the sandbox's own reasons (it won't run setuid `ps`
nor set a setuid bit):

- rush-sys `script::tests::breakpoint`
- rush-sys `actions::tests::signal_tree_pauses_and_resumes`
- rush-gate `watch::tests::watch_pauses_the_newest`
- rush-plugin `digest_is_gos`

Run rush-sys's process tests and rush-gate's watch test outside it.
rush-sys `actions::tests::end_tree_returns_once_the_tree_has_gone` passes
in it.

## Parity oracles

Go rush is gone from the tree; its last version is the `go-final` tag
(`git worktree add ../rush-go go-final` to run the Go side below).

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
- `cargo bench -p rush-convo --bench search` (RUSH_BENCH_TRANSCRIPT=<a
  transcript>): an open conversation searched as the command bar does on
  each key, letter by letter: Session::search's instructions per key and
  a digest of its hits, then Session::search_top (the bar's This session
  and This chat: only the hits a section shows, with a memo kept from key
  to key and a vocabulary of the words typos are looked for among, made
  on another thread as the bar opens) checked hit for hit against it.
- `cargo bench -p rush-ui --bench frame` (under `tools/sbtest.sh`): a frame
  of the Agents list, 300 agents at 160x48 — its rows, painted, and a key
  then the frame — in µs, instructions, bytes and allocations per frame.
  Then the same with ctrl+k's bar open over it. `-- profile` draws the
  list for 20 s instead, for `sample`; `-- profile-bar` the bar.
- A conversation keeps what's drawn, not what was sent: a step's
  structured result only for the keys something reads
  (`rush_convo::model::RESULT_KEYS`; no file contents, shell streams or
  MCP screenshots), and a shell's streams once when its output is them
  run together. The 125 MB "Scoping Trace Sharing" transcript read whole
  retains 12.7 MiB (30.2 before, 2026-10-30); its open TUI's footprint
  went 231-255 → 185-197 MB (the list and ctrl+k's held searches are
  most of the rest). ctrl+k lets go of what it held on Go's two-minute
  timer, not at the next search: 2-4 minutes after it, the live heap
  falls 124 → 79 MiB and the footprint 180 → 125 MB. A long
  conversation closed or replaced is freed on a thread of its own and
  its pages given back (`rush_sys::mem::let_go`).
- Footprint: `/usr/bin/footprint -p <pid>` idle on the list, and with the
  largest transcript under `~/.claude/projects` open; side by side with Go.
- Idle: `top -l` CPU and wakeups over 60 s with nothing changing.
- `rush --soak [30s] [200x50]`, as Go's: the whole view run with no
  terminal, then CPU, max RSS and what reached the screen. Rust draws into
  memory and counts the rows that changed each frame as the bytes a
  terminal would get, so its "terminal" line is near Go's, not equal; it
  prints threads where Go prints goroutines, and has no heap or GC lines.
  Run it against a throwaway HOME under `tools/sbtest.sh` when testing.
- Timing log (Rust-only, off by default): `RUSH_PERF_LOG=<file>` writes the
  process's start time and one line per frame (what it handled, µs spent
  handling and drawing, the UI thread's own CPU µs for each (`ucpu`,
  `dcpu`: steady on a busy machine), cells changed, and a key's latency from its
  arrival to the frame written); `RUSH_PERF_MARK=<text>` notes the first
  frame showing that text, and `RUSH_PERF_WATCH=<a|b|…>` each frame
  each text shows in and leaves, to time a placeholder (rush-draw
  `perf.rs`). The sweep bench
  (`~/.local/share/rush-sweep/perf/bench.sh`) reads it for startup, key
  and scroll latency.

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
- **The host itself.** rush starts every host with `MallocSpaceEfficient=1`
  (the host takes it out of its environment, so its agent doesn't inherit
  it; Rust-only): after `flood 15000` on the live bench a host's footprint
  is 4.6 MB against 21 MB, idle fresh 4.2 against 5.5, for about 0.3 s more
  CPU per 15,000 streamed steps and none idle. `malloc_zone_pressure_relief`
  at a turn's end gives nothing back on macOS 27's malloc.
- **A streaming turn's info.json.** The host writes what a turn is doing
  (detail, context tokens, cache warmth, updatedAt) to info.json at most
  every 250 ms, the latest always; any other change, and every change while
  idle, at once, as Go does (Rust-only). Writing it on every message was
  ~90% of a busy host's time and woke every watcher of the folder: `flood
  15000`, host CPU 8.5 s → 1.9 s, folder writes 89,592 → 81.
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
