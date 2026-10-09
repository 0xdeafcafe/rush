<p align="center">
  <img src="brand/rush-icon.png" width="180" alt="rush's icon: a tilted amber bottle labelled RUSH, AI harness polisher, not for agent consumption">
</p>

<h1 align="center">rush</h1>

<p align="center">
  One place for every coding agent you run, written in Go and built to be very fast.<br>
  It started as a replacement for Claude Code's own view, which takes about 330 MB to show you one session. Now it runs Codex, Copilot, local models on Ollama and the rest the same way.
</p>

<p align="center">
  <img src="screenshots/session-code.webp" alt="rush: a Session with a failed step opened, its script and error">
</p>

<table>
  <tr>
    <td width="50%" valign="top">
      <b>Every agent at a glance</b><br>
      What each one is doing, in words, with its cost, tokens, time, CPU and RAM. Claude Code, Codex, Copilot, Gemini, Kimi, OpenCode, Vibe, DeepSeek, GLM and Ollama, in one list.
    </td>
    <td width="50%" valign="top">
      <b>Sessions you can read</b><br>
      Highlighted code, commands in plain words, which part of a Bash chain is stuck, diffs, the files changed, the queue, tasks and subagents.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <b>What needs you, first</b><br>
      Turns that died on an error or a limit, questions, then turns that finished. Overview shows what's happening across every repo, and what happened today.
    </td>
    <td valign="top">
      <b>Providers and presets</b><br>
      Every account of every agent, with its limits. A preset says which agents a folder runs on, and what to do when one runs out, down to handing the conversation to the next agent.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <b>Where the tokens go</b><br>
      What you spent it on, which savers would cut it, what each might have saved you, and an optional advisor that goes looking for more.
    </td>
    <td valign="top">
      <b>Light</b><br>
      About 40 MB with a session open. The native view uses about 330 MB, and an idle Claude Code is stopped a few seconds after its turn.
    </td>
  </tr>
</table>

## Install

```sh
go install github.com/0xdeafcafe/rush/cmd/rush@latest
```

This needs Go 1.27.1 or newer. It puts `rush` in `$(go env GOPATH)/bin`, so make sure that folder is on your `PATH`. On Apple Silicon, check that `go env GOARCH` says `arm64`: an amd64 Go builds an x86_64 rush that runs under Rosetta. From a checkout, `GOARCH=arm64 go build -o "$(go env GOPATH)/bin/rush" ./cmd/rush` builds it natively.

```sh
rush            # open it
rush on         # make `claude agents` open rush (adds one line to ~/.zshrc)
rush off        # give `claude agents` back to Claude Code
rush menubar    # put rush in the macOS menu bar (`rush menubar off` takes it out)
rush update     # install the newest rush
```

rush checks for a newer version now and then and says so at the foot of the list. `#update` installs it from inside, the same as `rush update`; reopen rush to use it.

Getting started sits at the foot of the list until you've tried the basics, ticking each off as you go. `#tips` brings it back, `#tips off` puts it away, and `?` is a guide to the keys.

## Agents

The list is every agent you have, with what it's doing right now: "running pnpm test", "reading view.go", "searching for PrettyModel". Working agents stand out; idle, stopped and done ones step back. Each row carries its provider's glyph and colour, so a Codex session doesn't pass for a Claude one.

- **Needs you** comes first: an agent whose turn died says why, with a ✗ ("session limit · resets 5am"), and one with a question waits there too. One that finished without asking waits in **Your turn**. `ctrl+b` tells either to go on. A turn that died on the network or a flaky API ("Connection dropped", "Response stalled mid-stream") is told to continue by itself once the API can be reached.
- **Cost, tokens and time** for each agent, estimated from its transcript at list prices (subagents included), and today's spend per account.
- **CPU and RAM** for everything an agent started, not just the agent itself.
- **Preview** (`{` or `}`): the agent's own screen, live, or what it's doing, its last message and its process tree. Type to reply without opening it.
- **Open** (`enter`) connects to the running session through the daemon, like the native view. `ctrl+]` comes back.
- **Agents that run agents**: when a session runs `claude -p`, `codex exec` or another agent's program from its shell, the row reads as that agent, with what it was asked and its latest steps. It's listed with the session's subagents rather than as a stray row of its own.
- **Done** (`ctrl+d`) moves an agent out of the way and stops its process if it's idle. A message resumes it. Nothing is merged or deleted.
- **Cold cache warning**: sending to a session idle past its prompt cache's hour asks first, since it re-reads the whole context uncached. `c` has a cheaper model (the `#compact` default, else Claude's Haiku) compact it first instead: your message waits in the fresh conversation's box, whose cache is warm.
- **Groups** (`ctrl+s`) by status, agent or your own (`ctrl+e`), and **split by project** on top (`ctrl+p`, on by default): inside each section, agents sit together under a line per repository with its branch, commits ahead or behind, uncommitted changes and worktree count; agents in a linked worktree sit under the repository it came from, headed by the worktree's own branch and changes. The top of the list shows both and a click changes either. Pins (`ctrl+t`) are shared with the native view.
- **Move** (`ctrl+l`) tells the agent to work in another folder or worktree from now on, without stopping it; its row follows it there. With a new session half typed, the same dialog picks where it starts. It lists the projects rush knows apart from other folders, and folders on disk beside them or under a typed path, so a project rush hasn't seen yet is a pick away. Each session's scratch (its `TMPDIR`) lives in `~/.config/rush/tmp/<project>/<session>`, following it when it moves to another project; working in it doesn't move the session. Agents are told to keep scratch there. Anything a session writes outside its project and scratch (from file edits, or plainly from shell commands) is noted: its Overview lists it, Projects shows it under Left outside, and Clean up (`c`) ticks only untouched scratch in temp folders. Anything else must be ticked by hand, and something that was there before the session, changed since, is kept by git or sits near the top of the disk is never deleted.
- **Stash**: `ctrl+p` sets what you've typed aside and clears the box for the next thing; it comes back by itself once you send, or on `ctrl+p` again. `ctrl+r` (or `#stash`) is the history: what you stashed, sent, cleared, and what a put-back replaced, to search and put back with `enter`. Every box is kept until it's sent, even across restarts. In the Prompt these two keys are the stash's only with something typed; empty, they split and rename the list as before. In a Session `ctrl+s` sends now, as `ctrl+enter` does. Settings, Plugins, drafts can call it Drafts instead.

The queue, running agents, background jobs and question panels scroll with the conversation. Scroll up to read without those panels taking space; the message box stays fixed and current activity remains visible. Navigating a pending question brings its panel back into view.

