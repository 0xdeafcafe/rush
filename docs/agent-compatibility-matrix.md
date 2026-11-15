# Rush compatibility matrix — 2026-10-01

Current working-tree snapshot, including uncommitted adapter changes. This documents Rush integration, not everything each native CLI or model can do. Extracted from all 21 registered adapters and 38 feature definitions. No authenticated inference or destructive session operations were performed for this review.

✓ = adapter declares support; ✓* = qualified support (notes below); — = not exposed by Rush; P = planned, not implemented. A tick is not a new live test. Model abilities, accounts and runtime protocol negotiation can further restrict availability.

Levels are the repository’s own labels: full / tested / preview. They describe integration maturity, not a guarantee for every feature or model.

## Harness and provider pairings

| Adapter | Model provider / route | Harness | Level | Status |
|---|---|---|---|---|
| anthropic-pi | Anthropic | Pi | preview | Current |
| antigravity | Google | Antigravity CLI | preview | Current |
| claude | Anthropic | Claude Code | full | Current |
| codex | OpenAI | Codex | tested | Current |
| copilot | GitHub | Copilot | tested | Current |
| deepseek | DeepSeek | dsh | preview | Current |
| deepseek-claude | DeepSeek | Claude Code | preview | Current |
| deepseek-pi | DeepSeek | Pi | preview | Current |
| gemini | Google | Gemini CLI | preview | Legacy history; new-session selection replaced by Antigravity |
| glm | GLM | ZCode | preview | Current |
| glm-claude | GLM | Claude Code | preview | Current |
| glm-pi | GLM | Pi | preview | Current |
| kimi | Moonshot | Kimi CLI | tested | Current |
| ollama | Ollama | Claude Code | tested | Current |
| ollama-codex | Ollama | Codex | tested | Current |
| ollama-pi | Ollama | Pi | tested | Current |
| ollama-vibe | Ollama | Vibe | tested | Current |
| openai-pi | OpenAI | Pi | preview | Current |
| opencode | OpenCode | OpenCode | preview | Current |
| pi | Pi | Pi | tested | Current |
| vibe | Mistral | Vibe | preview | Current |

Pi and OpenCode can expose providers configured in their native tools; those dynamic catalogs are not an assertion that Rush has a dedicated account/usage integration for every provider. Anthropic/OpenAI cross-harness Pi routes use API keys, not transferable native subscriptions. Ollama pairings require a compatible tool-capable local model. Image support also depends on the model.

## Main harnesses

