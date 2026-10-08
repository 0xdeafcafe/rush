# Plugins

A plugin adds to every rush-mode session: tools Claude can call, subagents, text in the system prompt, a memory it keeps across sessions. It can also run agents of its own: start them, follow what they say, message them. Any MCP server can be one, unchanged.

Every plugin runs sandboxed and does only what you approved. It can't read your files, reach the internet (except hosts it named and you approved), start programs, or take over your agents.

Plugins you install run on macOS only for now. Elsewhere rush won't run them rather than run them unsandboxed. [Bundled plugins](#bundled-plugins) run everywhere.

| | |
|---|---|
| [Using plugins](#using-plugins) | install, approve, list, revoke |
| [What a plugin can't do](#what-a-plugin-cant-do) | the sandbox, in short |
| [Taking part in rush's screen](#taking-part-in-rushes-screen) | events, overview sections, commands, settings, intercepts |
| [Writing one](#writing-one) | with Claude and the skill, or by hand |
| [Examples](examples) | `neighbours`, `kanban`, `delegate`, `memory`, and `autodrafts` and `reconnect` (UI hooks) |
| [ARCHITECTURE.md](ARCHITECTURE.md) | the processes, the boundaries, why it's built this way |
| [Protocol](skills/write-rush-plugin/references/protocol.md) · [Manifest](skills/write-rush-plugin/references/manifest.md) · [Sandbox](skills/write-rush-plugin/references/sandbox.md) | the reference |

## Using plugins

A plugin is a folder in `~/.config/rush/plugins/<name>` holding a `plugin.json` and its program. Nothing runs until you approve it:

```sh
rush plugin list            # what's installed, approved and running
rush plugin check <name>    # is it valid, and what would approving it allow
rush plugin approve <name>  # read what it may do, and say yes
rush plugin revoke <name>   # stop it
rush plugin logs <name>     # where its output goes
```

Approving shows, in plain words, what the plugin can read, write and reach, and what it adds to your sessions. It also records every file in the folder: if one changes, the plugin stops until you approve it again. The prompt text, subagents and tools your sessions get come from the manifest as you approved it, not as it is on disk now.

New sessions pick up approved plugins. A running session picks them up the next time its Claude Code starts, which happens after it rests. Each of a plugin's tools asks you before it runs, like any tool, unless you allow it.

Plugins run under `rush plugind`, one small process rush starts once a plugin is approved or bundled and on, and that ends when none is.

### Bundled plugins

Some plugins come with rush. Their code is rush's own, so they need no approval and run without the sandbox, on every system, as `rush plugin run <name>`; what they may do in rush's screen and to your sessions is still their manifest's, checked by the broker as for any plugin. Each is on until you turn it off, in Settings, Plugins, or with `rush plugin off <name>` (`on` to turn it back). A plugin you install with the name of a bundled one doesn't run.

**gate** (off until you turn it on) queues intensive programs your agents run, so a handful of sessions don't all start `tsc` at once. Each session rush starts finds a shim for each queued program first on its PATH, whatever the agent, and Claude Code sessions also get a hook that puts a Bash call running one (`npx tsc`, `pnpm exec vitest` too) under `rush gate run`; the program then waits for a slot, and `rush gate status` shows who runs and who waits. `rush gate run [--name N] [--dir D] -- cmd…` does the same by hand. Its settings, in Settings, Plugins:

- **Programs**: the names to queue (default `tsc, tsgo, go, cargo, webpack, vite, vitest, jest, next, rustc`).
- **Shared across**: one queue per `worktree`, per `repo` with all its worktrees (the default), or for the whole `system`.
- **At once**: 1 to 8 (default 2). **Between starts**: 0s to 30s (default 5s).
- **Per program**: `name=scope/at once/between starts`, any part left out, such as `tsc=system/1/10s, go=worktree/4`.

A slot is a locked file, so it frees itself however its holder ends. A program run from inside a queued one (`go vet` under `go test`) doesn't queue again.

**hex** (off until you turn it on) adds a hex view of what agents' steps print and read: `v` on a step cycles to its bytes, coloured by kind, with an ASCII column, and binary output opens in it. With it off, `v` still switches between text, pretty and image.

## What a plugin can't do

- **Your files.** It reads its own folder and the system's libraries, and writes only its data folder, `~/.config/rush/plugin-data/<name>`, plus any other paths its manifest names under `read` and `write`.
- **The network.** It has none, unless its manifest names `host:port` pairs. Those it reaches through rush's proxy, over HTTPS only, and never your machine or your network, whatever a name resolves to.
- **Programs.** It can't start any. It can ask rush to run the ones its manifest names under `exec`, which run outside the sandbox, as you. Approval lists each.
- **Your environment.** It gets a clean one: none of your tokens or Claude Code settings.
- **Your agents.** It may list and watch them, and see their folder, branch, state, cost and context use, never what was said. It may message, follow and stop only agents it started. Those run in folders you approved (in a new worktree, if it asks), never in a mode that skips asking you, with at most 4 at once and 30 an hour. With `queue` it may also queue a message, marked as its own, to other agents in those folders that still ask you before acting, and with `queued` send now, or drop, a message you queued in one, by its place, without seeing it.
- **Your agent list.** With `sidebar` it may arrange it: its own sections, and a name for each agent it knows by Claude Code session id. The list offers that as a group-by mode, and your own renames win. It sees nothing more by it.
- **Your machine's time.** It runs at utility QoS, and is ended if it grows past its memory limit (256 MB by default).

The full list is in [the sandbox reference](skills/write-rush-plugin/references/sandbox.md), and how each rule is enforced is in [ARCHITECTURE.md](ARCHITECTURE.md#the-boundaries).

## Taking part in rush's screen

A plugin can also take part in rush's own window, as far as its manifest's `ui` list says, and approval names each:

- **`events`**: hear what the agent list shows happen: a session opened or left, a turn started or ended, a session stopped by an error and of what kind, the network going away and coming back. Never what was said.
- **`input`**: see what you type in a message box, and set it. It's everything you type, so approve it with care.
- **`intercept`** (needs `input`): be asked before a message you send goes, and change it, hold it back with a reason, or ask you a question about it first, with a key for each answer or a line to type. All plugins together get 400 ms; after that the message goes as it was, and one that misses three times in a row isn't asked again until it restarts. The plugin has 2 minutes to act on the answer you pick.
- **`overview`**: add sections to a session's overview, and a short status to its row.
- **`notify`**: show a short message at the bottom of the screen, a few at a time.
- **`send`** (needs `events`): send a message as if you'd typed it, only to sessions in its workspaces that still ask you before acting, ten a minute at most.

`commands` adds commands to the `#` commands, the command bar and the keymap, as `plugin:<name>.<command>`. `settings` adds settings under Settings, Plugins; rush keeps their values where the plugin can't write, and tells it when you change one. `cli` adds commands to rush's CLI, as `rush <plugin> <command>`: the plugin runs them, with what it may do, and rush prints what it answers; `rush help` lists them.

None of it can slow rush down. rush hands the broker events without waiting, draws what plugins added from a copy it already holds, and a plugin that falls behind only loses its own oldest events. What a plugin added goes when it stops.

## Writing one

### With Claude

The [`write-rush-plugin`](skills/write-rush-plugin) skill teaches Claude to write, test and debug plugins. It knows the protocol, the manifest and the sandbox, starts from a template in Go, Python or Node that's tested in the real sandbox, checks its work with `rush plugin check`, and tests the tools with its `call.py`. Approving stays with you.

```
/plugin marketplace add 0xdeafcafe/rush
/plugin install rush-plugin-dev@rush
```

Then ask: *"write me a rush plugin that …"*.

### By hand

1. Copy a template: [`go`](skills/write-rush-plugin/templates/go) (a static binary, the sturdiest), [`python`](skills/write-rush-plugin/templates/python) or [`node`](skills/write-rush-plugin/templates/node). Each is one file with no dependencies, and offers three tools: two keep notes in the data folder, and one calls rush back to list agents.
2. Rename it, change its tools, and keep the plumbing.
3. Put it in `~/.config/rush/plugins/<name>/`, run `rush plugin check <name>`, then `rush plugin approve <name>`.
4. Call its tools without a session: `python3 skills/write-rush-plugin/scripts/call.py <name> list`, then `… call <tool> '{"arg": "value"}'`.

In short, it talks to rush on **fd 3**, one end of a socket pair rush made for it. Each message is JSON-RPC 2.0 behind a 4-byte big-endian length. rush calls `initialize`, `tools.list` and `tools.call`; the plugin can call `sessions.list`, `sessions.watch`, `sessions.start`, `sessions.send`, `sessions.queue`, `sessions.subscribe`, `sessions.stop`, `exec`, `sidebar.set` and the `ui.*` methods, if its manifest asks for them. A manifest with `"protocol": "mcp"` makes it an ordinary MCP server on stdin and stdout instead.

```jsonc
{
  "name": "delegate",                 // lowercase, digits, dashes; the folder's name
  "command": ["delegate"],            // a path in the folder, or absolute (an interpreter)
  "tools": true,                      // offer its tools to sessions (always, for "mcp")
  "sidebar": true,                    // may arrange rush's agent list
  "sessions": ["list", "start", "read", "send", "control"],
  "workspaces": ["~/Source"],         // where it may start agents
  "network": ["api.example.com:443"], // what it may reach
  "memoryMB": 64,
  "agents": { "reviewer": { "description": "…", "prompt": "…" } },  // as delegate:reviewer
  "prompt": "…"                       // added to every session's system prompt
}
```

Every method and event: [protocol](skills/write-rush-plugin/references/protocol.md). Every field: [manifest](skills/write-rush-plugin/references/manifest.md). When something won't start: [sandbox § troubleshooting](skills/write-rush-plugin/references/sandbox.md#troubleshooting).

## Examples

On Apple Silicon, build with `GOARCH=arm64` if `go env GOARCH` says `amd64`: an x86_64 plugin can't start in the sandbox, which blocks Rosetta.

- [`neighbours`](examples/neighbours) is the smallest useful plugin, and the one to read first: one tool that tells Claude which other agents are working in the same repository, and on which branch, so it doesn't trip over them. About 100 lines of Go, with only the `list` capability.

  ```sh
  mkdir -p ~/.config/rush/plugins/neighbours
  go build -o ~/.config/rush/plugins/neighbours/neighbours ./plugins/examples/neighbours
  cp plugins/examples/neighbours/plugin.json ~/.config/rush/plugins/neighbours/
  rush plugin approve neighbours
  ```

- [`kanban`](examples/kanban) connects rush to [kanban-code](https://github.com/langwatch/kanban-code), the board that shows coding agents as cards. It shows how a plugin works with another tool on your machine: it reads the tool's files and drives its CLI through `exec`. With it:
  - Claude can read the board, and the card it's working on, with the issue, the PR, failing checks and unresolved review threads.
  - Claude can start an agent on a card, in the card's worktree or a new one, tagged with the card. The plugin then has kanban-code link the card to the agent's conversation (`kanban relink`), so the card follows it across the board.
  - When a card's PR fails a check or gets a new review thread, the agent working on it is sent a message when its turn ends.
  - rush's agent list can show the board: `ctrl+s` (or `/by plugin:kanban`) groups agents by column, In Progress, Waiting, In Review, Backlog and Done, in board order, under their cards' names. Agents with no card go to Other, folded. The plugin reads `links.json` every 2 seconds and sends `sidebar.set` when the board changed.

  It needs kanban-code's app running for the link, and its CLI at `~/.local/bin/kanban`, where the app installs it. Change `workspaces` in `plugin.json` to where your projects are.

  ```sh
  mkdir -p ~/.config/rush/plugins/kanban
  go build -o ~/.config/rush/plugins/kanban/kanban ./plugins/examples/kanban
  cp plugins/examples/kanban/plugin.json ~/.config/rush/plugins/kanban/
  rush plugin approve kanban
  ```

- [`delegate`](examples/delegate) lets Claude hand work to agents of its own, follow them and message them. It's written in Go, uses every session capability, and brings a `reviewer` subagent.

  ```sh
  mkdir -p ~/.config/rush/plugins/delegate
  go build -o ~/.config/rush/plugins/delegate/delegate ./plugins/examples/delegate
  cp plugins/examples/delegate/plugin.json ~/.config/rush/plugins/delegate/
  rush plugin approve delegate
  ```

- [`memory`](examples/memory) is the reference MCP memory server, `@modelcontextprotocol/server-memory`, run unchanged: a knowledge graph Claude keeps across sessions, stored in the plugin's data folder.

  ```sh
  mkdir -p ~/.config/rush/plugins/memory && cd ~/.config/rush/plugins/memory
  npm install @modelcontextprotocol/server-memory
  cp <rush>/plugins/examples/memory/plugin.json .
  rush plugin approve memory
  ```

- [`autodrafts`](examples/autodrafts) and [`reconnect`](examples/reconnect) rebuild two of rush's own features with the UI hooks, to show they're enough: `autodrafts` keeps what you typed and didn't send and puts it back (`ui: events, input, notify`; it sees everything you type), and `reconnect` sends `continue` to a session an outage stopped once the network is back, with backoff (`ui: events, send, notify`). rush's built-in drafts and retry stay as they are. Each has its own `go.mod`, uses only the standard library, and says how to install it in its README.