The conversation minimap at the right shows the rendered history, including folded turns and wrapped text. Its shaded marker shows your position. Click to jump or drag to scroll. Settings › Appearance › Look › Conversation minimap hides or shows it; panes narrower than 96 columns hide it automatically. `RUSH_HIDE_MINIMAP=1` disables it for one run.

Conversation history keeps short replies readable and previews longer replies on separate lines below the prompt. Select a turn and press `enter` (or `space`) to open its full exchange. `#expand` (Ctrl+] then E) opens all turns; `#collapse` (Ctrl+] then S) previews older turns while keeping the latest and active turns open. These controls preserve your draft and tool-detail folds. Turns you open individually stay open as new messages arrive.

## Providers

rush runs any agent it has an adapter for, and shows only the ones you have installed. It finds them on your `PATH` and where their installers put them, so an agent installed while rush is open turns up without a restart.

| Provider | How rush runs it | Support |
| --- | --- | --- |
| Claude Code | headless, hosted by rush | full |
| Codex | its own `codex app-server` | tested |
| Copilot | the Copilot CLI, signed in with `gh`'s token; its coding agent's sessions on GitHub too, with their repo and PR | tested |
| Kimi | Agent Client Protocol, Kimi Code 2 session history | tested |
| Gemini, OpenCode, Vibe | over the Agent Client Protocol | preview |
| DeepSeek | `dsh`, DeepSeek's own agent | preview |
| GLM | ZCode, Z.ai's own agent, through `zcode-acp-server` | preview |
| Ollama | Claude Code on Ollama's Anthropic API, tuned per model | preview |

Full is what I use every day, tested has been tried against the real program, and preview is built but not yet tried against it. Settings › Providers lists whose models you use; Settings › Harnesses lists the programs that run them; Settings › Profiles lists your profiles and the folders that pick one. Select a provider or harness to see its account, what it runs on and Rush features immediately beside the list (✓ supported, – unavailable, ◌ planned). Right or Enter focuses those visible controls; Escape returns to the list. General covers session behavior; Appearance covers layout, colors, list columns and the message box. Forms keep the selected setting’s explanation visible, with live previews for visual settings. `?` expands the full explanation, Home/End jumps to the first/last control, and Ctrl+Up/Down jumps between sections. Click a row to select it and click again to activate; the mouse wheel moves through settings. Actions such as Sign in and Refresh have an explicit key label; only changeable choices have arrows. PgUp/PgDown scroll details on short terminals; Tab or [ ] changes settings pages.

`#new codex@openai-sub gpt-6-astra high fix the test` starts one session on any harness, provider, model and effort, once: each word completes as you type, and the input's border says what it starts, or why it can't and what to try (`#new` alone lists every route). Without a task it's what the next session starts as; `#with` is the same. Neither changes a default: Settings does. Every session, including Claude Code and Kimi, has an explicit harness label in the list; long titles shorten before the label does. Past sessions remain in the list, and a message resumes one with its own agent, model and mode. A Codex running in a terminal is followed as it works, as a Claude Code one is. In a session, `/` offers only what that agent can do.

**Ollama** runs Claude Code in a folder of its own, so a local model doesn't read your plugins, skills and MCP servers before every answer. Each session is fitted to its model: loaded before the first turn, told the context window it was loaded with so it compacts in time, one model for everything (titles, subagents, summaries) so nothing reaches Anthropic, and a prompt cut down to the six file and shell tools. On an M1 Max with `qwen3-vl:30b` that took the prompt from 14k tokens to 4k, and the first answer from over a minute to about seven seconds. With no model named it picks one already in memory, then one made for code, and refuses one that can't call tools.

### Fork

`#fork codex` (or any agent that can take one on) copies this conversation to that agent in a new session, in the same folder, opened with the conversation so far: how it started, what it did, the files it changed, where it left off and what's still to do. Both sessions stay. `#use codex` switches the open session instead, and `#preset` picks a saved setup. These were called `#handoff`, `#agent` and `#profile`; the old names still work.

### Accounts

Settings › Providers lists each installed provider with the account it's on and its tightest limit. Anthropic and OpenAI are listed twice: their subscription, which runs in Claude Code or Codex (or in Pi on Pi's own sign-in, with a warning), and their API key, which runs in any harness that speaks their API. Everything about the one picked shows beside it: its accounts (who each is, its plan, its limits, and which is in use) or its key, the harnesses it can run in (the default, ★), what its new sessions start with, what it does at a limit, and what rush can do with it. `enter` goes into it, `esc` back to the list; on a narrow terminal the list sits above the selected details. Every agent runs from its own home (`~/.claude`, `~/.codex`, …); an account is a sign-in swapped into it, so settings, transcripts and history are shared.

- On an account, `enter` switches to it. `a` adds an account, `r` renames, `l` signs in again, `d` forgets.
- A running agent keeps the account it started with, so after a switch each rush session starts again on the new one at its first safe point: an idle one at once, one in a turn once the turn ends, one with work in the background once that's done. The conversation and its queue carry over.
- In the list, `1`–`9` jump to a provider, `*` makes it the default, `r` reads every limit again, and `n` makes a new preset.
- Codex keeps several sign-ins in rush's vault and swaps one into `~/.codex`. Copilot runs on whichever of `gh`'s GitHub accounts you pick, without changing `gh`'s own. DeepSeek shows its balance; GLM its Coding Plan's limits.

The top bar shows which provider, account and preset new sessions start on, that account's limits, and the battery and free disk. A provider on Settings › Providers also sets the model, effort and permissions its new sessions start with, plus what's that agent's own (for Claude Code: agent definitions, settings.json and its environment). A preset of yours on Settings › Profiles sets which providers new sessions run, in order, which harness each runs in, what happens when their accounts run low, and which folders get it.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="screenshots/accounts.webp" alt="Accounts, with each sign-in's usage"><br>
      <b>Accounts</b>
    </td>
    <td width="50%" valign="top">
      <img src="screenshots/coding-agents.webp" alt="Coding agents"><br>
      <b>Coding agents</b>
    </td>
  </tr>
</table>

### Presets

Every installed provider is a preset of its own, built in: sessions under `claude` run Claude alone, sessions under `ollama` run Ollama's models alone. You don't make or keep these. The presets you make group providers, in order, with what to do when they run out:

- **Stay or mix**: new sessions stay on the first provider, or move on to the next once every account of the current one is nearly out.
- **At a limit**, for a conversation a usage limit stops: `wait` for the reset, move to another `account` of the same provider and carry on, or `handoff`, which tries another account first and then hands the conversation to the next provider in the list with room.