| Feature | claude | codex | kimi | vibe | antigravity | pi | copilot | opencode | deepseek | glm |
|---|---|---|---|---|---|---|---|---|---|---|
| Run in rush mode | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Resume | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓* | ✓ |
| Fork | ✓ | ✓ | — | — | — | ✓* | — | — | — | — |
| Rewind | ✓ | — | — | — | — | —* | — | — | — | — |
| Interrupt | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Guide mid-turn | ✓* | ✓* | — | — | — | — | — | — | — | — |
| Switch model | ✓ | ✓* | ✓* | ✓* | —* | ✓* | ✓* | ✓* | ✓* | ✓* |
| Effort | ✓ | ✓ | — | ✓* | ✓* | ✓* | — | — | — | — |
| Permission modes | ✓ | ✓* | ✓* | ✓* | ✓* | —* | ✓* | ✓* | — | ✓* |
| Plan mode | ✓ | — | ✓* | ✓* | ✓* | —* | ✓* | ✓* | — | ✓* |
| Images | ✓ | ✓ | ✓ | ✓ | —* | ✓ | ✓ | ✓ | ✓* | ✓ |
| Questions | ✓ | ✓ | ✓ | ✓ | —* | ✓* | ✓ | ✓ | — | ✓ |
| Subagents | ✓ | — | — | — | — | —* | — | — | — | — |
| Other models as subagents | ✓ | ✓ | ✓* | ✓* | — | — | ✓* | ✓* | ✓* | ✓* |
| Background tasks | ✓ | — | —* | —* | — | — | —* | —* | —* | —* |
| Context breakdown | ✓ | ✓* | — | — | — | ✓* | — | — | — | — |
| Compact | ✓ | ✓ | ✓* | — | — | ✓ | — | — | — | — |
| Commands and skills | ✓ | P | — | — | — | ✓* | — | — | — | — |
| Side questions (btw) | ✓ | — | — | — | — | — | — | — | — | — |
| cd and add-dir | ✓ | — | — | — | — | — | — | — | — | — |
| MCP | ✓ | ✓ | ✓ | ✓ | — | —* | ✓ | ✓ | ✓ | ✓ |
| Hooks | ✓ | —* | — | — | — | — | — | — | — | — |
| Plugins | ✓ | — | — | — | — | — | — | — | — | — |
| Statusline | ✓ | — | — | — | — | — | — | — | — | — |
| Its own screen | ✓ | — | — | — | — | — | — | — | — | — |
| Hand-off in | ✓ | ✓ | ✓ | ✓ | — | ✓ | ✓ | ✓ | ✓ | ✓ |
| Hand-off in whole | ✓ | ✓ | — | — | — | — | — | — | — | — |
| rush's instructions | ✓ | ✓ | — | — | — | ✓ | — | — | — | — |
| Settings files | ✓ | P* | — | — | — | ✓* | — | — | — | — |
| Usage history | ✓ | — | — | — | — | — | — | — | — | — |
| Running sessions | ✓ | ✓ | — | — | — | ✓* | ✓* | — | — | — |
| Past sessions | ✓ | ✓ | ✓* | ✓* | ✓* | ✓ | ✓ | P | —* | P |
| Remote sessions | — | — | — | — | — | — | ✓* | — | — | — |
| Account switching | ✓ | ✓ | — | — | — | — | ✓* | — | — | — |
| Sign-in | ✓ | ✓ | ✓* | ✓* | ✓* | ✓* | ✓ | — | — | — |
| Limits | ✓ | ✓ | ✓* | —* | —* | —* | ✓* | — | ✓* | ✓* |
| Pricing | ✓ | P | P | P | — | —* | P* | P | — | P |
| Efficiency | ✓ | P | — | — | — | — | — | — | — | — |
| Memory | ✓ | P* | — | — | — | ✓* | — | — | — | — |

## Ollama pairings

| Feature | ollama | ollama-codex | ollama-pi | ollama-vibe |
|---|---|---|---|---|
| Run in rush mode | ✓ | ✓ | ✓ | ✓ |
| Resume | ✓ | ✓ | ✓ | ✓ |
| Fork | ✓ | ✓ | ✓* | — |
| Rewind | — | — | — | — |
| Interrupt | ✓ | ✓ | ✓ | ✓ |
| Guide mid-turn | ✓* | — | — | — |
| Switch model | ✓* | ✓* | ✓* | ✓* |
| Effort | —* | —* | —* | —* |
| Permission modes | ✓ | ✓* | —* | ✓* |
| Plan mode | — | — | — | — |
| Images | ✓* | ✓* | ✓* | ✓* |
| Questions | — | ✓ | ✓* | ✓ |
| Subagents | —* | — | — | — |
| Other models as subagents | ✓ | ✓ | — | ✓* |
| Background tasks | — | — | — | —* |
| Context breakdown | — | ✓* | ✓* | — |
| Compact | ✓* | ✓ | ✓* | — |
| Commands and skills | — | — | — | — |
| Side questions (btw) | — | — | — | — |
| cd and add-dir | — | — | — | — |
| MCP | — | — | — | ✓ |
| Hooks | — | — | — | — |
| Plugins | — | — | — | — |
| Statusline | — | — | — | — |
| Its own screen | — | — | — | — |
| Hand-off in | ✓ | ✓ | ✓ | ✓ |
| Hand-off in whole | ✓ | ✓ | — | — |
| rush's instructions | ✓ | ✓ | ✓ | — |
| Settings files | — | — | — | — |
| Usage history | — | — | — | — |
| Running sessions | — | ✓ | ✓* | — |
| Past sessions | — | ✓ | ✓ | P |
| Remote sessions | — | — | — | — |
| Account switching | — | —* | —* | —* |
| Sign-in | — | —* | —* | —* |
| Limits | —* | —* | —* | —* |
| Pricing | ✓* | ✓* | ✓* | ✓* |
| Efficiency | — | — | — | — |
| Memory | — | — | — | — |

