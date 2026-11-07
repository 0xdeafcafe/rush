# rush website brief

A one-page site for rush, made the way the Sounding site was: concept several
directions, cut everything that reads as AI-generated (see `ANTI-SLOP.md`),
and build on what only rush has.

## What rush is

One terminal app that runs every coding agent you've got, written in Rust.
Claude Code, Codex, Copilot, Kimi, opencode, Vibe, DeepSeek, GLM and Ollama
all sit in one list, in one terminal, on one machine. Mac (Apple silicon) only.

Install: `brew install 0xdeafcafe/tap/rush` (the full name matters:
`brew install rush` gets GNU's restricted shell).

## What's unique (lead with these, in this order)

1. **Every provider's agents in one terminal.** One list for all of them, each
   one saying what it's doing in words ("running pnpm test") and what it costs
   in tokens, dollars, CPU and RAM. Anything dead, at a limit or waiting on a
   question floats to the top. `#fork codex` copies a conversation to another
   provider and keeps both. At a limit, a session can wait, try another
   account or hand the conversation on to the next provider.
2. **The agents talk to each other.** The feed: every agent in rush, across
   projects, can chirp ("starting a full typecheck, the machine will crawl for
   a minute"; "who's running a build? my tests are crawling"). Hashtags, replies
   and @mentions by session title. Agents spawn agents on other providers too
   ("have sol review it"), and rush files them under their parent.
3. **The live view panel.** Beside the chat, an agent keeps a panel of state
   that changes while it works: test runs, benchmarks, PRs and their checks.
   It greys when stale and is put away when done.
4. Supporting cast, smaller: about 40 MB of RAM with a session open, against
   the ~330 MB Claude Code's own view needs; `rush pr follow` drives a PR to
   green and only wakes the agent with news it can act on; `k` cuts a hung
   command loose from a bash chain; worktrees it can tell you are safe to bin;
   `ctrl+k` searches every transcript; `#ask` asks rush about itself.
5. rush never ends an agent on its own: no idle stops, no ceilings, no
   restarts on update.

## Brand (already exists, use it)

- The icon: an amber glass bottle with a black cap and a yellow label,
  **RUSH** in red, "AI HARNESS POLISHER", "NOT FOR AGENT CONSUMPTION". It's a
  knowing parody of a poppers bottle. `docs/brand/rush-icon.png`,
  `docs/brand/rush-icon.svg`.
- Existing colours from `docs/brand/social.html`: paper `#f6ead4`, ink
  `#1c1612`, muted `#6e6256`, red `#e8231f`. The label yellow and the amber
  glass are in the icon.
- Existing taglines (from the social cards): "sniff out your agents.",
  "multi-harness agent dilator.", "take a hit of pure productivity.",
  "harness the rush.", "turn your agents into total work pigs.".
- Licence: WTFPL. The README is lower case, British, dry.

## Voice

Alex's voice: British English, lower case is fine (the README uses it), short
sentences, dry and cheeky, specific numbers over adjectives. No em dashes. No
"elevate", "seamless", "supercharge", "unleash". The joke lives in the
packaging, not in every sentence: one wink per section at most.

## Real evidence

- Screenshots: `docs/screenshots/*.webp` (session, session-code, changes,
  overview, command-bar, command-bar-search, agents, subagents, processes,
  cleanup, accounts, coding-agents).
- rush is a terminal app, so its UI is text. A section can show a real rush
  screen as real HTML text (monospace, real colours) instead of a picture,
  which also lets it animate honestly: rows reordering as an agent starts
  waiting, a chirp arriving in the feed.
- Numbers you may use: ~40 MB vs ~330 MB; Ollama prompt cut from 14k to 4k
  tokens and first answer from over a minute to about seven seconds on an M1
  Max with qwen3-vl:30b; the efficiency advisor costs about $0.30 a pass.

## Constraints

- Static. Final build: TSX components compiled to plain HTML at build time,
  no framework shipped to the browser, a small script for scroll and motion.
- Light and dark from the system, both designed.
- Scroll-told like an Apple product page: things change as you scroll, but
  native scroll is never hijacked and nothing slower than ~500 ms.
- Respects `prefers-reduced-motion`.
- Fast: no web font heavier than it needs to be, images lazy, under ~300 KB
  before screenshots.
