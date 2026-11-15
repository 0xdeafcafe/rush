# Remote rush

Agents already run in detached hosts that outlive every view and accept many
clients at once (`internal/host`). Remote rush puts an authenticated HTTP
server in front of those hosts. This is the simpler plan that replaced
[remote-rush-plan.md](remote-rush-plan.md); that plan is kept as research.

```
 phone / laptop browser ──https──▶ rush serve (hub) ──▶ its own hosts (unix sockets)
 rush session --on box  ──https──▶        │
                                          └──proxy /m/<name>/…──▶ rush serve on <name> ──▶ its hosts
```

## Pieces

- **`rush serve`** (`internal/remote`) runs on every machine. It listens on
  `127.0.0.1:7878` by default, and only ever runs work on its own machine.
  - `GET /api/sessions` lists sessions, like `rush session list`.
  - `POST /api/sessions` starts one, through `rush session start`'s own code.
  - `GET /api/sessions/{id}/events` is a Server-Sent Events stream. A running
    host's lines are decoded and sent as neutral events (`{"t":…,"e":…}`),
    with the host's replay first. A host that isn't running sends its saved
    transcript tail instead. Reading never wakes a sleeping host. History
    starts with the transcript's last 600 events (a `cut` event says there's
    more); `?full=1` sends all of it (up to 50,000), which the web app's
    *Show earlier* and `rush remote attach` ask for.
  - `GET /api/sessions/{id}/events/before?cursor=…&limit=…` pages that history
    back: one page a call, each page's events oldest first (the app sets them
    above the seam it cut at), the answer carrying the next cursor and whether
    there's more. The first pull has no cursor. A cursor is `<end>-<at>` — two
    byte lengths into the transcript — while the adapter has a history window
    (Claude Code, Codex); without one it is `<drop>`, a count of the newest
    events to skip, which follows appends but not rewrites: a transcript that
    shrank under a byte cursor answers `{"reset":true}`, and the app asks for
    the stream again. A cursor can't reach inside a line, so a page overruns
    its limit rather than split one.
  - `GET /api/sessions/{id}/overview` is a session's numbers read from its
    whole transcript: turns (with each one's cost), the model and cwd, tokens
    in and out with cache reads and writes, the context it holds and its
    window (the transcript's, else the one its agent runs the model with),
    what it has spent, its models with what each has cost, and its tools by
    count.
  - `GET /api/sessions/{id}/subagents` lists them as they sit beside the
    transcript (type, description, model, the call that spawned them, depth,
    born), each with what it last said, and
    `GET /api/sessions/{id}/subagents/{sub}/events` pages a subagent's own
    transcript back like the session's.
  - `GET /api/feed` serves rush's shared local board (`rush feed list`)
    whole — threads with their messages, each author backfilled with the
    project its session says when the chirp didn't carry one — and whether
    the feed is on. `POST /api/feed` posts as You: `{"op":"post","text":…}`
    (the first line is the title), `{"op":"reply","id":…,"text":…}` and
    `{"op":"resolve","id":…,"resolve":bool}`; mentions are kept by their ids
    as the CLI keeps them, and a feed that's off answers 400 (the user turns
    it on with `#feed on`).
  - `POST /api/sessions/{id}/op` sends one host op from an allowlist: send,
    allow/deny, interrupt, stop, model/effort/mode, stop_task, background,
    queue ops, away/back, limit (carry on at the reset or not), context,
    tell (a subagent) and without. `send` wakes a sleeping host first
    (`host.Ensure`), as `rush session send` does. A send's images come as
    bytes (`pictures`: png, jpeg, gif or webp, at most 8, each under 20 MB, 64 MB a message);
    serve writes them to the session's scratch folder and the host reads them
    from there, so no client path ever reaches the agent.
  - `POST /api/git` runs one read-only git command (`rev-parse`, `diff`,
    `ls-files`, `log`, `show` and the like) in a session's folder, or its
    repository's top. It's how the web app's *Changes* and an attached
    session's changes view read the remote's working tree.
  - `GET /api/others` lists the machine's agents' sessions that Rush didn't
    start (what the local list finds with each agent's discovery), and
    `POST /api/sessions` with `{"resume": SESSION}` carries a past one on in
    Rush with its own agent and folder. One still running in a terminal is
    only listed.
- **Hub = a serve with peers.** `remote.json` in the state folder lists other
  machines (`name`, `url`, `token`). The hub proxies `/m/<name>/api/…` to them
  with their token. Peers come from local config only, never from requests.
  Machines reach each other over Tailscale, with no enrolment or node channel.
- **Auth.** Each serve has one random token in `remote-token` (0600). Native
  clients (rush on another machine, a hub) send it as `Authorization: Bearer`.
  A browser signs in once, with the token or a pairing code, at `/api/login`,
  and becomes a **device** of its own (`remote-devices.json`, 0600): its
  `HttpOnly; SameSite=Strict` cookie (`Secure` behind HTTPS) is its own random
  token, stored hashed. A cookie from before devices (the machine token) is
  made a device at the app's first request. Requests that change anything must
  be JSON POSTs, and a present `Origin` must match the host. That blocks
  cross-site form posts and other cross-site requests.
- **Remote mode** (Settings › Remote in rush) keeps serve running from login
  on, as a LaunchAgent (`com.github.0xdeafcafe.rush.serve`, log `serve.log`
  in rush's folder), reached over Tailscale and letting in the web app and
  SSH unless remote.json already says otherwise. Off stops it and takes its tailnet mappings down. macOS
  only; elsewhere run `rush serve` as a service of your own.
- **Pairing a device.** Settings › Remote › *Pair a device* asks serve
  (`POST /api/pair`, with the token) for a one-time code: 8 characters
  (`K7QM-3XHP`, no letters that read as others), working once, for 10
  minutes. The sheet shows it as a QR code of the app's address with
  `#pair=CODE` (the rush bottle in its middle; error correction H, or Q from
  the size that puts an alignment pattern there), and typed, for SSH. A
  fragment never reaches a server log. The app trades the code for its cookie
  at `/api/login`. Ten wrong codes in a row void every live one. The sheet
  watches the device list and says *Paired* when the device turns up.
- **Devices.** `GET /api/devices` lists them (kind web or ssh, name from the
  browser or SSH client, created, last seen, what it has open now);
  `POST /api/devices/{id}/revoke` unpairs one and ends what it has open (its
  event streams, its SSH connection and rush) at once. Last seen is written
  at most once a minute while in use, and when an SSH connection ends. Settings
  › Remote lists them; `x` twice unpairs one.
- **SSH** (`"ssh": true`): serve's own SSH server (`sshd.go`,
  golang.org/x/crypto/ssh) on `127.0.0.1:2222` (`sshListen`), and on the
  tailnet's port 2222 (`sshPort`) through `tailscale serve --tcp` while
  serve is reached over Tailscale. Its host key is ed25519, in `remote-ssh-host-key`;
  the pair sheet shows its fingerprint. Only public keys get in: a paired
  key's straight away; any other, once it has proven it holds the key, is
  asked for a pairing code (keyboard-interactive), which pairs that key. A
  session is rush itself, the whole TUI, on a pty: one rush per device, kept
  30 minutes after its connection drops. Connecting again shows the same rush
  (the terminal modes it set are sent again and a SIGWINCH has it draw it all),
  so it needs no tmux; a second connection takes it over from the first.
  `ssh host command` is refused: there's no shell, only rush.
  It never takes the one-view lock (`RUSH_VIA=ssh`): while a rush on the Mac
  holds it (`instances.Led`, read every 2s) it follows that one
  (`ui.Model.Follow`), showing and doing what you ask but leaving switching
  logins, sleep, orphans, tidying and notifications to it; with none, it
  leads, and a rush opened on the Mac later takes over.
- **Web app** (`internal/remote/web`) is embedded plain HTML and JS, in
  rush's own look (one monospace face, its colours from `internal/ui/style.go`,
  dark and light). Its CSS is Tailwind, built from `webstyle/app.css` by
  `go generate ./internal/remote` (which installs Tailwind 4 in a scratch
  folder) and committed, so `go install` needs no Node; styles the Tailwind
  build has no class for are hand-written after it, in its variables. Routes
  are the hash alone (`#/`, `#/s/<machine>/<id>`, `#/new`, `#/devices`,
  `#/fleet`); each screen is a scope whose streams and timers end when you
  leave it, and whose late answers are dropped, so nothing draws over the
  screen you're on. Lists update their rows in place, keyed by id. The
  session list comes from `GET /api/sessions/stream` (SSE: the list when it
  changes, a keepalive every 15s, and first the browser's own device id); a
  peer without it is polled. It also serves a PWA manifest and a service
  worker.
  - A conversation pages its history back as you scroll up (the endpoint
    above), keeps only a window of it in the DOM, and draws what a turn
    holds as the TUI does: tool calls with their arguments and output,
    files read and edited (a diff), todos, thinking, plans, subagent
    spawns, questions and their answers.
  - A session carries tabs beside its conversation: its **subagents** (each
    one's status, model and activity, its own conversation a sheet away),
    its **shells** (its tasks and background commands, to stop or read),
    its **changes** (the working tree, as the changes sheet), and an
    **overview** (the endpoint above, drawn as bars, tables and money).
    Switching tabs keeps the session's stream running underneath and its
    scroll position; the route is `#/s/<machine>/<id>/<tab>`.
  - Every list of sessions (home, the sidebar, the fleet) draws the same
    rows: a rule per state (needs you, working, idle, asleep, stopped) with
    its count and spend, each session newest first with what it needs or is
    doing, and the runs it spawned (`meta.spawnedBy`) nested under it with a
    count of those at work.
  - On a wide screen (1100px and up) a session keeps the machine's list
    beside it, the one you're in marked, kept current from
    `/api/sessions/stream`.
  - **The fleet** (`#/fleet`) is the dashboard over every machine: counts
    of what needs you, what's working, runs working, idle and the spend;
    then every peer's sessions (through the hub's `/m/<name>` proxy) in one
    list by state, each named with its machine when there's more than one;
    and this machine's feed — threads with replies and resolving, a chirp to
    post when the feed is on. Wide, the feed sits beside the sessions;
    narrow, it's a tab.
  - The page's Content-Security-Policy allows no inline styles: what's
    sized at run time (the context bar, the composer's height) is set
    through the CSSOM, and the rest are classes in `app.css`.
- **Native terminal, two ways.** `rush session --on <name> list|start|send|interrupt|stop|info|watch`
  goes over HTTP; `<name>` is a peer in `remote.json` (`rush remote add`), or `self`.
  `list --others` lists sessions Rush didn't start there, and `start --resume SESSION`
  carries one on.
  And **`rush remote attach <name>`** puts that machine's sessions in this
  machine's own rush view: they list, open, show history, and take messages,
  approvals, answers, stop and queue edits like local ones (`internal/remote/attach.go`).
  Images pasted in an attached session's box are read here and sent as bytes.
  Its changes view reads the remote's git through serve (`convo.RemoteGit`,
  set to `remote.Git`), with paths kept under the placeholder folder below.
  It does it by standing in for their hosts: per remote session a local folder
  with an info file and a host socket, speaking the host protocol to the view
  and serve's HTTP to the machine, so the view is unchanged. Each shows as
  `name @machine`, marked remote (`Info.Remote`) so the view and host refuse
  local actions on it. Its folder is shown under a permissionless placeholder
  folder of rush's own (`remote-cwd/<machine>/…`), which nothing can enter, so
  a path that also exists here is never the session's. Each stand-in folder
  holds a `bridge.json` marker; attach touches or removes only folders with
  its own marker, taking another id when a derived one is already someone's.
  Attach runs in a terminal until ended (one per machine), then removes its
  folders.
- **Starting an agent there from the TUI.** In the Prompt, `#on MACHINE
  [folder] [agent=KIND] task…` sends one `POST /api/sessions` to that peer
  (the same as `rush session --on MACHINE start`). The folder is typed as the
  *machine's* path (`~` is its home, the default) and is validated there
  (absolute, allowed by its `workspaces`, not rush's own folder); it is never
  resolved as a local path. The agent, its account, model and files stay on
  that machine, which uses its own defaults. This machine's start options
  (`#new`/`#with`/`#preset`, the start sheet) and pasted images are not sent;
  images are refused. Local starts are unchanged. The session shows in the view
  once `rush remote attach MACHINE` is running (within about 3s); if it isn't,
  `#on` says to run it. Nothing is retried, and while one `#on` waits another
  isn't sent (the box gets its text back). On any failure the whole command
  goes back in the Prompt, unless something was typed there meanwhile, which
  is never overwritten (the flash then carries the command). A refusal shows
  the machine's reason. If the request went out but no whole answer came back
  (a timeout, a dropped link, or a 2xx reply that couldn't be read), it says
  the session *may or may not have started*: check the list before sending it
  again.
- **Plugins and choices**, all set from Settings › Remote, and all off by
  default, so nothing is reachable from elsewhere until you say so. The
  switches are Optional bundled plugins (`internal/remote/plugins.go`), so
  Settings › Plugins and `rush plugin on|off` turn them too:
  - `remote`: remote mode, above. Turning it on or off installs or removes
    serve's LaunchAgent (the bundle's `Switched`).
  - `remote-tailscale` and `remote-cloudflared`: how other devices get to
    serve. Neither is this machine only.
  - `remote-ssh`: rush itself over SSH, above: the terminal, kept like tmux.

  And in remote.json:
  - `web`: the web app; off, serve answers only rush's own clients.
  - `attachPeers`: keeps `rush remote attach` running for each peer, from
    serve (shown in Settings only once a peer is added).

  A remote.json from before, with `reach` or `ssh`, has them moved into the
  plugins the first time it's read, and remote mode found running turns the
  `remote` plugin on. `rush serve` reads the switches every 3s
  (`remote.Expose`) and exposes itself as they say. Only the one `rush serve`
  that holds the listen port does, so two serves never compete:
  - Tailscale: `tailscale serve --bg --https=8443 http://127.0.0.1:<serve port>`
    (`tailscalePort` in remote.json picks another). It first reads `tailscale serve
    status --json` and, if that https port already serves anything, leaves it and
    says so. It removes the mapping only if it made it and it still reads as it
    made it; a failed start cleans nothing up; off-then-on waits for the old
    worker to finish first.
  - Cloudflared: a `cloudflared tunnel run --token-file` child. Turning it off
    stops only that child.

  Neither touches agents, other serve mappings, DNS, or the user's own tunnels.
- **Notifications.** The browser subscribes with Web Push (VAPID). The hub sends
  a short notice when an agent asks for approval, asks a question, or fails. The
  notice says which agent, never what it asked.

## Safety boundaries

- The page shell is public; everything under `/api` and `/m` needs the token.
  A cookie (browser) request that isn't a GET must be JSON, from the page's own
  origin (`Origin` equals the configured origin or the Host; a cross-site
  `Sec-Fetch-Site` with no Origin is refused). Bearer requests (rush, a hub)
  carry no ambient credential, so they skip the origin check. CSP allows only
  scripts and styles from serve itself.
- A hub forwards only `/m/<peer>/api/…`, to peers in `remote.json`, with the
  peer's token; the browser's cookie and Origin never reach a peer.
- Remote ops are an allowlist (`remoteOps`); rewinds, compacting, restarts,
  relogin and anything carrying a file path stay with the terminal there (they
  work on transcripts or accounts of the machine the view is on). An away's
  check-ins can't be less than a minute apart.
- Images cross only as bytes of an allowed type and size, written under the
  session's own scratch folder with a random name.
- Remote git runs only commands from `gitReads`, in a folder of one of the
  machine's sessions (or its repository's top), with options that write, run
  something or move the repository (`--output`, `--ext-diff`, `--textconv`,
  `-c`, `-C`, `--git-dir`, `--work-tree`, …) refused, every path or revision
  argument inside that repository, and `core.fsmonitor` and external diffs off.
  It is a reader for the changes views, not a sandbox: the token can already
  start an agent that runs anything there.
- A paired device is trusted as the token is: a web device can do what the
  app does, and an SSH device runs rush itself as you, with everything rush can
  do on this machine. Pair only your own devices; unpair a lost one in
  Settings › Remote. SSH takes no passwords: a key must prove itself before a
  code is even asked for. Serve's SSH and its tailnet mapping are tailnet only,
  like the app.
- Resuming takes only a session id from the machine's own listing of others,
  never a path.
- A remote client can't start a session in rush's own folder (tokens, push
  key), and when `remote.json` has `"workspaces": [...]` only in those folders
  (symlinks resolved).
- Ops have no acknowledgement, so the web app and `rush session --on` send each
  once and never retry; the events stream (GET, which does reconnect) shows
  whether it landed.
- Push: payloads follow RFC 8291 (checked against its appendix A vector in
  `push_test.go`), VAPID per RFC 8292, and only known push services' hosts are
  ever sent to.

## Deliberately left out (add when needed)

- Command journals and exactly-once delivery. A retried send can duplicate, as
  it can locally. Clients don't retry ops on their own.
- Renaming devices, and pairing without rush open on the machine (a code
  comes only from Settings there, or `POST /api/pair` with the token). Rotate
  a machine's token by deleting `remote-token` and restarting serve; that
  doesn't sign out devices, which have their own.
- SSH port forwarding, `exec`, subsystems (sftp) and agent forwarding: a
  session is rush and nothing else.
- SQLite, node channels, connection fencing.
- Choosing a machine on the view's other new-agent keys, the start sheet, or
  with images/presets/accounts: only `#on` targets a machine. `host.Spawn` is
  untouched and always local.
- Rewind, fork, compact and restart of an attached session: each needs the
  transcript on the view's machine, so serve would need ops of its own for them.
- Sessions Rush didn't start in the attached view's list: they're in the web
  app and `rush session --on … list --others`. A resume uses the agent's
  default account there, not necessarily the one the session was found under.

## Custom domain (e.g. `rush.thuis.forbes.red` over Tailscale)

A Tailscale certificate only covers `*.ts.net`. Either use the `.ts.net` name
that serve prints when reached over Tailscale, or put your own reverse proxy (Caddy and
similar) on the hub's tailnet address. Give that proxy a certificate for the
exact name, or for `*.thuis.forbes.red`, issued with ACME DNS-01. Point DNS
for the name at the hub's tailnet IP, either DNS-only public records or split
DNS, and proxy to `127.0.0.1:7878`. Set `"origin": "https://rush.thuis.forbes.red"`
in `remote.json` so links and push subscriptions use that one origin. Web Push
on an iPhone works only after the app is added to the Home Screen, and the
phone needs the tailnet to open the app.

## Tested, and not

Tests (`go test ./internal/remote`, run with `-race`): auth, origin and
CSRF; the hub proxy; the op allowlist and workspace rules; RFC 8291 and VAPID;
the tailscale fake binary (happy path, conflicting mapping, failed start,
mapping changed underneath, competing serves, rapid toggle, the reach switch);
`Run` ending when serving fails; every web control reaching a fake host as the
browser sends it (create, send, steer, stop, approve, always, deny, answer,
stop task, queue ops, limit, tell, away and back, an image as a file in the
session's scratch folder, bad images refused, no paths through); history as a
600-event tail with `cut`, then whole with `?full=1`; remote git's checks
(writes, options, paths outside, folders no session has) and the view's own
`convo.WorkingTree` reading a real repository through serve; others listed
newest first without Rush's own, and resumed with their own agent and folder;
and `rush remote attach` end to end as two homes and two processes, with the
host client the view uses (it lists, opens, receives the approval, allows it,
sends an image that arrives as bytes in a file on the remote while its local
path doesn't cross, is told a disallowed op isn't available, a second attach is
refused, folders go on exit).

`#on` is covered by dispatch-level tests (`internal/ui/oncmd_test.go`: a
failure puts the command back, a pending start isn't doubled, typing meanwhile
survives) and was smoked in a real TUI under tmux, in a scratch `RUSH_HOME`,
against a fake serve with plugins unused: `#on box /srv/app fix it` made one
POST, and the session showed as `fixit @box` through `rush remote attach`.
Then `rush open <alias-id> --hosted` in a second tmux window, text typed in its
composer and enter, reached the fake serve as exactly one `send` op
(`{"op":"send","text":"hello fake host"}`), with no other POST and no local
agent started. (An earlier run sent via `rush session send` instead; this one
is the typed composer.) Typing into the fleet-list Prompt too fast after
startup can drop the `#on ` prefix and start a local session: the smoke sent
keys literally, one second apart.

The web app was rebuilt (2026-10-09) after its home list, redrawn every 4s,
and a late fetch of the others list drew over an open session: the browser
run now also checks that an open session stays open, that the list comes as
a stream and isn't asked for again, that rows stay the same elements as it
updates, and the Devices screen (this browser and an SSH key listed, one
unpaired). Driven in headless Chrome: `internal/remote/testdata/controls.js` against
`RUSH_FIXTURE=<dir> RUSH_FIXTURE_PORT=<port> go test ./internal/remote -run
BrowserFixture` (fake hosts, nothing real; see the file's header) covers sign-in,
approve, always, deny, a question answered, send, steer, stop, create, a failed
send keeping its draft with no retry, one request while pending, typing
during a send kept, away, an image picked and sent, *Changes* of a real
repository, and a session Rush didn't start resumed from *Sessions Rush didn't
start*. The real view (`rush open <id> --hosted` in tmux) opened an
attached session and a message typed there reached the fake host.

Real Tailscale (the Mac app's CLI, 2026-10-09): serve run by launchd mapped
https 8443 to itself, and the page and an authorised `/api/sessions` answered
over the tailnet name. Run outside a terminal, the app's binary is a CLI only
with `TAILSCALE_BE_CLI=1`, which serve sets.

Devices and SSH (`devices_test.go`, `sshd_test.go`): each sign-in is its own
device with its own cookie, named from its browser; revoking one locks out
only it and ends what it has open; an old cookie becomes a device. Over SSH,
with x/crypto's client against serve's server: an unpaired key is refused, a
wrong code is refused, a right one (typed in lower case with its dash) pairs
the key; reconnecting needs no code and reaches the same rush, which gets its
modes again and redraws; revoking ends that rush and the key can't get back
in. Ten wrong codes void a live one. The QR code with the bottle decodes in
OpenCV, full size and at 0.4×, for short and long tailnet names
(`TestQRLinesPNG`).

Real SSH (2026-10-09, the real `ssh` client in tmux against a serve in a
scratch `RUSH_HOME`): the unpaired key was asked for the code, the code
paired it, and rush's whole TUI drew; after the connection was killed, a new
one at another width got in with no code and showed the same rush (the
first-run dialog still open, the same process), redrawn at its width; revoking
over the API closed the connection and ended that rush.

Not verified: a real cloudflared tunnel, SSH from a phone app over the tailnet, push through Apple/Google/Mozilla, a phone, DNS, and
approving/sending to a live agent from the browser, a live agent reading an
image sent remotely, *Show earlier* in Chrome (the fixture has no transcript),
and the attached session's changes view in a real TUI (tested through
`convo.WorkingTree`, not drawn). Not built: rewind, fork, compact and restart
on attached sessions answer "isn't available"; the attached view's list doesn't
show sessions Rush didn't start; notifications poll every 4s; peers that are
offline are skipped.

## The rich web app (Rust port, 2026-11-15)

Four things the port added to the app, each behind the same token:

1. **History pages back.** The stream still opens with its tail and a `cut`;
   scrolling near the top pulls `events/before` pages (the endpoint above)
   and sets them above the seam, keeping only a window of the conversation
   in the DOM. A `reset` answer (the transcript shrank under a byte cursor)
   sends the app back to the stream.
2. **The conversation draws what a turn holds** — calls with arguments and
   output, diffs, todos, thinking, questions — as the TUI draws them.
3. **A session has tabs**: subagents (each with its own conversation a
   sheet away), shells (tasks and background commands), changes, overview.
4. **The fleet** (`#/fleet`): every peer's sessions beside the feed.

Tested (`crates/rush-remote/src/tests_server.rs`, table-driven through the
handler): pages back under both cursors — the byte cursor through line
boundaries, the count cursor through appends, a shrunk transcript answering
`reset`, a bad cursor 400; a page's events come oldest-first *within* the
page while the pages themselves arrive newest-block-first, so a test's
bookkeeping reverses the page order, not the concatenation. The tabs:
overview read from a transcript whose turn ends say what the session had
spent (a turn's own slice is the difference), subagents listed with their
last words, their own events paged, unknown ones 404, each behind the
token. The feed: an empty board with the feed off, posting refused while
off, then a chirp, a reply and a resolve as You through the state config's
switch, and every malformed post 400.

## Open questions

- **The fleet's feed is one machine's.** The fleet screen spans every
  peer's sessions, but its feed pane reads the board of the serve you
  opened — each machine's board is its own (`rush-room` keeps one per
  home), and nothing merges them. Right thing or not: should the fleet
  ask each machine and interleave the boards, or is one board the point
  (agents you share a home with)? Left as one machine's for now, the
  simplest reading of "the feed" on a phone.
- **No framework.** The app stays hand-written JS on purpose — no Node at
  install, assets embedded, keyed in-place updates and manual windowing
  already written. If the app outgrows that, the first thing to try is a
  vendored Preact + htm (about 11 KB, no build step), not React or Svelte.
- **Live-tried, and not.** None of the four was driven against a real
  `rush serve`: CI builds a binary artifact only for pushes to main, so a
  branch has nothing to run, and local builds are off (CLAUDE.md). They
  are covered by the handler tests above, and the app's JS only by
  `node --check` — the fixture harness that drove the Go app in a real
  browser (`testdata/controls.js`) has no Rust-port counterpart yet;
  bringing it over is its own piece of work. A throwaway-home fixture
  for that harness is drafted at `crates/rush-remote/testdata/mkfixture.py`
  (a claude transcript with calls, todos and a subagent, the feed on),
  waiting for a binary to serve it.
- **`?full=1` and the paged history are two doors to the same room.** The
  stream's whole-history mode is what *Show earlier* used; the app now
  pages instead, but `rush remote attach` still asks for `full`. Whether
  `full` should go (and attach page too) is open; both work today.