## Other provider–harness pairings

| Feature | anthropic-pi | deepseek-claude | deepseek-pi | glm-claude | glm-pi | openai-pi |
|---|---|---|---|---|---|---|
| Run in rush mode | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Resume | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Fork | ✓* | ✓ | ✓* | ✓ | ✓* | ✓* |
| Rewind | —* | —* | —* | —* | —* | —* |
| Interrupt | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Guide mid-turn | — | ✓* | — | ✓* | — | — |
| Switch model | ✓* | ✓ | ✓* | ✓ | ✓* | ✓* |
| Effort | —* | —* | —* | —* | —* | —* |
| Permission modes | —* | ✓ | —* | ✓ | —* | —* |
| Plan mode | —* | ✓ | —* | ✓ | —* | —* |
| Images | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Questions | ✓* | ✓ | ✓* | ✓ | ✓* | ✓* |
| Subagents | —* | ✓ | —* | ✓ | —* | —* |
| Other models as subagents | — | ✓ | — | ✓ | — | — |
| Background tasks | — | ✓ | — | ✓ | — | — |
| Context breakdown | ✓* | ✓ | ✓* | ✓ | ✓* | ✓* |
| Compact | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Commands and skills | —* | —* | —* | —* | —* | —* |
| Side questions (btw) | — | ✓ | — | ✓ | — | — |
| cd and add-dir | — | ✓ | — | ✓ | — | — |
| MCP | —* | ✓ | —* | ✓ | —* | —* |
| Hooks | — | ✓ | — | ✓ | — | — |
| Plugins | — | ✓ | — | ✓ | — | — |
| Statusline | — | ✓ | — | ✓ | — | — |
| Its own screen | — | ✓ | — | ✓ | — | — |
| Hand-off in | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Hand-off in whole | — | ✓ | — | ✓ | — | — |
| rush's instructions | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Settings files | ✓* | ✓ | ✓* | ✓ | ✓* | ✓* |
| Usage history | — | ✓ | — | ✓ | — | — |
| Running sessions | —* | —* | —* | —* | —* | —* |
| Past sessions | —* | —* | —* | —* | —* | —* |
| Remote sessions | — | — | — | — | — | — |
| Account switching | —* | —* | —* | —* | —* | —* |
| Sign-in | —* | —* | —* | —* | —* | —* |
| Limits | —* | —* | —* | ✓* | ✓* | —* |
| Pricing | —* | —* | —* | —* | —* | —* |
| Efficiency | — | ✓ | — | ✓ | — | — |
| Memory | ✓* | ✓ | ✓* | ✓ | ✓* | ✓* |

## Legacy Gemini

| Feature | gemini |
|---|---|
| Run in rush mode | ✓ |
| Resume | ✓ |
| Fork | — |
| Rewind | — |
| Interrupt | ✓ |
| Guide mid-turn | — |
| Switch model | ✓* |
| Effort | — |
| Permission modes | ✓* |
| Plan mode | ✓* |
| Images | ✓ |
| Questions | ✓ |
| Subagents | — |
| Other models as subagents | ✓* |
| Background tasks | —* |
| Context breakdown | — |
| Compact | — |
| Commands and skills | — |
| Side questions (btw) | — |
| cd and add-dir | — |
| MCP | ✓ |
| Hooks | — |
| Plugins | — |
| Statusline | — |
| Its own screen | — |
| Hand-off in | ✓ |
| Hand-off in whole | — |
| rush's instructions | — |
| Settings files | — |
| Usage history | — |
| Running sessions | — |
| Past sessions | ✓* |
| Remote sessions | — |
| Account switching | — |
| Sign-in | ✓* |
| Limits | ✓* |
| Pricing | P |
| Efficiency | — |
| Memory | — |

## Important qualifications

