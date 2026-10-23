# Providers, harnesses, models: one rush

Status: agreed 2026-09-30; Settings and Starting redesigned 2026-10-01, and
simplified 2026-10-23 (Providers, Harnesses, Config; profiles removed).

## Principle

You use rush, not Claude Code or Codex. A session is a model from a provider,
run by a harness. rush names things by provider, account and model; the
harness is a detail you can see, never the headline.

## The three things

| Thing    | What it is                                  | Examples |
|----------|---------------------------------------------|----------|
| Provider | who serves the model, and how you pay       | Anthropic subscription, Anthropic API, OpenAI subscription, OpenAI API, Ollama, GLM, DeepSeek, Kimi, Mistral, Gemini |
| Harness  | the program that runs the session           | Claude Code, Codex, Pi, OpenCode, Vibe, Gemini CLI, Kimi CLI |
| Model    | what the provider serves, from its own list | opus[1m], gpt-6-astra, qwen3-vl:30b |

Accounts belong to a provider and are named by you (alex, work).

Compatibility is the provider's: Anthropic's subscription runs in Claude
Code, OpenAI's in Codex, and either in Pi on Pi's own sign-in (with a
warning); API-key and local providers run in any harness that speaks their
API. `agent.Compat` says which, and why not.

## Settings: Providers, Harnesses, Config

Agreed 2026-10-23, replacing Profiles and the Compose sheet: three things
to set up once, and a new session that asks for its harness and a model.
Every chat has one leader harness, and the leader controls the models: it
calls only what its row of Config lets it, for itself, its subagents and
the helper agents it starts.

```
 SETTINGS (set up once)                    NEW SESSION (every time)
 ┌─────────────┐                           ┌──────────────────────────┐
 │ Providers   │  sign in / API key        │ harness: Claude Code ▾   │
 │ Harnesses   │  every harness, then      │ model:   Opus        ▾   │
 │             │  what one changes         │            enter start   │
 │ Config      │  which harnesses are on,  └──────────────────────────┘
 │             │  which models each calls
 └─────────────┘
```

- **Providers**: your accounts only. Set up first (signed in, a key kept,
  Ollama found), then the rest. Each has its accounts or key, its limits,
  and `*` makes it the one new sessions start on. Where it runs and what
  it starts with are not here any more.
- **Harnesses**: *Every harness* first: effort, permissions ("the
  harness's own" or "approve everything") and attribution, said once.
  Then each harness, showing only what it changes from that, with
  "was …" beside it, and its own settings (settings file, environment,
  sign-in). An effort a harness doesn't offer is left to its own default.
- **Config**: one table, harnesses down the side, providers across.
  - space on a harness switches it on or off; off, rush never starts it.
  - a cell is ★ (the provider's default harness), ● on, ○ off, ⚠ works
    through the harness's own sign-in, · can't run. space switches a cell,
    `*` makes it the ★.
  - enter on a cell picks its models: all, or some, and the one it starts
    with.
  - under the table: what rush does at a usage limit: another account
    (the default), wait for the reset, or move on to the next provider
    with room.
  - when a provider's ★ harness is switched off, it starts by default in
    its first cell that is on.
  - a row is what that harness may call when it leads a chat: its own
    model, and its subagents' and helper agents' (spawn_agent, its shell's
    stand-ins). A harness may only run providers in itself: a chat Claude
    Code leads starts no Codex agent unless Codex leads one of its own.

Profiles (Presets) and folder rules are gone. An older config's default
profile is moved in once: its first provider becomes the default, its
harness that provider's ★, its model and effort that cell's, and its
limit choice the one under the table. The rest stays in the config
(`profiles`, `folderRules`), unread, so an older rush still has them.

## Starting and switching

- The **New session** sheet (shift+tab, alt+m) has two rows. *harness*,
  the leader: any harness on with a provider ready, the next session's
  first. *model*, the models its Config row may call, grouped by
  provider; typing filters the list, and a new leader keeps the model
  when it may call it. A ⚠ line shows only when the harness can't do
  something (rewind, images, resume, subagents). enter starts it, or
  makes it the next session's when nothing is typed.
- `#new [harness][@provider[:account]] [model] [effort] [task]` still
  starts one session on any route, once.
- Switching a running session (alt+m in a session) keeps its sheet of
  provider, harness and model columns, without the Preset row; its models
  are those its harness may call.
- Neither changes a default: Settings does.

## Usage everywhere

- The header's usage bar shows every provider with limits, compressed: one
  mini meter each (Claude, Codex, Copilot, Gemini, Vibe…), the lowest first
  when there isn't room.
- A session's header names its agent (profile or harness:account and
  model); when its usage runs low it says where you could switch to, the
  provider or account with the most room.
- Vibe's and Gemini's limits are read like Claude's and Codex's, from what
  their own programs report; Gemini's free allowance shows as such.

## Stalls

- Claude Code sessions rush hosts run with Claude Code's stream watchdog on
  (`CLAUDE_ENABLE_STREAM_WATCHDOG=1`), so a silent stream is cut and retried.
- rush marks a turn with no stream activity for 2 minutes as stalled, on its
  working line, with a key to interrupt and send again.
- Settings > General says what's on.

## Order of work (run in parallel where files don't overlap)

1. Stalls: watchdog on, stalled marker, settings line.
2. Model: provider identities split by billing; provider > harness use and
   default; provider x harness defaults; profiles carry provider, account,
   harness, model, effort. Older configs still read.
3. Settings page as above; Capabilities page removed.
4. Start sheet, `/profile` and `/agent` with autocomplete, in-session switch,
   `<harness>:<account>` names.
