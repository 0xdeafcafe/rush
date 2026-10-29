# Subagents across harnesses

Status: built 2026-10-01. Replaces the Haiku relay agent types (`RUSH_TASK`).

Any rush session, whatever its harness, can hand a task to any agent rush
runs, foreground or background, through rush's own MCP server, or start
one as a session of its own beside it (`start_agent`, below). The child is
a rush-hosted session like any other, so it has its own transcript, steps,
cost and stream, and the UI draws it under the call that started it.

```
 parent session (Claude / Codex / Kimi / Copilot / OpenCode …)
   │  mcp__rush__spawn_agent {agent, prompt, model?, effort?, background?}
   ▼
 rush MCP server ── in the parent's host (Claude, in process)
   │               or `rush mcp-tools --session <parent>` (everyone else)
   │  host.Spawn(Config{Kind, Model, Prompt,
   │                    Meta{spawnedBy: parent},
   │                    PromptExchange: parent → child})
   ▼
 child host (rush host run <child>) ── runs the agent through its Driver
   │  each turn end: sessions/<child>/answer.json
   │  report file set (background)? → Ensure(parent) + SendExchange(result)
   ▼
 foreground: the tool call waits for answer.json and returns it; if the
             child's turn hangs (5m silent, see host/hang.go) it returns
             then, the child retried in the background and reporting back
 background: returns "<agent> started as agent <id>" at once; the answer
             arrives later as a message from the child (an exchange row)
```

## The tools

| Tool | Input | Returns |
|---|---|---|
| `spawn_agent` | `agent` (a name its description lists: `astra`, `haiku`, `kimi-code-k3`; or a model, harness or provider by its own name: `gpt-6.1-sol`, `sol 6.1`, `codex`, `codex astra`), `prompt`, optional `model`, `effort`, `background` | the answer, or the child's id when `background` |
| `start_agent` | `agent`, `prompt`, optional `model`, `effort`, as `spawn_agent` | the new session's id, at once |
| `agent_result` | `id` (a child's or a sibling's), `wait_seconds` (≤600) | the answer once done, else how it stands |
| `agent_send` | `id`, `prompt`, `background` | a follow-up's answer; the child keeps its conversation |

The list of agents is `host.picks(leader)`: every installed provider on
each of its models, named as the old agent types were (`defName`), with
its model id beside a short name (`astra (gpt-6-astra)`). A Claude
session's own models stay native subagent types (`--agents`, the Agent
tool), left out of the list and named in its last line.

Which providers, and where they run (`host.takes`), agreed 2026-10-23:

| Provider | Runs in | Models |
|---|---|---|
| one the leader's harness runs (its row of Settings › Config, cell on) | the leader's harness | what that cell may call |
| any other installed one | the harness it's set to run in (its ★, or its first cell on when that's off) | what that cell may call |
| one whose harness is switched off | not offered | |

So a chat Claude Code leads reaches Codex's astra and sol, Kimi, GLM and
the rest with no setup, and Config still switches a harness off or keeps
a cell to some models. Config is read on every call, so a change in
Settings reaches a running session at its next `spawn_agent`. The tool's
description is what the harness read when the session started (Claude
Code asks `tools/list` once), so it can be behind; the names are resolved
at call time, and a name that fits nothing gets an error listing every
name that does.

How a name is found (`host.find`, `agentmatch.go`), first that fits:

1. a listed name, as written or cleaned (`Opus 1M` is `opus-1m`), or a
   model's id (`gpt-6.1-sol`, any case);
2. an agent, provider or harness by its own name or label (`codex`,
   `kimi`, `Codex`): its default model;
3. the agent whose words (name, model, harness, provider) hold every word
   of the call's, a version kept whole (`sol6.1` is `sol` and `6.1`), the
   one with fewest words of its model left over first: `sol 6` is
   `gpt-6-sol`, `codex astra` is `gpt-6-astra`. A call naming only a
   harness or provider this way gets its default model.

With `model` too, `agent` and `model` are first tried together (`codex` +
`astra`); else `agent` alone, and `model` is matched against that agent's
models, or passed to its harness as written.

## Siblings: start_agent

`start_agent` starts an agent as a session of its own beside the caller,
not its child: what you'd get had you started it yourself, in the
caller's folder. A separate tool rather than a flag on `spawn_agent`,
because almost nothing of `spawn_agent` holds for it: there's no answer to
wait for, nothing to background, no step to draw it under, and a call the
UI takes for a spawn (`agtools.IsSpawn`) would look for a child under it.

```
 caller ── start_agent {agent, prompt, model?, effort?}
   │  host.Spawn(Config{Meta{startedFrom: caller},
   │                    PromptExchange: caller → sibling})
   │  events: spawned {by: start_agent, from: caller}
   ▼
 sibling: top-level row (no spawnedBy), its own idle time, not stopped
          with the caller (StopAll follows spawnedBy only), no workPrompt;
          answer.json kept, so agent_result and agent_send work, but
          sent to the caller only for an agent_send with background
```

Its opening card says `by <caller>` (`convo.Options.StartedFrom`).

## How each harness gets the tool

| Harness | How |
|---|---|
| Claude Code | SDK MCP server in the parent's host (`agent.ToolServer.Handle`) |
| Codex | `mcp_servers.rush` in `thread/start` config, `tool_timeout_sec = 3600` |
| ACP agents (Kimi, Copilot, OpenCode, Gemini, Vibe, DeepSeek, GLM) | `mcpServers` in `session/new` |
| Pi, Antigravity | no MCP: the shell lines (`agentsPrompt`) through rush's stand-ins |

Background results come back the same way for every harness: the child's
host sends a message into the parent's host (waking it if it rests), as an
exchange from the child, which the parent's agent takes as its next turn, or
as guidance mid-turn where the agent can take that.

## Parent and child in the UI

- The child's config carries `Meta.spawnedBy`; fleet links it to the parent.
- The parent's `spawn_agent` step is a spawn (`convo.Step.Spawn`). A child
  whose parent's harness names the call (Claude sends `_meta`
  `claudecode/toolUseId`) carries it as `Meta.spawnStep` and goes under that
  step; otherwise the step's window claims the hosted child that was asked
  exactly its prompt (`ui.claim`). From there it's drawn as every spawned agent is: the
  subagent panel, the subagent view and breadcrumb, status, steps and cost.
- The child's first message is a `PromptExchange` from the parent, so its
  conversation names the parent, not "you".
- Every place a run is listed names it the same way (`convo.RunTag`,
  `ui.subName`): what it was asked, then its model, and its harness's mark
  and name only when that isn't the parent's (`Review PCI · ◎ Codex
  astra`). Marks are the same everywhere too: its harness's spinner while
  it works, then ✓ done, ✗ failed, ⏹ stopped.