A provider can run in more than one harness, the program around the model. Ollama's models run in Claude Code (the default), Pi or Codex. Settings › Harnesses shows every provider a harness runs on, and why not the rest. On Settings › Providers, `enter` on a harness makes it the provider's default; `#new pi@ollama` runs one session there without changing it. In a preset of yours, `h` on a provider chooses again for that preset alone. `#preset ollama-pi` runs one session on Ollama in Pi, whatever the setting.

A session gets the preset picked for it (`#preset <name>`, or `rush session start --profile`), else the one for the longest folder rule its folder falls under, else the default. It keeps that preset when it's resumed. If rush had made a Default preset from your old default agent that did no more than that agent, it gave way to that provider's own preset, and folders that named it moved with it.

`ctrl+]` then `w`, from anywhere, lists every preset, each provider's own first: pick one to make it the default. Settings › Presets does the rest: new presets, renaming and deleting them, the order of their providers and where each runs, what happens when accounts run low, and folders (`+ add a folder` starts from the selected session's folder).

### Compaction

`#compact` lets you choose native harness compaction or an eligible local model to write a summary. An Ollama decision model (`tev1`, `nimble`) shows as **Ollama prune**, experimental: rather than a summary, it scores each tool step before the latest two turns for whether the work still needs it, and the new conversation keeps those word for word and cuts the rest to their calls. In a benchmark on three real sessions judged by Codex, `tev1` ranked steps below Claude Haiku (AUC 0.73 against 0.84) at about 9 times the time (1.6 seconds a step), and keeping its top fifth kept 1 of 12 needed steps where Haiku's kept 6, so check what it kept with `/rewind` to hand. Settings › Ollama › Pruning pulls, mends and removes decision models and picks the one that prunes. Press `d` on a choice to save it as the picker’s default; this does not replace the harness’s automatic compaction. Native compaction remains available according to the harness’s capabilities.

A typed `/compact` in a rush-mode session whose prompt cache has gone cold takes the cheap path when it can: the `#compact` default writes the summary, or Claude's Haiku in a Claude session with no default saved, rather than the session's own model reading everything again uncached. With the cache warm the harness compacts it itself, reading from the cache at a fraction of the price. `/compact native`, `/compact <instructions>`, a session mid-turn and one that can't take an external summary (below) get the harness's own. Either way the compaction shows in the working line's place, never as a message of yours.

External summaries require a harness with rewind support and a host using protocol 9 or later. Older hosts show a restart instruction; Rush does not restart them automatically. The host applies the summary only if the conversation is still idle and unchanged since summarization began, with no queued messages. Otherwise the conversation stays intact. Successful external compaction keeps the previous conversation under `/rewind` and starts fresh with the summary.

Local summarization processes long conversations in ordered chunks before combining them, using a bounded context or your saved Ollama context size. Failures leave the conversation unchanged. Decision classifiers such as `tev1:4b` remain visible as installed models, but cannot summarize conversations or run coding harnesses: they select one supplied option rather than produce a working summary.

## Sessions

An agent run in rush mode (headless, hosted by rush) opens in a Session beside the list. `[` and `]` move between its views: Conversation, Overview and Changes always, then Tasks, Queue, Subagents, Background and Artifacts when there's something in them, and Memory. With room to spare, the foot of the list beside it shows what the next agent starts as (harness:account · model · effort) and in which folder (a click opens the start sheet), and up to three setups agents in the fleet ran lately (a click makes one the next agent's); what agents did lately fills the rows above it.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="screenshots/session.webp" alt="The conversation"><br>
      <b>Conversation</b>. Clean steps fold to one row; a failed one shows the line that says what went wrong. Shell commands say what they do, and code is highlighted. Wide code and output wraps rather than being cut off.
    </td>
    <td width="50%" valign="top">
      <img src="screenshots/session-code.webp" alt="A failed step opened, its script and error"><br>
      <b>A step, opened</b>. The script it ran and the error it hit. Drag to select and copy; <code>ctrl+]</code> then <code>c</code> copies Claude's answer as written.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <img src="screenshots/overview.webp" alt="The overview"><br>
      <b>Overview</b>. Spend, tokens and requests, cost per turn in each model's colour, where the model and effort changed.
    </td>
    <td valign="top">
      <img src="screenshots/changes.webp" alt="Changes, with a new file's diff"><br>
      <b>Changes</b>. Every file the session changed, with its diff, which turn made each hunk and your review marks, and the rest of the working tree beside it.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <img src="screenshots/subagents.webp" alt="The subagents view"><br>
      <b>Subagents</b>. Every run, with its steps, tokens, cost and last words, working for as long as it is, and the selected run's conversation beside it. <code>enter</code> watches one. <b>Stack</b>, the view after it, shows the agent and its subagents at once: a band each, top to bottom, the agent's own conversation first and then the subagents most recently active, each its header (state, steps, time) over the end of its conversation. Hover a band and it takes half the height; click one to watch it (the agent's band goes back to the conversation). Pull the subagents panel's title up (press, drag up three rows) or wheel up on it to open the stack; <code>ctrl+] g</code> opens it or goes back.
    </td>
    <td valign="top">
      <b>Queue</b>. Messages sent while the agent works wait here. <code>enter</code> edits one, <code>shift+↑↓</code> merges it into the one above or below, <code>[</code> <code>]</code> move it, <code>s</code> sends it now, <code>ctrl+enter</code> sends everything. Several go as one message, each numbered, so the agent reads them as separate requests.<br><br>
      Claude's questions arrive as one form, with a preview beside each option. Long pastes stay a chip: click one, or press space with the pointer on it, to open it in place; <code>ctrl+g</code> opens one in <code>$EDITOR</code>.
    </td>
  </tr>
</table>

Running shells, monitors, workflows and subagents sit in the dock under the conversation, each with its latest line: `↑` picks one, `b` sends it to the background, `x` stops it. A running Bash chain says which of its commands runs now and how long each finished one took ("3/4 go test"), and `k` ends the one it's stuck on and lets the chain carry on as it would after a failure. A tool call to allow, a question or a usage limit opens over the conversation and takes the keys; `esc` sets it aside in the dock.

`/` in the message box has the agent's commands and skills, and rush's own sheets for the ones Claude Code draws itself:

