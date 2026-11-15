# Every model as a harness's own subagents (Rust-only)

A session can hand work to any model rush runs: kimi, astra, sol, opus, glm and the rest. Where the harness has its own subagent mechanism and rush can register types in it and answer the call, those models show up as that harness's own subagent types under their plain names. Starting kimi from Claude Code looks like starting one of Claude Code's own subagents. Behind the scenes rush runs the model the way `spawn_agent` does: it resolves the name, starts the model as a rush-hosted child of the session (Meta `spawnedBy`), and passes the child's final answer back as the call's result.

Code:

- `crates/rush-core/src/adapter/others.rs`: what an adapter is handed (`OtherAgent`, `Delegate`, `DelegateCall`), plus tiers and traits.
- `crates/rush-host/src/native.rs`: which models are offered, how a tier picks a model, what each harness is told, and the session's Delegate.
- `crates/rush-claude/src/others.rs`: Claude Code's stand-ins and hooks.
- `crates/rush-codex/src/others.rs`: Codex's `run_agent` tool.
- `crates/rush-host/src/subtool.rs`: `spawn_agent` also takes a tier.

## Per harness

Each harness was checked against the version installed on 2026-11-15.

| Harness | Native mechanism | How rush registers the models | How rush answers the call | Fallback |
|---|---|---|---|---|
| Claude Code 2.1.319 (and anything riding it: DeepSeek, GLM, Ollama) | `Agent` tool with `--agents` definitions | Each model and tier gets an `--agents` definition: its name, a description (provider, tier, traits), and a stand-in on `haiku` with `maxTurns: 1` and `tools: []`. The SDK `initialize` request registers rush's PreToolUse and PostToolUse hook callbacks for `Agent\|Task`. | PreToolUse stores the real task and swaps the call's input for "Reply with the single word: ok." (`run_in_background` false; `model` and `isolation` removed). PostToolUse runs the task on the requested model and puts the answer in the Agent tool's own output (`updatedToolOutput`, `content`). One hop: a single haiku turn per call. | `spawn_agent` for a model at a particular version or effort |
| Codex 0.169 (and Ollama on Codex) | `spawn_agent` / `wait_agent` (multi-agent). It only runs Codex's own models, and no other tool can take that name. | `thread/start` gets `dynamicTools: [run_agent]`. `run_agent` has the same shape as `spawn_agent` (`agent_type`, `message`, `background`), and `agent_type` is an enum of the model and tier names. | Codex sends an `item/tool/call` request, and rush replies `{contentItems:[{type:"inputText",text}], success}` with the model's answer. No model sits in between, so it's zero-hop. The call is drawn as a subagent. | On a resumed thread (dynamicTools are only accepted on `thread/start`), rush's `spawn_agent`, named in Codex's terms |
| Kimi CLI 0.36 | `Agent` tool with built-in types | No way to add types at launch without writing the user's config, and no hook that could answer the call | — | rush's `spawn_agent` MCP tool, presented as its own subagents (`OTHERS_NOTE`) |
| Vibe 3.1 | `agents.spawn`, TOML profiles via `VIBE_AGENT_PATHS` | A profile can name a model, but only on an endpoint Vibe talks to directly | — | MCP `spawn_agent` (`OTHERS_NOTE`) |
| OpenCode 1.17 | `task` tool, agents in `OPENCODE_CONFIG_CONTENT` | An agent is a model on one of OpenCode's providers | — | MCP `spawn_agent` (`OTHERS_NOTE`) |
| Copilot | Agent files picked up via `--add-dir` | An agent file can only run Copilot's models, so this would be one hop | — | MCP `spawn_agent` (`OTHERS_NOTE`) |
| Gemini CLI, dsh, ZCode | Gemini has A2A remote agents; dsh and ZCode have none rush can reach | — | — | MCP `spawn_agent` (`OTHERS_NOTE`) |
| Pi 1.5 (every provider) | None (an extension could add one) | — | — | Shell stand-ins (`agents_prompt`), as before |
| Antigravity | None rush can reach | — | — | Shell stand-ins (`agents_prompt`), as before |

