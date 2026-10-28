# Porting rush to Rust — the whole plan

Go rush is ~187k lines in 40 modules (plus ~65k of tests). This ports all of
it in eight milestones. Each one ends with a Rust rush you can run, measured
against the Go one. The Go version keeps shipping until milestone 8.

## Rules for every milestone

- **Same on-disk state.** Rust reads and writes the files Go does
  (`~/.config/rush` state and settings, plugin config, the events log, the
  feed), so you can switch between the two at any point and back again.
- **Same command line.** Agents call `rush view`, `rush feed`, `rush pr`,
  `rush watch`, `rush spawn`, … from their shells and plugins; output and exit
  codes must match byte for byte. Each subcommand gets a parity test that
  runs both binaries.
- **Same plugin protocol.** Plugins are other processes; the protocol
  `plugind` speaks doesn't change, so existing plugins keep working.
- **Measured.** Every milestone ends with Go vs Rust: footprint (idle, with a
  big session open, with N sessions hosted), idle CPU and wakeups, frame
  time, transcript open time. Rust must not be worse on any of them.
- **One workflow per milestone:** understand → port (crates in parallel) →
  check (parity, footprint, review by another model) → fix. Then the
  milestone is committed and reviewed by you before the next starts.

## Milestones

| # | milestone | Go modules | Go lines | new crates |
|---|-----------|------------|----------|------------|
| 1 | **Read-only Claude**: agent list, open a session read-only, `--dump`, `--render` | agent, adapters/claude (reading), convo (model, render), photon hl, ui list + pane | ~12k | core, hl, convo, claude, draw, ui, rush |
| 2 | **Host Claude**: start, send, interrupt, permissions, message box, queue, rewind; sessions survive closing rush | host, adapters/claude/headless + daemon, agent/event, state, events, instances, health, fswait, proc, pgguard, script; ui input box, permission cards, sending, decisions, questions | ~25k | host, state, events |
| 3 | **Whole fleet + machine**: process trees, worktrees and whether binning loses anything, dev servers left running, subagents and agents running agents, PRs on rows | fleet (rest), proc, sysinfo, actions, ghpr, netwatch, netproof; ui machine, subagents, jobs, shells | ~13k | fleet, gh |
| 4 | **Every provider**: codex, kimi/vibe/opencode (ACP), copilot, gemini, antigravity, pi, ollama, deepseek, glm, cross-harness switching | adapters/*, harnessup, agent/usage (quotas) | ~23k | one crate per adapter family: codex, acp, … |
| 5 | **Places**: overview, zen, ctrl+k jump and transcript search, history, help, keymap | ui (overview, zen, cmdbar, history, keyhelp…), convo find/search/indexer, cards, exchange, keymap | ~30k | search |
| 6 | **Settings and spend**: settings (every tab), accounts and limits, efficiency and the advisor, onboarding, providers, `#ask` | ui settings_*, accounts, usage, onboard, askrush; efficiency, advisor | ~25k | efficiency |
| 7 | **Plugins**: plugind, the plugin runtime and approvals, bundled plugins, agtools MCP server, statusline, gate, rooms, community feed, push-later, squeeze | plugin, plugind, hooks, bundled/*, agtools, statusline, settingsfile, gate, room, community, pushlater, squeeze | ~18k | plugin, plugind, bundled |
| 8 | **Outside rush + cutover**: pr drive, watch, serve/remote, menubar (native, objc2 like decline), update, `rush on/off`, every remaining subcommand; then retire Go | prdrive, watch, remote, menubar, update, cmd/rush | ~15k | pr, watch, remote, menubar |

Sizes are Go lines without tests; ui is split across 2–6 by screen.

## Order and why

```
1 read-only ─→ 2 host ─→ 3 fleet ─→ 4 providers ─┐
                                                 ├→ 7 plugins ─→ 8 outside + cutover
                 5 places ─→ 6 settings ─────────┘
```

- 2 before everything else: hosting is what makes rush usable day to day,
  and it fixes the shapes (events, state) the rest builds on.
- 3 next because every row and screen reads the fleet.
- 4 and 5–6 are independent of each other after 3 and can run as parallel
  workflows.
- 7 needs the screens plugins draw into (5–6) and the hosts they hook (2).
- 8 last: the outward-facing pieces (GitHub, remote, menubar) and then the
  switch-over.

## Cutover (end of 8)

1. Parity sweep: every subcommand, every screen at 3 widths, every adapter's
   history read, against Go.
2. A week of daily use on Rust with Go as fallback (same state files).
3. `cargo install` path and `rush update` moved to the Rust binary; README.
4. Go code removed in one commit; `rust/` moves to the repo root.

## Risks

- **ui is 70k lines**, a third of rush. It's spread over 1, 2, 3, 5 and 6 by
  screen so no workflow has to hold it all.
- **Bubbletea behaviour** (focus, mouse, paste, key names on macOS) is
  reproduced in `rush-draw`; `rush --render` snapshots catch drift.
- **Plugin and CLI compatibility**: covered by running both binaries in the
  parity tests, not by reading code.
- **Adapters track moving targets** (Codex, Kimi, … change their wire
  formats): port each from its Go code *and* re-test against the real CLI.
