# The rush plugin protocol (version 1)

## Transport

**rush plugins** (`"protocol": "rush"`, the default): fd 3 is one end of a unix socket pair rush created for the plugin. `RUSH_IPC_FD=3` says so. Nothing else can connect to it. Every message, both ways, is:

```
[4 bytes: length N, big-endian uint32][N bytes: one JSON-RPC 2.0 message, UTF-8]
```

N is at most 16 MB; a bigger frame closes the connection. There is no newline, and nothing to escape.

**MCP plugins** (`"protocol": "mcp"`): standard MCP stdio, one JSON message per line on stdin and stdout. rush initializes the server once, with protocol version `2025-06-18` and no client capabilities, and forwards `tools/list` and `tools/call` from every session to that one process. It offers no sampling, roots or elicitation. The server's `instructions`, if any, go to each session.

JSON-RPC 2.0 throughout: a request has `id`, `method`, `params`; a reply has the same `id` and either `result` or `error: {code, message}`; a notification has no `id` and gets no reply. **Both sides send requests**, so a rush plugin must tell a reply (no `method`) from a request (`method` and `id`) from a notification (`method`, no `id`). Handle requests concurrently. Notifications arrive in order.

## Lifecycle

1. The broker checks the plugin's files against the approval, starts it (sandboxed, at utility QoS), and sends `initialize`.
2. The plugin answers within 15 seconds. It is then "running", and sessions' tool calls reach it.
3. When fd 3 closes, the plugin exits. If the plugin exits, or its fd 3 closes, or it goes over its memory limit, the broker starts it again after 1s, 2s, 4s … up to 60s, resetting once a run lasts a minute.
4. If its files change, the broker stops it and doesn't start it again until the user re-approves.

## rush → plugin

### `initialize` (request)

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocol": 1,
  "name": "notes",
  "dataDir": "/Users/you/.config/rush/plugin-data/notes",
  "sessions": ["list"],
  "workspaces": ["/Users/you/Source"],
  "network": [],
  "exec": [],
  "ui": ["events", "overview"],
  "commands": ["summarize"],
  "settings": {"verbose": "false"}
}}
```

Answer `{}` (anything but an error). `sessions`, `workspaces`, `network`, `exec` (the names of the programs it may run), `ui` and `commands` (their names) are what the plugin was approved for; `workspaces` are resolved to real paths. `settings` is each setting's value: what the user set, or its default.

### `tools.list` (request)

Answer `{"tools": [ ... ]}`, MCP tool definitions:

```json
{"name": "note", "description": "Keep a note for later sessions.",
 "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"], "additionalProperties": false}}
```

Tool names: letters, digits, `_` and `-`. Claude sees them as `mcp__rush-<plugin>__<tool>`, 64 characters at most in all.

### `tools.call` (request)

```json
{"jsonrpc":"2.0","id":7,"method":"tools.call","params":{
  "session": "a1b2c3d4",
  "name": "note",
  "arguments": {"text": "the build needs Go 1.27"}
}}
```

`session` is the id of the rush session whose Claude called the tool. When rush knows it, the call also carries `sessionId` (Claude Code's id for the conversation, the one its transcript and hooks use), `cwd`, and `meta` (what the session was started with, see `sessions.start`). They tell the plugin which of its things the caller is: a `my_task` tool, say, looks up the task by `meta` or `sessionId` rather than asking Claude. Answer an MCP `CallToolResult`:

```json
{"content": [{"type": "text", "text": "Noted."}], "isError": false}
```

Tool failures are `isError: true` results with a message Claude can act on. Reserve JSON-RPC errors for broken requests. A call may take up to 10 minutes. The user is asked to allow each call before it reaches the plugin.

### `session.event` (notification)

This is sent for each session the plugin follows with `sessions.subscribe`. It covers only what happens after the subscription, not history. `params.session` is the id, and `params.type` is one of:

| `type` | Other fields | Meaning |
|---|---|---|
| `info` | `state`, `detail`, `needs`, `costUsd` | Its state changed. `state` is `starting`, `working`, `blocked` (waiting for the user), `idle` or `stopped`. `detail` is what it's doing in words; `needs` is what it's blocked on. |
| `sent` | `text` | A message was sent to it (by anyone). |
| `text` | `text` | Claude said this (main thread, not subagents). |
| `tool` | `name`, `doing` | Claude used a tool; `doing` says what, in words. The tool's output is never sent. |
| `result` | `text`, `isError`, `costUsd`, `turns` | A turn ended; `text` is Claude's final answer. |
| `closed` | | The session's host went away; subscribe again once it's back if needed. |

### `ui.event` (notification) — needs `events` or `input`

Something happened in one of rush's windows. Sent without waiting for the plugin: one that falls behind loses its oldest events, and `input.changed` for the same box is sent only as its latest.

```json
{"kind": "turn.ended", "ui": "main", "at": "2026-09-28T10:00:00Z",
 "session": {"id": "a1b2c3d4", "sessionId": "5f0c…", "name": "fix the flaky test", "agent": "claude",
             "cwd": "/Users/you/Source/app", "repo": "/Users/you/Source/app", "branch": "main", "state": "idle", "hosted": true}}
