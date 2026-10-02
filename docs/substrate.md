# Substrate

Status: proposed 2026-10-02. Nothing built yet.

## The idea

The nitrate substrate is a dumb inference layer between an agent harness
(Claude Code, Codex, ...) and the provider's API. Every agent points its
base URL at it once; from then on, which account, model or provider a
request goes to is decided in one place, and every agent follows without a
restart. "Change my instance once and everything works."

It is a local HTTP proxy and nothing more:

- forward each request to the real API,
- stream the response back unbuffered (SSE passes through byte for byte),
- tee both directions to a log rush reads.

It never rewrites a request. Anything smarter lives in rush.

## What rush gets from it

- Token counts, cost, context size and rate-limit headers read off the
  wire, instead of reconstructed from transcripts after the fact.
- One place to switch account or provider.
- Later, if wanted: failing over to another account when one hits its
  limit. That is a policy, so it is the first thing to argue about keeping
  out.

Once it reads tokens and limits off the wire, the conversation view's
counts move onto it (rush-ec offered, 2026-10-02). Harness clocks don't
change: a paused Bash command's timeout (`internal/ui/shells.go`) and the
30m background-task nudge (`internal/host/longtask.go`) stay as they are.

## Two ways to run it

One package, `internal/substrate`, run two ways. Config key `substrate`:

| Value      | What runs                                                        |
|------------|------------------------------------------------------------------|
| `embedded` | Inside rush, for the agents rush starts. Default; unset means this. |
| `daemon`   | A standalone `nitrate` process that owns the accounts. Agents started outside rush can point at it too. |

Rush starts the embedded one only when no daemon is listening, so there is
one copy of the forwarding code and nobody has to migrate: both values work
from day one. If "embedded" is ever renamed, `embedded` stays accepted.

The package is `substrate`, not `embedded`, so it does not collide with the
UI's `m.embedded` (one session shown in a pane, `internal/ui/sidebar.go`).

## Wiring

Adapters already add to the agent's environment (`o.Env` in
`internal/adapters/codex/codex.go`). Pointing an agent at the substrate is
one more entry there:

- Claude Code: `ANTHROPIC_BASE_URL`.
- Codex: its OpenAI base URL setting.

Harnesses with no base-URL override bypass the substrate and work as today.

## Open before building

1. ~~**Subscription auth.**~~ Answered 2026-10-02: it works, if we pass
   through untouched.
   - Claude Code documents it: `ANTHROPIC_BASE_URL` alone keeps the
     claude.ai login as the credential, with its limits and billing.
     Forward `anthropic-beta` (it carries the OAuth capability) and every
     other header as-is. Never set a gateway credential: that swaps the
     subscription for per-token billing.
     (code.claude.com/docs/en/llm-gateway, .../llm-gateway-protocol)
   - The line not to cross: subscription tokens are for Claude Code and
     claude.ai only. The substrate forwards Claude Code's own requests; it
     must never take the token and send requests of its own.
   - Codex: ChatGPT login goes to a different backend than the API. Route
     it with a custom provider in `~/.codex/config.toml` pointed at the
     ChatGPT path, with `OPENAI_API_KEY` unset. Verify by hand first.
2. **Where the log lives** and how long it is kept: under the state dir
   (`state.Dir()`), per session.
3. **Daemon discovery:** a fixed socket or port under the state dir.

## Aside: state dir

`state.Dir()` falls back to `~/.config/agtop` while `~/.config/rush` has no
`config.json`. Moving the old folder's contents across, with nothing
running, ends the fallback.