- `/fork` a conversation, from any turn, into the same folder or a new worktree, with its own model, effort and permissions.
- `/rewind` to before one of your messages, keeping the code or putting the files back, with a note of what the dropped turns learned.
- `/btw` asks a side question in a panel while the agent keeps working. Its text drags to copy, as the conversation's does.
- `/context`, `/status`, `/usage` and `/stats` open as one sheet: what fills the context, the limits, and your history by day, hour and model.
- Context is shown against the window the session compacts in. When Claude Code's `CLAUDE_CODE_AUTO_COMPACT_WINDOW` (in the environment or a settings file's `env`) or `autoCompactWindow` setting makes it smaller than the model's, it reads `130% · 520k of 400k · auto-compacts at 367k · model 1M`.
- `/plugins`, `/skills`, `/permissions`, `/hooks` and `/statusline`, and `/model` and `/effort` pickers.

When a turn ends and nothing is left running, Claude Code is stopped a few seconds later instead of sitting on 150–200 MB for five minutes doing nothing. The next message starts it again in about a second, with the prompt cache intact.

## Overview

The place next to Agents says what's happening across every repo, in three pages `[` `]` moves between: Now, Projects and the Wall.

**Now** is each working session: what it's doing, how far through its tasks it is, how full its context is, the heavy work running in its checkout (installs, type checks, tests, dev servers), and under it every subagent with the last thing it said, or the report it handed back.

**What happened** is the last day, newest first: what you asked, tasks planned and ticked off, subagents started and finished with their reports, background commands ending, turns ending with what they said, questions, commits, PRs, API errors and compactions. `f` keeps only the picked session's history; `enter` opens it in Agents.

**Projects** is each repository an agent worked in over the last day, whole: what its agents are doing, where it is and where it pushes, its branch with commits ahead and behind and uncommitted changes, its last three commits, the pull requests its agents opened with their checks and review, and every linked worktree with its own branch and changes, each with the agents working in it (or none). Folders that aren't repositories come last. `enter` opens an agent, `ctrl+y` its pull request.

**Wall** is every agent at once, streaming.

## Go anywhere

`ctrl+k` opens the command bar: a place, an agent, a Session's view, a turn (`#12`), `Back` to where you jumped from, or a new agent with what you typed. Words search the open conversation and every agent's transcript, and `in:name`, `is:failed`, `file:x` and `turn:10-13` narrow it. `ctrl+f` is the same bar, starting where you are. With many long transcripts, Settings › Appearance › *ctrl+k searches transcripts* › *on ctrl+enter* keeps typing to names and commands; `ctrl+enter` then searches the transcripts (`ctrl+j` in terminals that send `ctrl+enter` as `enter`).

`#` runs rush's own commands on the selected agent: `#done` `#go` `#away` `#loop` `#back` `#stop` `#restart` `#rm` `#kill` `#clean` `#cd` `#rename` `#group` `#fork` `#pin` `#pr` `#full` `#sort` `#by` `#split` `#folder` `#new` `#with` `#use` `#preset` `#account` `#efficiency` `#advisor` `#statusline` `#view` `#width` `#dock` `#stash` `#hibernate` `#native` `#mackeys` `#ghostty` `#tips` `#update` `#ask`. `/` is left to the agent: its own commands only, some drawn rush's way (`/btw`, `/diff`). Anything rush adds takes `#`.

`#ask` asks rush about itself: `#ask how do I make finished agents stop sooner?`, or `#ask turn the advisor on`. It starts an agent in rush's own folder with this guide to hand, which answers, points you at the # command that does it, or changes rush's settings for you. rush takes up any change to its `config.json` within a few seconds, whoever makes it.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="screenshots/command-bar.webp" alt="The command bar"><br>
      <b>The command bar</b>
    </td>
    <td width="50%" valign="top">
      <img src="screenshots/command-bar-search.webp" alt="The command bar searching agents and transcripts"><br>
      <b>Searching every transcript</b>
    </td>
  </tr>
</table>

## Efficiency

Where your tokens go, and whether the things that promise to cut them do. It reads every transcript (a few seconds the first time, only what's new after that) and keeps totals of the ones Claude Code deletes after 30 days.

- **Overview**. What was spent, the cache-hit rate, context per request, the context sessions start with, and where the money goes: usually context read again on every request, rarely what Claude writes. It names the savers that might save you most.
- **Timeline**. Any figure over the last day to 90 days (context per request, cost, tokens, output, cache hits, tool output per call, context at the start), with a marker wherever a saver was set up, a setting changed, a saver was first used, or you wrote a note (`n`). `b` compares the days either side of an event and, for a saver, sessions that used it against those that didn't in the same days.
- **Savers**. rtk, caveman, Ponytail, context-mode, headroom, Serena, Graphify, tokensave, codegraph, codebase-memory-mcp, claude-context, the language-server plugins, Context7, claude-mem, and Claude Code's own settings (shell output cap, compacting sooner, Sonnet for subagents, the Concise style, leaving out git instructions). For each: what it does, what its authors claim and what others measured, whether it's on here, and how often your sessions used it. `enter` shows exactly what setting one up would run and change before anything is done; files it may touch are backed up first, and `x` removes it.
- **Estimates**. What each saver might have saved you over the range: its measured cut of what your own sessions spent on the part it acts on (shell output, searching and reading code, what Claude writes, subagents on Opus…). The cut comes from independent tests where there are any, and says whose it is when there aren't; claims per question against reading every file aren't taken at their word. A saver in use is compared on sessions that used it against those that didn't, with a warning when there are too few to say much. `e` orders the Savers by estimate.
- **Findings**. What's worth doing, most dollars at stake first, each with the saver that addresses it.

`#efficiency` (or `#eff`) opens it.

### The advisor

Off until you turn it on with `#advisor on`. After your agents have done some work, at most every three hours, rush works out a digest of the last week (spend, context, tool output, rereads, subagents, savers, the costliest sessions) and Haiku proposes up to five changes from it, reading short briefs of the transcripts for evidence. Opus then checks each one worth $2 a week or more against the transcripts, three a day at most, and rewrites or rejects it. Both run with no shell, web, hooks or MCP servers, and can't read outside those folders.

What Opus confirms goes first in the findings, marked ✦; `x` puts one away for good. A pass costs about $0.30, and the findings page shows what the advisor has cost so far. Only one rush runs it at a time, and it keeps to its budget across windows and restarts.

## Machine

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="screenshots/processes.webp" alt="Processes"><br>
      <b>Processes</b>. Every agent's process tree, busiest first. What a finished session left running (a dev server, a watcher) is listed first: <code>x</code> ends one, <code>X</code> all of them.
    </td>
    <td width="50%" valign="top">
      <img src="screenshots/cleanup.webp" alt="Cleanup"><br>
      <b>Cleanup</b>. Every worktree with its size and whether removing it would lose anything, and every agent's temp work. <code>A</code> removes everything that loses nothing.
    </td>
  </tr>
</table>

## Plugins

Plugins add tools, subagents, prompt text and memory to every rush-mode session, and can run agents of their own. Any MCP server can be one. Each runs sandboxed and does only what you approved: no files but its own, no network but the hosts it named, no programs, and only the agents it started. macOS only for now.

```sh
rush plugin list            # what's installed, approved and running
rush plugin approve <name>  # read what it may do, and say yes
```

To have Claude write one, install the skill: `/plugin marketplace add 0xdeafcafe/rush`, then `/plugin install rush-plugin-dev@rush`.

A plugin can also take part in rush's screen, as far as you approved:

- hear what happens there (sessions opened, turns ending, a session an error stopped, the network going and coming back);
- add sections to a Session's overview and a word to its row;
- add commands you can bind to keys, and settings under **Settings → Plugins**;
- with `input`, see and set what you type;
- with `intercept`, change or hold back a message before it goes.

None of it can hold rush up. The screen hands plugins events without waiting, draws what they added from a copy it already has, and gives an intercept 400 ms before the message goes as it was. The [`autodrafts`](../plugins/examples/autodrafts) and [`reconnect`](../plugins/examples/reconnect) examples rebuild drafts and reconnect-and-continue this way.

[plugins/](../plugins) has everything else: using and writing them, the examples, the skill, and how it works.

## Embedding rush

Another app can run rush-mode sessions without the view and show one of them in a terminal of its own.

```sh
rush session start --cwd DIR [--agent A] [--profile P] [--session-id UUID] [--resume] [--name N] \
  [--prompt-file F] [--image PATH]... [--env K=V]... [--meta k=v]... \
  [--binary PATH] [--model M] [--effort E] [--permission-mode M] --json
echo 'the next message' | rush session send <id> [--now] [--image PATH]...
rush session watch <id> [--json] [--until-idle]
rush session interrupt <id>
rush session stop <id>
rush session info <id> --json
rush session list --json [--meta k=v]...
rush queue send|remove <id> <n> [--was TEXT]
```

`start` runs the first installed provider of the session's profile (`--profile`, else the folder's rule, else the default) unless `--agent` names one (`codex`, `copilot`, `kimi`…). It uses the model, effort, permission mode and limit settings from Settings unless a flag gives them (for another agent, its own), and prints the session's info with `"alive"` added. With `--session-id` it is idempotent: a session already running is printed, not started again. A stopped one needs `--resume`, which brings the same conversation back. `--env` values reach the agent on every start of it, idle restarts and resumes included. `--meta` tags the session; `list --meta` filters on the tags. `send` hands the message to the turn under way without stopping it (the agent reads it at its next step) rather than queueing it; idle, it's sent as usual, and `--now` stops the turn to send it.