```

| `kind` | Needs | Other fields |
|---|---|---|
| `session.seen` | `events` | a session rush shows: each open or at work today when the plugin connects (up to 150), then each new one |
| `session.opened` / `session.left` | `events` | its Session came into or went out of view |
| `turn.started` / `turn.ended` | `events` | |
| `session.stopped` | `events` | `error: {kind, message, retrying}` when an error did it; `kind` is `limit`, `auth`, `offline`, `retryable`, `too-long` or `other`; `retrying` is rush continuing the session itself (its own sessions, and others for a day, after `offline` or `retryable`), so a plugin needn't |
| `network.down` / `network.up` | `events` | no `session` |
| `input.changed` / `input.sent` / `input.cleared` | `input` | `text`, the message box; `input.changed` also has `box`, the whole box (below) |

`ui` names the rush window. Only `input` events carry `text`; a `session` absent from one means the Prompt's box. `input.changed` with empty `text` is a box emptied by hand (`input.sent` and `input.cleared` say so themselves). A plugin that connects while the API is unreachable hears `network.down` at once.

### `ui.command` (request)

`{"plugin": "…", "command": "summarize", "ui": "main", "session": {…}, "box": "a1b2c3d4", "input": {…}}`: the user ran one of its `commands`, on `session` if one was selected (as in `ui.event`, or absent). `box` is whose message box had the keys: a session's id, or `""` for the Prompt (where new sessions start). `input`, to a plugin with `input` only, is that box as it was when the key was pressed, whole:

`{"text": "see [Image #1] and [#2 4 lines: first…last]", "cursor": 3, "pastes": {"2": "…"}, "images": {"1": "/path/a.png"}}`

`text` is as the box shows it, with a long paste as its chip and an image as its marker; `cursor` counts characters from the start; `pastes` and `images` are what each chip and marker stand for. The Prompt's images are attachments rather than markers in its text, numbered 1, 2, 3. Answer `{}` within 10 seconds; the work may go on after.

### `ui.picked` (request)

`{"plugin": "…", "pick": "history", "item": "a1", "action": "enter", "ui": "main", "box": "…", "input": {…}}`: the user chose `item` from the plugin's pick `pick` (see `ui.pick`) with the action on key `action`. `box` and `input` are as in `ui.command`. Answer `{}` within 10 seconds.

### `ui.intercept` (request) — needs `intercept`

`{"hook": "before-send", "ui": "main", "box": "a1b2c3d4", "session": {…}, "text": "…", "asks": ["input"]}`: a message is about to go. `box` is whose message box it's in, a session's id or `""` for the Prompt; `session` is the session it goes to, absent for one it starts. `text` has each paste in full. `asks` lists the kinds of ask the window can show beside one with choices; it is absent from an older rush. Answer one of:

```json
{"action": "allow"}
{"action": "rewrite", "text": "the message, changed"}
{"action": "rewrite", "replace": [{"old": "sk-…", "new": "{{secret:KEY}}"}], "append": "\n\na line at the end"}
{"action": "block", "reason": "shown to the user"}
{"action": "ask", "id": "q1", "question": "Store KEY as a secret?", "detail": "a line under it",
 "choices": [{"key": "y", "label": "save it", "enter": true}, {"key": "n", "label": "send as is", "esc": true}]}
