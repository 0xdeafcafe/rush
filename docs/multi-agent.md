# rush for every agent

rush is written for Claude Code throughout. This is the plan for making the core agent-agnostic, then adding Codex, Copilot CLI, Kimi CLI, Mistral Vibe and others as adapters. It has four stages: refactor, architecture, usage, then the agents themselves. The architecture comes first in this document because the refactor needs a shape to aim at.

## Where Claude Code is wired in today

- **Identity.** `claude.Account` (a `~/.claude*` config dir) is the account type everywhere: state, actions, daemon, efficiency, statusline and about 15 UI files. Logins are Claude OAuth credentials swapped in and out of the keychain.
- **The agent row.** `fleet.Agent` embeds `claude.Job`, which is Claude's background job file. Discovery reads `~/.claude/sessions/<pid>.json`, `jobs/*/state.json` and the daemon roster, and checks processes by `comm == "claude"`.
- **The event model.** `headless.Event` is already a typed layer between the wire and the renderer, and `convo.Session.Apply` is fed by both live sessions and JSONL transcripts through it. Its types still follow Claude's protocol, though: `Block{Type:"tool_use"}`, `Usage` with Anthropic's cache fields, `PermissionRequest` with `Suggestions`, and task types like `local_bash`.
- **The wire.** `rush host` sends Claude's raw stream-json lines to every client (UI, menubar, plugind, `rush session`), and each client decodes them itself.
- **Tools.** convo and efficiency switch on Claude's tool names (`Bash`, `Edit`, `TodoWrite`, `AskUserQuestion` and about 30 more) and read Claude's input keys (`command`, `file_path`) and structured results (`structuredPatch`, `stdout`).
- **Usage.** `claude.Usage` has exactly two windows, `FiveHour` and `SevenDay`. The UI, statusline, menubar, state (`SwitchAt`) and `--dump` all read them by name.
- **Features.** Rewind, fork, skills, hooks, plugins, the statusline hook, CLAUDE.md memory, `/permissions`, the Claude settings tab, the efficiency savers catalog and `rush on` are all Claude features. Other agents have some of them, in other shapes.

Some of it is already neutral and needs no work: `proc`, `cellw`, `fswait`, most of `plugin`, the queue, idle-stop and retry logic in `host`, and most of the UI chrome.

## Architecture

### Four words, kept apart

| Word | Meaning | Examples |
|---|---|---|
| **Agent** (kind) | A program that does coding work, and the adapter that drives it. | `claude`, `codex`, `copilot`, `kimi`, `vibe`, `gemini` |
| **Profile** | One config home for one agent: where its settings, transcripts and sessions live. | `~/.claude`, `~/.codex`, `~/.copilot`, `CLAUDE_CONFIG_DIR=…` |
| **Account** | Who pays, and whose quota is used. A credential with a plan. | A Claude Max sign-in, a ChatGPT Plus sign-in, a GitHub Copilot seat, a Moonshot API key |
| **Session** | One conversation run by one agent, in one profile, on one account. | A live `claude -p`, a Codex thread, a past transcript |

Today `claude.Account` stands in for both Profile and Account, and Logins are a second, Claude-only kind of Account. Keeping the four apart is what makes usage work across agents: a Copilot seat can run Claude and GPT models, and one ChatGPT account can be signed into several Codex profiles.

### Packages

```
internal/agent/               the core domain. Imports nothing agent-specific.
  agent.go                    Kind, Adapter interface, Capabilities, registry
  profile.go                  Profile, Account, Credential
  session.go                  Session (live or past), State, Todo, Task, PR
  event/                      the neutral event model (replaces headless.Event on the wire)
  tool/                       ToolKind and typed inputs and results
  usage/                      Quota, Window, TokenUsage, Pricing, the shared cache (usagecache.go moves here)

internal/adapters/
  claude/                     the adapter; claude/, headless/ and daemon/ under it are Claude Code's own packages
  acp/                        a generic Agent Client Protocol client (JSON-RPC over stdio)
  codex/                      codex app-server driver, ~/.codex/sessions rollouts, ChatGPT rate limits
  copilot/                    ACP via `copilot --acp`, ~/.copilot, premium-request quota
  kimi/                       ACP via `kimi acp`, Moonshot usage
  vibe/                       ACP via `vibe-acp`, Mistral usage
  gemini/ …                   the same pattern; each is thin when ACP does the driving

internal/fleet, host, convo, ui, statusline, menubar, efficiency, plugind
                              depend only on internal/agent. They find adapters through the registry.
```

`cmd/rush` imports every adapter for side effects (`_ "…/adapters/codex"`), which registers it, so the core never names one. rush-lint's `rushadapters` check (`tools/lint/adapters.go`) reports any import of `internal/adapters/...` from a package under `internal/` other than the adapters; `cmd/` and `tools/` register them, and tests may import them.

### The adapter interface

An adapter is a set of optional parts. The core asks for each one and hides the feature when it isn't there, rather than every adapter stubbing everything.

The interfaces are in `internal/agent/adapter.go`. In short:

