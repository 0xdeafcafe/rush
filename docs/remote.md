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
    transcript tail instead. Reading never wakes a sleeping host.
  - `POST /api/sessions/{id}/op` sends one host op from an allowlist: send,
    allow/deny, interrupt, stop, model/effort, stop_task, and queue ops.
    `send` wakes a sleeping host first (`host.Ensure`), as `rush session send` does.
- **Hub = a serve with peers.** `remote.json` in the state folder lists other
  machines (`name`, `url`, `token`). The hub proxies `/m/<name>/api/…` to them
  with their token. Peers come from local config only, never from requests.
  Machines reach each other over Tailscale, with no enrolment or node channel.
- **Auth.** Each serve has one random token in `remote-token` (0600). Native
  clients send it as `Authorization: Bearer`. The browser posts it once to
  `/api/login` and gets a `HttpOnly; SameSite=Strict` cookie, `Secure` behind
  HTTPS. Requests that change anything must be JSON POSTs, and a present
  `Origin` must match the host. That blocks cross-site form posts and other
  cross-site requests.
- **Web app** (`internal/remote/web`) is embedded plain HTML, JS and CSS.
  There's no build step, so `go install` still works. It also serves a PWA
  manifest and a service worker.
- **Native terminal, two ways.** `rush session --on <name> list|start|send|interrupt|stop|info|watch`
  goes over HTTP; `<name>` is a peer in `remote.json` (`rush remote add`), or `self`.
  And **`rush remote attach <name>`** puts that machine's sessions in this
  machine's own rush view: they list, open, show history, and take messages,
  approvals, answers, stop and queue edits like local ones (`internal/remote/attach.go`).
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
- **Tailscale and cloudflared** are Optional bundled plugins: off by default,
  and each is a switch, like `hex`. `rush serve` watches the switches (every
  3s, `remote.Expose`) and, while one is on, exposes itself. Only the one
  `rush serve` that holds the listen port does, so two serves never compete;
  the plugins themselves start no process and need no manifest network/exec
  rights (`validateBundled` is unchanged):
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
- Remote ops are an allowlist (`remoteOps`); rewinds, restarts, away, relogin
  and anything carrying a file path stay with the terminal.
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
- Pairing flows, per-device credentials and revocation lists. Rotate a
  machine's token by deleting `remote-token` and restarting serve.
- SQLite, node channels, connection fencing.
- Choosing a machine on the view's other new-agent keys, the start sheet, or
  with images/presets/accounts: only `#on` targets a machine. `host.Spawn` is
  untouched and always local.

## Custom domain (e.g. `rush.thuis.forbes.red` over Tailscale)

A Tailscale certificate only covers `*.ts.net`. Either use the `.ts.net` name
that the Tailscale plugin prints, or put your own reverse proxy (Caddy and
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
mapping changed underneath, competing serves, rapid toggle, plugin switch);
`Run` ending when serving fails; every web control reaching a fake host as the
browser sends it (create, send, steer, stop, approve, always, deny, answer,
stop task, queue ops, no paths through); and `rush remote attach` end to end as
two homes and two processes, with the host client the view uses (it lists, opens,
receives the approval, allows it, sends a message whose image path doesn't cross,
is told a disallowed op isn't available, a second attach is refused, folders go
on exit).

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

Driven in headless Chrome: `internal/remote/testdata/controls.js` against
`RUSH_FIXTURE=<dir> RUSH_FIXTURE_PORT=<port> go test ./internal/remote -run
BrowserFixture` (fake hosts, nothing real; see the file's header) covers sign-in,
approve, always, deny, a question answered, send, steer, stop, create, a failed
send keeping its draft with no retry, one request while pending, and typing
during a send kept. The real view (`rush open <id> --hosted` in tmux) opened an
attached session and a message typed there reached the fake host.

Not verified: the real tailscale (only `serve status --json` of `{}` was read
from the installed one, and the mapping JSON shape is as its source describes),
a real cloudflared tunnel, push through Apple/Google/Mozilla, a phone, DNS, and
approving/sending to a live agent from the browser. Not built: browser image
attachments; listing sessions not started by Rush; history beyond what serve
replays; the view's other keys on attached sessions (rewind, fork, restart,
away and the like) answer "isn't available"; git, files and diffs of the remote
working folder aren't shown in the view; notifications poll every 4s; peers
that are offline are skipped.