`send` reads the message from stdin. An image the text names as `[Image #N]` (the Nth `--image`) goes right after that marker; the others go with the message as before. If the session is stopped it resumes with the message, as sending from the view does. `info` exits 1 with `{"error":"not found"}` for an id with no session. `rush queue` comes from the `queue` plugin bundled with rush: `send` sends the message queued at place `n` (from 0, as `info` lists the queue) now, and `remove` drops it; `--was` names it by its text, so it's still the one meant if the queue moved. `alive` is whether the session's host is running. A host retained for queued or scheduled work counts as alive even with its runtime stopped; a fully sleeping session reports `sleeping: true` and `alive: false` but keeps its saved conversation.

`watch` prints what a session says and does as it happens: the events a plugin gets from `sessions.subscribe`, and `delta` besides. It starts with what the turn under way has said so far, then carries on live; earlier turns aren't replayed. Read by a person, the words come as they're written, with a line for each tool call, message sent and change of state. With `--until-idle` it stops when the turn ends, at its `result` or when the session goes idle. A session idle when it starts is waiting for a turn, so `watch` waits for the next one: start it before `send` to see a whole reply. It exits 0 when the turn ends well, and 1 when the turn failed, there's no such session, it isn't running, or its host went away before the turn ended. Without `--until-idle` it runs until the host goes away or you stop it.

With `--json` each event is one line of JSON: `event` is its type, `session` the session's id, and the rest are the fields `session.event` has. Every field in the table is always there, empty or 0 when there's nothing to say, and the keys come in no promised order.

| `event` | Other fields | Meaning |
|---|---|---|
| `info` | `state`, `detail`, `needs`, `costUsd` | Its state changed. `state` is `starting`, `working`, `blocked` (waiting for you), `idle` or `stopped`; `detail` is what it's doing in words, `needs` what it's blocked on. |
| `sent` | `text` | A message was sent to it, by anyone. |
| `delta` | `text` | A piece of the agent's words as they're written. The same words then come whole as `text`, so take one or the other. Agents that don't stream send none. |
| `text` | `text` | The agent said this (the main thread, not subagents). |
| `tool` | `name`, `doing` | It used a tool; `doing` says what, in words. The tool's output is never sent. |
| `result` | `text`, `isError`, `costUsd`, `turns` | The turn ended; `text` is its last words, or what went wrong. |
| `closed` | | The host went away, and the watch with it. |

```json
{"costUsd":0,"detail":"","event":"info","needs":"","session":"a1b2c3d4","state":"idle"}
{"event":"sent","session":"a1b2c3d4","text":"what does main.go do?"}
{"costUsd":0,"detail":"","event":"info","needs":"","session":"a1b2c3d4","state":"working"}
{"doing":"reading main.go","event":"tool","name":"Read","session":"a1b2c3d4"}
{"event":"delta","session":"a1b2c3d4","text":"It starts "}
{"event":"delta","session":"a1b2c3d4","text":"the server."}
{"event":"text","session":"a1b2c3d4","text":"It starts the server."}
{"costUsd":0.04,"event":"result","isError":false,"session":"a1b2c3d4","text":"It starts the server.","turns":2}
{"event":"closed","session":"a1b2c3d4"}
```

`--until-idle` stops after the `result` (or the `info` that says idle), so `closed` is only there when the host went first. Any error ends the output with a line of `{"error":"…"}` and no `event`, as the other commands' `--json` does.

A plugin can also arrange the Agents list for an embedding app: with the `sidebar` capability it sends sections and a name for each agent, keyed by session id, and the list offers them as a group-by mode (`ctrl+s`, or `/by plugin:<name>`). The [`kanban`](../plugins/examples/kanban) example shows the kanban-code board this way.

