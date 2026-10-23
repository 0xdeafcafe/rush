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

A repository can ship plugins for whoever works in it: a folder `.rush/plugins/<name>` with a `plugin.json` and an `install.sh` that builds the plugin into `$RUSH_PLUGIN_DIR` (by hand, `$(rush plugin dir)/<name>`). When a session is open in that repository, rush offers to install it, showing the script it would run, as you, and then asks you to approve it as above. A newer `version` in the repository's `plugin.json` is offered as an update; "Not now" holds until the version changes.

New sessions pick up approved plugins. A running session picks them up the next time its Claude Code starts, which happens after it rests. Each of a plugin's tools asks you before it runs, like any tool, unless you allow it.

Plugins run under `rush plugind`, one small process rush starts once a plugin is approved or bundled and on, and that ends when none is.

### Bundled plugins

Some plugins come with rush. Their code is rush's own, so they need no approval and run without the sandbox, on every system, as `rush plugin run <name>`; what they may do in rush's screen and to your sessions is still their manifest's, checked by the broker as for any plugin. Each is on until you turn it off, in Settings, Plugins, or with `rush plugin off <name>` (`on` to turn it back). A plugin you install with the name of a bundled one doesn't run.

**gate** (off until you turn it on) shares the machine between the intensive programs your agents run, as an OS would rather than a queue (see [docs/gate-sharing.md](../docs/gate-sharing.md)). Each session rush starts finds a shim for each gated program first on its PATH, whatever the agent, and Claude Code sessions also get a hook that puts a Bash call running one (`npx tsc`, `pnpm exec vitest` too) under `rush gate run`. A run starts once there's memory for what it's expected to need, or at once when nothing else gated runs; waiters go fairly, the agent with the fewest runs going first. Its CPU is never held back: its processes are reniced as it ages (5 after 30s, 10 after 2m, 15 after 10m), so short checks finish first. A watchdog stops a run past its time or three times its memory, or whose agent is gone, and while memory is critical pauses the newest run until it's back to normal, telling the agent why. `rush gate status` shows each run's program, agent, age, nice and memory, and each waiter's place and why. `rush gate run [--name N] -- cmd…` does the same by hand. Its settings, in Settings, Plugins:

- **Programs**: the names to gate (default `tsc, tsgo, tsrs, go, cargo, rustc, webpack, vite, next, vitest, jest, playwright, golangci-lint, oxlint, eslint, biome`).
- **Time limit**: how long a run may go (default 30m). **Kept free**: the share of memory no run starts into (default 15%).
- **Per program**: `name=memory/time limit`, either left out, such as `tsc=6G/45m, vitest=4G`. Defaults: tsc and tsgo 4G, vitest, jest, next, webpack and vite 3G, playwright, go, cargo and rustc 2G, the lint tools 1G.

A ticket is a locked file, so it frees itself however its holder ends. A program run from inside a gated one (`go vet` under `go test`) isn't gated again. The slot pool's old settings (shared across, at once, between starts, busy above) are ignored.

Past the gate, a guard stops the largest program under any agent, or named as a gated program wherever it runs, once the memory left falls to the floor or the kernel calls it critical (**Last resort**, default 5%, or off).

**ts** (off until you turn it on) gives every agent TypeScript code intelligence: `definition`, `references`, `implementation`, `hover`, `callers`, `callees`, `outline`, `diagnostics` and `find_symbol`, as tools in rush-mode sessions and as `rush ts <tool> --file F --line N --symbol S` from a shell. One language server answers for each checkout, shared by every agent in it: [tsrs](https://github.com/maschwenk/tsrs) where it's installed (`npm i -g @maschwenk/tsrs`, or in the project), else the project's tsgo. It starts through the gate, and is stopped past its memory cap (**Memory cap**, default 4G) or once idle (**Idle**, default 10m); `rush ts status` shows them. Why: [docs/typescript.md](../docs/typescript.md).

**drive** (off until you turn it on) drives pull requests for your agents. `rush pr follow <pr>` gives an agent what's left before merge, in the order to do it, then rush follows the PR and wakes the agent only with news it can act on: a failed check with its log, a trusted review finding, a conflict, ready for approval. `rush pr reply` and `rush pr resolve` answer review threads, and agents are told to use them. Off, `rush pr` says so, and the PRs rush follows stop being followed.

**watch** (off until you turn it on) has agents wait on a pull request's checks with `rush watch pr`, which asks GitHub for them cheaply and says once what happened, instead of polling it. Agents are told to use it, and in Claude Code sessions `gh pr checks --watch` becomes `rush watch pr` and a sleep before asking GitHub is turned back. Off, agents aren't told and their commands go as they are.

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
- **`intercept`** (needs `input`): be asked before a message you send goes, and change it or hold it back with a reason. All plugins together get 400 ms; after that the message goes as it was, and one that misses three times in a row isn't asked again until it restarts.
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