- **Adapter** (every adapter has this part): `Kind`, `Name`, `Features`, `Level`, `Profiles`.
- **Discoverer**: sessions running outside rush, and past ones.
- **Driver**: `Start(ctx, StartOptions) (Conn, error)`. A `Conn` gives `Events`, `Send`, `Answer(approvalID, optionID)`, `Interrupt`, `SetModel`, `SetMode` and `Close`. An **Answerer** also takes answers to questions.
- **HistoryReader**: a transcript read back as the same events.
- **QuotaSource**: `Quota(ctx, Profile, Account)`. It takes the profile because Codex's sign-in lives in the profile folder.
- **Accounts**: `Accounts`, `Current`, `Switch` and `SignIn`.
- **Pricer**: `Cost(model, TokenUsage)`.
- **Commander**: slash commands and skills.

`Features` says, for every rush feature, whether the adapter has it (`agent.Supports(kind, feature)`), whole interfaces and smaller things alike: `Rewind`, `Fork`, `Resume`, `Images`, `Effort`, `Modes`, `Plan`, `Subagents`, `Background`, `Questions`, `Context`, `Compact`, `MCP`, `Hooks`, `Plugins`, `StatusLine`, `Screen` and the rest. `ui/caps.go` maps each rush command to the feature it needs, so `/rewind` only shows for agents that can. (It was a `Caps` bit set until 2026-09-28.)

### The event model

`headless.Event` becomes `agent/event`, with the Claude-shaped parts taken out:

- `Init{SessionID, Model, Cwd, Mode, Version, Tools, Commands, MCP}`
- `MessageStart`, `Delta{Part, Kind: text|thinking|toolInput}`, `PartStart`
- `Message{Role, ID, Model, Parent, Parts []Part, Tokens *usage.TokenUsage}`
- `Part` is a sum: `Text`, `Thinking`, `ToolCall{ID, Name, Kind tool.Kind, Input tool.Input, Raw json.RawMessage}`, `ToolResult{CallID, Text, IsError, Result tool.Result, Raw}`, `Image`.
- `Approval{ID, Call ToolCall, Reason, Options []ApprovalOption}`. ACP's `session/request_permission` sends the options, and Claude's `can_use_tool` has allow, deny and always-allow; one list of options covers both.
- `Question{ID, Questions}`, split out from Claude's AskUserQuestion, so any agent with structured questions uses the same card.
- `TurnEnd{Reason, Err, Cost, Tokens, Duration}`, `Compacted{…}`, `QuotaUpdate{usage.Quota}`, `TaskStarted/Updated/Done` (with a neutral `TaskKind`: shell, subagent, monitor, workflow), `Plan{[]Todo}`, `Status`, `Other{Adapter, Raw}`.

Tools are normalised once, in the adapter:

```go
package tool
type Kind int  // Shell, Read, Edit, Write, Search, Glob, Fetch, WebSearch, Subagent,
               // Todo, Question, PlanMode, MCP, Notebook, Other
type Input struct {           // the common fields, filled by the adapter
    Command, Path, Pattern, Query, URL, Description string
    Edits []Edit              // old → new, for Edit, MultiEdit, apply_patch, ACP diffs
    Server, Tool string       // MCP
}
type Result struct { Stdout, Stderr string; Exit *int; Patches []Patch; Lines *LineSpan }
```

convo's `render.go` switches on `tool.Kind` rather than on `"Bash"`. The Claude-only extras (`SendMessage`, `ScheduleWakeup`, `EnterWorktree`, `Monitor`) stay as `Kind: Other` with the adapter's name, and get drawn by name the way they are now, so nothing that renders today is lost. ACP already sends a tool `kind` (read, edit, delete, move, search, execute, think, fetch) and diff content, so ACP adapters get this nearly for free.

### The host

`rush host` runs a `Driver` instead of `headless.Session`, and sends **neutral events** to clients (JSON with a `"t"` type tag), not Claude's lines. The replay ring stores the same neutral events. This is the one wire change: it bumps `Proto` to 4. An older host keeps working until its session idles out, because the client still reads Proto ≤3 lines through the Claude adapter's decoder.

The host's own logic stays agent-neutral: queue, idle stop and resume, retries, branches, pending approvals. The Claude-specific pieces (usage-limit parsing, the `continue` after a reset, `ownTraffic` filtering, the checkpoint env) move behind the Claude driver. Wherever the host matches on error text, it now asks the driver: `Classify(err) → {Retryable, Auth, TooLong, Limited(until)}`.

### Discovery

`fleet.Loader.Load` loops over `registry × profiles` and calls `Discoverer.Live` and `Past`, then adds rush-hosted sessions from `host.List()` as it does now. `fleet.Agent` stops embedding `claude.Job` and holds `agent.Session` plus the fleet fields (CPU, memory, pins, group). Each adapter does its own process matching; `isClaudePID` moves into the Claude adapter.

## Usage

### The model