- **Codex compaction:** `/compact` and #compact call native `thread/compact/start` (Codex and Ollama-in-Codex). Conversation rollback (`thread/rollback`) is deprecated and is not file restoration. These are integration gaps/distinctions, not evidence that Codex cannot compress. [Official app-server documentation](https://learn.chatgpt.com/docs/app-server).
- **Codex hooks:** rush has no /hooks command for Codex, but Settings › Harnesses › Codex › rush's gate and context guard (off until you turn it on, and asked first) writes rush's hook into hooks.json in Codex's folder and trusts it in config.toml's `hooks.state`, through Codex's own app-server. Then every Codex session, rush's or one started outside rush, runs it before and after each shell command (PreToolUse and PostToolUse on `Bash`, and PreCompact): search and context guard refusals everywhere, gate rewrites only in sessions rush runs. Codex runs only hooks you trusted and skips one passed with `-c` for one session, so nothing is passed per session. Turning it off takes it out of both files.
- **External compression:** only Claude and Ollama currently implement the summarizer interface. Codex is not an available summarizing engine. The target session must advertise rewind, have a protocol-9-or-newer host, and pass unchanged-session/idle/queue guards. Most harnesses are therefore excluded. Native compaction is a separate path. Ollama classifiers such as tev1 are excluded as summarizers.
- **Inherited rewind declarations corrected:** `deepseek-claude` and `glm-claude` do not implement `agent.Rewinder` or `agent.Brancher`; their rewind flags are now disabled. A registry regression test requires both interfaces for every advertised rewind feature. Their other inherited ticks remain declarations, not verified end-to-end coverage; provider-specific image/tool behavior needs checking.
- **ACP harnesses:** Kimi, Vibe, Copilot, OpenCode, dsh and ZCode use protocol negotiation. Resume depends on advertised load/resume support; images depend on advertised image input and model ability; model/permission/plan choices depend on what the running CLI returns. A static yes is not a guarantee that a particular installation offers the control.
- **Antigravity:** preview, text-only streaming input in the current adapter. Model selected before startup; effort/permission/plan settings are startup selections. History covers Rush-started conversations, not native imports. No Rush quota display or interactive question/approval protocol. Signed-in model execution remains unverified in the preceding implementation pass.
- **Pi:** fork means the whole conversation, not rewind to a selected message. Context is occupancy, not a composition breakdown. Questions are extension select/confirm. Rush does not expose Pi permission modes; its adapter notes that tools run without asking. Extension features are not automatically Rush features.
- **Kimi:** native login, configured model discovery, local history, and Kimi Code plan quota are integrated. Its native `/compact` is exposed. No Rush account swapping, rewind, or mid-turn steering is declared. Its saved wire history reader and account handling are specific to Kimi; its conversation UI is shared.
- **Vibe:** native login and saved local history are integrated. No Rush account switching or remaining-quota display; effort depends on the model. Ollama-in-Vibe is a separate adapter with a different feature set.
- **Usage semantics differ:** Claude/Codex quota windows, Copilot premium requests, DeepSeek API balance, GLM Coding Plan (read in ZCode, Claude Code and Pi, with ZCode's key or the Coding Plan key Settings › Providers keeps) and Kimi Code plan limits are distinct measures. Local Ollama pricing means no provider token charge, not zero machine/energy cost.
- **Other models as subagents (Rust-only):** ✓ = every model rush runs is one of the harness's own subagent types (Claude Code: `--agents` stand-ins on haiku whose Agent call rush's hooks answer, one haiku turn per call; Codex: a `run_agent` dynamic tool rush answers, no extra model). ✓* = through rush's `spawn_agent` MCP tool, told as its own subagents. — = rush's shell stand-ins only. Tiers (top, workhorse, quick, with long-context, vision or cheap) are named the same way. See [native-agents.md](native-agents.md).
- **Shared Rush features:** composer, queue persistence, transcript row controls, agent-message rows and community/discussion views are core UI features. The adapter’s “Subagents” flag refers to native subagent integration, not whether Rush can launch a separate peer session.

## Kimi screenshot

The blue “agent message” row is rendered by `internal/convo/exchange.go` for all harnesses. It now puts the harness first and reserves width for both sender and receiver, truncating long names by terminal-cell width. Session IDs stay in the full copied attribution rather than crowding the heading. This shared change applies to every harness. The screenshot shows the earlier requested message delivered to the Ollama Context Compression System.

## Exact adapter notes

### anthropic-pi