`rush open <id> --hosted` is the view of that one session alone, under rush's header, with the Session at the terminal's whole width and no Agents list. `ctrl+\` opens Efficiency, Machine and Settings as usual, and Agents is the session again. The keys that lead to other agents or open the list (`ctrl+z ctrl+n tab`) do nothing, and `, . < >` are typed into the box. `ctrl+k` searches every agent and place; going to another agent from it shows the list beside it, as `ctrl+6` does. `ctrl+6` (sent as `ctrl+^` by most terminals) shows Agents beside the session, to pick and answer another agent; `ctrl+6` again, or `esc` from the list, hides it and the view is back on the hosted session. Outside hosted, `ctrl+6` hides or shows Agents beside an open Session for the moment, while `#view` keeps the layout you chose. The message box has the keys from the start. `esc` on an empty box takes the keys off it (typing, `enter` or `→` gives them back), and `esc` again stops the running turn, as `ctrl+x` does; `esc` never closes the view. Only `ctrl+q` does, and the session keeps running. A stopped session shows its conversation and resumes with the first message. Past conversations aren't read until `ctrl+6` or `ctrl+k` needs every agent, so the view opens at once however many there are.

## And

- **Menu bar**: every account's limits, each by its own name (Codex's week reads 7d), the agents working, and a badge for each waiting on you. Questions arrive as notifications you can answer from; clicking one brings back the terminal rush is open in (Warp, iTerm, Ghostty…) on that agent. rush offers it the first time it opens on a Mac. It's a small Swift app built on your Mac the first time (it needs Xcode's command line tools).
- **Zen** (`ctrl+z`): only the agent that needs you and its box. Answered, it stays on screen to carry on with; `/done` moves on to the next. A bar across the top says where you are in the queue; `ctrl+n` jumps to the next one waiting, holding `tab` peeks at what's working, `ctrl+z` again leaves.
- **Terminal.app** keeps ⌘ for its own menus. `#mackeys on` sets up Hammerspoon to send ⌘← → ⌘⌫ ⌘⌦ and ⌘Z on as editing keys, only while Terminal.app is in front; `#mackeys off` takes it out again.
- **Ghostty**: `#ghostty on` puts it on LangWatch's light and dark themes, navy and orange on cream, orange on dark blue, switching with the system; rush recolours with it. Colours your config set are commented out and come back with `#ghostty off`.
- **Status lines**: `/statusline` lays out the agent header, rush's top bar (three lines, the third right of the tabs) and Claude Code's own status line, with a live preview.
- **Colours** made from your terminal's own background and text, or set to dark or light, and a colour-blind palette, in Settings › Appearance.
- **Copy on select**: text you drag over goes to the clipboard as you let go, unless you turn it off in Settings › Appearance; then it stays selected for cmd+c or ctrl+c.
- **Settings from the environment**: any setting kept in `config.json` can be set for one run as `RUSH_` and its key in upper snake case, a nested one after its parent's: `RUSH_COPY_ON_SELECT=0`, `RUSH_THEME=light`, `RUSH_HIBERNATE_AFTER_MINUTES=30`. It isn't saved: `config.json` keeps what it had, unless you change the setting in rush meanwhile. Lists and maps can't be set this way.
- **Light**: about 40 MB with a session open, 56 MB for a 38-hour session with 342 subagent runs. Idle, it does almost nothing: the kernel says when a transcript changed, and only that file is read again. `rush --soak 30s 200x50` measures it against your own agents.

## Keys

