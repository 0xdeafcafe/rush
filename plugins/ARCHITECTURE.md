# How plugins work

This is the inside view: the processes, what crosses each boundary, and why it is built this way. For using and writing plugins, see the [README](README.md).

## The processes

```
  YOU                                                                    ~/.config/rush/
   │  rush plugin approve|revoke|list|check                              ├─ plugins/
   ▼                                                                      │   ├─ approved.json   digest + manifest as approved
 ┌──────────────────────┐   reload / status                               │   ├─ broker.sock     0600, rush only
 │ rush CLI / rush UI │ ─────────────────────┐                          │   ├─ broker.log
 └──────────────────────┘                      │                          │   └─ <name>/         code + plugin.json (read-only to it)
                                               ▼                          └─ plugin-data/<name>/ its only writable place (HOME, TMPDIR)
 ┌─────────────────────────────┐   framed JSON-RPC    ┌──────────────────────────────────────────────────────────┐
 │ rush host run <id>          │   on broker.sock     │ rush plugind   (one per machine, flock'd)               │
 │  one per rush-mode session  │ ───────────────────▶ │                                                          │
 │                              │   "mcp" {plugin,     │  fromRush:  mcp · reload · status                       │
 │  start():                    │    session, msg}     │                                                          │
 │   ForSession() from          │ ◀─────────────────── │  runner per plugin ─ supervise: verify digest → start →  │
 │   approved.json →            │   JSON-RPC reply     │                      restart w/ backoff (1s … 60s)       │
 │   --agents  plugin:agent     │                      │  watch every 5s: memory footprint > limit → kill         │
 │   --append-system-prompt     │                      │  watch every 30s: files changed → kill (refused)         │
 │   Initialize(rush,          │                      │                                                          │
 │     rush-<plugin>…)         │                      │  fromPlugin: capability check → host.List / Spawn /      │
 │                              │                      │    Dial(id).Send/Interrupt/Stop / subscribe (own only)   │
 │  mcp_message for rush-* ──▶ │                      └─────────┬──────────────────────────────────┬─────────────┘
 │   thread → Broker::mcp       │                                │ fd 3: socketpair                 │ stdin/stdout
 └───────┬──────────────────────┘                                │ 4-byte len + JSON-RPC            │ NDJSON (MCP)
         │ stdin/stdout stream-json                              │ calls both ways                  │
         ▼  control_request mcp_message / can_use_tool           ▼                                  ▼
 ┌──────────────────────────────┐        ┌─ taskpolicy -c utility ───────────┐  ┌─ taskpolicy -c utility ───────────┐
 │ claude -p (headless)         │        │ ┌─ sandbox-exec (deny default) ─┐ │  │ ┌─ sandbox-exec (deny default) ─┐ │
 │  tools: mcp__rush-<p>__*    │        │ │ protocol "rush" plugin       │ │  │ │ protocol "mcp" plugin         │ │
 │  each call → permission      │        │ │  e.g. delegate (Rust)         │ │  │ │  e.g. server-memory (node)    │ │
 │  prompt to YOU               │        │ │  tools.list / tools.call      │ │  │ │  tools/list / tools/call      │ │
 └──────────────────────────────┘        │ │  sessions.* / log  ──▶ broker │ │  │ │  (can't call rush)           │ │
                                         │ └──────────────┬────────────────┘ │  │ └───────────────────────────────┘ │
                                         └────────────────┼──────────────────┘  └───────────────────────────────────┘
                                                          │ only allowed socket: 127.0.0.1:<own port>
                                                          ▼
                                         ┌──────────────────────────────────┐
                                         │ Proxy (per plugin, in plugind)   │ CONNECT only · approved host:port only
                                         │                                  │ resolves, refuses loopback/private/
                                         └──────────────┬───────────────────┘ link-local/CGNAT → internet
                                                        ▼
```

- **Session host** (`rush host run <id>`, one per rush-mode session, already there before plugins). When it starts Claude Code, it reads `approved.json` and adds the approved plugins' subagents (`--agents`) and prompt text (`--append-system-prompt`). It registers each plugin's tools as an in-process MCP server, `rush-<name>`, the same way rush's own `mcp__rush__show` works. When Claude Code sends an `mcp_message` for one, the host passes it to the broker on a thread of its own and replies with the answer. The session never blocks on a plugin.
- **Broker** (`rush plugind`). There's one per machine, held by `flock`. Hosts start it when a plugin is approved, and it exits once none is. It runs every approved plugin, answers hosts, and checks every call a plugin makes.
- **Plugin**, one process per plugin, shared by every session. It runs as `taskpolicy -c utility sandbox-exec -p <profile> <program>`.
- **Proxy**: one per plugin with `network`, inside the broker. It's the plugin's only way out.