The native list is `native::NATIVE`, which holds harness names, so every provider that rides Claude Code or Codex gets the same treatment.

### What a session is offered

`native::offers` builds the list from every agent the session's harness can hand work to, the same set `spawn_agent` offers. That set respects the adapters the user has turned on in Settings › Config and the accounts they've added. Each entry has its plain pick name (`kimi`, `astra`, `glm`, `flash`) and a description, for example: "Frontier intelligence. OpenAI's gpt-6-astra; top tier, vision."

`native::others_of` leaves out the harness's own models, because the harness already offers those (opus in Claude Code, sol in Codex). It then appends the tiers: `top`, `workhorse` and `quick`. For each trait that some of a tier's models have and others don't, it also adds a `{tier}-{trait}` entry, for example `workhorse-vision`. Sign-ins aren't checked when the list is built, so it stays the same for the whole session; they're checked when a call is made.

### One call, one run

In Claude Code the PreToolUse hook starts the requested model right away, while the stand-in runs, so the hosted run exists as soon as the stand-in does. The PostToolUse hook then waits for the model's answer. The hosted run's `spawnStep` is the Agent call's `tool_use_id`, the same id Claude Code records on the stand-in. When a run rush hosts has that id, the Subagents panel leaves out Claude Code's run for the call (`Runs::rebuild` in `rush-ui-session-core/src/subs.rs`). That leaves exactly one row, and it is labelled `top · astra` or the model's name. Pressing enter on the call opens that run's conversation. On the Orchestrator board the call's lane names the same model and shows it at work for as long as it is (`lanes` in `rush-convo/src/render/draw/presets.rs`).

### Background and parallel calls

- **Claude Code:** the PostToolUse hook runs on its own thread, so several Agent calls can be in flight at once. A call with `run_in_background` still returns at once: the Delegate starts the child in the background and returns its id. The stand-in itself always runs in the foreground, so the hook gets to answer. The model's answer then arrives as rush's existing agent message.
- **Codex:** each `item/tool/call` is answered from a thread of its own. `background: true` returns the id right away, and the answer arrives as rush's message.
- While a Claude Code call is waiting, the session isn't handed off (`pass` refuses).

## Tiers

There are three tiers:

- **top:** hard reasoning, design and final review
- **workhorse:** most coding
- **quick:** search, summaries and mechanical edits

Three traits can sit on top of a tier: `long-context`, `vision` and `cheap`.

- **Where a model's tier comes from:** `others::place` reads the words in the model's id. It sits beside the model lists: opus, astra, fable, reasoner, pro and max are top; haiku, luna, flash, mini, devstral and highspeed are quick; anything else is workhorse. A local model is quick and cheap. A context window of 500k or more, or `1m` in the name, adds long-context.
- **Moving a model:** the user can put a model in another tier in Settings › Config. Select a cell, open its model list and press `t`. The choice is saved in `Config.modelTiers`. Moving a model back to the tier its name gives removes the entry.
- **Naming a tier:** a tier can be named wherever a model can: a Claude Code subagent type, Codex's `run_agent` `agent_type`, or `spawn_agent`'s `agent`. A trait can follow it, as in `quick long-context`, `quick-long-context` or `top vision`.
- **Which model a tier picks:** `native::tier_pick` chooses among the tier's models that have every requested trait, are signed in (`cfg.calls`) and have quota left. Quota counts as left unless every limit reading for that adapter or provider is nearly used up; no readings counts as room. Among those, it takes the provider with the fewest calls currently held (`native::HELD`). On a tie, the first listed wins: the leader's own harness, then the order `spawn_agent` lists them in. A background call's hold is released when the child writes its answer. A foreground call's hold is released when the call returns.
- **How it shows:** the run is labelled `tier · model` (for example `top · astra`) through the Meta `spawnLabel`. The card, the Subagents panel and the Orchestrator board use that label as the run's name, and a foreground answer begins with it.