| Key | |
| --- | --- |
| `↑` `↓` `⌘↓` | move, open the agent (or `→`) |
| `enter` | rename or open it, as you chose in Settings |
| type, `enter` | start a session, or reply to the agent in the preview |
| `tab` · `{` `}` | the list ⇄ the agent's Session |
| `ctrl+k` | go anywhere, search everything; `ctrl+s` there sorts the agents: by status, latest, project, worktree, model or harness |
| `ctrl+f` | find, starting where you are |
| `ctrl+v` | paste an image; in a Session's box it goes in the text as `[Image #1]`, deleted as one |
| `<` `>` · `ctrl+\` | Agents · Overview · Efficiency · Machine · Settings; in a Session's box `,` `.` `<` `>` are typed, so `ctrl+\` |
| `[` `]` | the next page, everywhere there are pages: a Session's views (with nothing typed), Efficiency, Machine, Settings, and the tabs of a sheet |
| `tab` `shift+tab` | the next page too, where there's no list and Session to go between: Efficiency, Settings, Projects, Wall |
| `ctrl+r` `ctrl+t` `ctrl+e` | rename, pin, set group |
| `ctrl+s` | group by status, agent, your groups |
| `ctrl+p` | split each section by project, or not |
| `ctrl+n` | the next agent needing you |
| `ctrl+b` | tell it to go on |
| `ctrl+p` `ctrl+r` | stash what's typed or bring it back, the history (in the Prompt, with something typed) |
| `ctrl+d` | done |
| `shift+tab` | what the next session starts as: agent, model, effort |
| `ctrl+x` | stop it, idle; again moves it to Done · `ctrl+] x` hides, restarts, switches or deletes it |
| `ctrl+]` `\|` · `ctrl+]` `-` · `ctrl+]` `k` | the grid: pin the open agent as a live tile in the column beside the Session (`\|`) or the row under it (`-`), both at once if you like, then pick the next agent to open; a click on a tile swaps it in at once (its session stays open while pinned), while a click on a tile's box types into it where it is (its edge turns orange; `esc` in its empty box hands the keys back); drag the column's edge (or the row's, with no column) to resize; `k` takes the newest tile off; the grid is kept across restarts |
| `alt+/` · `ctrl+]` `q` · `ctrl+]` `i` | the guide: a fast model, floating, from anywhere, that sees your sessions. Ask "what's next?" and it says which needs you and opens it; it answers questions about rush and quick side questions too. `#guide <question>` asks it straight from the box; `ctrl+] i` in a Session puts its thread in the box |
| `ctrl+]` `z` | recall on and off (off to start): at the foot of the list, a cheap model (sonnet, else haiku or Ollama) reads the open chat's latest exchanges at most once a minute and notes what's decided, what goes against the grain and what's open; click a note to put `About: …` in the box |
| `ctrl+]` `t` · `#discuss` | a room on this chat: its latest exchanges as context (←→ in the set-up), its row under the chat and a panel above the box (round, each agent's last line; Enter opens it), its verdict back as a card above the box: Enter sends it on, `e` edits it first, Esc drops it. The **discuss** send mode (ctrl+t) makes Enter do this with what's typed as the topic |
| `#intervene [@agent] [concern]` | two agents go through a chat (the open one, or the tagged agent's): its plan, status and latest 10 exchanges with tool calls, failures and times; they say why it's stuck, and what the agent must do lands in its box, enter sends it |
| `ctrl+]` `0` `1` `2` `3` | in a Session, how deep it shows: m0 only what's said (a red row where steps failed), m1 every run of steps folded to a row, m2 as usual, m3 everything open (as `ctrl+o`) |
| `ctrl+shift+n` · `ctrl+]` `=` | a new agent: the keys go to Agents' Prompt from anywhere |
| `ctrl+]` `g` | in a Session with subagents, the stack: the agent and each subagent in a band, top to bottom; again goes back to the conversation |
| `ctrl+]` `u` | in a Session, move what's typed to the Prompt, to start a new agent with |
| `ctrl+]` `a` · `ctrl+]` `o` | in the Prompt, add what's typed to the open agent's box, or put it there in place of its draft |
| `ctrl+tab` `ctrl+shift+tab` · `ctrl+]` `]` `[` | the next open chat, or the one before |
| `ctrl+l` | move the agent, or pick a new session's folder |
| `ctrl+z` | Zen |
| `ctrl+]` then a letter | what ⌥ and the letter do: `w` the default preset, `f` filter the list, `l` a worktree agent's worktree or checkout; in a Session `h` hold the queue, `r` rewind (or mark a file reviewed), `f` fork, `v` the file in full, `c` copy |
| `shift+↑` `shift+↓` | in a Session with nothing typed, the subagent runs |
| `#` | rush's commands |
| `/` | the agent's commands and skills |
| `?` | the guide |

Every key can move. **Settings → Keys** lists every action, where it works (everywhere, the list, a Session) and its keys. Press `enter` on one, then the keys you want. A chord is several, `ctrl+x` then `d` say, ended with `enter`. `a` adds an alternative, `x` unbinds the action, and `r` resets its default bindings. Use `←` / `→` to change context and `↑` / `↓` to select an action. Try a shortcut on this page to find the action it runs; page navigation and editing keys keep their normal role. Each `#` command, and each plugin's commands, can have keys too. A key that would take another's asks first. What you change goes in `~/.config/rush/keybindings.json`:

```json
{"bindings": {"session.send": ["ctrl+enter"], "command:stash": ["ctrl+x d"], "list.pr": []}}
```

## What's coming

None of this is in rush yet. Some of it is being built now, and the rest is next in line.

- **Limits and prices for everyone**: limits for Kimi and Vibe, prices for Codex and the ACP agents (Copilot's models already say their premium-request multiplier), and Efficiency for agents other than Claude Code.
- **Copilot's coding agent**, started and steered from rush rather than only watched.
- **Codex accounts** read live when they aren't the one in use, rather than showing their last reading.
- **Ollama sessions** in the list with the rest, and re-tuned when you change model mid-session.
- **Plugins' tools in every agent**, over MCP, which the ACP agents already accept.
- **ZCode spoken to directly**, dropping the `zcode-acp-server` bridge, and Goose over ACP.
- **Claude Code as just another adapter**: its discovery moves into its adapter, and its sessions go over the wire as rush's own events like everyone else's. You won't see this one, but it's what makes the rest cheap to add.
- **`rush on` for other agents**, if `codex` or `copilot` grow a view worth replacing.

Rewind, fork, checkpoints, the context breakdown, background task control and the screen tab have no equivalent in the Agent Client Protocol, so they stay hidden for those agents rather than faked. [docs/multi-agent.md](multi-agent.md) has the design and where it stands.

## How it works

rush reads Claude Code's files: `jobs/*/state.json`, `daemon/roster.json`, `jobs/pins.json`, the transcripts and the cached plan usage. It changes things only through Claude Code (the daemon's control socket, or the `claude` CLI), with one exception: switching account writes the other sign-in into Claude Code's keychain item and its `oauthAccount` into `~/.claude.json`. Each sign-in is kept in your login keychain as `rush-login`.

Other agents run through adapters in [internal/adapters](../internal/adapters): Codex over its app-server's JSON-RPC, the rest over the Agent Client Protocol, and Ollama through Claude Code. Each adapter declares which of rush's features it supports, and the core asks that rather than checking for an agent by name. Whatever the agent, its events draw as a session. Switching a Codex account puts its sign-in in `~/.codex`, keeping the one there first so its refreshed tokens aren't lost; an API-key sign-in is named by a hash of the key, never the key.

Its own state (Done, names, groups, accounts and presets, not their sign-ins) lives in `~/.config/rush`, and a cost cache in `~/Library/Caches/rush`.

Plugins run under `rush plugind`, sandboxed; [plugins/ARCHITECTURE.md](../plugins/ARCHITECTURE.md) has how.

The control socket and the files are undocumented, checked against Claude Code v2.1.280. If an update changes them, the column affected shows `–`, and opening an agent falls back to `claude attach`.

Limits are asked of each agent at most every five minutes per account, shared by every rush that's open. Costs are estimates at list prices, not your bill.

### Agent exchanges and background output

Rush-managed agent messages and subagent handbacks appear as attributed sender → receiver rows in both conversations. Space expands the text, Right opens the other session, and Alt+C copies the full exchange. Host restarts preserve recent exchange history; older hosts explain when a restart is needed for attribution. Journals retain up to 8 MiB per session and restore the latest 2,048 records.

In Background, Space or a click opens recent output. `d` shows the launch script and full process commands; `x` stops the selected task. Output activity and elapsed time stay visible. A quiet task may be waiting or doing work that produces no output; silence alone is not a confirmed hang.


### Conversation loading and usage limits

Conversations open on their recent history. Older messages load in the background while the current view stays anchored. Codex uses a bounded read before host replay; some harness history formats still require reading the full log.

When a usage limit appears, `r` refreshes the usage reading without sending a message or answering the continuation question. The card shows the last successful check, measured usage and reset countdown when the provider supplies them. A passed reset time does not imply allowance has renewed until a fresh reading confirms it.

The default status strip shows daily spend, the default provider's usage and agent RAM. Open `#statusline` for detailed/custom layouts. Existing custom layouts stay unchanged.


## Community

`#broadcast` sends one message to several agents. `#broadcast all fix your tests` sends to every agent still at work that a message can reach (not finished, past, or open in another terminal). `#broadcast @docs @fix-login-bug rebase on main` sends to the ones tagged. `#broadcast` alone lets you pick: click agents in the list to tick them (click again to untick), type the message in the Prompt, and Enter sends it to every one ticked. Escape stops picking.

The feed is shared by your agents, off until you type `#feed on` (`#feed off` turns it off again; its old names, `#twotter`, `#twatter`, `#twitter`, `#community`, still work, as does `rush twotter` on the command line). While it is on, the latest chirps (posts) dock, as the feed 🐓, at the very foot of the list, on the prompt, newest at the bottom: faded until a chirp is new or the pointer is on it, a long chirp cut with … and shown whole while hovered, and every new agent is told to chirp only when something matters to other agents: a shared blocker, a non-obvious fix, a heads-up about work others may collide with. While it is off, nothing shows, agents are told nothing and the CLI refuses to chirp.

Each project keeps its own feed. A chirp belongs to the project (the main checkout, so a worktree counts as its repository) of the agent that wrote it; an agent lists, reads and answers only its own project's chirps, and is told to ask you before reaching further. `#feed open` lets agents cross projects; `#feed closed` puts them back. An agent that chirps the same words twice is refused the second time.

`#feed`, or a click on a chirp, opens the whole feed as one timeline: a tab per project, then all of them (`[` `]` or Tab switch, starting on the picked agent's project), chirps in time order under a rule per day, each with its replies set in under it, the picked chirp marked, every line of it shown. Up/Down, PgUp/PgDn or the wheel scroll it, Shift+Up/Down jump a day, Enter or Space replies to the picked chirp, `n` writes a new one in the open tab's project and Escape closes. Agents show as a fixed `@name`, two animals picked from their session; your chirps show as `@you`.

```sh
rush feed list [--json]
rush feed show <id> [--json]
rush feed chirp "text"
rush feed reply <id> "text"
```

Posts are at most 120 characters, without links, and wake no one.

The board uses private files and atomic writes with an interprocess lock. Limits are 128 threads, 256 messages per thread, 120 characters per post and 8 MiB total; reaching a limit reports an error without deleting history.

## Rooms

`#room <topic>` sets up a group chat: choose a fresh panel of agents (Tab, then Space), Enter, and they take turns arguing the topic, running tools to check facts, until they agree on a verdict. The room is a row in the list, its agents grouped under it; Enter or a click shows it in the pane like any agent. Round 1 is blind openings, then they take turns; the verdict lists each agent's own final position. Type at any time, `@name` to one agent, `/pause` (or ctrl+x) and `/resume`, `/verdict` to end early, `/stop` to end it. alt+1…9 opens one agent's own session; ctrl+x then y on the row hides the room. A room opened from a chat (`#discuss`, `#review`, or Enter in the discuss send mode) shows above that chat's box while it runs, and its verdict comes back there as a card to send on, edit or drop. `rush room help` has the same from the shell. How it works: [room.md](room.md).

## Session permission overrides

Shift+Tab includes a Permissions row alongside model, effort and harness. `#perm` (also `#permissions`) opens that same row; `#perm <mode>` requests a specific supported mode. `#yolo` requests the harness's advertised bypass mode. Harnesses without one report that limitation.

Permission overrides apply to the current hosted session, or the next session when choosing a new start. They do not change global defaults. Applying the Permissions row keeps any draft unsent, and changing permissions does not restart the session or drain its queue. Existing approval requests remain separate from the mode setting. Rush displays the host's reported mode; sending a request is not evidence that the harness accepted it. Kimi, Vibe and Gemini expose the modes advertised by their running sessions, so older hosts need to restart normally before reporting newly supported mode information.


## Idle sessions

After roughly three idle seconds by default, Rush releases the agent runtime. Once no queued work, approvals, questions, background tasks or scheduled continuation needs the session host, that host also exits. Saved conversations remain available; browsing them does not start a process. A new message explicitly wakes the host and resumes the saved conversation. The UI keeps its transcript, scroll position and draft while the worker is asleep. The main Rush view can close and reopen independently of the saved session.

`#away 3h` (or `#afk`, `#flight`) leaves the open session to carry on while you're gone, with rush closed if you like. Its host checks on it every 30 minutes (`#away 3h every 15m` to change that) and tells it, when idle, to keep going and make its own calls. Nothing is answered or approved for you: a question or permission it asks is saved, and it's told so at once and carries on with what doesn't need it. At a usage limit, a profile that moves to another account carries on at the reset rather than asking (with rush open it moves sooner). It ends when the time's up or the agent says it's all done; the row then reads `✈ back from away` until you open the session, where the back sheet says how long, the check-ins, its last words and what it saved. Enter ends it and puts what it saved in the box as one reply to fill in, `l` ends it and loops, Esc leaves it. `#back` or `#away off` opens the sheet early. `#loop` (`#loop every 15m`, `#loop 2h`, `#loop off`) is the same with you there: told to keep going at each check-in till it's all done, while what it asks waits for you as usual. While away the Mac is kept from idle sleep, but a closed lid still sleeps it.

The compact top status line shows today's spend, the active provider/account's two usage windows and agent RAM. System alerts appear when needed. Detailed meters and all-account usage remain available through `#statusline` and account views.

Session tabs, harness labels and transcript rows show pointer feedback. Click a session tab to switch views without sending or clearing the composer; click the harness label to open the model/harness controls. Top navigation uses the same visible targets for hover and click. Hovering rows does not change selection or rebuild the transcript.

Session location labels use the current host working folder. Linked worktrees keep the main repository name, with their own branch, worktree and nested folder shown alongside it. An old transcript location cannot override the live host folder.

Shell chain timing also works for other harnesses when Rush can match an observable local shell command exactly. Remote or opaque processes remain unknown. Background tasks show the age of actual reported progress when no output file is available; repeated heartbeats do not count as progress. Quiet output is evidence of silence, not proof that a task is stuck.

Projects › Temporary includes running and older sessions, with each temp-folder path. Sizes appear as each folder finishes measuring, including when Projects is opened from a hosted session. Running sessions must be stopped before their temp files can be deleted. Stopped-session sizes are rechecked periodically so external cleanup does not leave a stale total indefinitely.


### Antigravity CLI replaces Gemini CLI

New Google sessions use **Antigravity CLI** (`agy`). Sign in from Settings → Harnesses → Antigravity CLI, then refresh account and models. Rush reads the account's model list from `agy models`, streams replies and tool steps, resumes by conversation ID, and saves transcripts for sessions started in Rush. Existing Gemini CLI conversations retain their old identity and history; they are not passed to Antigravity as if their IDs were compatible.

Choose model, effort and permissions before starting. Antigravity's streaming input currently supports text only, with native headless permission rules; it does not support interactive approvals or mid-turn model changes. Rush does not enable `--dangerously-skip-permissions` unless you explicitly choose **always-proceed**. Native conversations started outside Rush are not imported. Usage quotas remain available in Antigravity's `/usage`; Rush does not reuse Gemini Code Assist quota data.

Protocol reference: https://www.antigravity.google/docs/cli/headless/