```

A rewrite gives the whole `text`, or changes it in place: each `old` becomes `new` wherever it is, then `append` goes at the end as it is. In place, the window keeps the box's pastes as chips; with `text`, the box becomes that text.

Plugins are asked in name order, each seeing the text as the ones before left it; a `block` stops it. Each has 250 ms, and all together 400 ms. No answer in time, or an error, counts as `allow`; three in a row and the plugin isn't asked again until it restarts.

**Asking first.** An `ask` stops the chain and shows the user `question`, `detail` and a key for each choice, in place of sending. It may carry `replace` and `append` too: what the plugin changed so far, put in the box while the question shows. Choices: 1 to 6, each `key` a lowercase letter or a digit, `label` up to 40 characters; `enter` on one makes enter choose it, `esc` on one makes esc choose it. ctrl+c, and esc when no choice takes it, cancel: nothing is sent and the box stays as it is. `question` is up to 120 characters, `detail` 400, `id` 128 bytes. An ask that breaks these counts as `allow`.

**Asking for a line of text.** When `asks` has `"input"`, an ask may carry `input` and becomes a line to type:

```json
{"action": "ask", "id": "q2", "question": "Name the secret", "detail": "a line under it",
 "input": {"value": "OPENAI_API_KEY", "error": "", "enter": "save"},
 "choices": [{"key": "b", "label": "back", "esc": true}]}
```

The line starts as `value` with the cursor at its end, and takes the usual editing keys and pastes. Enter answers with `key: "enter"` and `value`, what was typed, trimmed. Letters and digits are typed, so the ask has at most one choice and it must be on `esc`; with none, esc cancels as ctrl+c does. `enter` is what enter does, in a word or two (`ok` if left out). To refuse what was typed, answer with the same ask again, `value` as typed and `error` saying why: it shows under the line until the line changes. `value` and `error` are up to 200 characters, one line. Never send `input` when `asks` lacks `"input"`: an older rush would show only its choice.

### `ui.intercept.answer` (request) — needs `intercept`

`{"hook": "before-send", "ui": "main", "box": "…", "session": {…}, "text": "…", "asks": ["input"], "id": "q1", "key": "y"}`: the user chose `key` in the plugin's `ask` with that `id`. For an ask with an `input`, `key` is `"enter"` and `value` is the line typed. `text` is the message as it stands now, with the ask's own changes in it. Answer as to `ui.intercept`: `allow`, `rewrite`, `block`, or another `ask` (the next question about the same message). It has 2 minutes, so it can run a program first (`exec`). The plugins after it in name order are then asked as usual. An answer that doesn't come, or an error, holds the message back, since the choice may have been to keep something out of it.

A plugin that keeps secrets out of messages is built on this: its ask stores a pasted token (through `exec`) only when the user says yes, and its rewrite puts a reference in the token's place.

### `ui.settings` (notification)

`{"values": {"verbose": "true"}}`: the user changed a setting; these are all its values now.

### `cli.run` (request)

`{"command": "send", "args": ["a1b2c3d4", "0"], "cwd": "/Users/you/src/app"}`: the user ran `rush <plugin> send a1b2c3d4 0`, one of its `cli` commands. `cwd` is the folder they ran it in, to go by, not to read. Answer `{"stdout": "…", "stderr": "…", "exit": 0}` within 2 minutes: rush prints `stdout` and `stderr` (a megabyte of each at most) and exits with `exit`. What the command does, it does as the plugin, with what its manifest allows.

## plugin → rush

Each call is checked against the approved manifest. A refused call gets error **`-32001`** with a message saying why. Session ids are short strings like `a1b2c3d4`.

### `sessions.list` — needs `list`

No params. Returns every rush-mode session:

```json
[{"id": "a1b2c3d4", "sessionId": "5f0c…-…", "name": "fix the flaky test",
  "cwd": "/Users/you/Source/app/.claude/worktrees/flaky", "repo": "/Users/you/Source/app/.claude/worktrees/flaky",
  "branch": "fix-flaky", "worktree": true,
  "state": "working", "detail": "running go test ./...", "needs": "", "model": "claude-opus-5-5",
  "permissionMode": "acceptEdits", "costUsd": 0.42, "contextTokens": 81234, "queued": 1,
  "startedBy": "kanban", "meta": {"card": "card_2x…"}, "startedAt": "…", "updatedAt": "…"}]
