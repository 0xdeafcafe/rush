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
  `docs/performance-review-2026-10-01.md`). A frame whose rows are
  those on screen is neither painted nor written (rush-draw
  `Canvas::keep`), and what several parts of one frame each ask (the
  accounts, which read folders on disk) is asked once a frame
  (rush-ui `frame/framememo.rs`).
- **Same look.** Screens copy the Go TUI exactly (`docs/screenshots`,
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
| `rush-watch`  | `rush watch pr`: waiting on a PR's checks for an agent (exit 0/1/2/3 as Go); `rush watch run` (run.rs, new in the port): the same for a GitHub Actions run with no PR (a branch's or commit's newest run per workflow, or a run id), through the Actions REST API; a branch's runs cancelled with the branch at another commit end as cancelled, saying where the branch is and whether that commit has the one watched (GitHub's compare), so the agent decides whether to wait on it; the gh answers every watcher shares (ETag, flock, `watch/gh`) and the rate limit every watcher sees (`watch/gh/ratelimit.json`: each answer's X-RateLimit-Remaining and Reset, so all slow down together as it runs low, and once it's spent or GitHub says slow down, the time nobody asks before: Retry-After, else the reset, else 1 min, at most 1 h); a PR watch's live record; the hook that turns polling (`gh pr checks --watch`, `gh run watch`, sleep loops) into them | `internal/watch`, `cmd/rush/watch.go` |
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

Claude's spend cache is kept between runs in `costs.json` in rush's
cache folder (version `spend::CACHE_VERSION`). A `costs.json` the Go rush
left is of another format and version, so it is dropped and rebuilt; the
old `costs.json` is removed at the first load. Measured 2026-10-29, ~860 rows:
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
   last one sent, and polls (every second, Go's tick) only while an
   agent works or has a process; an idle terminal's own session file,
   watched, says when it starts again. A busy agent's files, changing on
   and on, are read at most once a second (`GAP`): one wait its Stop
   ends, not 50 ms naps (20 wakes a second while an agent worked).
   Discovery also reads what each live agent's processes use
   (memory, CPU, how many): the loader lists the process table as Go does
   (one sysctl), which spawn folding and the machine's rows need, and
   fills memory and CPU only for the trees it shows. A reading while
   agents work makes rush's sessions again and keeps every other row
   (past conversations, finished jobs, other agents' past sessions) as
   the last whole reading made it (`Loader::incremental`,
   rush-fleet loader/partial.rs), so it costs what the sessions do, not
   the hundreds of rows: a whole reading comes when the watch tells of
   anything but a session's own folder (rush's state, the sessions
   folder, a projects folder, a job, a terminal session), when a
   session is new or gone, an adapter's live sessions differ, a past
   conversation no session has was written to (`Discoverer::past_changes`
   numbers each listing), a checkout's branch moved, a kept row's spend
   moved, and at least once a minute. Only the remaining rows' transcripts
   are priced before it, and a live row's preview is read again only
   when its transcript's size or time moved. The menu bar's feed shares
   its watcher with its loader the same way, and reads rush's config and
   state only when they change.

On macOS the UI and terminal reader threads run in the user-interactive
class (rush-draw `qos`), as an app's main thread does: a CLI's threads
otherwise share the default class with every compiler an agent starts,
and with ten builds going a frame waited 250 ms to 3 s for CPU
(stalls.log, load 160 on 10 cores). A program takes on the class of the
thread that starts it, so what rush starts to keep running (hosts, the
plugin broker, reminders, ollama, rooms, prdrive, pushlater, the sshd
shell) goes back to the default in its pre_exec (`rush_sys::qos::plain`).

4. **netwatch** (rush-net): whether the API can be reached. It checks
   once at start, when the network changes, and when a job fails on what
   looks like the network; on Go's timings (2 min steady, 30 s shaky, 5–30
   s down) only while something needs it: a holder of
   `netwatch::want_checks()` (the TUI holds one for its whole life, as
   Go's watchNet starts at Init: the top bar's "net steady / net shaky /
   offline" needs it; a session's host and `--render` hold none), the API
   down, or a job held for it. Rates are read only while a
   `watch_rates()` guard lives (#network). A host's netwatch, unwanted,
   doesn't wake. The TUI hears a change through `netwatch::on_change`,
   called on netwatch's own thread (a try_send): no thread of the UI's
   waits on it.
5. **A host's** (`rush host run`, one per live session, so dozens): five
   with Claude Code running between turns, eleven before (2026-11-02);
   see The host.

6. **Plugin connections** (rush-plugin `rpc.rs`, `rpcread.rs`;
   Rust-only: Go reads each in a goroutine). One `plugin-rpc` thread per
   process reads every JSON-RPC connection made on a descriptor
   (`Conn::new`'s socket, `Conn::lines_fd`'s MCP stdout pipe): it waits
   in one kqueue on all of them, reads what's ready without blocking
   (recv with MSG_DONTWAIT on a socket, whose descriptor the blocking
   writer shares; a pipe set non-blocking) and cuts it into frames or
   lines with the same limits and words as the blocking readers. No
   handler runs on it. While a connection's queue is empty, a reply goes
   straight to its call and a request to a `plugin-rpc-call` thread
   (1 MB stack); a notification, a parse error's answer, a refusal, the
   end, and anything after one of these not yet taken, wait in the
   queue, which a `plugin-rpc-queue` thread works through and leaves
   once it's empty, so notifications are taken in order and finished
   before a later reply or request is taken, as when the reader ran
   them itself. While the queue has work the connection isn't read (its
   descriptor disabled in the kqueue until the queue's thread has done
   it), as a reader running a handler read nothing: a plugin slow to
   take events fills its socket and the broker's bounded queue for it
   drops the rest, rather than this queue growing. Every thread it starts takes on the rush folders
   (`rush_state::thread_dirs`) of the thread that made the connection.
   An idle connection has no thread: plugind with 14 plugins and a
   window had 15+ reader threads, now 1; 20 idle pairs in a test process
   add 1 thread, not 40 (`tests/rpc_threads.rs`). `Conn::framed` and
   `Conn::lines` on a reader with no descriptor keep a thread each.
   `cargo bench -p rush-plugin --bench rpc` (CI's M1, 3 cores, against
   the same socket read by a thread of its own; two runs): a call's
   round trip 60-64 → 44-53 µs; 1,000 notifications 1.04-1.31 →
   1.12-1.39 ms, no clear change (the queue's thread is started again
   each time a burst drains).

### The plugin service's threads (Rust-only)

`rush plugind` keeps no thread per plugin while it runs. Go's has a
goroutine each to supervise, wait, read and write; the port first had a
thread each (65 threads idle with 13 bundled plugins and one other, each
bundled one a `rush plugin run` process of 2-3 threads and ~3.2 MB: 13
processes, 27 threads, 42 MB, 2026-11-02). Now:

- Supervising: a thread starts a plugin and says hello, then ends. Its
  exit, its connection closing or the broker's stop starts what comes
  next on a fresh one (`Runner::hand_off`); only a backoff after a crash
  holds one.
- Waiting for exits: one thread, `rush-reap`, on one kqueue
  (`rush_sys::reap`, EVFILT_PROC), for every plugin process.
- Writing: an outbox has its thread only while something waits to go.
- The memory and files watch looks every 5 s only while a plugin runs as a
  process; the development-mode watch every 2 s only while a plugin is in
  development mode. Otherwise both wait for a reload or a plugin starting.
- Bundled plugins that read nothing of their process's own (environment,
  folder, stdio) and start no programs run inside the broker
  (`rush_plugin::bundled::StartFn`): the same protocol on a socket pair,
  no process, no memory limit (no footprint of their own to measure),
  what they'd print on the way out in their log, a panic as they start
  caught and restarted as a crash would be. That's all thirteen of the
  ones above; the cleaners, ts and haven keep their process. Fourteen
  such plugins started inside a broker, idle, add 2 threads and 1.1 MB
  of footprint (`rush-plugind/tests/bundled.rs`, CI 2026-11-02), where
  thirteen processes held 27 threads and 42 MB.
- The broker's own threads get 1 MB stacks, outboxes 256 KB
  (`rush_sys::threads`).
- Reading: the process's one shared reader (item 6). A handler that
  panics, in any connection, answers its call with a server error
  ("panicked: …") rather than leave it to its deadline.

### The TUI's threads (Rust-only)

No thread waits for a time alone, or for one thing another thread
already waits beside:

- What only waits for a time waits on the UI thread's own deadline
  (`Model::after`, frame/work.rs, in `App::wake_at`): the list's folder
  asks, Projects' 2 s refresh, the plugin picks' first look, and the look
  for updates, which wakes as its last look turns four hours old (it
  woke each minute). Each was a thread asleep (rush-later, rush-updates).
- `Model::later`'s work goes to a thread idle from the last piece, else
  a new one (`rush_sys::threads::Pool`, rush-later, ended once idle 10
  s): none waits behind another, and a reading of the list a second
  apart no longer starts a thread for each piece it hands off.
- `rush reload`'s SIGUSR1 is heard in the terminal reader's poll
  (`Loop::on_signal`), not by a rush-reload thread.
- yap's follower, with no yap running, waits on yap's folder (a socket
  can't be opened to be watched) and looks again a minute on at the
  latest; it looked every 2 s, 30 wakes a minute with its plugin on.

Measured in `crates/rush-ui/tests/threads.rs` (alone in its file), on
CI, 2026-11-02: the whole view started headless (`rush_ui::start`, as
`run` starts it; no terminal, no agents, no plugin broker) keeps 15
threads once the list is read, later's idle ones included (16 before:
rush-updates, rush-net and a rush-later asleep among them; the live
TUI adds the terminal reader, its host link and yap's). An idle minute
on that quiet list, stepped through on the model's own deadlines
(`Model::wake_on`, `Model::instant`): 6 wakes, the chores of Go's tick
and the look for updates (before: 5 here, and rush-updates' minute
besides), 5 frames (a chore draws again only a screen showing more than
the list). The live UI the same day, mostly idle on screen while agents
worked: 22 long-lived threads and many short ones (a thread per later
piece; discovery's gap napping 50 ms), 5.9% CPU over 17 minutes; two
thirds of the UI thread's busy time was the session's dock
(`Session::job_kind` hashing every task's call id for each run, every
frame; `agent_of` making every phase per run) and the Changes tab's
count (copying every file's patches out each frame); see
`cargo bench -p rush-convo --bench jobs`.

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
half second.

A host runs few threads (Rust-only). Its main thread waits in one kqueue
(`server/serve.rs`) for its socket's clients, its folder (it stops once
the socket is gone), the process that started it and its quitting, where
Go has a goroutine each and the port had a thread each, the folder's
waking every 30 s. The look at its tmp every 10 minutes is a job on its
timers' thread. With them run its agents' queue and the events log's
writer. Claude Code's session holds one more, its output's reader, which
reads stderr in the same poll; stdin is written by a thread that lives
only while something is queued, and the process group's guard
(`rush_sys::pgguard`) has none: it's reaped as it's released. Idle
between turns, a Claude Code host ran 11 threads (`ps -M`, 2026-11-02),
now 5; `rush-host/tests/threads.rs` counts an idle host's own, 7 → 4, and
`rush-claude/tests/threads.rs` a waiting session's, 4 → 1, each file its
own process. It wakes for its tmp look and its idle timer only, not
every 30 s. A connected client adds two (its reader and writer).

rush-fleet reads hosted sessions through its `Hosts` trait, which
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
(`Hosts::wake`); a stopped one resumes. Showing a live session opens
its host through `Hosts::open` (rush-host's `open::open`, Rust-only): a
refused or missing socket is read against the host's info first. A host
that has gone (stopped, asleep, or ended without tidying, its socket
left behind and refusing every dial) is a stopped session, shown from
its transcript with nothing said; its leftover socket is removed
(`host-socket-removed`) unless a start holds its wake lock. A host still
starting is waited for up to 5 s, its folder and its exit watched. Only
a host that runs and doesn't answer, or a dial failing some other way,
is a failure: said once, in plain words, where the session is (under
its own box when it's open; on the pane beside the list, and in #log,
when the list has the keys), and logged as `session-open-failed` with
its reason and the raw error, so `rush why` shows it. Go said
"couldn't open the session: <error>" under whatever box had the keys,
again at every reading. A send without a view
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
SIGUSR1 is a #reload, heard in the terminal reader's poll; a hosted view
isn't listed.

### Moving a host onto a new build (Rust-only)

A host moves onto a newly installed rush in place, its agent running on
(`rush-host/src/upgrade.rs`, `server/upgrade.rs`). Asked by the `upgrade`
op, it waits until it's between turns (idle, nothing queued, asked,
retried or running in the background, no start or stop under way), then:

1. asks the installed rush `rush host can-upgrade <id>`: a rush from
   before this answers with its usage, one whose adapter can't take its
   agent up says why. Either way the host stays where it is.
2. holds its agent through the Conn's `Upgrader::pass`: the adapter writes
   what's queued for the agent, stops writing, and stops its reader
   between two reads (Claude's reader polls its stdout and a wake pipe), so
   nothing the agent says is lost: what was read of a line so far is
   handed on as `unread`. It refuses while the agent waits on an answer.
3. writes `sessions/<id>/upgrade.json` (0600): its info, the replay ring,
   the session's commands and context lines, its listener's fd, and the
   agent's `Upgrade` (pid, fds by name, unread, the adapter's own state:
   for Claude its session id, sign-in, account and control request count).
4. clears close-on-exec on the listener and the agent's fds (stdin,
   stdout, stderr, and the pgguard's pipe: closed, the guard would end the
   agent's group) and execs `rush host adopt <id> <state>` with the lock
   held, so nothing changes while it goes.

exec keeps the pid, so the agent stays its child and its owner watch,
the list's pid and the session's socket are unchanged. Every other file
closes, clients' connections among them: a client sees its connection end,
finds the socket still answers (the listener's backlog holds the dial
across the exec), and dials again, replayed as after any other let go
(live.rs `resolve_close`). The new build (`server::adopt_with`) reads the
state once and removes it, takes the listener, puts back info (same
generation, its own `exe`), the ring and the rest, takes the agent up
through the adapter's `Adopter` with the same tools, tap and sink a start
gives it (no initialize, no prompt sent again), and serves on. The guard
is taken over too (`pgguard::adopt`), and the agent is reaped by pid
(`reexec::reap`), as no std Child has it any more.

If the exec fails, the fds go back to close-on-exec, the state file is
removed, the adapter is told to `keep` (its reader and writer carry on)
and `Info.upgrade` says `failed: why`. Nothing is ever stopped to move.

What survives: the agent process and its pipes, the conversation, the
replay, queue/hold/away/decisions state, costs, the turn count. What
doesn't: a client's open connection (it redials), timers (idle sleep and
away are armed again; the rest only run mid-turn), a caffeinate held while
away (started again). Adapters: only Claude Code's (`rush-claude`
conn.rs `Upgrader` and `Adopter`) today; a host on any other agent has
`Info.upgrades` false and stays on its build, as does a remote host or one
from before this.

Who asks: the leading rush's look at the hosts (Settings › Updates,
`watch_hosts`) once the installed binary has settled, without Restart on
install (a move stops nothing); enter on Settings › Updates › Sessions and
`#upgrade` (asked: a failed move is tried again); and `rush reload`. The
sweep (`client::sweep_over`) asks every movable host on an older build,
mid-turn ones too, which move once their turn ends; hosts that can't
move are counted and said, never restarted. A row whose host's
`exe` is older than the installed rush shows a faint `old` (`old build`
in full) beside its badge.

Restarting an agent restarts its host too (`client::restart_agent`): the
agent is going down anyway, so there is no old host worth keeping. Every
restart goes this one way: the close question's restart (session and
list), the sign-in card's carry on (`Op::RestartAgent`), and a login
switch's restart of a session still at its limit. The installed rush
(`reload::installed_exe`: `selfexe::resolved`, a Homebrew rush by its opt
link) is asked `host preflight` first, and one that can't run the session
leaves it running, untouched. Then the host is stopped, its agent with
it, and a new host started from the installed rush (the same build or a
newer one, the same way) on the saved config: same id, folder, worktree,
conversation (resumed once it has begun), model, effort and the rest,
with the queue and decisions read back from its info. The text given
(`continue` after a limit or a sign-in) is sent once it's up. Clients see
the connection close and dial the new host. A start that fails is put on
the rush before it (`roll_back`) and the error says why; the config is
kept, so a message or another restart starts it again. It goes through
the adapter interface only, so every agent restarts so. A host's own rest
of its agent (a login moved, a `without` change) stays in that host: it
isn't a restart asked for.

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
when `RUSH_FOLLOW` is set to anything: a rush that must never lead, e.g. a
benchmark or test on a copied home, doing no housekeeping and never
starting a lost session's host. Over SSH (`RUSH_VIA=ssh`) it takes
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

## One answer for a memory (Rust-only)

The host keeps how you answered a Write or Edit into the session's own
memory folder (the harness names it: `MemoryReader::memory_dir`, Claude
Code's `projects/<repo>/memory`). For the rest of that turn, up to two
minutes, the agent's next request to write the same note or its index,
MEMORY.md, is answered the same way without a card, and logged as
`permission-auto` (reason `memory`). Any other path still asks.
`rush-host/src/server/memallow.rs`.

## The runaway guard (Rust-only)

Three checks that keep a busy few hours from spending a week's limit,
each off until set (config.json `guard`; Settings › Usage › Runaway
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

## Settings › Usage (Rust-only)

What rush does on its own to save tokens and money and to keep agents in
check, on one page after General (`PAGE_USAGE`, rush-ui-settings
`usage.rs`), in this order:

- **Prompt cache**: keeping idle conversations warm, compacting long
  waits, waiting subagents' caches and known-slow commands, each
  harness's subagents' cache life, and the two numbers they share (the
  table in "Keeping the prompt cache warm" below). config.json
  `cacheKeep` and `subagentCache`.
- **Context guard**: the bundled plugin's four rules and their numbers
  (the table in "The context guard"). Its own settings, read as the page
  opens (off the UI) and written as you change them, so its hook sees
  them at the next call. While the plugin is off the section's note says
  so and where to turn it on.
- **Runaway guard**: pace, agents at once, away (moved from General;
  config.json `guard`).
- **Advisor**: on or off (moved from General's Help and advice, which is
  now Help; config.json `advisor`). It has no other setting.

Left where they were, on purpose: General's Hibernate, Disk and memory,
Stalls (the machine and the list, not tokens), and "#compact starts on"
(your own /compact's choice, not something rush does by itself); Config's
"At a usage limit" (it's the order of that page's grid); Claude Code's
"When a usage limit hits" and "Keep its cache warm" (that harness's own,
on its page). The harness pages keep "Keep its cache warm" and
"Subagents' cache" too, the latter the same value Usage shows.

## Keeping the prompt cache warm (Rust-only)

A provider caches a session's prompt for a while after its last request.
A message after it lapses pays to write the whole context again. Unless
turned off (it's on by default), just before an idle session's cache
lapses, rush sends one small request
that reads the whole cached context: the cache's life starts over at a
tenth of the price. That request is called a bump below.

Prices, as a share of the input price, per token of context:

| provider | cache write | cache read | cache lives |
|---|---|---|---|
| Anthropic, 5-minute cache | 1.25× | 0.1× | 5 min after each request |
| Anthropic, 1-hour cache | 2× | 0.1× | 1 h after each request |
| OpenAI (GPT-5 family) | 1× (no extra) | 0.1× | 5–10 min idle, up to 1 h |
| Gemini (implicit) | 1× (no extra) | 0.1–0.25× | minutes, not stated |
| DeepSeek | 1× (no extra) | 0.1× | hours (on disk) |

What one idle stretch costs, for a context of C tokens (Anthropic):

```
                       5-minute cache             1-hour cache
message, cache warm    0.1 C                      0.1 C
message, after lapse   1.25 C  (+1.15 C)          2 C  (+1.9 C)
one bump               0.1 C + ~300 tokens        0.1 C + ~300 tokens
bumps that equal one   ~11                        ~19
  lapse
```

A bump is worth it only if you come back while the cache is kept: 3
bumps (0.3 C) pay off if the chance you come back in those 15 minutes is
over 26% (0.3 / 1.15). That chance falls the longer a session sits, so
the rule keeps the cache for a short while after the turn ends and then
lets it go:

- A bump goes when the cache has under a minute left (`LEAD`), the
  session went idle less than **15 minutes** ago (Settings › Usage ›
  Prompt cache › for), its context is at least 20k tokens (Smallest worth keeping), and the bumps this idle stretch stay under
  half a rewrite (5 for a 5-minute cache, 9 for an hour's; Most refreshes
  per wait sets another number) and under the rewrite itself counting each bump's ~300
  tokens of question and answer.
- A 5-minute cache kept 15 minutes takes 3 bumps (at about 4, 8 and 12
  minutes idle) and lapses at about 17 minutes: 0.3 C spent to save 1.15
  C. Example: 150k tokens on Opus at $5/M input: a lapse adds $0.86, the
  3 bumps cost $0.23.
- An hour's cache (Claude Code signed in) is left alone at the default:
  it lapses at 60 minutes idle, past the 15. Set "for" to 2 hours and it
  gets one bump at 59 minutes (0.1 C to save 1.9 C if you're back in hour
  two).
- 20k tokens is the floor because below it a rewrite costs cents ($0.13
  at 20k on Opus at $5/M) and Claude Code's own instructions and tools are
  most of the context; the bump's own ~300 tokens matter only far below
  that.
- A cache that has lapsed already is never bumped: that bump would be
  the full rewrite it's there to save.

Not compacted instead, unless set. Compacting before the lapse costs a
read of the context, a summary of a few thousand output tokens (5× the
input price) and a write of the summary next time: about 0.3 C at 150k,
paid whether you come back or not, against a 1.15 C rewrite paid only if
you do. It also loses detail the agent then reads back from files, which
costs turns. Claude Code already compacts itself when its window fills.
So by default, past the 15 minutes the cache just lapses.

Compacting long waits (Settings › Usage › Prompt cache › compact long
waits: off, after 1 hour, after 2 hours; `cacheKeep.compactAfter` in
minutes). Past an hour or two idle, coming back to the session soon is
unlikely, so the 1.15–1.9 C rewrite is likely to be paid later anyway,
and a compaction now (about 0.3 C) makes the next message write the
summary instead. The rule, in `cachekeep::decide`: at the look a minute
before the cache lapses, with the context at least the smallest worth
keeping, a session whose cache would lapse once it has been idle about
that long (`warm_until − idle_since + LEAD ≥ compact_after`: within the
minute a bump goes early) is compacted (`Verdict::Compact`) rather than
bumped or let go. Main conversations only (Claude Code can't compact a
subagent from outside), and only an agent with `Feature::Compact`
(`KeepIn.compact`). Never one working, with a decision waiting (`needs`,
`decisions`), something running in its background (`tasks`,
`background`), whose cache has lapsed already, nor again until it has
had a turn of its own after the compaction's (`Stretch.compacted_turn`,
carried from one idle stretch to the next: the compaction is a turn of
its own, so the turn count must pass it by 2). An hour's cache (Claude
Code signed in) lapses at an hour idle, so "after 1 hour" compacts it at
about 59 minutes; set "for" to 2 hours as well and it's bumped once at 59
minutes and compacted at about 119 with "after 2 hours". A 5-minute
cache reaches an hour only if it's kept warm that long, which its bumps'
cap (5) doesn't allow at auto. How: the guard's `HostOps::compact` sends
`/compact` to the session's host, as the compact sheet's own compaction
does, and logs `cache-compacted` (context, ttl, idleFor, warmUntil, after
in minutes, why) for `rush why`; a failure logs `cache-compact-failed`.

Never bumped: a session asleep (its program has exited; waking it costs
more than the bump), done, working, with a queue, held by the runaway
guard or by you (alt+h), stopped by a limit or retrying an API error, on
an account past its pace, or while you're away (the guard's away
check). A rush id may be bumped once in 2 minutes (`RETRY`).

How. The pure rule is `rush_core::cachekeep` (`decide`, the multipliers,
`worth`). The leading rush's runaway guard look (every 5 s,
rush-ui-accounts `guard.rs keep_warm`, gated by `lead::leads` as the
guard is) weighs each idle hosted session and keeps per session its idle
stretch (when it went idle, its bumps). The host's `keep_warm` op (host
proto 12) sends the bump through the Conn's `CacheKeeper`, found only on
an agent with `Feature::KeepWarm`; it refuses a session that isn't idle,
has a queue, has no agent running, or whose cache has lapsed. The bump
isn't use: the idle wait to sleep runs on, no turn starts, nothing is
sent as a message or queued, no notification, and the op isn't logged as
a setting. Once back, the host sets `cacheWarm` to now plus the cache's
life and logs `cache-kept` (context, ttl, idleFor, warmUntil, costUsd
the read, savesUsd the lapse it avoids, at the agent's own prices); a
failure logs `cache-keep-failed`.

The cache's life: the host reads it from each request's usage. Any
5-minute write makes it 5 minutes (the newest part of the context lapses
first), else an hour's write makes it an hour; a request that wrote
nothing leaves it as it was. Unknown, `cacheWarm` stays an hour after the
last request, as before, and nothing is bumped. The header's "cache warm"
reads the host's `cacheWarm` when there is one.

Claude Code: the bump is a `side_question` control request with an empty
history. Claude Code answers it from the last request's own cache-safe
params (`getLastCacheSafeParams`: the same system prompt, tools and
messages), so it reads the main conversation's cache; nothing is written
to the transcript and no turn starts. A process that has made no request
yet (just woken) has nothing to answer from and replies null; that, or a
synthetic stand-in answer, is a failure, and `cacheWarm` stays. Other harnesses don't declare
`KeepWarm` yet and are skipped: Codex has no request outside a turn;
OpenAI, Gemini and DeepSeek charge no extra to write the cache, so a
lapse there costs 0.9 C at most and DeepSeek's cache lives for hours.

Settings: Usage › Prompt cache (its own page beside General, with the
context guard, the runaway guard and the advisor; see "Settings › Usage"
above), kept in config.json's `cacheKeep`; a config that says nothing is
today's rule (`CacheKeep::settings` gives `cachekeep::Settings::default`):

| row | choices | default | `cacheKeep` | read by |
|---|---|---|---|---|
| Keep idle conversations warm | on, off | on | `on` (written only as false) | the guard's look |
| ‧ for | 5, 15, 30, 60, 120 minutes | 15 | `minutes` | the guard's look |
| ‧ compact long waits | off, after 1 hour, after 2 hours | off | `compactAfter` | the guard's look |
| Keep waiting subagents' cache | on, off | on | `subagents` (written only as false) | the host, as a session starts |
| ‧ slow commands in background | on, off | on | `ahead` (written only as false) | the gate, every command |
| Claude Code subagents' cache | automatic, 5 minutes, 1 hour | automatic | `subagentCache` (by harness) | the host, as a session starts |
| Smallest worth keeping | 10k, 20k, 50k, 100k tokens | 20k | `minTokens` | both |
| Most refreshes per wait | auto (half a rewrite), 2, 3, 5, 9 | auto | `refreshes` | both |

"Keep idle conversations warm" is on by default: a bump is a request
rush makes on its own, and on a plan it counts against the account's
limits, but three bumps cost about a quarter of the rewrite they save.
Each harness that can has "Keep its cache warm" on its own page (as
Usage says, on, off; `harnesses`): off there is off for its subagents
too. "Claude Code subagents' cache" is the same stored value as the harness
page's "Subagents' cache"; only harnesses with `Feature::SubagentCache`
show it, labelled with the harness's name. The numbers that stay
constants: `LEAD` (a minute), `RETRY` (2 minutes), the prices, each
bump's ~300 tokens and each step-in's ~1,000.

### Subagents' cache (Rust-only)

Everything outside the main conversation (subagents, workflows,
background and helper requests) has a prompt cache of its own, 5 minutes
unless Claude Code is told otherwise (2.1.316: the environment variable
`CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL`, which wins, or the setting
`subagentPromptCacheTtl`: "5m" or "1h"; `ENABLE_PROMPT_CACHING_1H=1` makes the
unset case an hour). A subagent that waits on a long command past its
cache's life pays to write its whole context again (1.25× the input price
against 0.1× for a read). A 1-hour cache writes at 2× but lapses far less.

