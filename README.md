<p align="center">
  <img src="docs/brand/rush-icon.png" width="180" alt="rush's icon: a tilted amber bottle labelled RUSH, AI harness polisher, not for agent consumption">
</p>

<h1 align="center">rush</h1>

<p align="center">
  one terminal app to run every coding agent you've got, written in go.<br>
  it sits at about 40 mb of ram with a session open, a fair bit less than the ~330 claude code's own view needs to show you one.
</p>

<p align="center">
  <img src="docs/screenshots/session-code.webp" alt="rush: a Session with a failed step opened, its script and error">
</p>

## install

```sh
go install github.com/0xdeafcafe/rush/cmd/rush@latest
```

needs go 1.27.1 or newer, and `$(go env GOPATH)/bin` on your path. if you're on apple silicon check `go env GOARCH` says `arm64`. if you're on intel, lol.

```sh
rush            # open it
rush on         # make `claude agents` open rush
rush off        # give it back
rush menubar    # limits and waiting agents in the macos menu bar
rush update     # fresh bottle
```

`?` has the keys, and `#ask <question>` asks rush about itself (it can change its own settings for you too).

## what it does

- **the list**: all your agents, what each one's doing in words ("running pnpm test") and what it's costing you in tokens, dollars, cpu and ram. anything that's died, hit a limit or is sat waiting on a question floats to the top, `ctrl+b` tells it to crack on, and `ctrl+x` (or esc twice in a session) asks whether to close it, restart it, switch its harness or model, or delete it for good.
- **sessions**: steps fold away, code's highlighted, and when a bash chain hangs it tells you which command it's stuck on - `k` is the safety shears, cutting that one loose so the rest of the chain carries on. every hunk in a diff knows which turn wrote it too.
- **overview**: what's going on across your repos right now, and everything that happened today.
- **efficiency**: where your tokens are going, and whether rtk, serena and the rest of the token savers are doing anything for you. there's also an opt-in advisor where haiku does the digging and opus checks its homework, for about $0.30 a pass.
- **machine**: process trees, dev servers a finished session left running, and each worktree with whether binning it would lose you anything (`A` bins all the ones that wouldn't).
- **zen** (`ctrl+z`): the agent that needs you and its message box, then the next one.
- **`ctrl+k`**: jump anywhere, and search all the transcripts while you're at it.
- **agents running agents**: when a session shells out to `claude -p` or `codex exec`, rush files them under its subs with what they were asked and what they're up to.

rush also puts the cap back on an idle claude code a few seconds after its turn ends, where it'd otherwise sit on 150-200 mb for five minutes doing nothing, and your next message has it back up in about a second with the prompt cache still warm.

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/session.webp" alt="The conversation"></td>
    <td width="50%"><img src="docs/screenshots/changes.webp" alt="Changes, with a new file's diff"></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/overview.webp" alt="The overview"></td>
    <td><img src="docs/screenshots/command-bar-search.webp" alt="The command bar searching agents and transcripts"></td>
  </tr>
</table>

## conversation keys

The same keys work with every harness: **Enter** or **Space** expands or collapses the selected row; Enter sends only from the message box, so a draft left there is never sent while a row is selected. **Esc** returns to the message box without clearing that draft. Typing also returns to the box; Space types a normal space when the box has focus. **Shift+Tab** opens model, effort and harness controls, and **F1** shows the current shortcuts.

## providers

rush runs anything it has an adapter for, and only shows the ones you've got installed.

| provider | how | support |
| --- | --- | --- |
| claude code | headless, hosted by rush | full |
| codex | `codex app-server` | tested |
| copilot | copilot cli, via `gh`'s token | tested |
| kimi | agent client protocol, Kimi Code 2 history | tested |
| Antigravity (`agy`) | persistent streaming JSON | preview |
| opencode, vibe | agent client protocol | preview |
| deepseek | `dsh` | preview |
| glm | zcode, via `zcode-acp-server` | preview |
| ollama | claude code on ollama, tuned per model | preview |

full is what i use every day, tested has been run against the real thing, and preview is built but hasn't been tried against it yet.

- **switching**: `#new codex@openai-sub` starts a session on codex, once, and `#handoff codex` passes the bottle, so codex picks up in a new session with the conversation so far, what changed and what's left to do.
- **accounts**: all your sign-ins per provider, with their limits. switch account and each rush session moves over once it's safe, sane and between turns, bringing its conversation and queue with it.
- **profiles**: which providers a folder runs on and in what order, and what happens when one taps out - wait for the reset, try another account, or hand the conversation on to the next provider. each provider comes with one of its own out of the box.
- **harnesses**: some providers can be strapped into more than one, so ollama's models run in claude code by default, or in pi or codex if you'd rather.
- **ollama**: sessions get a prompt cut down to the six file and shell tools, and nothing reaches anthropic. on an m1 max with `qwen3-vl:30b` that took the prompt from 14k tokens to 4k, and the first answer from over a minute to about seven seconds.

## plugins

plugins add tools, subagents, prompt text and memory to the sessions rush hosts, and can run agents of their own (any mcp server can be one). each one's tied up in a sandbox - its own files, the hosts it named and no programs - and `approve` reads you its limits before you say yes. macos only for now.

```sh
rush plugin list
rush plugin approve <name>
```

more in [plugins/](plugins).

## embedding

other apps can run sessions headless and show one in a terminal of their own.

```sh
rush session start --cwd DIR [--agent A] [--profile P] [--session-id UUID] --json
echo 'next message' | rush session send <id>
rush session interrupt|stop|info <id>
rush session list --json
rush open <id> --hosted
```

## how it works

rush reads claude code's files (job state, roster, transcripts) and only changes things through claude code itself, via its daemon socket or the cli. the one exception is switching accounts, which writes the sign-in into claude code's keychain item and `~/.claude.json`. other agents go through [adapters](internal/adapters).

the socket and the files aren't documented and any claude code update can change the formula, so rush is checked against claude code v2.1.280. if an update moves things around, the affected column shows `–` and opening an agent falls back to `claude attach`.

costs are estimated at list prices, so your bill might say something different.

## more

[docs/guide.md](docs/guide.md) is the full manual, what's coming included, and it's also what `#ask` reads.

sold as an ai harness polisher. not for agent consumption.