```

- `sessionId` is Claude Code's id for the conversation, empty until it has started. Other tools (Claude Code's hooks, transcripts in `~/.claude/projects`, anything that reads them) know the session by it.
- `repo` is the top of the git checkout `cwd` is in, `branch` what it has checked out (the first 8 characters of a commit when detached), and `worktree` is true for a linked worktree.
- `contextTokens` is how much context the last request sent, the conversation's size as the model sees it. `queued` is how many messages wait for its turn to end.
- `startedBy` is the plugin that started it, or empty; `meta` is what that plugin tagged it with.

What was said is never included.

### `sessions.watch` / `sessions.unwatch` — needs `list`

`sessions.watch` takes no params and returns the list, as `sessions.list` does. From then on, rush checks every 2 seconds and sends:

| Notification | `params` | When |
|---|---|---|
| `session.changed` | a session, as in the list | one appeared, or anything about it changed |
| `session.gone` | `{"id": "…"}` | its host's files were removed (a stopped session stays, as `"state": "stopped"`) |

Call it once `initialize` has been answered (before then it fails; ask again). Watching again replaces the old watch. `sessions.unwatch` ends it. It also ends when the plugin restarts.

### `sessions.start` — needs `start`

```json
{"cwd": "/Users/you/Source/app", "prompt": "Update the changelog for 2.3",
 "name": "changelog", "model": "sonnet", "effort": "medium", "permissionMode": "acceptEdits"}
