# Emeritus: an honourable handover

Status: built 2026-11-09 on `rush/emeritus`, finished on
`rush/emeritus-finish` (its settings, the retired session in the UI, the
nudge and baton drawn as rush's own, a cap on handovers in a row, the
tool reaching running agents), off until `emeritus.on` is set. Designed
2026-10-02; two adversarial reviews and three lab tests shaped it (the
third at ~300k context, which changed the trigger, its timing and the
baton); results at the bottom. The Q&A fork is not built: the lab found
it never used and dearer than a whole successor.

An agent near the end of its context writes a baton, rush starts a fresh
agent on it, and the old one retires for good. It isn't a phoenix: the old
agent isn't burnt, it becomes emeritus. Its transcript stays, and an opt-in,
isolated fork of it could answer a question, at a measured price.

```
 A: context ≥ min(300k, 70% of window), measured by the host,
    at a turn's end, prompt cache still warm
   │ nudge (advisory, once) ─ A writes the baton: judgement only
   │ mcp__rush__retire {baton}  → "retiring after this turn"
   ▼ turn ends
 A's host, journalled in handover.json:
   1. freeze A: refuse what you type, hold other agents' messages;
      refuse while A's background tasks run
   2. append facts: branch, HEAD, git status, diffstat, transcript path, cost
   3. start B (A's folder, kind, model, effort, mode, account, parent),
      wait for its first request to come back
   4. B started? commit: A retired (terminal), children, reports, reminders
      and waiting messages → B, handover logged for Efficiency
      B failed? roll back: B retired, unfreeze A, tell it why     │
                                                                   ▼
 A stopped for good; baton + transcript on disk          B owns the task
```

## The trigger

One threshold for main agents and subagents alike: hand over at
`emeritus.tokens` (300k) of context, or `emeritus.percent` (70%) of the
model's window if that's smaller. The host measures it from what its
agent's last request sent (`Info.contextTokens`), against the window the
adapter knows for the model. 70% alone was wrong for 1M windows: at 700k
each call's re-read of the context has long dominated.

The nudge and the handover happen at a turn's end, while A's prompt cache
is warm (`Info.cacheWarm`, kept from each request's cache write and TTL, as
cachekeep.rs reads them). A cold baton turn writes A's whole context back
into the cache: $2.24 at 280k on the 1-hour rate, which made the lab's
baton dearer than `/compact`. So an idle agent whose cache has lapsed is
never nudged.

| Session | Nudged when |
|---|---|
| One you started | At a turn's end that ended `done`, with nothing queued, no decisions card, no permission waiting, no background task, no limit or retry |
| A subagent rush hosts (spawn_agent), or one started beside (start_agent) | When another agent's message brings new work while it's idle and warm: that message waits behind the nudge. Not when its task is done: it may get no more work, and its answer has gone to its parent. This is the lab's "task boundary, before new work arrived" |

Once nudged, it isn't nudged again until its context has grown a quarter of
the threshold more: a nudge it turned down (its task nearly done) isn't
asked turn after turn. The nudge is advisory; the agent may say so and carry
on.

## The baton

`<session dir>/baton.md`. A writes the judgement, rush appends the facts.

| Section | Holds | Who |
|---|---|---|
| Destination | The task's goal, in a checkable form. The whole goal, not A's last instruction | A |
| Core ideas | The 3 to 5 things that matter most | A |
| Methods | How the work is done: commands, the loop | A |
| Journey | What failed or surprised, and why | A |
| Limits | ✅ good enough / ❌ never acceptable. Durable rules only, not "don't do X yet" | A |
| Tests that pin text | The tests that assert literal strings or snapshots of what A changed (prompts, rows, help text), by file and name | A |
| Key files | A short map: the files and functions that matter, a line each | A |
| Next action | One line | A |
| State | Folder, branch, HEAD, `git status --short`, `git diff --stat HEAD`, transcript path, cost and context | rush |