## One tool call, end to end

```
  Claude              claude -p            host                  plugind                 plugin (sandboxed)
    │                     │                  │                       │                          │
    │ tool_use            │                  │                       │                          │
    │ mcp__rush-memory__ │                  │                       │                          │
    │ search_nodes ──────▶│ can_use_tool ───▶│ not auto-allowed:     │                          │
    │                     │                  │ shown to YOU ── allow │                          │
    │                     │◀──── allow ──────│                       │                          │
    │                     │ mcp_message ────▶│ ownTraffic: hidden    │                          │
    │                     │ server=rush-    │ from UI clients       │                          │
    │                     │ memory, id=9     │ Broker::mcp ─────────▶│ runner.mcp               │
    │                     │                  │  {plugin, session,    │  initialize/ping: itself │
    │                     │                  │   msg}                │  tools/*: wait(ready) ──▶│ its own id, not
    │                     │                  │                       │                          │ Claude's
    │                     │                  │                       │◀──────── result ─────────│
    │                     │                  │◀── reply, id=9 ───────│ rpcReply(Claude's id)    │
    │                     │◀─ ReplyMCP ──────│                       │                          │
    │◀── tool_result ─────│                  │                       │                          │
```

The other way, for rush-protocol plugins only:

```
  plugin ── sessions.start {cwd, prompt} ──▶ plugind: has "start"? cwd inside a workspace? mode default/acceptEdits/plan?
                                                     under 4 live and 30/hour? ──▶ host.Spawn(StartedBy: plugin)
  plugin ── sessions.send / subscribe / stop {id} ──▶ id matches ^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$ ?
                                                     info.StartedBy == plugin ? ──▶ host.Dial(id)
  plugind ── session.event {info|text|tool|sent|result|closed} ──▶ plugin   (tool calls by what they do, never their output)
```

## The boundaries

Each boundary is enforced by something the plugin can't touch.

| Boundary | What guards it |
|---|---|
| Plugin ↔ the machine | **The sandbox profile**, which starts from `(deny default)`. The plugin can execute only its own program, read its folder, system libraries and its manifest's `read` paths, write only its data folder and its manifest's `write` paths, and connect only to its proxy's port. The broker builds the profile from the approved manifest (`crates/rush-plugin/src/sandbox.rs`). The programs in the manifest's `exec` are the one way out: the broker runs them itself, outside the sandbox, with the plugin's arguments after their fixed command line (`crates/rush-plugind/src/exec.rs`). |
| Plugin ↔ the network | **The proxy.** It allows CONNECT only, to approved `host:port` pairs only. It resolves names itself and refuses loopback, private, link-local and CGNAT addresses, so a plugin can't rebind DNS to the user's machine or LAN. |
| Plugin ↔ rush | **The broker's capability checks.** Each `sessions.*` call needs its capability. `read`, `send` and `control` need a session the plugin started, because other sessions may run with permissions the plugin was never given. Session ids are validated before they become paths. Starting sessions is limited to the plugin's workspaces and to modes that ask the user, with rate and concurrency caps; so is `worktree` creation, which needs the checkout itself inside a workspace, with branch names git can't take for options. `sidebar.set` is checked, cleaned of escape sequences, and written by the broker to a file of rush's; the UI reads only the files of plugins approved with `sidebar`. `queue` is the one reach past its own sessions: a message, marked as the plugin's, queued to a session in its workspaces that still asks the user. |
| Plugin ↔ other processes | **The socket pair on fd 3**, which has no path on disk, so nothing else can connect. The broker's socket is 0600 in a 0700 folder, and the sandbox denies plugins any unix socket. |
| Plugin ↔ Claude | **Claude Code's permission prompt.** Plugin tools are never added to `--allowedTools`, unlike rush's own drawing tool, so every call asks the user unless they allow it. |
| On disk ↔ what runs | **The approval.** `approved.json` holds a SHA-256 over every file in the plugin's folder (names, modes, contents, symlink targets) and the manifest as approved. The broker checks the digest before every start and every 30 seconds. Sessions get agents, prompt and tools from the approved manifest, never from the file on disk. |
| Plugin ↔ the user's CPU and memory | **Resource limits.** `taskpolicy -c utility` keeps the plugin at a low QoS. The broker checks its memory footprint every 5 seconds and kills it past `memoryMB`. (`taskpolicy -m` isn't enforced without root, so the broker enforces the limit itself.) |

## Why it is built this way