- Fork (supported): the whole session, not from a message
- Rewind (not exposed): its /tree is its own screen's
- Switch model (supported): any model Pi has, at once
- Effort (not exposed): as the provider serves it
- Permission modes (not exposed): Pi asks before nothing: every tool runs
- Plan mode (not exposed): only as an extension
- Questions (supported): an extension's select and confirm
- Subagents (not exposed): only as an extension
- Context breakdown (supported): how full it is, not what fills it
- Commands and skills (not exposed): not for a provider in another's harness yet
- MCP (not exposed): only as an extension
- Settings files (supported): settings.json, yours and the project's
- Running sessions (not exposed): not for a provider in another's harness yet
- Past sessions (not exposed): not for a provider in another's harness yet
- Account switching (not exposed): one API key, kept in Settings › Providers
- Sign-in (not exposed): an API key, not a sign-in
- Limits (not exposed): paid per token with the API key
- Pricing (not exposed): the provider's own prices, which rush doesn't know
- Memory (supported): its AGENTS.md files

### antigravity

- Switch model (not exposed): choose before starting; streaming sessions cannot switch models
- Effort (supported): selected at startup
- Permission modes (supported): selected at startup; headless tools follow native policy
- Plan mode (supported): selected at startup
- Images (not exposed): agy's streaming input accepts text only
- Questions (not exposed): native headless policy; no interactive approval protocol
- Past sessions (supported): conversations started in Rush; native imports are not available
- Sign-in (supported): Antigravity's native Google sign-in
- Limits (not exposed): use /usage in Antigravity; Gemini Code Assist limits do not apply

### claude

- Guide mid-turn (supported): at its next step

### codex

- Guide mid-turn (supported): steered into the turn
- Switch model (supported): from the next turn
- Permission modes (supported): read-only, auto, full-access
- Context breakdown (supported): how full it is, not what fills it
- Settings files (planned): config.toml
- Memory (planned): its AGENTS.md files

### copilot

- Switch model (supported): when the agent offers models
- Permission modes (supported): the agent's own
- Plan mode (supported): when the agent has a plan mode
- Background tasks (not exposed): ACP has no background tasks
- Running sessions (supported): the coding agent's, on GitHub
- Remote sessions (supported): view only
- Account switching (supported): gh's accounts
- Limits (supported): premium requests
- Pricing (planned): its models' premium-request multipliers

### deepseek

- Resume (supported): without replaying the conversation
- Switch model (supported): when dsh offers models
- Images (supported): on models that take them
- Background tasks (not exposed): ACP has no background tasks
- Past sessions (not exposed): dsh's session logs are compressed and still change
- Limits (supported): the API key's balance

### deepseek-claude

- Rewind (not exposed): provider-specific transcript branching and file rewind are not integrated
- Guide mid-turn (supported): at its next step
- Effort (not exposed): as the provider serves it
- Commands and skills (not exposed): not for a provider in another's harness yet
- Running sessions (not exposed): not for a provider in another's harness yet
- Past sessions (not exposed): not for a provider in another's harness yet
- Account switching (not exposed): one API key, kept in Settings › Providers
- Sign-in (not exposed): an API key, not a sign-in
- Limits (not exposed): paid per token with the API key
- Pricing (not exposed): the provider's own prices, which rush doesn't know

### deepseek-pi

- Fork (supported): the whole session, not from a message
- Rewind (not exposed): its /tree is its own screen's
- Switch model (supported): any model Pi has, at once
- Effort (not exposed): as the provider serves it
- Permission modes (not exposed): Pi asks before nothing: every tool runs
- Plan mode (not exposed): only as an extension
- Questions (supported): an extension's select and confirm
- Subagents (not exposed): only as an extension
- Context breakdown (supported): how full it is, not what fills it
- Commands and skills (not exposed): not for a provider in another's harness yet
- MCP (not exposed): only as an extension
- Settings files (supported): settings.json, yours and the project's
- Running sessions (not exposed): not for a provider in another's harness yet
- Past sessions (not exposed): not for a provider in another's harness yet
- Account switching (not exposed): one API key, kept in Settings › Providers
- Sign-in (not exposed): an API key, not a sign-in
- Limits (not exposed): paid per token with the API key
- Pricing (not exposed): the provider's own prices, which rush doesn't know
- Memory (supported): its AGENTS.md files

