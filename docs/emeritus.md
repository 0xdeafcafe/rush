# Emeritus: an honourable handover

Status: design, 2026-10-02. Not built. Two adversarial reviews and two lab
tests shaped it; results at the bottom.

An agent near the end of its context writes a baton, rush starts a fresh
agent on it, and the old one retires for good. It isn't a phoenix: the old
agent isn't burnt, it becomes emeritus. Its transcript stays, and an opt-in,
isolated fork of it can answer a question, at a measured price.

```
 A (context ≥ 70%, host-measured)                B (fresh)
   │ nudge (advisory) ─ A writes the baton: judgement only
   │ mcp__rush__retire {baton}  → "retiring after this turn"
   ▼ turn ends
 rush, under A's lifecycle lock:
   1. freeze A: refuse new input, wait out Background tasks and children
   2. append facts: branch, HEAD, git status, diffstat, transcript path, cost
   3. start B (A's folder, kind, model, mode, account), wait for its first turn
   4. B started? commit: A retired (terminal), children and reports → B
      B failed? roll back: unfreeze A, tell it why                 │
                                                                   ▼
 A stopped for good; baton + transcript on disk          B owns the task
   (optional) ask_predecessor → isolated fork of A ── one answer ─▶ B
```

## The baton

`<session dir>/baton.md`. A writes the judgement, rush appends the facts.

| Section | Holds | Who |
|---|---|---|
| Destination | The task's goal, in a checkable form. The whole goal, not A's last instruction | A |
| Core ideas | The 3 to 5 things that matter most | A |
| Methods | How the work is done: commands, the loop | A |
| Journey | What failed or surprised, and why | A |
| Limits | ✅ good enough / ❌ never acceptable. Durable rules only, not "don't do X yet" | A |
| Next action | One line | A |
| State | Branch, HEAD, `git status --short`, diffstat, transcript path, cost | rush |

The template says plainly: scope limits the user gave A for one turn ("phase
1 only", "don't start X") are not limits for B. Test 2 broke on this.

## Rules

| Rule | Why |
|---|---|
| `retire` is a request; it runs after A's turn ends | A can only call a tool during a turn, and retiring needs A idle |
| Handover is one persisted transaction: freeze, append, start B, commit or roll back | B can fail to start (auth, limits) after A is gone; a crash mid-way must not leave two owners or none |
| Retired is terminal, enforced in `Spawn`, `session start --resume` and message admission, not only `Ensure` | Every one of those can wake a saved host |
| A's children and report destinations move to B; A's parent is told B took over | Background results would otherwise wake A, or land nowhere B can see |
| B inherits A's permission mode, model and account | The CLI start path uses the profile's defaults |
| A's composer is read-only, with a link to B. Nothing is silently redirected | Typing into A must not quietly instruct someone else |
| The 70% check is host-owned and deduplicated; the nudge is advisory | Plugin `turn.ended` comes from what a TUI window saw: absent headless, doubled across windows |
| `lineage` counts the chain; B may ask only its direct predecessor | Bounds chains |
| Claude only, to begin with | Only Claude can run the isolated fork |

## Asking the predecessor: off by default

A fork costs a cold read of A's whole context: removing the tools changes
the cached prefix. In the lab test one answer on a 38k context cost $0.47,
three times what B spent finishing the task. No successor in either test used it.

When switched on (`emeritus.questions = N`), each question is:

```
claude -p --resume <A> --fork-session --tools "" --strict-mcp-config \
  --no-session-persistence --disable-slash-commands   (+ summary.go's isolation)
```

with A's model and account, a spend and time cap, one at a time per A, and
"unavailable" when A's context plus the question won't fit (never a silent
summary of the memory being asked).

## Rush APIs it rides on

| Need | Existing API |
|---|---|
| Start B, tagged | `host.Spawn` with `Meta{predecessor, lineage}`. CLI: `rush session start --cwd DIR --prompt-file baton.md --meta predecessor=<A>` |
| Context and cost | `rush session info <id> --json` → `contextTokens`, `costUsd` |
| Retire | `rush session stop <A>` plus the new terminal flag |
| Lineage | `rush session list --json --meta predecessor=<A>` |
| `/emeritus` | a plugin command (`plugins/examples/delegate` shape) |
| Fork answer | the summary.go pattern |

New host work: `retire` (deferred, transactional), the terminal flag at
every entry point, moving children and reports, the host-owned context
check, and the read-only row. The lab prototype is
`emeritus-lab/tools/ask-predecessor`, plus `emeritus-lab-judge/*.sh`.

## Against what exists

Compaction keeps decisions and corrections (`convo/compactby.go`), and the
usage-limit `handoff` is bounded too. The baton's case is that A chooses
what matters while it still knows, and B starts with a clean context in a
separate session. That has to be measured against compaction, not assumed:
test 2 does.

## Reviews

| Round | Reviewer | Changed |
|---|---|---|
| 1 | Claude Opus | retired made terminal; Q&A moved from the live A to a tool-less fork; git state appended by rush; refuse while busy; B as sibling with A's mode; lineage |
| 2 | Codex (astra) | `retire` deferred to turn end; handover as a transaction with rollback; terminal flag at every entry, not just `Ensure`; children and reports move to B; fork isolation beyond `--tools ""`; context-fit check; Q&A off by default; host-owned trigger; no silent input redirect; compaction comparison corrected |

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

Verdict:
- ✅ The baton works: both fixed-template successors passed hidden checks, starting at ~36k context.
- 💰 Baton plus successor ($0.70) came in just under compaction plus continuing ($0.80), and B ends with a clean, separate session.
- ❌ The template is the risk: v1 leaked A's temporary scope into B's limits. The fixed wording is now in the baton section.
- ❌ Q&A: never used in four successor runs, and one answer cost more than a whole successor. Off by default is right.
- ⚠️ Small sample: two tasks, short contexts (35 to 45k). The case for Emeritus grows with A's context; rerun at 300k+ before building.
