# plugin.json

The manifest lives at `<plugins>/<name>/plugin.json`. Unknown fields are an error, so a typo can't silently drop a restriction. Run `rush plugin check <name>` after every edit.

| Field | Type | Default | Rules |
|---|---|---|---|
| `name` | string | required | `^[a-z][a-z0-9-]{0,30}$`, and the same as its folder. Its tools are `mcp__rush-<name>__*`. |
| `description` | string | | Shown at approval and in `rush plugin list`. |
| `version` | string | | Shown at approval; sent as the MCP server version. |
| `category` | string | | What it's for, so rush lists it with plugins that do the same kind of thing, in Settings, Plugins and `rush plugin list`: one of `messages`, `voice`, `agents`, `code`, `pull-requests`, `machine`, `views`, `memory`, `integrations`, `other`. None is Other. It grants nothing. |
| `command` | string[] | required | The program and its arguments. The program is a path inside the plugin folder (no `..`), or absolute (an interpreter). It is run from the plugin folder. |
| `protocol` | `"rush"` \| `"mcp"` | `"rush"` | `mcp` means an MCP stdio server. It can't have `sessions`. |
| `env` | object | | Extra environment. `${DATA}` expands to the data folder and `${PLUGIN}` to the plugin folder, and a leading `~/` to the user's home (the plugin's own `HOME` is its data folder), so it can find the files it was given in `read` or `write`. It can't override `PATH`, `HOME`, `TMPDIR`, `RUSH_*`, or (with `network`) the proxy variables. |
| `tools` | bool | false | Offer its tools to every rush-mode session. Always on for `mcp`. |
| `sessions` | string[] | | Any of `list`, `start`, `read`, `send`, `control`, `queue`, `queued`. See `protocol.md`. |
| `workspaces` | string[] | | Absolute paths or `~/…`, not `/`. Required with `start`, `queue` and `queued`. |
| `network` | string[] | | `host:port` pairs: exact names, or public IPs, no wildcards. HTTPS through the proxy only. |
| `read` | string[] | | Extra absolute paths it may read, for an interpreter's libraries or another tool's files. |
| `write` | string[] | | Extra absolute paths it may write (and read), beyond its data folder: another tool's files. Not `/` or your home folder. |
| `exec` | object | | Programs rush runs for it **outside the sandbox, as the user**, by name (`^[a-z][a-z0-9-]{0,40}$`): each a command line whose program is an absolute path or `~/…`. The plugin adds arguments with the `exec` call. Not for `mcp` plugins. |
| `memoryMB` | int | 256 | 1–8192. The broker kills it above this footprint. |
| `agents` | object | | Subagents for every rush-mode session, keyed by name (`^[a-z][a-z0-9-]{0,40}$`), each as Claude Code's `--agents` takes them. `description` and `prompt` are required; optional `tools`, `model`. They appear as `<plugin>:<name>`. 64 KB in all. |
| `sidebar` | bool | false | May arrange rush's agent list with `sidebar.set`: its own sections, and a name for each agent. The list offers it as a group-by mode named after the plugin. Not for `mcp` plugins. |
| `ui` | string[] | | Any of `events`, `input`, `intercept`, `overview`, `notify`, `send`, `views`: what it may do in rush's own screen. See `protocol.md`. `intercept` needs `input`; `send` needs `events` and a workspace. Not for `mcp` plugins. |
| `commands` | object[] | | At most 32, each `{"name", "description", "key"?}`: `name` as `^[a-z][a-z0-9-]{0,30}$`, unique; `description` at most 200 characters; `key` a key it suggests, taken only if free. Shown as `plugin:<name>.<command>`; run as a `ui.command` request. Not for `mcp` plugins. |
| `settings` | object[] | | At most 32, each `{"key", "title", "description"?, "type", "choices"?, "default"?}`: `key` as a command name, unique; `title` at most 60 characters; `type` `bool` (default `"true"`/`"false"`), `choice` (2–16 `choices`, default one of them) or `text` (at most 1000 bytes). Shown under Settings, Plugins; values come in `initialize` and `ui.settings`. Not for `mcp` plugins. |
| `cli` | object[] | | At most 16, each `{"name", "usage"?, "description"}`: `name` as a command name, unique; `usage` (its arguments, as help shows them) and `description` one line of at most 120 characters. Run as `rush <plugin> <name> [args]`, which sends the plugin a `cli.run` request and prints its answer; rush's own commands take their names first. Not for `mcp` plugins. |
| `prompt` | string | | Added to every rush-mode session's system prompt under a heading naming the plugin. 16 KB at most. |
| `requires` | object | | `{"os"?, "arch"?, "bin"?}`, each a string array. `os` (`runtime.GOOS` values, e.g. `"darwin"`) and `arch` (`runtime.GOARCH`) gate by platform; `bin` names programs that must be on `PATH` (bare names, not paths). Any value within a field is enough; every field given must be met. Unmet, the plugin never runs, bundled or installed — no error, just left out of `rush plugin list` as off, with the reason. |
| `provides` | string array | | Services it answers for other plugins and rush, dotted lowercase names like `cleanup.report`: rush calls its `service.call` when one is asked for (protocol.md). A rush-protocol plugin only. |
| `uses` | string array | | Services it may ask other plugins for with `services.ask`. It's asking whoever provides them, not named plugins. |