- **One broker, not a plugin per session.** Listing, starting and following agents cuts across sessions, and has to keep working with rush's UI closed. The broker is the one place that holds that and enforces it. Hosts stay as they were: they gained a forwarder, not a policy engine.
- **A socket pair on fd 3.** It has no path, so there's nothing to find, guess or race for. It needs no auth handshake, and the sandbox can keep denying every socket. One connected stream per plugin is also the fastest local transport there is. A tool call's round trip through the host, broker and plugin costs tens of microseconds, next to model calls measured in seconds.
- **Length-prefixed JSON-RPC.** Reading a message takes two reads with no scanning, and the size is capped at 16 MB before anything is allocated. JSON-RPC is what MCP already speaks, so tool calls pass through almost untouched, and a plugin can be written in anything, with no code generation or schema compiler. Shared memory or protobuf would add a dependency, and room for bugs, to save time that doesn't matter here.
- **MCP plugins as they are.** Any MCP server works without changes: the broker is its one MCP client, and gives each call its own id. That's how the memory example is the reference server, unmodified.
- **`sandbox-exec`.** It's the one sandbox on macOS that needs no root, entitlements or signing. Apple has deprecated the command-line tool but still ships it, and Claude Code relies on it too. Where there's no sandbox, as on Linux, `Supported()` refuses to run installed plugins. Bundled plugins (`crates/rush-plugin/src/bundled.rs`, and their code in `crates/rush-bundled`) are rush's own code: the broker runs them as `rush plugin run <name>`, unsandboxed, on every system, over the same fd 3 protocol and capability checks.

## Files

| Path | What it is |
|---|---|
| `crates/rush-plugin/src/manifest.rs` | The manifest and its validation |
| `crates/rush-plugin/src/plugin.rs` | Loading, the digest, approvals, and `for_session` (what hosts add to Claude Code) |
| `crates/rush-plugin/src/rpc.rs` | The JSON-RPC connection: framed (fd 3) and line (MCP stdio) codecs, concurrent calls both ways |
| `crates/rush-plugin/src/sandbox.rs` | The plugin's environment, the sandbox profile, and the `taskpolicy`/`sandbox-exec` command; a refusal elsewhere |
| `crates/rush-plugin/src/proxy.rs` | The per-plugin CONNECT proxy |
| `crates/rush-plugin/src/client.rs` | The host's side: finding and starting the broker, and forwarding MCP messages |
| `crates/rush-plugin/src/sidebar.rs` | Checking and keeping a plugin's `sidebar.set`, and reading it back for the list |
| `crates/rush-plugin/src/cmd.rs` | `rush plugin list/check/approve/revoke/logs`, and the plain-words approval text |
| `crates/rush-plugind/src/broker.rs` | The broker: lock, socket, reload, the memory and digest watch, status |
| `crates/rush-plugind/src/runner.rs` | One plugin's life: start, handshake, restart with backoff, the MCP bridge |
| `crates/rush-plugind/src/sessions.rs` | What plugins may do to sessions, and following them |
| `crates/rush-plugind/src/watch.rs` | Watching the list for changes, and the checkout each session is in |
| `crates/rush-plugind/src/exec.rs` | Running a manifest's `exec` programs for the plugin |
| `crates/rush-ui-plugin-ui/src/sidebar.rs` | The list's group-by mode for a plugin's sections and names |
| `crates/rush-claude/src/conn.rs` | Routing a session's `mcp_message` for a plugin to the broker |
| `plugins/sdk` | `rush-plugin-sdk`, the plugin's side: the crate the example plugins and the Rust template are written on. It shares no code with the crates above, which check what it sends |

The tests exercise it for real:
- `crates/rush-plugin/tests/examples.rs` runs a probe plugin inside the sandbox, which tries to read secrets, write elsewhere, start a program, and reach the internet, localhost and rush's socket. Each attempt must be denied by the sandbox itself. It also runs the `neighbours` and `reconnect` examples there, as rush would.
- `crates/rush-plugind/tests/examples.rs` builds the delegate example and drives it through the broker as a host would, covering permission refusals, ids crafted as paths, restarting after a crash, and shutting down on revoke. It runs the kanban example too, sandboxed, against a board and a `kanban` CLI of its own: reading the board and the calling session's card, reaching the card's agent, and linking the card to it through `exec`.
- `crates/rush-plugind/src/api_tests.rs` covers `exec` (only named programs, only in workspaces), which sessions `queue` may reach, what `sessions.start` refuses in `meta` and `worktree`, and reading a session's checkout and branch.
- Each example's own tests, and the SDK's, run with the rest of the workspace's.