```

Two more fields, both optional:

- `worktree: {"name": "…", "branch": "…", "base": "…"}` runs the session in a new git worktree of the checkout `cwd` is in, at `<checkout>/.claude/worktrees/<name>`, where Claude Code puts its own. `branch` is the new branch (`worktree-<name>` if left out) and `base` what it starts from (`HEAD` if left out). `name` defaults to one made from the branch or the session's name. The checkout must be inside the plugin's workspaces too.
- `meta: {"card": "card_2x…"}` tags the session, and comes back in `sessions.list`, `session.changed` and every `tools.call` from it. At most 16 keys, of letters, digits and `_ . -`, with values of at most 1 KB.

Only `cwd` and `prompt` are required. It returns `{"id": "…", "cwd": "…"}`, `cwd` being where the session runs (the worktree's folder, if it made one). What rush enforces:

- `cwd` must be absolute, exist, and be inside one of the plugin's `workspaces`.
- `permissionMode` must be `default` (the default), `acceptEdits` or `plan`.
- `effort` must be `low`, `medium`, `high`, `xhigh` or `max`.
- The prompt is at most 100 KB.
- The session is named `<plugin>: <name>`, runs on the user's account and defaults, and is marked as the plugin's.
- A plugin can have at most 4 sessions running at once, and start at most 30 an hour.

### `sessions.send` — needs `send`, own sessions only

`{"id": "…", "text": "…", "now": false}`. When the agent is busy, the message queues and goes when its turn ends; `now: true` delivers it mid-turn instead. Text is at most 100 KB. Returns `{}`.

### `sessions.queue` — needs `queue`

`{"id": "…", "text": "…"}`. Queues a message to **any** session in the plugin's workspaces, not only its own, as a queued message of yours would be: it goes when the turn ends, or now if the session is idle. Returns `{}`. What rush enforces:

- The session runs in one of the plugin's workspaces (a session it started always may).
- Its permission mode asks the user first: `default`, `acceptEdits` or `plan`. A session in `bypassPermissions` or `auto` is refused.
- The text is at most 100 KB, and goes with a first line saying who it's from, `[from the rush plugin <name>]`, so neither Claude nor the user takes it for the user's.

### `sessions.queued.send` / `sessions.queued.remove` — needs `queued`

`{"id": "…", "index": 0, "was": "…"}`. Sends now, or drops, the message waiting at `index` (from 0) in the queue of any session in the plugin's workspaces (a session it started always may), and returns `{}` once the session says it's gone. The plugin never sees queued text: `sessions.list` gives only how many are `queued`. `was` is for rush's own bundled plugins only (naming a message by its text would let a plugin test guesses at what's queued): from any other plugin it's refused. Without it the message at `index` now is the one meant.

### `sessions.subscribe` / `sessions.unsubscribe` — needs `read`, own sessions only

`{"id": "…"}`. Returns `{}`; events follow as `session.event`. Subscribing again replaces the old subscription. Subscriptions end when the plugin restarts.

### `sessions.interrupt` / `sessions.stop` — needs `control`, own sessions only

`{"id": "…"}`. Interrupt stops the current turn; stop ends the session (its conversation is kept, and the user can resume it). Returns `{}`.

### `exec` — needs the program in `exec`

```json
{"name": "kanban", "args": ["show", "card_2x…"], "stdin": "", "cwd": "/Users/you/Source/app"}
```

Runs a program the manifest names in `exec`, **outside the sandbox, as the user**: its fixed command line, then `args`. `cwd` must be inside the plugin's workspaces; without it, the program runs in the plugin's data folder. It gets the user's environment. It returns when the program exits:

```json
{"code": 0, "stdout": "…", "stderr": "", "truncated": false}
```

A non-zero exit is a result, not an error. Limits: 64 arguments of at most 4 KB, 1 MB of stdin, the first 1 MB of each of stdout and stderr (`truncated` says if more was cut), a minute to run, and 4 programs at once.

### `sidebar.set` — needs `sidebar`

Arranges rush's agent list: the plugin's own sections, in its order, and a name and place for each agent it knows, by Claude Code session id (`sessionId` in `sessions.list`).

```json
{"title": "Kanban",
 "sections": [{"title": "In Progress"}, {"title": "Waiting"}],
 "agents": {"8c76706f-1c00-4aed-9c6d-7509f3033943": {"name": "Fix login bug", "section": "In Progress", "order": 0}}}
```

The list offers it as a group-by mode, `plugin:<name>`, labelled with `title`. In that mode the plugin's sections replace rush's, agents sort by `order` inside each, and each shows `name` unless the user renamed it in rush. Agents it doesn't place go to a folded section, Other. Each call replaces the last; empty params or no `sections` clear it. Returns `{}`. What rush enforces:

- At most 32 sections and 2000 agents; titles at most 64 characters, names 200. Section titles are unique.
- Every id matches `^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`, and every agent's `section` is one of `sections`.
- Escape sequences and control characters are removed from every string, and line breaks become spaces.

rush keeps it in `~/.config/rush/plugin-sidebar/<name>.json`, outside the plugin's reach, and removes it when the plugin is revoked. It only labels agents rush shows anyway: it grants the plugin nothing.

### `ui.overview.set` — needs `ui` `overview`

```json
{"session": "a1b2c3d4", "sections": [
  {"id": "ci", "title": "CI", "lines": [{"text": "3 checks failing", "tone": "bad", "url": "https://github.com/…"}]}]}