## Other models across harnesses: GLM

When a provider rides the leader's own harness with an endpoint of its own (GLM's Coding Plan in Claude Code, DeepSeek in Claude Code), any session can start it by name: `glm`, `glm-5.3`, `flash`, `glm 5.3 flash`. Before this change, `takes()` refused GLM from a Claude Code session whenever the Claude Code cell for GLM wasn't on. GLM's ★ points at ZCode, and ZCode isn't installed. Now, if the leader's harness has no cell for the provider, the general rule applies and resolves to `glm-claude`. The pair also lists the provider's own models (`glm-5.3`, `glm-5.3-flash`; `deepseek-chat`, `deepseek-reasoner`) rather than Claude's names. The test is `agents::tests::glm_in_claude_code_is_offered_to_every_harness`.

GLM in Claude Code is set up the way [Z.ai's guide](https://docs.z.ai/devpack/tool/claude) says (`cross::zai_env`):

- **Models:** opus and sonnet run on the model picked, or `glm-5.3[1m]` when none is picked. Haiku and Claude Code's background jobs run on `glm-5.3-flash[1m]`. A picked GLM-5 model is passed with `[1m]`, as the guide names both.
- **Context:** 1M (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW`), and rush shows a 1M window for it.
- **Other settings:** `API_TIMEOUT_MS=3000000` and `DISABLE_TELEMETRY=1`.

The test is `cross::tests::glm_in_claude_code_is_set_up_as_zai_says`. A side effect: the stand-in on `haiku` runs on GLM's Flash in a GLM session.

## Tests

- `rush-core` `others::tests`: tier placement by family (table) and parsing tier queries (table).
- `rush-host` `native::tests`:
  - tier picks spread across providers (table), including calls made at the same time
  - the exact catalogue each harness sees: Claude Code's and Codex's type names, descriptions and tiers line
  - offers ignore sign-ins
  - a model moved in Settings stays in its new tier
  - quota is treated as room unless every reading is full
- `rush-host` `agents::tests::glm_in_claude_code_is_offered_to_every_harness`.
- `rush-claude`:
  - `others::tests`: stand-ins, PRE/POST hooks as a table, nothing done when there's no delegate
  - `conn::tests::another_model_answers_an_agent_call`: a fake `claude` process gets the hooks in `initialize`, sends PreToolUse and PostToolUse `hook_callback`s, and the test checks the replies
- `rush-codex`:
  - `others::tests`: the tool spec and calls (table)
  - `conn::tests::another_model_answers_run_agent`: a fake app-server gets `dynamicTools` on `thread/start` and none on resume, and a `run_agent` call is answered while another tool is refused

## Open questions

- **Claude Code's stand-in costs one haiku turn per call.** A zero-hop route needs one of two things from Claude Code: a hook that can skip running a subagent and still return a result (PreToolUse `deny` caps its reason at about 2000 characters, so it can't carry an answer), or a custom agent `model` that points at another endpoint.
- **Resuming a stand-in with SendMessage** gets the stand-in, not the model. A follow-up has to be a new Agent call, or `agent_send` to the hosted child.
- **Codex `dynamicTools` on resume** isn't accepted, so `run_agent` is missing from threads resumed after an upgrade. Those fall back to `spawn_agent`. It's not yet verified whether Codex keeps a thread's dynamic tools across `thread/resume` once it has them.
- **Vibe and OpenCode could go zero-hop** by pointing a profile's endpoint at a small OpenAI-compatible server inside rush that answers with the model's reply. Not built.
- **Pi could get native subagents** through an extension loaded with `-e`. Not built.
- **The tier word lists are one central table** (`others.rs`). A provider that lists its own tiers could feed them in instead.
- **Live checks:** none of this has been run against a real `claude` or `codex` yet. Only the fake-process tests cover the protocols.