Measured. `rush_core::subcache` is the pure rule: a `Tracker` per
subagent takes its requests in order, with a way to price tokens (the
agent's own `Pricer`; the core holds no price). A request is a lapse when
more than the cache's life passed since the subagent's last request (5
minutes until a request wrote a 1-hour cache; the newest part lapses
first, so any 5-minute write makes it 5), it wrote the cache, and it read
back less than the last request sent. The tokens that lapsed are the last
request's size less what was read, up to what was written (at least 1,000);
the lapse costs their write less their read. `SubCache` adds up `lapses`,
`cost`, the part of it an hour's cache would have avoided (a gap under an
hour: `avoidable`), and `hour`: the dearer write (2× against 1.25×) on every
5-minute token written, bar the rewrites it would have avoided. An hour
would save `avoidable − hour` when that's positive (`hour_saves`).

Claude Code reports it from its transcripts: `Totals.track` follows each
transcript's requests as they are priced (a request begins at its first
line's time), and `spend.rs` adds a subagent's or workflow agent's into
`Spend.sub_cache` (the spend cache's version is 2 for it). Shown:

- the Overview's Cache section, a line from the session's own requests
  (any harness's, a run at a time): `subagents' cache  4 lapses · $1.40 ·
  1-hour would cost $0.60`, then ` · 1-hour would save $0.80 (Settings ›
  Claude Code)` when it would have paid and the harness can set it;
- the Projects list's section line, from each agent's `Spend.sub_cache`:
  `· $1.40 lapsed, 1-hour would save $0.80`.

There is no `rush spend` command in the port, so none shows it.

Set. `Feature::SubagentCache` ("subagentcache") is the capability: the
harness lets rush say how long its subagents' cache lives; only Claude
Code declares it. Settings › Harnesses › Claude Code › Subagents' cache
(also on Settings › Usage › Prompt cache as "Claude Code subagents' cache", one stored value) is automatic (sets nothing, the default), 5 minutes or 1 hour, kept in
`config.subagentCache` by harness kind. The host reads it as a session
starts (its first start, a wake, or a move onto a new build), into
`StartOptions.subagent_cache`, and the Claude adapter puts
`CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL=5m|1h` in the environment it starts
Claude Code in; automatic sets nothing. A running session keeps what it
started with.

### Keeping a working subagent's cache (Rust-only)

A subagent waiting on a long command (a test run, a build) past its
cache's life pays to write its whole context again when the command
returns. Its next request is certain, so there's no bet on whether it
comes back: the only question is whether stepping in costs less than the
rewrite. rush steps in, with no request of its own and no LLM call.

What Claude Code 2.1.316 lets an outside process do, read from its
binary (the stream-json control protocol and the hooks):

- (a) A request against a subagent's own cache: none. `side_question`
  (the main keep-warm's bump) answers from `getLastCacheSafeParams`, the
  main conversation's last request, and takes no agent; nothing else
  reads a subagent's context from outside its turn. `send_task_message`
  is refused in print mode (only a remote host has it).
- (b) Moving its running call to the background: yes. The
  `background_tasks` control request with a `tool_use_id` (ctrl+b's)
  looks through every foreground task, a subagent's included (a Bash or
  PowerShell call becomes one after 2 seconds, with its `agentId`), moves
  it and answers `{"backgrounded": true|false}`. The command runs on; the
  subagent's call returns at once ("Command was manually backgrounded by
  user with ID: …. Output is being written to: …"), it makes its next
  request while the cache is warm, and the command's end reaches it as a
  notification. A subagent's background command stops when it gives its
  final answer, and Claude Code tells it so.
- (c) Before a call: the PreToolUse hook's answer may set `updatedInput`
  (`run_in_background: true`) and `additionalContext`, and its input has
  `agent_id` for a subagent's call.

The rule is `rush_core::subkeep::decide`, beside `cachekeep::decide`: a
step-in goes when the subagent's cache has under a minute left (`LEAD`),
it waits on calls its agent can move (all of them), its context is at
least 20k tokens, and its step-ins stay under half a rewrite (5 for a
5-minute cache, 9 for an hour's): the same two numbers as the idle
sessions' (Usage › Prompt cache's smallest worth keeping
and most refreshes per wait; `decide` takes `cachekeep::Settings`) and under the rewrite counting each
one's ~1,000 tokens beside its read (the call's new result, and the few
words the subagent says before it checks back). A cache already lapsed,
a subagent making its own request, one finished, or one idle (nothing
can refresh it) is let be. For 150k tokens on a 5-minute cache: a
step-in costs 0.1 C + 1k ≈ 16k tokens' worth, a lapse 1.15 C ≈ 172k;
at Opus's $5/M input, $0.08 against $0.86.

The host follows each subagent from its agent's own events
(server/subkeep.rs `SubWatch`): its messages carry the call that started
it as their parent, so it knows when its last request was, how long its
cache lives (by what that request wrote, as for the main conversation),
its context, the calls it waits on, and which of those its agent has
made a foreground task of (`TaskStarted`, not in the background). A
timer goes a minute before the soonest cache of a subagent waiting on a
call lapses; the look runs on a thread of its own, and the moves go
through the Conn's `SubagentKeeper` (`Feature::SubagentKeepWarm`, Claude
Code's "by moving its long shell call to the background") off the
host's lock. Each move is a `subagent-cache-kept` event (agent, calls,
command, context, ttl, sinceRequest, steps, costUsd the extra request,
savesUsd the lapse), a move that found nothing `subagent-cache-keep-
failed`, and a cache let go where rush looked `subagent-cache-let` with
why, once a stretch. Its row in the dock says `‹ moved to background to
keep its cache` (the host's `subKept`, by the subagent's call, until it
ends). It's on with Usage › Prompt cache's "Keep waiting subagents'
cache", its own switch apart from the idle sessions' (`cacheKeep.subagents`,
`CacheKeep::subagents_on_for`: off where the harness's own "Keep its cache
warm" is off), read as the agent starts.

(c) a step earlier: the host notes how long each shell command took
(`rush_state::took`, took.json under rush's folder, by the folder the
session started in, only those of 30 seconds or more, the newest 200 a
project), and puts that folder in its agent's environment as
`RUSH_PROJECT_DIR`. rush-gate's PreToolUse hook, for a subagent's Bash
call (`agent_id` set) not already in the background whose command took
at least its cache's life less a minute last time (`subkeep::ahead`;
the life from `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL` or
`ENABLE_PROMPT_CACHING_1H` in its environment, else 5 minutes), answers
`run_in_background: true` with a note: it took that long, check on it at
least every 4 minutes (`check_back`) in short waits, and don't answer
before it ends. It's a `subagent-cache-ahead` event. Its switch is
Usage › Prompt cache's "slow commands in background"
(`cacheKeep.ahead`, `ahead_for`: only while the subagents' cache is kept),
read at every command; the threshold stays the cache's life less a
minute.

### Cheaper model for routine subagents (Rust-only)

Opt-in, off by default (Settings › Usage, its own section after Context
guard; `rush-ui-settings/src/cheap.rs`). Routine
subagent work (a described edit, a lookup, a search, running and reading
tests, a mechanical refactor) doesn't need the strongest model. rush calls
no model to decide; it only tells the agent and, optionally, sets one
environment variable.

What Claude Code really offers (read from 2.1.316's binary, and
`rush-efficiency`'s catalog already knew the variable):

- The Agent tool's `model` parameter: `sonnet`, `opus` or `haiku`.
  Per call, and it wins over everything below. This is the lever for
  "on".
- A subagent definition's `model:` frontmatter, under the call's.
- `CLAUDE_CODE_SUBAGENT_MODEL`: the default for every subagent that names
  none; `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` overrides even a named one
  (rush never sets that). It would downgrade design and debugging too, so
  it is only the separate "all subagents" choice, never what "on" does.

Setting `config.cheapSubagents`: `mode` ("" off, `on`, `all`; "all" is
offered only while some harness has `Feature::SubagentModelAll`) and
`models` (harness kind to model). Each harness's choices come from
`Registry::cheaper_models`: the models in its own list that its pricer
prices below its default model, dearest first, so Claude Code offers
Sonnet (the default choice) then Haiku; a harness with none says "not
offered". Features: `SubagentModel` (the agent takes the model from
rush's words: Claude Code yes; Codex planned, its models aren't priced)
and `SubagentModelAll` (rush sets the one model for all: Claude Code,
through `StartOptions.subagent_model`, which the Claude adapter turns
into `CLAUDE_CODE_SUBAGENT_MODEL`).

"On" adds one part to the system prompt (`rush-host/src/cheap.rs`,
`prompt(model)`), word for word the same for the same model, after the
feed and before the worktree note (the parts a setting can change come
last, so the rest stays cached); off, nothing is added. rush's own
`spawn_agent` takes a model, so the same words cover it. The host reads
the setting as a session starts, like the subagents' cache.

The Efficiency place counts subagents whose model is Sonnet or Haiku
(`View.sub_cheap`, `sub_cheap_cost`) and the saving: the same tokens
priced at the agent's default model less what they cost
(`price::saved_against_default`, from the existing price tables). It is
an estimate: the strong model might have taken more or fewer turns. It
shows as a finding, and in the advisor's digest. Subagent rows in
sessions already show their model, read from the subagent's transcript.

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
running now. While any Bash call of the session runs (`Session::shell_running`)
the shells are read on every look too, opened or not, as Go read them on
every fleet read (snapload.go:60), so the conversation can say which command
of a chain runs. A shell whose own output file is empty is read from the
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
  A decisions card that comes while you're typing (text in the box, a
  key within `hold::HOLD_MS`, 3 s) is held off the box
  (`session_cards::hold`, beyond Go): `decisions_card` skips it and the
  box's bottom edge counts what waits. It shows once the box is empty
  and 3 s pass without a key; with text left it waits for alt+j (open,
  text kept: the card sits above the box) or alt+shift+j (the box's
  `Editor` set aside whole and put back once the card is answered or
  set aside). The list's needs-you marks don't read the hold.
  Enter on a decisions card's pick sends (beyond Go, which asked "Send
  these answers?" first): with several decisions it picks and moves on,
  and the pick that leaves none unanswered sends them all together
  (`answer_question` → `send_answers`, the combined `decision_answer`
  text). ⇧enter (`session.decide.edit`; alt+enter too, where the terminal
  can't tell it from enter, as ctrl+enter, the kitty keyboard in
  rush-draw) puts the answers in the box instead (`Act::Edit`, after any
  text there, a blank line between); enter, or a click, with text in the
  box does the same, so nothing typed is sent over or lost. The review
  page stays for ← and a digit to reach. The hint names the keys from the
  keymap (`Conn::q_edit_key`).
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
  thread as they open and as what's typed changes. Their text opens on
  the folder you're in, to edit (`Picker::path`, past Go): ↑↓ put the
  row chosen in it while the rows stay as last typed, tab completes as
  a shell does (a chosen row whole with a /, else what the rows under
  it share), and enter takes the text when it's been edited past the
  row. The send bar's folder fits the room the own worktree chip and
  the model leave it, its middle then its start going first. Files
  dropped onto the terminal go to the side the pointer was last over
  (`split::drop_paste`, past Go): a drag from another window leaves the
  terminal unfocused, so the pointer crossing hadn't moved the keys.
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
  In the conversation (`render::draw::chain`), a running chain's step row
  says its place (`2/3 cargo test`, runningPart), each command its time
  beside it (`Session::part_marks`, partMarks: ✓ how long, the spinner
  and how long for the one running now, ✗ where it failed), and under the
  commands a row of the keys: k skips the command running now, x stops the
  whole chain, p pauses (with "↑ pick this step, then" until it's picked;
  new in the Rust port, Go showed them only in the hint row). Code's check
  line and Trail's step line say the place too, Chat's bubble foot and
  Catch-up's "Still going" name it; Code shows the keys, Trail once the
  step is picked.
  A pipeline is one command of the chain (one place, one time; k and p
  act on all its stages together) but drawn split by pipe
  (`shell::pipeline_stages`): the stage doing the work on the command's
  line, the filters under it behind a faint `│`, in the opened call and in
  a shell box's command list. The row's "2/3" names the first stage only.
  How a call's shell is found (`chainrun::runs_call`): Claude Code runs
  each Bash call as `zsh -c "source <snapshot> … && eval '<command>' <
  /dev/null && pwd -P >| <cwd file>"`, a quote in the command written
  `'"'"'`; the eval's word is unquoted (`eval_arg`) and compared whole. A
  call rush's gate ran as a script is found by its `rush-script
  …/<call id>.sh`; one another hook rewrote goes to the eval shell that
  started soonest after it. Shells are looked for right under the agent's
  process and under a sandbox there (bwrap, Codex's), for any harness.
  What a line runs (`render::shell::run_text`) is past do/then/if/while/!
  and brackets, its `\` lines joined and comment gone; a comment, a loop's
  head, done or fi runs nothing, and has no place ("2/3" counts only those
  that run) and no time. A comment's quote opens nothing. A loop or an if
  is laid out as written, each line given its command's time by what it
  runs. A call that runs one script file (bash build.sh, ./build.sh,
  source env.sh) has its commands read off the processes right under the
  script's (through its own subshells): the row says "now cargo test",
  the opened call lists the latest six with their times, and k kills the
  one running now ("then": "script"). A chain or script seen running is
  drawn opened in Normal wherever it is in the turn (another call beside
  it, or sent to the background with steps after it), not only as the
  newest step. k, p, x and b act on any harness's shell call
  (`Kind::Shell`), and on one returned whose command runs on.
- **A running command's output, live** (`convo::model::liveout`,
  `render::draw::liveout`, `session_core::livestep`; new in the Rust
  port). The agent sees a command's output only when it ends; rush shows
  it as it comes, under the running shell step: its last 8 lines, dim,
  colours dropped, `▸ N more lines` above them (enter on it opens up to
  400, `▾ fewer lines` folds them), and the real result in their place
  once the step ends. One generic model, filled however the harness
  allows: `Event::CallOutput` (a call's output so far, or what follows)
  kept on the session per running step, the last 256 KiB
  (`LIVE_KEPT`), dropped as the step ends; Codex sends
  `item/commandExecution/outputDelta`, an ACP agent its running execute
  call's text content (not Kimi's, whose content is the input). For a
  harness that streams nothing, rush looks itself: Claude Code sends a
  foreground Bash call's stdout and stderr to a file it reads at the end
  (`…/tasks/<id>.output`), so the call's shell's fd 1, asked of the
  kernel (`rush_sys::proc::fd_file`, proc_pidfdinfo
  PROC_PIDFDVNODEPATHINFO), is a regular file read on from where the last
  look left off (`livestep::read_out`, every LIVE_EVERY with the "now ·"
  line). A pipe, a terminal or /dev/null is never read: taking from a
  pipe would take the output from the agent. No wrapper around the
  command is needed, so the agent sees exactly what it would have.
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
  (Providers, Harnesses, Config) and Settings (General, Usage, Keys,
  Plugins, Updates, Remote), one dialog between them as in Go, its tabs drawn
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
  beside the column, the views are its last part (`side_views`).
  **Stale views** (Rust-only; Go greyed by turns alone, and one long turn
  never ended, so a view from 3 h ago still read live). Why it wasn't
  greyed: `view_quiet` skips a view made in the turn still under way, and
  the host's put-away needs four ended turns. Now the clock counts too,
  in `rush_host::views`: untouched `STALE_MS` = 10 min (a build or
  benchmark between two updates is rarely longer; a longer wait is what
  the agent should say) the panel greys it, its foot reading "made by the
  agent · not updated · 3h" with the "not updated" part in the warning
  tone, and its busy marks (◐ ◉) drawn as quiet dots, so it doesn't
  pretend to be live; untouched `AWAY_MS` = 45 min the host puts it away
  among the past views, whether the session works or not, pinned ones
  aside. The host looks every minute (`VIEWS_EVERY`)
  only once a session works and while it keeps agent views, off the
  UI thread (`Host::sweep_views`, `views::sweep`). The first time a view
  is stale while the session works, the agent is told through rush's
  notice inbox (the hook output rush's other notices use, so only
  harnesses with an inbox), once per view per untouched period (keyed by
  the view's last update), never while idle or with views off.
  **Firmer at each turn's end** (2 Nov 2026, after "far too often the gen
  ui is not updated or no longer relevant"; 12 of 51 views were set once
  and never again): every limit and the rule live in
  `rush_host::viewstale`, plain functions tested by a table
  (`at_turn_end`, `by_clock`). When a turn ends the host
  (`views_at_turn_end`, off its thread) greys each agent view whose rows
  are all done ("all done": every row or item with a state good or
  ticked, bars full, none busy, failing or waiting; a view of plain facts
  isn't finished), one the whole turn left alone ("not updated for a
  turn"), and one the turn changed under it ("out of date": a `git push`,
  merge, rebase, commit or `gh pr merge` the host saw in the turn's shell
  calls, `change_of`, after the view's last set, and the view is of
  branches, PRs or CI or names what the command named). The reason is
  written into the view's file (`View::greyed`, its time left as the
  agent set it), which the panel's foot shows. A view still grey and
  untouched `AWAY_TURNS` = 2 turns later is put away (3 untouched turns,
  as before, counted from the grey). The clock greys the same way
  ("not updated"). Each grey and put-away is a `view-greyed` or
  `view-put-away` event (with what looked: "turn end" or "the clock") for
  `rush why`. The message that starts the agent's next turn ends with one
  line naming the grey views after a blank line, the same words but the
  names: `rush: stale views: prs, tests. Set each again with what's
  current, or clear it.` (about 20 tokens). It says it is rush's in plain
  words and never wears a harness's own tags (`<system-reminder>` is
  Claude Code's: an agent rightly distrusts a look-alike in a user's
  message). Never in the system prompt, which stays the same for the
  cache; the conversation leaves it out of your message
  (`transcript::STALE_VIEWS_LEAD`, only as the message's last
  paragraph). The agent's set clears the grey. **Views off** (Settings, Agent views;
  config `viewsOff`, on by default): the panel and list lines hide, the
  prompt part and the `view` MCP tool aren't given to agents started
  afterwards, a call is refused and no sweep or notice runs. The agent's
  instructions were trimmed: the system prompt part 1235 to 560
  characters, the tool's description and schema about 3.4k to 1.9k.
  **What the history showed** (2 Nov 2026: 934 Claude Code and Codex
  sessions that had the tool, read-only, by a throwaway script): 33 set
  a view, 257 sets in all (7.8 a session, at most 71; 1.5 views a
  session, at most 9). Kinds used: list 105, table 88, compare 44,
  checklist 41, steps 38, bars 30, callout 24, kv 18, board 7, stats 2,
  log 1; never chart, heatmap, timeline, tree, hero, code, text,
  progress. Agents get the shape right: no unknown fields or tones, and
  one failed call in all (`"trend": "up"`). What they miss: 29 of 33
  never ran `rush view guide` (1 before its first view), so the tool's
  description is the guide; 46 of 109 turns that set a view said 60% or
  more of its words in the reply too, and 6 of the 7 status asks that
  used the panel also put a table in the chat; 20% of row texts are over
  30 and 20% of metas over 20; 73 of 112 tables (median 53 columns
  wide) had a cell cut at 46 columns, 12 of them to under half; 12 of 51
  views were set once and never again. The user's words: "far too often
  the gen ui is not updated or no longer relevant", "prose duplicated on
  sidebar and in main view, hard to know where to look". So: the prompt
  part (908 characters with the stale and actions words, ~225 tokens) and the tool's description say the
  chat-or-view rule and the room (row text under 30, meta under 20,
  tables of 2 or 3 short columns); a view is read forgivingly
  (`viewparse::parse_view`: numbers as strings, words as numbers, one
  item for a list, tone and kind words like "success" or "stat", a
  string of JSON; what can't be meant, like a trend of "up", is left out
  and the agent told in the tool's answer), and what is still wrong
  names its field (`view.blocks[2].items[0].value`) with the smallest
  right block of that kind; a table's word cell too wide for its column
  wraps to a second line before it is cut; `rush view example` has
  branches, before-after and test-run.
  **Worked, not just read** (Rust-only, after "it's always a bit
  crammed… You can't scroll, you can't click"). The wheel scrolls the
  panel, and its bottom edge says "↑ n · ↓ n more" (the panel's own
  marker, Go's). Set again by the agent, a view keeps where you scrolled
  it; one that shrank keeps as much as it has, its end at the bottom.
  → from the box with nothing typed gives the panel the keys where it
  stands in place of the column (`beside::focus`, `ViewsState::take_focus`,
  as the column takes them): ↑↓ move a cursor over what a click does (a
  row, a heading, a row of buttons), drawn on the selection's ground as
  a picked row is; pgup and pgdn scroll a page; enter, space or → do what
  a click does (open a row: its detail and buttons; press a button);
  on a row of buttons ← → pick one, lit as under the pointer; ← or esc
  give the keys back, and any other key goes back to the box
  (`ViewsState::key`, `beside::focus_key`). The tool's description and
  `rush view guide` ask for each row's next step as an action (a branch:
  land it; a failing test: show the log), and the branches, test-run and
  before-after examples have them. A table narrower than it needs
  (its columns' widest cells and two spaces between) gives up its last
  column, then the next, down to two, before a cell is cut or wrapped;
  the header ends in a quiet "+n" where there's room, and a row opened
  shows every column's cell. The panel's goldens (`testdata/*.txt`,
  `testdata/wide/views.*`) are Rust's since; a test that differs prints
  the whole drawing to copy in.
  Its sheets, the editor, the panel and
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
  `reader.rs` LateOsc), which are applied then. They're asked again as the window comes back
  into focus and, on macOS, as the system goes light or dark (the
  global preferences file watched with kqueue, as the system's own
  notice reaches only a main thread's run loop): the next frame writes
  the query and the input thread takes the answers as late replies
  (`rush_draw::ask_colours_again`, rush-ui `appearance.rs`). Mode 2031
  reports aren't used: crossterm reads `CSI ?` until `u` or `c`, so one
  would swallow the keys after it.
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
  its rows laid out 79 wide at most (`side::text_width`). Its width is
  rush's own (three quarters of that room, 44 at least) until you set one:
  alt+←/→ in a Session, with nothing typed, make it two columns narrower
  or wider, between 44 and all the room left of the text, and keep it as
  `columnWidth` in config.json (`Config::column_width`, saved as the
  list's width is; `sidecol::side_width_with`); they do nothing while the
  column isn't showing, and jump a word while the box has text. shift+←/→
  move the list's edge, in a Session as in the list; alt+←/→ in the list
  move it too. The conversation's view stays on alt+s, and the hint under
  the box says so ("alt+s conversation view", from the key the keymap has
  for session.preset), the first of the hint's keys to go when the row is
  too narrow. An opened Session (`Model::session`, with the
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
  crate decodes PNG, JPEG, GIF and WebP, as Go does. Where the terminal
  takes Kitty graphics, your messages' pictures and the message sheet's
  are sharp (Go's termimg): the image scaled to the cells it takes, kept
  as a PNG in rush-draw (`kitty.rs`), and drawn as Unicode placeholders
  (U+10EEEE, row and column diacritics, the image's id its colour), so
  the rows cache, scroll, clip and diff as text does. rush-draw sends an
  image (`a=T,U=1`, PNG, chunked, quiet) the first frame one of its cells
  is written and never again: a warm frame with a picture on screen
  writes 0 bytes more than the same frame without it (kitty/tests.rs
  `a_picture_is_sent_once`). Sent again only after another program had
  the terminal; past 64 MB kept the oldest are let go and deleted
  (`a=d,d=I`). A placeholder whose colour isn't a picture rush has
  (faded behind a box or beside the list) is drawn blank, as Go's
  termimg.Blank. Whether the terminal takes them is asked as it's
  taken, with the colours: `ESC _G i=31,s=1,v=1,a=q,t=d,f=24;AAAA ST`
  before DA1, an `OK` before DA1's answer a yes. Not asked inside tmux
  or screen; an answer after the 50 ms wait (a slow, remote link) is
  taken in by the input thread and dropped, pictures staying blocks;
  `RUSH_GRAPHICS=kitty|blocks` says outright. The Pictures setting
  (pixelated) and a step's thumbnail are blocks always; the message
  sheet's s flips the one shown. A picture's cells link with a quiet
  link (`Link::quiet`): a click on one is rush's (it opens), never
  written as OSC 8, so the terminal neither underlines it nor offers
  its own menu; a right click on one does nothing. The cell is still
  taken as Go's 10×20 (Go asked CSI 16t; the terminal keeps the image's
  shape within its cells either way).

## Conversation views (Rust-only)

The Conversation tab has five views, one for every session, kept in
config.json as `convoView` ("" is Normal). Each answers one question:

- Normal: everything, as always.
- Chat, "what did we say?": your messages in bubbles on the right, the
  agent's words (markdown) in bubbles on the left, at most 3/5 of the
  width; no steps or output. Images are chips; open decisions, questions
  and waiting permissions are a line in the bubble. The last bubble's
  foot names the speaker, the time, the steps and files, and opens the
  turn in Normal.
- Code, "what did it change?": a heading a turn (its files, +a −d, or "no
  code changes"), each changed file's diff from the edits' own patches
  (the Normal diff's line numbers, highlighting and grounds), 12 rows a
  file until enter opens the whole diff, and a line for each check (tests,
  builds, linters) with its result, time, and "→ passed next run" when a
  later run of it passed.
- Catch-up, "what happened while I was away?": one card for every turn
  since you last looked at the session (`Options.since`), drawn with the
  last turn; the turns it covers before that draw nothing, and the turns
  before it a row each (enter opens them in Normal). The card: needs you
  first (open decisions, waiting permissions, what the last message
  asks) with `[answer]`, Done (each answer's first sentence, no model
  call), Changed, Checks, Said to you (the rest of the last answer),
  Still going (running steps and background tasks). Never looked: the
  last four turns. A turn is new when it did anything after that: its
  end, or its last step's when the end came over an hour later (a turn
  left open is ended when rush reads it). "Last looked" is kept per
  session in the UI
  (session-core conversation.rs `looked_since`): a look runs while the
  conversation is drawn frame after frame (`Model::frames`); one begun
  after a frame without it, 30 s or more later, starts from the end of
  the one before.
- Trail, "where did the time go?": what you said, then a line a step on a
  time axis, steps of a kind quicker than 10 s side by side merged ("read
  4 files"), a bar as long as the step against the turn's longest, the
  slowest marked when it took 10 s and 30% of the turn, the reply's first
  sentence, and a line splitting the turn into thinking, tools (their
  spans, overlaps once) and waiting on you.

⌥s (session.preset, also ctrl+] g) goes through them; the tab row names
one other than Normal and a click on it opens a picker; enter or a click
on a turn or a step in any of them opens it in Normal. Changing view
keeps the picked row, else the turn at the top.

They are `Options.preset` (rush_convo::Preset), part of the renderer's
signature (and `since` with Catch-up): the same parsed turns, drawn a turn
at a time (render/draw/presets.rs), so only the turns in view are drawn
whichever is picked. Catch-up's first turn is counted again as turns come
(`Renderer.catch`), and its last turn's key carries the background tasks
it says are going. Checks are read from the command's words (cargo, go,
npm/pnpm/yarn/bun scripts, make, pytest, eslint, ruff, tsc, …) and its
exit code, never from one harness's own result shape.

`cargo bench -p rush-convo --bench render`, 3000 turns at 120 wide
(µs per frame):

| frame | Normal | Chat | Code | Catch-up | Trail |
|---|---|---|---|---|---|
| cold, at the end | 780 | 559 | 192 | 235 | 173 |
| warm, at the end | 4.3 | 5.8 | 2.2 | 18.5 | 4.3 |
| warm, mid-way | 4.0 | 5.5 | 1.9 | 18.4 | 4.1 |

Cold frames swing with the machine's load (a run under heavier load had
Code's at 3151 µs and Normal's main-bench one at 2925 µs); warm frames are
the steady cost. Catch-up's window holds ~50 turns of one row each before
the card, so its warm frame checks 50 turns' keys: still far under a
millisecond.

### Which view fits (Rust-only, off by default)

Settings › Interface › "Suggest a conversation view" (config.json
`viewSuggest`: "" off, "suggest", "switch" for "switch when I come
back"). Off, nothing runs: no per-session record, no question, no file.
On, rush looks only at two moments: you come back to a session (open it,
or look again after 10 minutes away or with finished turns you haven't
seen; a glance at the list and back isn't one), or a turn ends while it's
open. A moment waits while a key or the wheel moved in the last 3 s.

The rules (rush_convo::suggest, a pure function of a digest of counts
from the last finished turn and the time away) pick first: back with
unseen finished turns → Catch-up; files changed and checks run (the
Code view's check detection) → Code; a turn of 2 minutes or more mostly
in tools → Trail; mostly talk, two steps or fewer → Chat; a short turn
that changed nothing → Normal; anything else is unclear. Only then is the
session's harness's quick model asked, through the advisor's Ask (the
adapter's Querier, headless), off the UI thread, at most once per session
in 10 minutes, 30 s at most, with the digest's counts, the view in use
and the last views chosen, never the conversation's words; an answer that
fails or can't be read is no suggestion.

Suggest marks the view's tab with an orange ✦ and the line under the box
says `tab: Code fits · changed 4 files, ran tests  ⌥t take it`
(session.take_view_suggestion, ⌥t or ctrl+] T). Switch changes the view
only as you come back, never when a turn ends, and says so with ⌥t going
back. A suggestion left showing until the next moment, or another view
chosen, is turned down; per moment and view (`back:catch-up`) rush keeps
how often each was taken and turned down in view-suggest.json in rush's
folder, and after three turned down in a row it isn't made again (one
taken starts the count over). session-core viewsuggest.rs.

## Shell commands rush runs for you (Rust-only)

Two ways in, both only on your click or key, never by themselves:

- **A shell block in a reply** (```sh, bash, zsh, shell, console, or
  one whose first line is a `$ ` prompt) is drawn as a terminal box
  (rush-convo `render/draw/shellbox.rs`): `shell · <folder>` and ▶ run, ✎ edit,
  ⧉ copy on its top edge, the commands inside with a faint `$` (one the
  agent wrote is taken off; in a console block only the `$` lines are
  commands, the rest is example output and doesn't run). Its ref is
  `t<n>:sh:<item>:<k>` (the k-th shell block of item i of turn n,
  `shellbox::shell_blocks` finding them as the markdown does); picked,
  enter runs it, c copies it.
- **`!` typed into the empty message box** turns it to running commands
  (`Conn::bang`): its edge orange, `! shell · <folder>` and `enter runs ·
  esc back` on it; esc, or backspace at its start, turns it back. enter
  runs what's typed without waiting for the agent or interrupting it; its
  box (`t<n>:bang:<start>`) is drawn after the items its turn had then.
  In a session with no turn yet it's turn 0's (`t0:bang:<start>`): the
  Renderer draws those boxes above the first turn (`pre`, counted with
  the header rows), so it's the whole conversation until a turn comes.

The boxes are in every view (presets.rs), the same box and keys:

- **Chat**: a reply is cut at its shell blocks (`shellbox::pieces`), each
  block a box in its place between its bubbles (the bubble before it
  ends there, as a shown drawing's does); a `!` run is a box on your
  side, right-aligned, where it ran. The last bubble of words carries
  the turn's foot.
- **Code**: the commands you ran in the turn (▶ run or `!`) are lines
  with a check's look (✓/✗/✕/✱, `$ command`, its code, its time);
  enter opens its box under its line. A block not run is its box, with
  ▶ run, under the turn's changes.
- **Catch-up**: "You ran" lines for what you ran since you last looked
  (the newest 4), enter opening one's box under the card; one running
  is under "Still going".
- **Trail**: a run is a `you ran` line with its time bar, where it
  started among the steps (never the "slow" one).

A run (`rush_convo::ShellRun`, kept by the UI in session_core's
`shellbox.rs` and handed to the view as `Options::shells`) is the
session's folder, `$SHELL -lc`, stdin closed, run by a small `/bin/sh`
runner (`RUNNER`) in a session of its own (setsid), so rush quitting
doesn't end it. stdout and stderr go to one file,
`shell-runs/<session>/<start>-<ref>.log`; as the command ends the runner
writes its code beside it (`.exit`, by way of a `.part` file), and
traps TERM, INT and HUP until then so ✕ still has the code written. A
thread follows the file (every 50 ms) until the exit file is there or
the process is gone, and each frame's dock takes in what it has. Its box shows
the newest 12 rows of what it wrote, colours kept (SGR read into
spans), "… N more lines" above them; enter opens all of it. The foot says
how long it has run (✕ stop ends the group: TERM, KILL 3s on), then ✓ or
✗ and its code and time, with → send, ✎ draft, ≈ summary, ↻ again and ✕ (let go of
it). A block of several commands is a chain: its processes are read every
500 ms (`jobprocs::kids`) and fed to the same `Chain` an agent's Bash
calls use (chainrun.rs, now a type of its own), so each command says how
long it ran, the one running now is marked, and k (skip it) and p (pause
it, kept with the other paused commands that rush lets go on as it quits)
work as on an agent's.

What it printed stays on your screen. A block's run whose output is
short (`ShellRun::sends_itself`: 20 lines and 2000 characters at most,
none let go) and that ended by itself (not stopped, nor while rush was
closed) goes to the agent as it ends, as → send would, with a flash
saying so; Settings › Message box › "Short output of a command you run"
(`holdShortOutput`) has it wait instead. A `!` run never goes by
itself. Otherwise nothing goes to the agent until you pick one of the
three on a finished box's foot (their words shorten as the box narrows:
`→ send to agent`, `→ send`, `→`):

- **→ send** (s) sends "I ran `cmd` (exit 0):" and its last 80 lines
  (6000 characters at most) to the agent at once, as if typed and sent
  with enter: queued while it works, as the send mode says; what you
  were typing stays in the box. The foot then says `✓ sent`.
- **✎ draft** (d) puts the same text in the message box, after what's
  typed there, to change and send or not (`✓ in your message`).
- **≈ summary** (m) has the harness's quick model (the advisor's `Ask`,
  as view suggestions use, off the UI thread, 45 s at most) sum up its
  last 600 lines: whether it passed, each error and failing line with
  its path, counts (`SUMMARY_SYSTEM`). The summary goes to the agent at
  once under "A summary of a command I ran, `cmd` (exit 1):". While it
  runs the button says `≈ summing up…`; when it fails the box says why
  in red and nothing is sent.

Each goes through the session's own harness like any message.

**✎ edit** (e), on a box's top edge before it's run and after, turns the
box into an editor of its commands (`Options::shell_edit`, drawn by
`edit_box`): the message box's editing keys and undo, ⇧enter (alt+enter,
ctrl+j) a new line, ↑↓ between lines; enter runs the commands as they
stand, esc leaves them as they were. A run of edited commands is marked
(`ShellRun::edited`, `shell · edited` on its edge): the box shows, sends
and runs again (↻) what was edited; the agent's reply keeps what it
wrote. The runs are kept in rush's folder, `shell-runs/<session key>.json` (the newest 50), as
each starts and ends: a running one with its process (the runner's pid
and start time) and its file, not its output. Read back when the
session is opened again (`pick_up`), one that was running as rush went
is: still running when that pid is there with that start time (within
a second), its output read from the end of its file (the last 256 KiB,
the lines before counted) and followed on; done when its exit file is
there, with that code; else "ended while rush was closed". ✕ ends its
process group as before. A finished run's output is kept in the JSON
and its files go once that's written. Tests stand a fake in for the
shell (`shellbox::set_fake`), which also keeps nothing on disk.

## The context guard (Rust-only)

A bundled plugin, `context-guard` (rush-bundled `contextguard/`), on by
default, that steps into an agent's tool calls through its hooks to keep
what goes into its context lean. Four rules, each its own setting, all on
by default, each with a number you can set (Settings › Usage › Context
guard; Settings › Plugins › context-guard turns the plugin on or off and
says where its rules are, not showing them twice). Each step-in is
logged in the session's events (`hook-denied` with `rule`, `why`,
`tool`, `path` or `command`; `hook-squeezed` with `before`, `after`,
`cut`), so `rush why <session>` and `rush events -k hook-denied` show
what rush did and why.

```
  agent ── Read ──▶ PreToolUse ─┐                     ┌─ deny + why
  agent ── Bash ──▶ PreToolUse ─┼─▶ rush gate hook ───┤
  (ran) ──────────▶ PostToolUse ┤   context guard     ├─ output squeezed
  (failed) ───────▶ ...Failure ─┤   first, then the   └─ nothing
  compact/clear ──▶ PreCompact, ┘   search guard, gate
                    SessionStart
```

How. `StartOptions.tool_hook` (rush-core) is a command for rush's
context guard; the host sets it to `rush gate hook`, the gate's own
command. A harness with hooks installs it: Claude Code's
`conn::context_hooks` adds PreToolUse `Read`, PostToolUse
`Read|Bash|Edit|Write|MultiEdit|NotebookEdit`, PostToolUseFailure `Bash`,
PreCompact `*` and SessionStart `compact|clear`, beside the inbox's
hooks; before a Bash call the gate's own PreToolUse hook asks it.
`gate_hook` hands every input to `Host::context_step` first; the guard's
answer wins, and hooks after a call are never the gate's to rewrite.
After a Bash call the guard is given the command the agent asked for
(`hook::original`: the gate's wrap undone, a script run read back from
its script), not what ran in its place. The rules are pure functions
(`contextguard::rules`, `squeeze`); `contextguard::hook` reads the file
and the session's state (`<rush home>/context-guard/<session>.json`,
flock'd, as hooks for calls made at once run at once; let go after a week
untouched). The guard reads its switches and numbers at every hook, so a
change works at once, in running sessions too.

The settings are the plugin's own (`rush_plugin` settings.json, by its
manifest's keys), so the hook needs nothing else; `contextguard::rules_of`
turns them into `Rules`, which carries each rule's number to the pure
functions (`rules::read`, `rules::bash`, `squeeze::cap`). Unset, or a
value its choices don't allow, is today's number:

| row | key | choices | default |
|---|---|---|---|
| Don't re-read unchanged files | `reread` | on, off | on |
| ‧ smallest file it applies to | `reread-kb` | 1, 2, 4, 8 KB | 2 KB |
| Don't read huge files whole | `big-read` | on, off | on |
| ‧ huge means over | `big-read-lines` | 500, 1,000, 2,000 lines (24, 48, 96 KB) | 1,000 lines or 48 KB |
| Stop repeated failures | `repeat-fail` | on, off | on |
| ‧ after | `repeat-after` | 2, 3, 5 in a row | 3 |
| Trim command output | `squeeze` | on, off | on |
| ‧ longer than | `squeeze-lines` | 300, 600, 1,200 lines (a third kept at each end) | 600: first and last 200 |

The numbers that stay constants: a read with no limit covers 2,000 lines
(Claude Code's Read), a failure's last 12 lines say whether two were the
same, a squeeze is logged once it saved 1 KB.

What it keeps is per agent: the main conversation, or a subagent by its
`agent_id`, as a subagent's context is its own.

**1. No re-reading unchanged files** (`reread`). A Read of a file the
same agent already read, the same lines or more (a whole read, or a
same-or-larger offset+limit), with the file unchanged since (size and
mtime, and an FNV-1a hash of its content when it's 256 KB or less), is
refused: `context-guard: <path> is unchanged since you read it at
14:03:12 (all of it). Use what that read gave you…`. Never for a file
changed since, one the agent edited since (any Edit, Write, MultiEdit,
NotebookEdit forgets the read), after a compaction (PreCompact forgets
that agent's reads; SessionStart `compact` everyone's, `clear` all), for
a file under 2 KB (refusing costs about what reading does), or for an
image, PDF or notebook. A read is taken note of after it went through
(PostToolUse), so one refused by anything else isn't counted. A read
with no limit covers 2,000 lines (Claude Code's Read), the file's end
when it has fewer.

Data (1,500 recent Claude Code transcripts on the machine rush is worked
on, 19,611 reads): 40% of what reads put in context was a whole file
read again by the same transcript with no edit of it between (by
the transcript alone: some of those files had changed on disk, which the
rule checks); 39% of whole reads were of files under 2 KB. Claude Code
2.1.316 itself already answers a read of exactly the same offset and
limit of an unchanged file with a short "File unchanged" stub; the guard
adds a smaller range inside one read before, works per subagent, and
works for any harness that runs hooks.

**2. No reading huge files whole** (`big-read`). A Read with no offset
and no limit of a text file over **1,000 lines or 48 KB** is refused once
per file per agent, steering to `rg -n <pattern> <path>` and
offset/limit; asked again, the whole read goes through. Images, PDFs and
notebooks (by name, or a NUL in the first 8 KB) are left alone. Data
(the same transcripts): whole reads' p50 is 83 lines (2.9 KB), p95 611
lines (20 KB), p99 853 lines (36 KB); reads over 1,000 lines are 0.5% of
whole reads but 4.1% of all read text, over 50 KB 3.2%. Claude Code's own
Read stops at 2,000 lines and refuses past 25k tokens, so the guard's
line sits well under where the harness steps in.

**3. Stopping repeat failures** (`repeat-fail`). Each agent's last shell
command and how it ended: PostToolUse is a success, PostToolUseFailure a
failure whose exit (`Exit code N`, else "error") and output's end (its
last 12 non-blank lines, terminal codes out, digits as 0, spaces single,
so timings and ids don't count) are kept. The same command (words
single-spaced) failing the same way **3 times in a row**, with no edit in
between, has its 4th run refused: `context-guard: this command failed
the same way 3 times in a row (exit 1, the same output)… change something
first`. Any edit, a success, another command, or another outcome starts
over; an interrupt counts for nothing. Asked again straight after the
refusal, it runs (something may have changed that rush can't see). Data:
of 131,735 Bash calls, 7 were the 4th or later identical failure in a
row: rare, but each is a turn spent for nothing.

**4. Squeezing command output** (`squeeze`). Claude Code 2.1.316's
PostToolUse hook may answer `updatedToolOutput`, which "replaces the tool
output before it is sent to the model" for any tool, checked against the
tool's output shape (a mismatch keeps the original). So a Bash call runs
exactly as asked (its exit code, pipes and redirects untouched) and only
what the agent reads changes: `stdout` and `stderr` replaced, every other
field kept. A failed call (PostToolUseFailure) can't be replaced and is
left as it is. What `squeeze` does, deterministic, and twice is the same
as once:

- terminal codes out (CSI colours and cursor moves, OSC titles, charset
  switches);
- a carriage-return redraw (progress bars, spinners) down to its last
  state; CRLF to LF;
- spaces at line ends out; runs of blank lines made one, none at the
  start or end;
- a line repeated in a row written once with ` (×N)`, where that's
  shorter;
- past **600 lines**, the first and last **200** kept around `… K lines
  left out (the whole output: <path>) …`, the whole (squeezed) output in
  `$TMPDIR/rush-output/<tool_use_id>.out.txt` (the session's scratch
  folder), for the agent to read if it needs it.

Never squeezed: output that's a file's own text or a patch, whose exact
spaces an Edit needs (`cat`, `head`, `tail`, `sed`, `awk`, `nl`, `diff`,
`jq`, `grep`/`rg`/`ag`, `cut`, `tr`, `printf`…, `git
diff/show/blame/cat-file/grep`, `git log -p`, `git stash show`, anywhere
in the command), an image, output Claude Code already kept in a file
(`persistedOutputPath`), a background task's, and never a Read: Read
results are never touched. A squeeze is logged when it saved 1 KB or cut.

Measured (`squeeze::tests::measured_on_fixtures`, testdata/; tokens as
bytes / 4):

| output | bytes before | after | tokens before | after | saved |
|---|---|---|---|---|---|
| cargo build (105 crates, a progress bar, 12 warnings) | 16,168 | 5,542 | 4,042 | 1,386 | 66% |
| cargo test (240 tests, 1 failing) | 11,815 | 9,618 | 2,954 | 2,405 | 19% |
| npm install (spinner, deprecation warnings) | 34,414 | 325 | 8,604 | 82 | 99% |
| git log --oneline --color (60 commits) | 3,153 | 2,646 | 789 | 662 | 16% |
| pytest (412 tests, coloured) | 1,755 | 1,397 | 439 | 350 | 20% |
| a download's progress bar (curl -#) | 80,081 | 80 | 20,021 | 20 | 99.9% |

### What rush adds to an agent's context, word for word (Rust-only)

A provider caches the prompt in order: tools, system prompt, then the
conversation. A part that changes makes everything after it be written
again (1.25× or 2× the input price, against 0.1× for a read). The cached
parts are built as an agent's program starts (a new session, a wake from
sleep, a move to another account or build, a retry), so they must come
out the same word for word every time for the same session. Text added at
the end of the conversation (a tool's result, a hook's context, a
message) can't change what came before it, so times and counts there are
fine.

The cached parts, and what's done to keep them stable:

- **The system prompt rush adds** (`server::system_prompt`): constants
  (the task list, scratch folder, decisions, shell boxes, the agents
  note), then the session's own prompt, then the parts a setting or a
  move changes, last: views (Settings), the feed (Settings), the
  worktree note (`workspace_note`). It reads no clock and no state;
  `system_prompt_is_word_for_word_stable` builds it twice and with each
  changing part on and off and checks the rest stays put.
- **spawn_agent's list of agents** (its tool description, always loaded)
  and **Pi's list of agents in its prompt** (`agents_prompt`): no longer
  say whether each is signed in ("Not available right now", "availability
  unverified, so don't run it"). Sign-ins come and go between starts;
  spawn_agent says "needs its account checked" when it's called, in the
  turn. `cached_parts_ignore_sign_ins` builds each with every agent
  signed in and with none, byte for byte the same.
- **The session's own subagent types** (`--agents`, `own_def`): no
  longer left out while signed out; the session runs them on its own
  sign-in.
- Still able to change between starts, by what you do: which agents and
  models Settings › Config allows, programs installed or removed, a
  model list a harness keeps (Codex's cache, `ollama list`, an
  Antigravity snapshot older than a day), turning views or the feed on
  or off, a session moved to another worktree. JSON maps are written in
  key order (serde_json without `preserve_order`; `agent_defs` is a
  BTreeMap), so no part depends on a hash map's order.

At the end of the conversation (fine as they are): the inbox's
messages and rush's notices (`inbox.rs` `TELL_NOTE`, `NOTICE_NOTE`: a
view clicked, views gone stale, a stuck command stopped, the gate
stopping or pausing a run), reminders (their id and times), the away
check-in, long background tasks, a child agent's answer, revive, retry
and limit messages, pull request news, room turns, rush's own tools'
results (show, view, spawn_agent, agent_result, remind), the gate's
waiting line in a command's output, and the hooks' refusals (runaway
guard, search guard, watch, context guard) and squeezed output.

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

To check, run the tests under `sandbox-exec` with `tools/rush-tests.sb`
(through `tools/sbtest.sh cargo test -p <crate>`), a profile that denies
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
- `cargo bench -p rush-convo --bench jobs`: the dock's scan of a
  session's tasks for each run it shows, each frame, with
  `Session::job_kind` as it is and as it was (the call looked up for
  every task).
- `cargo test -p rush-ui --test threads`: the headless view's threads,
  and an idle minute's wakes and frames ("Threads").
- `cargo bench -p rush-convo --bench search` (RUSH_BENCH_TRANSCRIPT=<a
  transcript>): an open conversation searched as the command bar does on
  each key, letter by letter: Session::search's instructions per key and
  a digest of its hits, then Session::search_top (the bar's This session
  and This chat: only the hits a section shows, with a memo kept from key
  to key and a vocabulary of the words typos are looked for among, made
  on another thread as the bar opens) checked hit for hit against it.
- A run followed while it writes (Rust-only): the Subagents tab's picked
  run woke a reading of every run on each write, many a second
  (rush-ui-session-core `subfollow.rs`); now what it writes in the next
  150 ms comes in one reading. A run read whole for the UI (an agent
  rush hosted for the session, or every run while Changes is open) was
  read again on every write, and as the UI holds the last reading each
  one copied the whole conversation first; it's now read again once a
  second at most (`subs.rs` FULL_EVERY). A transcript's tail reads into
  a buffer the size of what it gained rather than 64 KB a time, a list
  of runs no longer copies each run's meta twice to list it, and the
  pages of runs let go are given back to the system. `cargo bench -p
  rush-convo --bench follow` (a run of 2,642 lines, 18 MiB, written ten
  a second, 2026-11-02): read on each write 233 → 84 MiB allocated in
  36,866 allocations; read whole and shared each write 6,280,862
  allocations and 2,011 MiB, once a second 661,372 and 303 MiB.
- A drawn run's text (`rush_core::text::Span::text`) is a
  `CompactString` (Rust-only): up to 24 bytes held in place, so a glyph,
  a word or a time is made and copied without the heap, and a turn's
  memoized rows copied into a frame copy only their longer runs.
  2026-11-02, the render bench (300 turns, w120): a warm frame 116 → 75
  allocations, a cold frame at the end 1,775 → 1,396, laying out all
  turns 249,038 → 203,859 (3,000 turns 3.39 → 2.94 million), time and
  rows kept the same; the frame bench's Agents list 3,321 → 2,522
  allocations a frame, a key then the frame 3,702 → 2,897, with ctrl+k's
  bar open 5,504 → 3,814. What a warm frame still makes is the live
  turn drawn again for its clock: each new line's list of runs, the
  memoized rows' lists, and the times it formats.
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
- The open bench also counts allocations made and still held, splits the
  retained heap (steps, calls, raw input, output, turns, requests), prints
  the hot types' sizes, and reads the transcript's subagent runs whole as
  the session view's subagents reader does. Measured 2026-11-02, before →
  after the data-layout changes: Item 56 → 40 B; the 119 MiB transcript
  makes 244,187 → 235,155 allocations and keeps 44,628 → 41,487 of them
  (12.7 → 12.6 MiB); the 19.5 MiB one 61,608 → 59,801 and 11,428 → 10,821.
  Its subagent runs are the bigger part: 45 runs retain 20.4 MiB, another
  session's 221 runs 82 MiB, so the reader lets go of a run once its numbers
  aren't followed (rush-ui-session-core `subs.rs`).
- A step's call id is held once, in the step (Rust-only, rush-convo
  `model/byid.rs`): its call's copy is taken into it, and the index of
  steps by id holds each step's number only, finding it by the id the
  step holds. 2026-11-02: the 119 MiB transcript 12.6 → 12.3 MiB, its
  allocations kept 41,499 → 35,581 and made 235,169 → 229,251; the
  68 MiB one 9.7 → 9.5 MiB and 32,823 → 28,189 kept.
- A Claude Code call's input is held once (Rust-only, rush-convo
  `model/lean.rs`): its raw JSON says again what rush read from it (the
  command, a file's contents, an edit's old and new text, the prompt),
  so the step keeps the JSON with each such string cut out, a byte no
  JSON text holds in its place naming the call's field that goes there,
  and puts it back together when asked (`Step::input_json`, now a Cow;
  `Step::input_len` for its length without it). A string is cut only
  where the field written as serde_json writes it is the very bytes the
  agent sent, so what's put back is the JSON as sent. 2026-11-02: the
  119 MiB transcript 12.3 → 10.6 MiB (raw input held 1.9 → 0.0 MiB, the
  allocations kept the same), reading it 196 → 205 ms and 229,251 →
  237,647 allocations made; the 68 MiB one 9.5 → 8.0 MiB, 120 → 125 ms.
- A followed run's numbers (Rust-only, rush-ui-session-core `subs.rs`
  `follow_run`): the reader reads a run's transcript whole, works out its
  numbers (the row's words, steps, times, tokens and cost), and keeps its
  conversation read on only while the run works (written in the last
  90 s, or a call of its out). A quiet one keeps its numbers, its
  transcript's size and its calls' ids hashed (for which background
  tasks it started); it's read whole again only once it grows. The run
  picked in the Subagents tab is read whole on its own, and let go when
  unpicked. The open bench's "followed as the reader keeps them", with
  every run followed (the Subagents tab open), 2026-11-02: the 119 MiB
  transcript's 45 runs 20.4 → 0.43 MiB (one written lately, kept read
  on), 1,103 allocations kept; the 221 runs 82.0 → 0.19 MiB in 1,527,
  a peak of 2.7 MiB while reading.
- `cargo bench -p rush-host --bench list` (`RUSH_REAL=1` for this home,
  read only; its lister follows, never reviving a host): the list's
  loader and a reading of it, the rows' and the loader's retained heap
  and allocations, and the row types' sizes. `RUSH_BENCH_PAUSE=1` holds
  it open for `heap` and `malloc_history`. Measured 2026-11-02 on a home
  of 774 rows, the loader alone retains 23.2 MiB in 175,376 allocations
  before, 16.6 MiB in 127,607 after: Agent 1616 → 1280 B (its PR and
  workspace boxed), Totals 480 → 448 B and Spend 320 → 304 B with their
  model and folder names shared (`rush_core::name::Name`: 16 bytes, a
  static str for the tool, role, level and media-type words known ahead,
  one shared copy for the rest) 23.2 → 21.0 MiB; the host lister keeping
  each session not awake as its HostInfo only, not its Info as well,
  21.0 → 18.0 MiB and 156,080 → 132,197 allocations; a Name in 16 bytes
  rather than 24, 18.0 → 17.8 MiB; the spend scanner holding each
  transcript's path once, 17.8 → 16.6 MiB and 132,091 → 127,607
  allocations. The subagent runs' follower (rush-claude `subruns.rs`)
  keeping each run once, in its meta files' order, by its id (the meta
  file's path made from it when it's read, not held beside it as its
  key and in the list), and no longer copying the list on every look:
  16.6 → 14.5 MiB and 127,680 → 120,318 allocations kept; a reading
  makes 78,768 → 60,326. The spend scanner keeping, of a transcript
  whose session isn't live, its stored price alone (the cost cache's
  totals: what it cost, and how far it was read and how big it was then),
  not its files' list and sizes as well: a scan looks at its files only
  when its transcript or its subagents' folders changed, then each whose
  size isn't the one stored is read on from where it was, and the stored
  prices are never dropped. The bench's figures with a second scan timed
  (the list makes one each reading): loader 14.5 → 13.7 MiB and 120,449
  → 117,121 allocations kept; the first scan 45 → 40 ms, a scan again
  7.8 → 5.9 ms. The loader's memo of idle rush sessions' rows (rush-fleet
  `IdleRow`) checking what a row was made from by a fingerprint of it,
  not by its profile, session, folder and state held again beside the
  row: 13.7 → 13.4 MiB and 117,209 → 110,184 allocations kept. The rows
  themselves are the rest of it (about 2.3 KiB a row).
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

## Background processes' memory (Rust-only)

What rush keeps running beside the view, measured 2026-11-02 on the
installed rush (0.1.0-beta.5; physical footprint, macOS 27, Apple
silicon) on a home of 1,244 rush sessions (987 asleep or stopped) and
about 900 rows in the list:

| process                         | footprint | threads |
|---------------------------------|-----------|---------|
| `rush menubar feed`             | 31 MB     | 5       |
| `rush serve`                    | 12 MB     | 11      |
| `rush host run <id>`, each      | 5–6 MB    | 11–13   |
| `rush watch run`                | 3 MB      | 2       |

- **The floor.** Any rush process is about 3 MB (`rush watch run`): the
  binary's own data and page tables. It links AppKit, AVFoundation,
  Speech and the rest for the menu bar app and push-to-talk; a C program
  linking the same frameworks sits at 1.9 MB idle against 1.7 MB with only
  CoreFoundation, CoreServices and IOKit, so a separate small binary for
  the background commands would save about 0.3 MB a process.
- **Thread stacks** are not where the memory is: pages a thread never
  touched aren't footprint. The feed's 5 stacks hold 128 KB, a host's 11
  hold 400 KB.
- **The feed** runs the list's own loader (rush-fleet), so it holds what
  the list holds of every row. `MallocStackLogging=lite` and
  `malloc_history -allBySize` on a second feed: ~40 MB live at its peak,
  ~12 MB of it rows of rush's sessions (the loader's idle rows, the whole
  reading's table of those asleep or stopped, and the rows made from
  them), 4.5 MB their infos as listed (rush-host's `Info` and the fleet's
  `HostInfo`, each session twice), 4.3 MB the spend scanner's state per
  transcript, 3.7 MB subagent runs counted.

What changed, each with its number:

- **A whole reading's idle table is boxed** (rush-fleet `partial.rs`
  `IdleEntry`): a slot held a session's info and row inline, 2,392 bytes,
  and 987 sessions take 2,048 slots, a 4.9 MB table. Boxed, a slot is 32
  bytes: 66 KB of table and 987 boxes, 2.6 MB; 2.3 MB less in the feed
  and in the list (`whole_reading_boxes_its_idle_rows`).
- **The feed keeps only the rows it can show** (rush-menubar
  `source::live_rows`), those working or waiting on you, the only ones the
  menu reads, and lets go of the reading before it waits: 900 rows, 5
  working, held 496,690 B, now 3,494 B (`tests/retained.rs`, run without
  the harness so it prints).
- **serve's notification watch lets go of its listing before it sleeps**
  (rush-remote `watch.rs`): every session's info, read every 4 seconds,
  was held through the sleep: 1,244 sessions, 2,378,662 B
  (`tests/listing.rs`).
- **The feed leaves past conversations out** (rush-menubar
  `FleetSource::from_state`, the loader's skip_past), decided
  2026-11-02: a headless run linked to a closed conversation is a working
  row of its own rather than folded into that conversation's, and
  Copilot's cloud sessions, listed among the past ones, no longer count
  as live. The list bench's loader read as the feed reads, on a home of
  774 rows nearly all rush's own sessions: 13.7 → 13.0 MiB retained,
  117,209 → 112,921 allocations; a home with more past conversations
  outside rush saves more.

Found, not done:

- A reading of rush's sessions that are awake alone: the feed still
  reads every rush session's info, asleep or stopped, to show the few
  working.
- Each rush session's info is held twice, as rush-host's `Info` and the
  fleet's `HostInfo` (4.5 MB here); `HostInfo` could share `Info`.
- serve's watch reads all 1,244 info files every 4 seconds; a kept Lister
  would read again only the changed ones (CPU), for ~2 MB of infos kept.
- A host's heap is mostly its replay (`RING_MAX`, 8 MB at most); its tmp
  trim sleeps on a thread of its own that a timer could replace (left to
  the work on threads).

## Footprint watch (Rust-only)

`.github/workflows/footprint.yml` measures rush after every push to main,
nightly (what changes outside rush: the runner, macOS) and by hand, so
memory and CPU creeping up as features land is seen the day it lands. A
report, not a gate: no number fails a build. Its scripts are in
`tools/footprint`.

What it measures, on a made-up home (`fixture.py`, a fixed seed: 241
Claude Code transcripts in 12 projects, one of 10 MB with 6 subagent runs,
and 400 stopped rush sessions; RUSH_REAL's real home is never read in CI):

- **Counts from the counting allocator**, which a busy runner doesn't
  change: the benches and tests above write theirs to `RUSH_BENCH_OUT`
  (a line each: name, kind, value). rush-host `--bench list` on that home
  (retained heap, allocations made and kept), rush-convo `--bench open` on
  its largest transcript (its subagent runs read whole and as the reader
  follows them), `--bench render` and `--bench follow` (its made-up run),
  rush-host's list again with past conversations left out (the menu bar
  feed's reading), rush-ui `--bench frame`
  (bytes and allocations per frame), rush-menubar `tests/retained.rs` and
  rush-remote `tests/listing.rs`.
- **Each long-lived process, idle** (`measure.py`), started in that home
  with nothing of the runner's (no RUSH_*/CLAUDE_* variables or tokens,
  PATH the system's and a fake `claude` that answers stream-json): the
  plugin broker (ready when its socket answers), `rush serve` (when it
  says where it listens), a host whose agent is the fake claude (when its
  first turn has ended: `rush session watch --until-idle`), `rush menubar
  feed` (its first reading) and the view with no terminal (`rush --soak`).
  Once each has used under 20 ms of CPU a second for 3 seconds running,
  60 seconds idle give its CPU (user and system, proc_pid_rusage) and its
  wake-ups (package idle and interrupt), each per minute; then its
  physical footprint (`/usr/bin/footprint`) and its threads (the fewest
  proc_pidinfo saw, sampled every 5 seconds: the long-lived ones).
- **The release binary's size.**

All of it is one `perf.json` (with the commit), compared two ways: with the
last main commit measured (the job summary's table, every run, before →
after for every number) and with the **baseline**, the numbers held to. A
number's baseline moves down when it gets better past its threshold, and
never up by itself, so a creep of 2% a commit is caught when the sum
passes the threshold. Thresholds, both parts needed:

| kind                         | past it when it rises by |
|------------------------------|--------------------------|
| allocations                  | 5%                       |
| bytes allocated (per frame)  | 5% and 1 KiB             |
| retained heap                | 5% and 256 KiB           |
| footprint                    | 10% and 1 MiB            |
| threads                      | any                      |
| idle CPU                     | 50% and 0.5 s a minute   |
| idle wake-ups                | 50% and 60 a minute      |
| binary size                  | 5%                       |

On main, a number past its threshold opens an issue, "Footprint went up:
…", labelled `performance`, or comments on the one this job has open and
updates its body; the run that finds them all back under comments so and
closes it. Closing it by hand (or running the workflow with `accept`)
takes the current numbers as the baseline. Each run's numbers are kept on
the `perf-history` branch: `runs/<time>-<commit>.json`, `latest.json`,
`baseline.json`. A run on another branch compares with main's and writes
only its summary. Runs on main wait for each other rather than cut one
short; GitHub keeps one waiting, so two pushes close together are measured
as one, and the comparison names the commits it spans.

## Milestones (full plan: PLAN.md)

1. **Read-only Claude** (this): the agent list (header, rows, summaries,
   columns) for Claude sessions — live, background jobs and past — and
   opening one session read-only (turns, folded steps, markdown, code
   highlighting, diffs). `rush --dump`, `rush --render`.
2. Host Claude: start, send, interrupt, permissions, the message box.
3. Other adapters.
4. Remaining screens, plugins, pr, remote, menubar, update; then the Go
   version retires.
