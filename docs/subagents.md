# Subagents across harnesses

Status: built 2026-10-01. Replaces the Haiku relay agent types (`RUSH_TASK`).

Any rush session, whatever its harness, can hand a task to any agent rush
runs, foreground or background, through rush's own MCP server. The child is
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
| `spawn_agent` | `agent` (a name its description lists: `astra`, `haiku`, `kimi-code-k3`, or a harness/provider such as `codex`), `prompt`, optional `model`, `effort`, `background` | the answer, or the child's id when `background` |
| `agent_result` | `id`, `wait_seconds` (≤600) | the answer once done, else how it stands |
| `agent_send` | `id`, `prompt`, `background` | a follow-up's answer; the child keeps its conversation |

The list of agents is `host.picks()`: every installed provider, in the
harness it's set to run in, on each of its models, named as the old agent
types were (`defName`). A Claude session's own models stay native subagent
types (`--agents`, the Agent tool) and are left out of `spawn_agent`.

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