The template says plainly: scope limits the user gave A for one turn ("phase
1 only", "don't start X") are not limits for B. Test 2 broke on this.
"Tests that pin text" is from the 300k lab: the original and the compacted
arm both broke such a test. "Key files" is there to cut B's re-orientation
(B made 55 calls to the baseline's 34). The retire op refuses a baton that
lacks a section.

B's first message says whose task it takes over, gives the baton, points at
A's transcript (to search, never to read whole), and carries the other
agents' messages that were waiting for A, by sender.

## Rules

| Rule | Why |
|---|---|
| `retire` is a request; it runs after A's turn ends | A can only call a tool during a turn, and retiring needs A idle |
| Handover is one journalled transaction: freeze, append, start B, commit or roll back. The commit point is A's config saying it's retired; a host cut short is settled at A's next start (`emeritus::recover`) | B can fail to start (auth, limits) after A is gone; a crash mid-way must not leave two owners or none |
| Retired is terminal, enforced at `client::spawn` (whatever config it's given, the one on disk is read), `ensure`, the host's start (`run_with`), and op admission in a running host, not only one of them | Every one of those can wake a saved host: a resume, a restart, a send, a revive |
| A rolled-back B is retired too (`emeritus_void`), with why | Two owners otherwise |
| A's children (spawnedBy) and siblings (startedFrom) move to B, with the answers owed to A (report files) and A's reminders; A's report to its parent becomes B's | Background results would otherwise wake A, or land nowhere B can see |
| A's parent is told B took over: a wait on A (spawn_agent, agent_send, agent_result) follows the chain of successors and says so; a child's answer to A goes to the live successor | The parent asked A, and B answers |
| B inherits A's folder, workspace, kind, model, effort, permission mode, account and parent | The CLI start path uses the profile's defaults |
| Nothing is silently redirected: what you type to A while it hands over, or after, is refused with B's id. Only other agents' messages move, named by sender. A message you queued during A's last turn calls the retire off | Typing into A must not quietly instruct someone else |
| Design call: typing during the retire turn calls the retire off, rather than holding your message for B or asking which you meant | You wrote to A, about A's work as A knows it; carrying it to B would be the silent redirect above, and asking would leave A frozen on a question. The note the agent gets ("your retire was called off because the user wrote to you") is drawn as rush's line above your words |
| A's background tasks: the retire is called off (A is told to wait for them and ask again) | They die with A's process or report to A's conversation; they can't move |
| The check is host-owned and deduplicated (the nudge kept in A's config); the nudge is advisory | Plugin `turn.ended` comes from what a TUI window saw: absent headless, doubled across windows |
| `lineage` counts the chain (B's meta `predecessor` and `lineage`) | Bounds chains, and lets a later reader follow them |
| A session `emeritus.depth` (3) handovers down its chain isn't nudged: past its threshold it's marked capped once (config `cappedAt`, info `capped`, event `emeritus-capped`), its conversation's card and its list row say so, and it carries on as any session does. The agent isn't told: that would cost a turn to say nothing it can act on. `retire` itself isn't refused at the cap: you may still ask it to hand over | Each baton keeps most of what A knew, not all (the lab's successors came within one hidden check), so a long chain drifts |
| An agent whose process started while Emeritus was off isn't nudged (`Em::unlisted`, set at each start): its tools lack `retire`. It gets the tool at its next start (a wake, a resume, a restart onto a new build), when the tool list is read again | A running agent's tool list is never changed mid-life: the tools are the start of the cached prompt, so adding one writes its whole context to the cache again, the cost Emeritus exists to avoid. Nudging it to call a tool it doesn't have would waste a turn |
| Every harness rush hosts, not Claude only | The handover itself needs only a rush host; only the Q&A fork needed Claude, and it isn't built |

## Settings

In rush's config.json:

| Key | Default | Means |
|---|---|---|
| `emeritus.on` | false | Nudge and offer the `retire` tool. Off until set |
| `emeritus.tokens` | 300000 (0 is the default) | The context, in tokens, past which a session hands over |
| `emeritus.percent` | 70 (0 is the default) | The share of the window past which it hands over, when that's smaller |
| `emeritus.depth` | 3 (0 is the default) | The most handovers in a row: a session that many down its chain isn't nudged again |

They're on Settings › Usage › Emeritus, after the context guard: the
switch, and once on, "past" (tokens), "or past" (share of the window) and
"handovers in a row". The help says what a handover costs (the baton's
turn, ~$0.16 warm; $2.33 cold, so never asked cold) and what the lab saw
it save ($1.94 against $2.85 at ~300k, 10 of 11 hidden checks to 11).

The tool list is read as an agent starts. Turning Emeritus on reaches a
running agent at its next start (woken, resumed, or moved onto a new
build), not mid-life: its tools are never changed while it runs (see
Rules), and until then it isn't nudged. The section's note and the
switch's help say so.

## In the UI

| Where | What |
|---|---|
| A's conversation | The nudge is rush's line (`↻ rush  context at 312k, past 300k · asked to hand over (Emeritus)`), not your message. The retire call is a row, `↻ handing over to a fresh agent  <the baton's Destination>`; opened, the baton is drawn as the Markdown it was written in (as the latest step, a line saying how long it is). A handover rolled back and a retire waiting on tasks are rush's lines too; a retire you called off by writing is rush's line above your words. `rush_convo::emeritus` holds these words; the host writes with them |
| A's box | Read-only: `retired · handed over to <B> · enter opens it`, typing refused with a flash naming B. Its header says `retired · its successor carries on` |
| B's conversation | Its card says `by rush, as a handover` and `↻ took over from "<A>" as it retired, on its baton · alt+o opens it`, then, at the cap, `no more handovers: 3 in a row is the most Settings › Usage › Emeritus allows`. Its first turn is rush's line `took over from "<A>" (rush session <id>) as it retired (Emeritus)`, the baton folded under it (`its baton · 16 lines ▸ show`) |
| The list | A session retired more than ten minutes ago folds into the row at the end of its chain of successors, its cost counted there; one just retired stays a row (`retired · handed over to its successor`) for that while, and one open as a session of its own stays a row while it's open. B's row shows its handovers, `↻n`, yellow once capped |

## Subagents

| Kind | Handover |
|---|---|
| rush's own (spawn_agent, start_agent; any harness): a rush-hosted session | Yes, the whole of it: same trigger, nudged when new work comes, same transaction. B takes A's place under the parent (its spawnedBy and spawnStep), the parent's wait follows to B, A's report goes to B |
| Claude Code's in-process subagents (the Agent tool, `--agents`) | Out of reach. They run inside Claude Code's process as a tool call of the parent's turn: rush reads their transcripts (subruns.rs), keeps their caches warm (subkeep.rs) and can leave one a message at its next tool call (the inbox), but can't stop one, start another in its place, or make the parent's Agent call return the successor's answer. A nudge would only cost a baton nobody could act on, so none is sent |
| Codex's own subagents (its `spawn_agent` tool), Kimi's children | Out of reach for the same reason: the harness owns their lifecycle, rush only reads them |

What rush can do for those is the parent's own handover: the parent is a
rush session, and its context, not its subagents', is what grows past the
threshold.

## Measurement

Each commit is logged to the Efficiency place's event log
(`efficiency/events.jsonl`, kind `handover`), with A's and B's rush
sessions (`from`, `to`), their agents' conversations (`fromSession`,
`toSession`) and the folder (`cwd`), so both transcripts are found
exactly, the agent, model, A's context and cost at handover, B's context at its first
request, the threshold that nudged it, the lineage, and whether A was a
subagent. That's what lets the Efficiency place later say what Emeritus
saved; it doesn't say it yet.

## Asking the predecessor: off by default, not built

A fork costs a cold read of A's whole context: removing the tools changes
the cached prefix. In the lab test one answer on a 38k context cost $0.47,
three times what B spent finishing the task. No successor in any test used
it. It isn't built, so `ask_predecessor` is never offered. If it is built,
each question is to be:

```
claude -p --resume <A> --fork-session --tools "" --strict-mcp-config \
  --no-session-persistence --disable-slash-commands   (+ summary.rs's isolation)
```

with A's model and account, a spend and time cap, one at a time per A, and
"unavailable" when A's context plus the question won't fit (never a silent
summary of the memory being asked), offered only to B and only about its
direct predecessor.

## How it's built

| Part | Where |
|---|---|
| The settings | `rush_state::Emeritus` (config.rs) |
| The words, the facts, B's config, the journal, recovery, the terminal check, moving children | `rush_host::emeritus` |
| The nudge, the retire op, the transaction | `rush_host` server/emeritus.rs, called from `on_turn_end` and `send_exchange` |
| The `retire` tool | `rush_bundled::agtools::retire`, served by `SubAgents` (in process for Claude, `rush mcp-tools` for the rest) |
| Its words in the conversation, read back | `rush_convo::emeritus`; drawn by render/draw.rs (`rush_ask`, `baton_under`) and draw/agtools.rs (the retire row and `retire_body`) |
| The settings rows | `rush_ui_settings::emeritus` |
| The list's folding | `rush_fleet` emeritus.rs (`fold_retired`), from the host info's `emeritus`/`successor` and meta `predecessor`/`lineage`, as `Agent::lineage` |
| The box, the keys, B's card | `rush_ui_session_core::emeritus`, conversation.rs (`Options::predecessor`), render/opening.rs |
| Following a successor | `subtool::wait_for`, `subtool::send_result` |
| The Efficiency event | `rush_efficiency::events::add_handover` |

Its events, in the session's events.jsonl: `emeritus-nudged`,
`emeritus-retire-asked`, `emeritus-called-off`, `emeritus-handover`,
`emeritus-committed`, `emeritus-rolled-back`, `emeritus-voided`,
`emeritus-adopted`, `emeritus-refused`, `emeritus-recovered`,
`emeritus-capped`.

## Left out

- The Q&A fork (below): never used in the lab, and one answer cost more
  than a whole successor.
- Holding what you type during the retire turn for B: see the design
  call in Rules.
- A header segment for the cap: it's said on B's card and its list row
  instead; /statusline has no Emeritus segment.

## Against what exists

Compaction keeps decisions and corrections (`convo/compactby.go`), and the
usage-limit `handoff` is bounded too. The baton's case is that A chooses
what matters while it still knows, and B starts with a clean context in a
separate session. That has to be measured against compaction, not assumed:
tests 2 and 3 do.

## Reviews

| Round | Reviewer | Changed |
|---|---|---|
| 1 | Claude Opus | retired made terminal; Q&A moved from the live A to a tool-less fork; git state appended by rush; refuse while busy; B as sibling with A's mode; lineage |
| 2 | Codex (astra) | `retire` deferred to turn end; handover as a transaction with rollback; terminal flag at every entry, not just `Ensure`; children and reports move to B; fork isolation beyond `--tools ""`; context-fit check; Q&A off by default; host-owned trigger; no silent input redirect; compaction comparison corrected |
| Lab 3 | 300k fork | one absolute threshold for main agents and subagents; nudge and handover only while A's cache is warm; baton gains "Tests that pin text" and "Key files"; each handover logged for Efficiency |

## Lab tests

Fake work in `emeritus-lab`, one worktree per run, driven only through
`rush session start/send/info/stop`. Hidden judge tests the agents never saw.

| Run | Model | Result | Cost |
|---|---|---|---|
| 1 · A: ledger phase 1 | Sonnet 5.5 | ✅ | $0.19 |
| 1 · baton | Sonnet 5.5 | ✅ concrete; flagged the CLI-path and exit-code traps | $0.22 |
| 1 · B from baton | Sonnet 5.5 | ✅ 4/4 hidden checks, 0 questions | $0.15 |
| 1 · control, no baton | Sonnet 5.5 | ⚠️ works, but put the CLI under `ledger/` (judge: 1/4 at the spec's path) | $0.16 |
| 1 · one fork answer | Sonnet 5.5 | ✅ useful | **$0.47** |
| 2 · A: limiter phase 1 | Opus 5.5 | ✅ | $0.30 |
| 2 · baton v1 | Opus 5.5 | ❌ handed down "never start Phase 2 unprompted" | $0.36 |
| 2 · B from baton v1 | Opus 5.5 | ❌ obeyed it and stopped; hidden test can't build | $0.19 |
| 2 · baton v2 (fixed template) | Opus 5.5 | ✅ whole goal, durable limits only | $0.44 |
| 2 · B from baton v2 | Opus 5.5 | ✅ hidden test green under -race, 0 questions | $0.26 |
| 2 · control: A after `/compact` | Opus 5.5 | ✅ hidden test green | $0.80 (compact $0.53) |

### Test 3: a real session at ~300k (2026-11-09)

A rush bug-fix subagent (Opus 5.5, effort medium) forked at transcript line
1047, at 307k context, a task boundary: it had just finished `fix/scratch`,
and the next instruction was new work (let the cleanup agent run read-only
commands and write its own $TMPDIR without asking, with session-scoped
rules, on a new branch). Ground truth is what the original did next. Every
arm got the same instruction; none could push, build or run CI (rush builds
only on CI), so each committed and reported. Eleven hidden checks, judged
from the diffs by me and by a blind Sonnet judge on shuffled patches; they
agreed on every check. Costs are priced from each transcript's usage at
claude-opus-5-5's rates ($4/M input, $20/M output, $0.20/M cache read, $5/M
5-minute write, $8/M 1-hour write), which match the CLI's `total_cost_usd`
to the cent.

| Run | Context, start → end | Result | Cost |
|---|---|---|---|
| 0 · original A, up to the fork | 24k → 307k | ✅ fix/scratch green | $15.55 |
| 0 · original A continuing, live and warm (reference) | 311k → 339k | ✅ 11/11 after one red CI round | $4.52 |
| 1 · baton v2 written by A | 278k | ✅ concrete conventions and traps; whole goal | $2.33 (cold: 280k written at the 1h rate) |
| 1 · B from the baton | 22k → 90k | ⚠️ 10/11: no chain test; prompt not told what runs unasked | $1.78 |
| 2 · A runs `/compact` | 310k → summary | — | $1.35 (cold, 5m write) |
| 2 · A continues after compact | 24k → 98k | ❌ 9/11: allows `find:*`, `git log:*`, `sort:*` behind glob ask-rules; breaks the existing prompt test (CI would go red) | $1.85 |
| 3 · A continues uncompacted (baseline) | 279k → 320k | ✅ 11/11, closest to the original | $2.85 (prefix happened to be warm) |

| Arm, from the fork | Measured | Warm-normalized (A's cache warm, as when live) |
|---|---|---|
| baton + successor | $4.11 | ≈ $1.94 (baton turn ≈ $0.16) |
| compact + continue | $3.20 | ≈ $1.95 to $3.20 |
| uncompacted | $2.85 | $2.85 |

Caveats: one task and one run per arm, so the quality gaps are one or two
checks wide; the fork was at a task boundary (a mid-task fork may favour
batons more, or less); the resumed arms ran under the main-session system
prompt and tools, not the subagent's; H9 was judged by reading; the
baseline's prefix was warm by accident, from the baton arm's cache.

### Verdict

- ✅ The baton works: every fixed-template successor passed its hidden
  checks or came within one, starting at 22 to 36k context.
- 💰 At ~300k, baton plus successor costs the same as compact plus continue
  once A's cache is warm (~$1.95 each), and both are about a third cheaper
  than continuing at 300k ($2.85) for a ~35-call follow-up. Cold, the baton
  cost more ($4.11 against $3.20): the CLI wrote A's 280k prefix at the
  1-hour rate. Hence the rule: nudge only while the cache is warm.
- ✅ Quality: baton B beat compaction. The compacted A allowed `find` and
  `git log` behind glob ask-rules, against the brief's "narrowest form",
  and repeated the original's CI-breaking prompt-test mistake. B's rules
  were safe; it skipped the prompt wording and a chain test.
- ⚠️ Neither handover matched staying uncompacted (11/11). On this sample the
  clean context bought cost, not quality.
- ⚠️ B made more calls than the baseline (55 against 34), re-learning where
  things live; it was still cheaper because each call reads ~50k, not ~300k.
  Hence "Key files" in the baton.
- ❌ The template is the risk: v1 leaked A's temporary scope into B's
  limits (fixed), and both A and the compacted arm broke a test that pins
  text (now its own section).
- ❌ Q&A: never used in five successor runs, and one answer cost more than a
  whole successor. Off by default is right, and it isn't built.
- 💰 Each handover is now logged for Efficiency, so the saving can be read
  from real use rather than three labs.