```go
package usage

type Window struct {
    ID       string        // "five_hour", "seven_day", "seven_day_opus", "primary", "month"
    Label    string        // "5h", "week", "Opus week", "premium requests"
    Span     time.Duration // 5h, 7d, 30d, 0 when the provider doesn't say
    Percent  float64       // 0–100, always filled, derived from Used/Limit when those are sent
    Used, Limit float64    // when the provider counts (Copilot: 212 of 300 requests)
    ResetsAt time.Time
    Scope    Scope         // which models it applies to; zero means all
}
type Scope struct{ Models []string } // e.g. Claude's Opus week, or a per-model Codex limit

type Quota struct {
    Account   AccountKey    // "claude:login:<uuid>", "codex:<account id>", "copilot:gh:<login>"
    Plan      string
    Windows   []Window
    Credits   *Money        // prepaid or overage balance, if any
    FetchedAt time.Time
    Source    Source        // live event, rush fetch, agent's own cache
    Problem   string
}

func (q Quota) Tightest(model string) (Window, bool)  // the window that stops this model first
```

- **Windows are a list**, so Claude's 5h and week, Claude's Opus week, Codex's primary and secondary (whose lengths come from `windowDurationMins`, not from position), and Copilot's monthly premium requests all fit without new fields.
- **A quota belongs to an Account, not to an agent or a model.** Sessions point at an Account. When two agents share one account (Claude Code and Copilot both on one GitHub seat, or several Codex profiles on one ChatGPT sign-in), they share one Quota and one line in the Accounts view.
- **Model scope.** A window with a `Scope` only counts for those models. `Tightest(model)` picks the window that decides whether a session can carry on: an Opus session on Claude reads the Opus week and the 5h window, a Sonnet session skips the Opus week. The auto-switch and the top-bar meter both use it, instead of `max(FiveHour, SevenDay)`.
- **Per-request multipliers.** Copilot charges premium requests per model (a multiplier each). `Pricer` returns either a price in dollars or a request multiplier, so "what did this session cost" reads "$3.20" on Claude and "14 premium requests" on Copilot.
- **Tokens.** `TokenUsage{Input, Output, CacheRead, CacheWrite map[ttl]int, Reasoning}`, where each adapter fills in what its provider reports. Cost is always computed by the adapter's `Pricer`, so efficiency stops pricing everything as Claude.

### Where readings come from

There are three sources, as today, but each adapter supplies its own:

| Agent | Live (from the session) | Fetched (by rush) | Agent's own cache |
|---|---|---|---|
| Claude Code | `rate_limit_event` | `api.anthropic.com/api/oauth/usage` | `.claude.json` |
| Codex | `token_count` / rate-limit notifications | `account/rateLimits/read` over `codex app-server` | rollout files |
| Copilot | none known yet | GitHub premium-request usage API | none |
| Kimi / Vibe / API-key agents | per-turn tokens only | provider billing API, if any | none |