```

Replaces the plugin's sections in that session's overview; `[]` removes them. `session` is the `id` from `ui.event`. At most 4 sections, ids as `^[a-z][a-z0-9-]{0,30}$` and unique, titles 60 characters, 24 lines of 200. `tone` is `dim`, `good`, `warn`, `bad`, `accent` or empty; `url` only `http(s)`, opened with enter. Returns `{}`.

### `ui.status.set` — needs `ui` `overview`

`{"session": "…", "text": "CI red", "tone": "bad"}`: a short status on the session's row, at most 24 characters; empty `text` removes it. Returns `{}`.

### `ui.notify` — needs `ui` `notify`

`{"ui": "main", "session": "…", "text": "…", "tone": "warn"}`: a message at the bottom of the screen, at most 200 characters, after the plugin's name and, with `session`, that session's; without `ui`, in every window. Three at once, then one a second. Returns `{}`.

### `ui.input.set` — needs `ui` `input`

`{"ui": "main", "session": "…", "text": "…"}`: sets that message box, at most 100 KB: the session's, or with `session` `""` the Prompt's. The window sets it only while that box is on its screen (a session's Session open, or the Prompt taking a new session), else it's dropped. Returns `{}`.

With `"box": {…}` instead of `text`, it sets the whole box as `ui.command` shows one: chips, images (absolute paths) and cursor, at most 64 of each. With `"if": "…"`, the window sets it only while the box's `text` is exactly that, so nothing typed since the plugin looked is lost; `"if": ""` sets only an empty box. The set is one undo away.

### `ui.box.note` — needs `ui` `input`

`{"session": "…", "text": "stashed · ctrl+p brings it back", "tone": "dim"}`: a short note, at most 60 characters, on the bottom edge of that session's message box, or with `session` `""` the Prompt's; the tones are as for `ui.status.set`. Empty `text` takes it off. It goes when the plugin stops. Returns `{}`.

### `ui.pick`

`{"ui": "main", "pick": {"id": "history", "title": "Drafts", "about": "…", "tabs": ["Sent", "Cleared"], "tab": 0, "items": [{"id": "a1", "tab": 0, "text": "first line is the row\nthe next two a preview", "meta": "2m ago"}], "actions": [{"key": "enter", "name": "put it back"}, {"key": "ctrl+d", "name": "forget it", "stay": true}], "empty": ["nothing sent yet", "nothing cleared"], "session": "…"}}`: shows that window a list to choose from, filtered as the user types, in tabs `[` and `]` go through. Each action is a key, `enter` or `ctrl+`/`alt+` with a letter or digit, and sends `ui.picked`; one that `stay`s keeps the list open without the item. `session` is whose message box it's about (`""` the Prompt). It's shown only in answer to the user: within 10 seconds of a `ui.command` or `ui.picked` for the plugin in that window, else refused. At most 500 items, 6 tabs and 6 actions. Returns `{}`.

### `ui.send` — needs `ui` `send`

`{"session": "…", "text": "…"}`: the window sends it as if the user had typed it and pressed enter, or continues a session an error stopped. The session must have come in a `ui.event`, with its `cwd` in the plugin's workspaces, and ask before acting (not `bypassPermissions` or `auto`). Text is 1 byte to 100 KB; ten a minute. Returns `{}`.

### `ui.settings.get`

No params. Returns `{"values": {…}}`, as in `initialize`.

Everything a plugin adds to the screen is cleaned of escapes and control characters, and goes when it stops or restarts.

### `log`

`{"message": "…"}`. This writes a line (at most 1 KB) to the broker's log. It works as a request or a notification. Plain stderr works too, and goes to the plugin's own log.

## Error codes

| Code | Meaning |
|---|---|
| -32700 | parse error |
| -32601 | method not found |
| -32602 | bad params (bad id, cwd doesn't exist, text too long …) |
| -32000 | something failed (the session isn't running …) |
| -32001 | not permitted: the capability (or `sidebar`, or the `ui` one) wasn't approved, the session isn't the plugin's, the cwd is outside its workspaces, the program isn't in `exec`, a limit was hit |
| -32002 | too often: `ui.notify` or `ui.send` past its rate; the same call may go later |