### gemini

- Switch model (supported): when the agent offers models
- Permission modes (supported): the agent's own
- Plan mode (supported): when the agent has a plan mode
- Background tasks (not exposed): ACP has no background tasks
- Past sessions (supported): saved local conversations
- Sign-in (supported): native terminal login; no account swapping
- Limits (supported): legacy Code Assist quota

### glm

- Switch model (supported): when the agent offers models
- Permission modes (supported): the agent's own
- Plan mode (supported): when the agent has a plan mode
- Background tasks (not exposed): ACP has no background tasks
- Limits (supported): the GLM Coding Plan's

### glm-claude

- Rewind (not exposed): provider-specific transcript branching and file rewind are not integrated
- Guide mid-turn (supported): at its next step
- Effort (not exposed): as the provider serves it
- Commands and skills (not exposed): not for a provider in another's harness yet
- Running sessions (not exposed): not for a provider in another's harness yet
- Past sessions (not exposed): not for a provider in another's harness yet
- Account switching (not exposed): one Coding Plan key, kept in Settings › Providers
- Sign-in (not exposed): a Coding Plan key, not a sign-in
- Limits (supported): the GLM Coding Plan's
- Pricing (not exposed): the plan's, not per token

### glm-pi

- Fork (supported): the whole session, not from a message
- Rewind (not exposed): its /tree is its own screen's
- Switch model (supported): any model Pi has, at once
- Effort (not exposed): as the provider serves it
- Permission modes (not exposed): Pi asks before nothing: every tool runs
- Plan mode (not exposed): only as an extension
- Questions (supported): an extension's select and confirm
- Subagents (not exposed): only as an extension
- Context breakdown (supported): how full it is, not what fills it
- Commands and skills (not exposed): not for a provider in another's harness yet
- MCP (not exposed): only as an extension
- Settings files (supported): settings.json, yours and the project's
- Running sessions (not exposed): not for a provider in another's harness yet
- Past sessions (not exposed): not for a provider in another's harness yet
- Account switching (not exposed): one Coding Plan key, kept in Settings › Providers
- Sign-in (not exposed): a Coding Plan key, not a sign-in
- Limits (supported): the GLM Coding Plan's
- Pricing (not exposed): the plan's, not per token
- Memory (supported): its AGENTS.md files

### kimi

- Switch model (supported): when the agent offers models
- Permission modes (supported): the agent's own
- Plan mode (supported): when the agent has a plan mode
- Background tasks (not exposed): ACP has no background tasks
- Past sessions (supported): saved local conversations
- Sign-in (supported): native terminal login; no account swapping
- Limits (supported): Kimi Code plans only, as its /usage reads them
- Compact (supported): its own /compact

### ollama

- Guide mid-turn (supported): at its next step
- Switch model (supported): any model Ollama has that calls tools
- Effort (not exposed): a model thinks or doesn't, as it was made
- Images (supported): on models that take them
- Subagents (not exposed): left out to keep the prompt small
- Compact (supported): at the window the model was loaded with
- Limits (not exposed): no limits but this machine's
- Pricing (supported): free: it runs on this machine

### ollama-codex

- Switch model (supported): any model Ollama has that calls tools
- Effort (not exposed): a model thinks or doesn't, as it was made
- Permission modes (supported): read-only, auto, full-access
- Images (supported): on models that take them
- Context breakdown (supported): how full it is, not what fills it
- Account switching (not exposed): no accounts, only this machine
- Sign-in (not exposed): nothing to sign in to
- Limits (not exposed): no limits but this machine's
- Pricing (supported): free: it runs on this machine

### ollama-pi

- Fork (supported): the whole session, not from a message
- Switch model (supported): any model Ollama has that calls tools
- Effort (not exposed): a model thinks or doesn't, as it was made
- Permission modes (not exposed): Pi asks before nothing: every tool runs
- Images (supported): on models that take them
- Questions (supported): an extension's select and confirm
- Context breakdown (supported): how full it is, not what fills it
- Compact (supported): at the window the model was loaded with
- Running sessions (supported): sessions written to in the last two minutes
- Account switching (not exposed): no accounts, only this machine
- Sign-in (not exposed): nothing to sign in to
- Limits (not exposed): no limits but this machine's
- Pricing (supported): free: it runs on this machine