The cache in `usagecache.go` (flock'd `usage.json`, backoff) is already keyed by string and takes a fetch function, so it moves to `internal/agent/usage` almost unchanged, keyed by `AccountKey`.

### Switching accounts

`fleet.NextLogin` becomes `usage.Next(accounts, model, stopped)`. It is generic over any adapter whose `Accounts` can switch, and it only switches between accounts of the same adapter. Moving a conversation to another agent is a different feature (see Later). `state.SwitchAt` applies to `Tightest(model)`.

## The refactor, in steps that each build and commit

Each step keeps behaviour identical. The golden tests in `convo` and the frame tests in `ui` are the safety net, and every step runs them.

1. **Neutral types, aliased.** Create `internal/agent` and `internal/agent/usage`, and move the types that are already generic into them: `Todo`, `Task`, `PR`, `SubagentStats`, `Preview`, `Convo`, `Command`, `TokenUsage`, `Totals`, and the usage cache. Leave `type X = agent.X` aliases in `internal/claude` so nothing else changes yet.
2. **Usage as a list.** Replace `claude.Usage{FiveHour, SevenDay}` with `usage.Quota{Windows}`, and add `Tightest`. Update ui, statusline, menubar, `--dump` and logins. Claude fills in `five_hour`, `seven_day` and `seven_day_opus` (the last is parsed today but not shown).
3. **Profile and Account.** Add `agent.Profile{Kind, Dir, Name}` and `agent.Account`, and change `state.Config.Accounts` to profiles with a `kind` (a JSON migration: old entries become `kind: claude`). Replace `claude.Account` in function signatures, and turn `Login` and `Vault` into the Claude adapter's `Accounts`.
4. **fleet.Agent off claude.Job.** Add `agent.Session` with the generic Job fields, and move `CLIVersion`, `RespawnFlags` and `Worker` into an adapter-owned `Extra any`. Move discovery into the Claude adapter's `Discoverer`.
5. **Tools normalised.** Add `tool.Kind` and `tool.Input` and fill them in the Claude decoder. Move `render.go`, `changes.go`, `commits.go`, `messages.go`, `efficiency/scan.go` and `claude.Doing` onto them, one file per commit, with the golden tests unchanged.
6. **Neutral events.** Rename `headless.Event` to `agent/event` with the shapes above. The Claude decoder produces them, and convo, plugind and menubar consume only them.
7. **The host speaks neutral events.** Put a `Driver` interface behind `host`, make `headless.Session` the Claude driver, move the wire to Proto 4, and keep reading Proto ≤3.
8. **Capabilities.** Add `Caps` gates on commands, sheets and tabs (rewind, fork, skills, plugins, hooks, statusline, memory, the Claude settings tab, efficiency savers, logins).
9. **Move.** Move `internal/claude`, `internal/headless` and `internal/daemon` to `internal/adapters/claude/…`, and add a lint check (`go list -deps`) that fails if a core package imports an adapter.

Steps 1–3 are mostly mechanical and quick. Steps 5–7 are the real work.

## Where it stands (2026-09-29)

Codex, Copilot, DeepSeek, Gemini, GLM, Kimi, OpenCode and Vibe run in rush mode alongside Claude Code. An agent shows only once its program is installed (`agent.Installed`, looked for on PATH and where installers put programs, again every few minutes); until then it is registered but absent from every list, picker and meter. Start one with `#with codex` in the Prompt, or `rush session start --agent codex`. Their sessions are listed with their agent's name: rush-hosted ones, past Codex sessions (a message resumes one), and Codex running in a terminal, followed as it goes. Codex has been tried for real, including an approval answered through rush. The ACP agents have only been checked as far as their handshakes, because none was signed in here; DeepSeek and GLM only against recorded fixtures, as neither was installed.

How it's built, which differs from the plan above in places:

- **Adapters** register themselves (`cmd/rush/adapters.go`): `internal/adapters/claude` (claude -p, logins, limits), `codex` (app-server, limits, history, past and running sessions), `copilot` (the CLI over ACP, the coding agent's sessions on GitHub and their logs, premium requests), `deepseek` (DeepSeek's own agent, DeepSeek Harness: `dsh --profile acp`, and the API key's balance), `glm` (Z.ai's ZCode, through `zcode-acp-server`, and the GLM Coding Plan's limits), and `acp` (one client, and the thin agents in `acp/agent.go`).
- **Remote sessions** (Copilot's coding agent) are `agent.Session`s with `Remote` set: listed with their repository and PR, view-only, followed every ten seconds while they work. rush doesn't start or steer them yet: that would mean `gh agent-task create`, or commenting on the PR.
- **The host** runs every agent, Claude Code included, through its adapter's `Driver`, and keeps its books (queueing, idle rest, resume, state, what a session needs, limits and retries) from rush's own events. What only some agents can do is an optional interface on their `Conn` (`agent/conn.go`: answering with changed input, asking the agent itself, context usage, stopping and backgrounding a task, the login check, the limit it keeps). A client says hello with its protocol first (`host/hello.go`, Proto 6): one that reads them gets every session as `rush_ev` lines; an older rush, which says nothing, still gets Claude Code's own lines for a Claude session. `host.Decoder` reads either kind as rush's own events, so today's clients read hosts an older rush started too. Answers to a question go as Claude's AskUserQuestion input (`host.AnswerInput`), which every host takes.
- **convo** is built from rush's own events alone: Claude Code's lines, from a transcript or an older host, go through `headless.Neutral` first. A step keeps its call and how it came out as rush's own (`tool.Call`, `tool.Output`), and its `Tool`, `Input` and `Result` in Claude's words beside them (another agent's call as the Claude tool of its kind, `convo/words.go`) for what still reads those. What a step waits on you for is rush's own too (`convo.Asking`: an approval, or an `event.Question`). Each step also has a `Kind`, and the renderer, the changes view, the overview, search, git cards, jobs and running chains switch on it and read the call's `tool.Input` (`st.in()`) rather than Claude's input keys; a Claude session draws byte for byte as before. Only Claude-only tools (SendMessage, Monitor, Skill, Show, Artifact, ToolSearch) still read their input by key, as they're drawn by name. The efficiency scan counts shell commands and reads by kind and `tool.Input` the same way.
- **Usage**: meters, account switching and the menu bar read `usage.Quota`, whose windows are named by their own labels. Other agents' limits are read through their adapter into `quotas.json` (`usage.Refresh`), and live sessions record theirs there too. Accounts shows every agent's accounts.
- **Commands**: a session offers only the commands its agent can do (`ui/caps.go`).

**Accounts** (step 3, as built, 2026-09-27). There are no config folders to manage any more: every agent runs from its own home (`~/.claude`, `~/.codex`, `~/.copilot`…), and an account is a sign-in rush puts in that home when you switch.

- **Installed agents only.** Every adapter stays registered, but an agent shows (in Accounts, `#with`, the default agent and the switching order) only when its program is found, on `PATH` or where its installer puts it (`agent.Programmer`, `agent.Installed`). The look is kept three minutes and redone when Accounts opens, so an agent installed while rush runs appears.
- **Accounts per agent.** `agent.Accounts` is `Current`, `Switch`, `SignIn` (a command to run in the terminal, then `done` to keep what it signed in to) and `Forget`. Which accounts there are and what they're called is rush's (`state.Config.Logins` for Claude, `SignIns` for the rest); credentials are in the vault (the keychain on macOS). Claude swaps its OAuth sign-in into `~/.claude` as before. Codex swaps `auth.json` in `~/.codex`, keeping the one there first. Copilot's accounts are gh's GitHub accounts: rush records which one Copilot runs on (`Config.Using`) and gives its sessions that account's token, leaving gh's active account alone (`agent.Known`, `agent.AnyAccountQuota`: every Copilot account's premium requests are read). ACP agents sign in through their own program, one account each.
- **Default agent and switching.** Profiles took these over (Track B, below). `Dispatch.Kind`, `Config.SwitchOnLimit`, `Config.AgentOrder` and the older `stayOnAccount` are still written from the default profile, for older rushes.
- **Folders migrated.** The older `~/.claude-*` folders (`Config.Folders`, still `accounts` in the JSON so older rushes read it) had their sign-ins taken in as Claude accounts once (`FoldersImported`); the config before is kept as `config.json.before-accounts`. Their past sessions are still listed and resume there.
- A pay-as-you-go account's money left is `usage.Quota.Balance` (DeepSeek's).

Still to do (2026-09-29, after Claude moved under its adapter):

1. **Logins for other agents.** `state.Config.Logins` and `Folders` are the logins agent's (`state.LoginsKind`, Claude Code), kept through its `state.LoginKeeper` and read through its `agent.PlanReader`. Other agents' sign-ins are `SignIns` through `agent.Accounts`; one list for both is the providers work (Track B).
2. **The spawn CLI.** `cmd/rush/spawn.go` still prints a spawned session's output as Claude Code's stream-json (`headless.Result`, `headless.PermissionRequest`), marked `// migration:`. `cmd/rush` is a registration point, so the lint allows it; reading `event`s instead would let it drop them.
3. **Rewind** (`ui/rewindsheet.go`) is Claude Code's own, gated on `FeatureRewind`, which only Claude declares. It stays so until a second agent can rewind.
4. **Efficiency and the advisor** read Claude transcripts, settings and memory, gated on `FeatureEfficiency`. The savers' install recipes run `claude plugin install` and `claude mcp add` (`efficiency/catalog.go`, the one file `named_test.go` still allows).
5. **Transcripts are Claude Code's words.** rush sessions write Claude Code's transcripts, so the scanner that prices them (`agent.SpendReader`) and the native reader convo and the host use (`agent.Native`) are the logins agent's and the native agent's. A second native agent would need its own.
6. Limits for Kimi and Vibe; prices for Codex and the ACP agents; efficiency for other agents; Codex's AGENTS.md in the memory view and its `config.toml` on its Settings page (both declared Planned); starting and steering Copilot's remote sessions; fleet finding Ollama's sessions, kept outside `~/.claude`.

DeepSeek and GLM, as found (2026-09-27): DeepSeek's own agent is DeepSeek Harness (`npm i -g @deepseek-ai/dsh`, a developer preview), whose ACP server is its `acp` profile. It resumes sessions without replaying them and has no `session/load`, so a resumed one shows only from where it picks up; its session logs (`~/.dsh/sessions`) are zstd-compressed and their format still moves, so rush doesn't read them. It keeps its key in `~/.dsh/.credentials.yaml` (or `DEEPSEEK_API_KEY`), which rush uses only for DeepSeek's free `/user/balance`, shown as the account's balance since it's money rather than a window. Z.ai's own agent is ZCode (open-sourced 2026-09-20; the desktop app, or its installer's `~/.local/bin/zcode`). It speaks its own protocol (`zcode app-server --stdio`) and has a headless `--output-format stream-json`, but no ACP, so rush runs it through the community bridge `zcode-acp-server` (`npm i -g zcode-acp`), which drives that same ZCode. A native adapter over ZCode's app-server would drop the bridge; its protocol is large and new. DeepSeek-TUI (now CodeWhale) and glm-acp-agent are community agents rush doesn't run.

Codex, as found: its `approvals_reviewer = "auto_review"` setting answers approvals itself, and rush leaves that as you set it. Its `primary` window can be the weekly one. It runs every command in `/bin/zsh -lc`, which rush unwraps.

Ollama (2026-09-28): `internal/adapters/ollama` runs Claude Code itself against Ollama's Anthropic-compatible API, in a config folder of its own (`<state dir>/ollama`), and tunes each session for the model (a smaller prompt, fewer tools; `tune.go`). It is an `agent.Rider` on Claude (`Rides() = claude`): it needs `claude` installed beside `ollama`, and `ollama` in a shell isn't taken for an agent spawn. Its level is tested. Every provider acts as a profile of its own, so `#profile ollama` in the Prompt runs the next session on Ollama while Claude stays the default. Effort, subagents and quota are declared No; fleet doesn't find its sessions yet, and a model changed mid-session isn't re-tuned.

## Features, profiles and providers (planned 2026-09-28)

The core isn't isolated yet. Claude Code is the implicit default: `canRun` lets Claude do everything and checks only other agents, `Caps` is sixteen coarse flags checked in two places, and there are 37 `"claude"` comparisons outside the adapters and about 300 `claude.X` references across 13 packages. What follows is the target, built in two parallel tracks.

### Track A: every rush feature is opted into

- **`agent.Feature`** names every rush action a session or provider might offer: resume, fork, rewind, images, effort, modes, plan, subagents, background tasks, questions, context breakdown, compact, MCP, hooks, plugins, statusline, screen, cd/add-dir, side questions (btw), mid-session model switch, interrupt, slash commands and skills, live discovery, past history, remote sessions, account switching, sign-in, quota, pricing, efficiency, memory report, and hand-off in (starting from another agent's transcript). Where a feature has an optional interface (`Driver`, `HistoryReader`, `QuotaSource`, `Accounts`, `Pricer`, `Commander`…), the interface stays the way it's implemented.
- **Adapters declare each one**: `Features() map[Feature]Support`, where `Support` is `Yes`, `No` or `Planned`, with an optional note ("needs a paid plan", "view only"). A feature an adapter doesn't list is `No`. A test checks that a feature declared `Yes` has its interface implemented, and the other way round.
- **One gate**: `agent.Supports(kind, feature)`. The core never compares a kind to `"claude"`. An empty kind (older state, drafts and host info) is made `claude` once, where it's read. `Caps` goes.
- **Claude is an adapter like the rest.** `canRun`, `otherAgent`, host `other()`, fleet's `otherAgent` and the process-name checks go through the registry or the adapter (`Programmer`, `Discoverer`). A test fails on any `"claude"` literal outside `internal/adapters/claude` and marked migrations.
- Then step 4 (`fleet.Agent` off `claude.Job`), then `claude.Account` → `agent.Profile` in host and fleet, and then retiring `FromNeutral`.
- **Hand-off**: `agent.Handoff(from Session) Input` renders a conversation (its first message, a summary of what was done, recent turns and open todos) as the opening prompt of a new session on another provider. The UI and profiles call it.

Track A as built:

- **Features** (2026-09-28). `internal/agent/feature.go` has `Feature`, `Support{Is, Note}` (`Yes`, `No`, `Planned`, and `.With(note)`), `Level` (`LevelFull`, `LevelTested`, `LevelPreview`), `AllFeatures()` in display order with labels, `Features(kind)`, `FeatureOf`, `Supports` and `LevelOf`. Every adapter declares its features and level: Claude is full, Codex and Copilot tested, the rest preview. `internal/adapters/features_test.go` checks each feature that is an interface against the adapter. `Caps` is gone: rush's commands are gated on features, and `/btw`, `/cd` and `/add-dir` on theirs rather than on being Claude's.
- **No agent by name** (2026-09-28, finished 2026-09-29). The core compares no kind to `"claude"`, and there is no built-in agent: `agent.Builtin`, `IsBuiltin` and `BuiltinKind` are gone. An empty kind in older state is Claude Code, read in once where it's read (`agent.Migrated`, and `agent.LegacyKind` where a kind is defaulted), marked `// migration:`. `canRun` gates Claude's commands through the registry like any other agent's. Fleet matches processes by each adapter's `Program`. `internal/agent/named_test.go` fails on a new `"claude"` literal outside Claude's own packages (everything under `internal/adapters/claude`) and lines marked as a migration; its `namedYet` list is down to the savers' install recipes (`efficiency/catalog.go`).
- **Hand-off** (2026-09-28). `agent.Handoff(agent.Conversation) agent.Input` renders a conversation (where it ran, its first message, its calls as `tool.Call`s put in words by `tool.Doing`, the files it changed, its last turns and its open todos, each cut to a size) as the first message of a new session. A convo step gives its call with `Step.Call()`, which reads Claude Code's words back out until `FromNeutral` goes. `convo.Session.Conversation(kind)` tells any session that way, and `ui.(*Model).conversationOf(*fleet.Agent)` reads one for a list row. `/handoff <agent>` in a session starts that agent on it in a new session, gated on `FeatureHandoffIn`; the one handed on stays as it is.
- **Rows off Claude's job file** (2026-09-28, the first half of step 4). `fleet.Agent` embeds `agent.Job`, the row's fields whichever agent runs it; `claude.Job` is that plus what Claude Code needs to start a job again (`CLIVersion`, `RespawnFlags`), kept in the row's `Extra` for relaunching.
- **Claude found by its adapter** (2026-09-29, the second half of step 4). The Claude adapter is a `Discoverer`: `Live` lists its background jobs (`Extra` is the `claude.Job`) and then every session file whose process is alive (`Extra` is the `claude.Session`), and `Past` every transcript in the projects folder (`Extra` is the `claude.Convo`), each read again only when its file changes. It is a `HistoryReader` too, reading a transcript back as rush's events through `headless.Neutral`. Fleet takes Claude's sessions from it and still adds what only a Claude row has (the daemon's workers, pins, PRs, subagents, processes). The features test has no exceptions left.
- **Every feature gates something** (2026-09-29). `internal/adapters/features_test.go`'s `TestEveryFeatureGated` walks `AllFeatures` and fails unless each is checked somewhere outside the adapters (`agent.FeatureX` in the code), or is listed as backed by an interface (`implements`) or by events (`eventBacked`: Questions, Remote). Each screen checks its own features (`ui/caps.go`: `screenNeeds`, `canScreen`), so `/context` opens for Codex, which declares Context, and the info sheet's tabs, Settings links and memory view each check theirs. Two features were added: `FeatureSettings` (settings files) and `FeatureStats` (usage history).
- **Claude's commands in its adapter** (2026-09-29). `internal/actions` keeps only what isn't an agent's (process trees, worktrees, notifications). Starting, stopping, removing, replying to, attaching to, signing in, opening the screen of and moving a job between accounts are `agent.Dispatcher`, `Stopper`, `Remover`, `Replier`, `Attacher`, `Loginer`, `Screener` and `Mover` (`agent/actions.go`), found with `agent.As[T](kind)`; the Claude adapter implements them (`adapters/claude/actions.go`). The advisor finds its program with `agent.Path`.
- **Claude's screens behind interfaces** (2026-09-29). Plugins (`agent.Plugger`, with `agent.Plugin` and `PluginChange`), skills (`Commander`), permissions and hooks (`SettingsFiler`, edited through `internal/settingsfile`), the status line (`StatusLiner`), an agent's own Settings page (`SettingsPager` for its settings file and env block, `Definer` for agent definitions), the memory view (`MemoryReader`, neutral `agent.Memory` files, groups and problems, so Codex's AGENTS.md can fill it), forks (`Brancher` copies part of a conversation or into a worktree; the agent's own `Choices` fill the sheet) and the History tab (`StatsReader`). A screen whose agent has no implementation stays hidden, and Codex declares Memory and Settings Planned.
- **Neutral rows and readings** (2026-09-29). A row's PRs, subagents, token use and halt are `agent.PR`, `agent.SubagentStats`, `usage.TokenUsage` and `agent.Halt`. Efficiency prices cache reads with `agent.Price` (the adapter's `Pricer`, falling back to its `DefaultModeler`'s model). The status line reads its plan limits as a `usage.Quota` through `LastQuotaReader`. Model names (`ModelNamer`, `agent.ModelName`), context windows (`ContextWindower`, `agent.ContextWindow`) and a smaller window it compacts in (`Compacter`, `agent.CompactionOf`, shown through `agent.Fill`) come from the session's agent. A turn cut short names the program that died, and search's filter for what the agent said is `is:agent` (`is:claude` kept as a migration).

- **Claude is only an adapter** (2026-09-29). `internal/claude`, `internal/headless` and `internal/daemon` are `internal/adapters/claude/claude`, `headless` and `daemon`, and nothing in the core imports them. The core reaches Claude Code through interfaces in `internal/agent`, each implemented in `internal/adapters/claude`: its transcripts' words (`Native`, `Neutral`), a session's cwd (`CwdReader`), subagent runs (`SubagentRuns`, `RunFollower`), its screen and daemon (`Joiner`, `Terminal`, `Mirror`), pins and the native view (`Pinner`, `SessionsViewer`), its home (`Homer`), the status line's folder and sign-in (`StatusLineProfiler`, `SignInReader`), plans (`PlanReader`, read as `usage.Reading`), logins (`state.LoginKeeper`, with `state.FoundLogin` and `state.Restored`), what a job has beyond its record (`JobKeeper`: workers, pins, linked PRs, watched paths), transcripts' whereabouts (`Transcripts`), scratch folders (`Scratcher`) and spend (`SpendReader`, pricing into `agent.Spend`). A live `agent.Session` says whether it's a job (`Job`), which job it runs (`JobID`), whether it's interactive and what its process is doing (`Status`, `StatusAt`). The keychain is `photon/keychain`; the vault and `state.Login` stay in state. rush-lint's `rushadapters` keeps it so.

### Track B: providers and profiles

- **Provider family always visible.** Every session row, the session header and the top bar show the provider with its own glyph and colour (Claude, Codex, Copilot, DeepSeek, GLM, Kimi, Vibe, Gemini, OpenCode), along with the account and the profile in use.
- **Profiles** (`state.Config.Profiles`) are named lists of providers plus a policy:
  - `providers`: ordered provider kinds. Accounts within each provider rotate as they do now.
  - `mix`: `stay` (only the first installed provider; when all its accounts are out, wait) or `mix` (new sessions go to the next provider once every account of the current one is out).
  - `onLimit`, for a *running* conversation that hits a usage limit: `wait` (continue at the reset, as `Dispatch.OnLimit` does now), `account` (another account of the same provider; the conversation carries on), or `handoff` (another account first, then hand it to the next provider in the list through `agent.Handoff`). Otherwise a running conversation stays where it is.
- **Which profile a session gets**: the one picked for it (the start picker, `#profile <name>` in the Prompt, `rush session start --profile`), else the longest matching folder rule (`Config.FolderRules`: a path prefix, `~` expanded, to a profile), else `Config.DefaultProfile`. `Config.ProfileFor(cwd, explicit)` is the one resolver, and `Profile.Pick(quotas)` gives the provider and account to start on.
- **Migration**: `Dispatch.Kind`, `SwitchOnLimit` and `AgentOrder` become a profile called "Default", made once. The old fields are still written for older rushes.
- **Providers page** (the Accounts page reworked): installed providers, each with its accounts and limits and its support level (`full`, `tested`, or `preview`: built but not tried against the real CLI), plus the feature matrix from Track A (✓, –, planned).
- **Profiles page**: create, rename, delete and reorder providers, set the policy, and manage folder rules (add one from the selected session's folder). A key switches the default profile or provider from anywhere, with a picker.

Track B as built (2026-09-29):

- **Profiles** are in `state/profiles.go`: `Profile{Name, Providers, Mix, OnLimit}`, `Config.Profiles`, `DefaultProfile` and `FolderRules`. `ProfileFor(cwd, explicit)` resolves; `Profile.Pick(Room)` gives the provider and account to start on, `Next` the provider to hand on to, and `PickFor(kind, Room)` an account of one provider or none (the advisor's gate). A `Room` is each provider's accounts, which is in use and which are nearly out, built by the UI from the readings it keeps (`ui.room()`); only providers whose program runs here count. The Default profile is made once from the old fields, and `SyncLegacy` writes them back from it.
- **Starting.** The Prompt, `#with`, `#profile <name>` (for the next session only), `rush session start --profile` and hand-offs all go through `ProfileFor`. The profile's name is kept on `host.Config` and `host.Info`, and so on `fleet.Agent`, so a resumed session keeps it.
- **At a limit.** `wait` leaves a stopped session alone (no account switch, no continue after one); `account` switches as before; `handoff` hands it, once, to the next provider in its profile with room that declares `FeatureHandoffIn`, through `agent.Handoff`, in the same folder and profile.
- **Seen everywhere.** Each provider has a glyph and colour (`ui/providers.go`). The top bar says the provider, account and profile new sessions start on; a session's header says what it runs on; rows of sessions on other providers carry the provider's glyph. Accounts and Agents show each provider's support level and feature grid.
- **Settings › Profiles** creates, renames and deletes profiles, orders their providers, sets `mix` and `onLimit`, and adds folder rules (from the selected session's folder). `ctrl+]` then `w` (or `alt+w`) switches the default profile, or which provider is first in it, from anywhere.

## Adding the agents

Each agent is its own milestone: it appears in the list, runs in rush mode, draws its history, and shows its usage.

1. **ACP client** (`adapters/acp`). JSON-RPC over stdio: `initialize`, `session/new`, `session/load`, `session/prompt`, `session/update` notifications (agent message chunks, thought chunks, tool calls with kind and diffs, plan entries), `session/request_permission`, `session/cancel`, `session/set_mode`, and the client-side `fs/*` and `terminal/*` methods. It maps all of this onto `agent/event`. Every ACP agent gets a Driver from this one package.
2. **Codex.** Use the native `codex app-server` (JSON-RPC) rather than ACP. It exposes rate limits, reasoning effort, approvals and thread resume directly, and the usage story needs the rate limits. The adapter also covers history from `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, profiles from `CODEX_HOME`, and discovery by process name `codex`.
3. **Copilot CLI.** A Driver through ACP (`copilot --acp`), profiles from `~/.copilot` (or `COPILOT_HOME`), history from its session-state files, and a quota of monthly premium requests from GitHub's API, with a model multiplier in `Pricer`.
4. **Kimi CLI** (`kimi acp`) and **Mistral Vibe** (`vibe-acp`). Thin adapters on top of `acp`, plus history readers for their session files, and quota from their billing APIs where one exists. Otherwise the adapter reports token spend only, with no windows.
5. **Gemini CLI, OpenCode, Goose.** The same ACP pattern, added when someone wants them.

For each one, before writing its history reader and quota source, check its on-disk session format and usage endpoint against the current release, since they change often.

### What an ACP-only agent can't do (yet)

Rewind, fork, file checkpoints, context-usage breakdowns, background task control, and the native "screen" tab have no ACP equivalent. Those features stay capability-gated and hidden for these agents rather than faked.

## Later

- **Hand a conversation to another agent** ("carry on in Codex"): render the transcript to a prompt and start a new session. This only makes sense once two agents run.
- **Plugins across agents.** `plugin.Contributions.Flags` emits Claude flags today. Tools already go through MCP, which ACP agents accept (`mcpServers` in `session/new`), so plugin tools can travel. Subagent and system-prompt contributions stay Claude-only.
- **`rush on`** for other agents' commands (`codex`, `copilot`), if they grow a view worth replacing.