Agents, prompt text and tools reach sessions **as approved**. Editing `plugin.json` changes nothing until the user approves again, and until then the plugin doesn't run.

## The environment the plugin gets

These values are fixed; the manifest can't change them:

| Variable | Value |
|---|---|
| `HOME`, `RUSH_PLUGIN_DATA` | `<rush>/plugin-data/<name>`, the only writable folder |
| `TMPDIR` | `<data>/tmp/` |
| `PATH` | `/usr/bin:/bin` (there's little point: it can't start programs) |
| `RUSH_PLUGIN` | its name |
| `RUSH_IPC_FD` | `3` (rush plugins only) |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` (and lowercase), `NODE_USE_ENV_PROXY=1` | its proxy, when it has `network` |

`LANG`, `LC_ALL` and `TZ` pass through from rush. Nothing else does: no tokens, no `CLAUDE_*`, no user `PATH`.

## Examples

**A rush plugin in Go** (a static binary; the most robust choice):

```json
{
  "name": "notes",
  "description": "Notes Claude keeps across sessions.",
  "version": "1",
  "command": ["notes"],
  "tools": true,
  "sessions": ["list"],
  "memoryMB": 32
}
```

**Python / Node** (Homebrew's interpreter; its libraries are readable automatically):

```json
{ "command": ["/opt/homebrew/bin/python3", "-I", "plugin.py"] }
{ "command": ["/opt/homebrew/bin/node", "plugin.mjs"] }
```

**An off-the-shelf MCP server** (`npm install @modelcontextprotocol/server-memory` inside the plugin folder):

```json
{
  "name": "memory",
  "description": "A knowledge graph Claude keeps across sessions.",
  "protocol": "mcp",
  "command": ["/opt/homebrew/bin/node", "node_modules/@modelcontextprotocol/server-memory/dist/index.js"],
  "env": { "MEMORY_FILE_PATH": "${DATA}/memory.jsonl" },
  "prompt": "You have a long-term memory in the mcp__rush-memory__* tools. Search it at the start of a task; record durable facts."
}
```

**A plugin that runs agents of its own:**

```json
{
  "name": "delegate",
  "command": ["delegate"],
  "tools": true,
  "sessions": ["list", "start", "read", "send", "control"],
  "workspaces": ["~/Source"],
  "agents": {
    "reviewer": {
      "description": "Reviews a change for correctness bugs. Use after a non-trivial change.",
      "prompt": "You review code changes for correctness …",
      "tools": ["Read", "Grep", "Glob", "Bash"]
    }
  }
}
```

**One that works with another tool on your machine** (kanban-code, here): it reads the tool's files, drives its CLI, starts a session per card in its own worktree, and queues review feedback to the sessions working the cards:

```json
{
  "name": "kanban",
  "command": ["kanban"],
  "tools": true,
  "sidebar": true,
  "sessions": ["list", "start", "read", "queue"],
  "workspaces": ["~/Source"],
  "read": ["~/.kanban-code"],
  "exec": { "kanban": ["~/.local/bin/kanban", "--json"] },
  "prompt": "Your task may be a kanban card: call mcp__rush-kanban__my_card to read it."
}
```

`exec` and `write` reach outside the sandbox, and approval says so. Name the narrowest program, and prefer `read` to `write`.

**One that calls an API:**

```json
{
  "name": "linear",
  "command": ["linear"],
  "tools": true,
  "network": ["api.linear.app:443"],
  "env": { "LINEAR_KEY_FILE": "${DATA}/key" }
}
```

Secrets: the plugin can't read the user's files or environment, so a key must live in its data folder. Ask the user to put it there (e.g. `~/.config/rush/plugin-data/linear/key`) rather than putting it in `plugin.json`: the manifest is shown at approval and is part of the plugin's folder.