### ollama-vibe

- Switch model (supported): any model Ollama has that calls tools
- Effort (not exposed): a model thinks or doesn't, as it was made
- Permission modes (supported): Vibe's own
- Images (supported): on models that take them
- Background tasks (not exposed): ACP has no background tasks
- Account switching (not exposed): no accounts, only this machine
- Sign-in (not exposed): nothing to sign in to
- Limits (not exposed): no limits but this machine's
- Pricing (supported): free: it runs on this machine

### openai-pi

- Fork (supported): the whole session, not from a message
- Rewind (not exposed): its /tree is its own screen's
- Switch model (supported): any model Pi has, at once
- Effort (not exposed): as the provider serves it
- Permission modes (not exposed): Pi asks before nothing: every tool runs
- Plan mode (not exposed): only as an extension
- Questions (supported): an extension's select and confirm
- Subagents (not exposed): only as an extension
- Context breakdown (supported): how full it is, not what fills it
- Commands and skills (not exposed): not for a provider in another's harness yet
- MCP (not exposed): only as an extension
- Settings files (supported): settings.json, yours and the project's
- Running sessions (not exposed): not for a provider in another's harness yet
- Past sessions (not exposed): not for a provider in another's harness yet
- Account switching (not exposed): one API key, kept in Settings › Providers
- Sign-in (not exposed): an API key, not a sign-in
- Limits (not exposed): paid per token with the API key
- Pricing (not exposed): the provider's own prices, which rush doesn't know
- Memory (supported): its AGENTS.md files

### opencode

- Switch model (supported): when the agent offers models
- Permission modes (supported): the agent's own
- Plan mode (supported): when the agent has a plan mode
- Background tasks (not exposed): ACP has no background tasks

### pi

- Fork (supported): the whole session, not from a message
- Rewind (not exposed): its /tree is its own screen's
- Switch model (supported): any model Pi has, at once
- Effort (supported): its thinking level, from the start
- Permission modes (not exposed): Pi asks before nothing: every tool runs
- Plan mode (not exposed): only as an extension
- Questions (supported): an extension's select and confirm
- Subagents (not exposed): only as an extension
- Context breakdown (supported): how full it is, not what fills it
- Commands and skills (supported): prompt templates and skills; not packages' or extensions'
- MCP (not exposed): only as an extension
- Settings files (supported): settings.json, yours and the project's
- Running sessions (supported): sessions written to in the last two minutes
- Sign-in (supported): its own /login; no account swapping
- Limits (not exposed): each provider keeps its own limits
- Pricing (not exposed): Pi prices each turn itself
- Memory (supported): its AGENTS.md files

### vibe

- Switch model (supported): when the agent offers models
- Effort (supported): model-supported thinking levels
- Permission modes (supported): the agent's own
- Plan mode (supported): when the agent has a plan mode
- Background tasks (not exposed): ACP has no background tasks
- Past sessions (supported): saved local conversations
- Sign-in (supported): native terminal login; no account swapping
- Limits (not exposed): Vibe says its plan, not what's left of it

## Evidence and verification

- Registry: `cmd/rush/adapters.go`, `internal/agent/feature.go`, `internal/agent/harness.go` and each registered adapter’s `Features()` / `Level()`.
- Runtime caveats: `internal/adapters/acp/session.go`, `internal/adapters/antigravity/antigravity.go`.
- Compression/rewind: `internal/ui/compactsheet.go`, `internal/ui/rewindsheet.go`, `internal/adapters/cross/cross.go`, `internal/adapters/claude/summary.go`, `internal/adapters/ollama/summary.go`.
- `TMPDIR=/tmp go test ./internal/adapters -run 'TestFeaturesMatchInterfaces|TestFeaturesKnown|TestLevels' -count=1` passed. This checks declared interface coverage and levels, not live provider behavior; the added `TestRewindRequiresWorkingInterfaces` also prevents unsupported rewind declarations.
- CSV includes every adapter/feature cell and its exact declared note, with audit-status and limitation notes.
