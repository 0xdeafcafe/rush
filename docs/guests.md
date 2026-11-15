# Guests: other models in one conversation (Rust-only)

Any conversation rush hosts can have guests: other agents, of any
provider and harness rush runs, who answer in the same conversation,
under their own name, having seen what was said. You ask one by its
name; agents ask each other the same way. Only one of them changes
files at a time.

```
you      › @kimi does this migration look safe?
◐ Kimi → you
           The index rebuild locks the table; batch it…
you      › fix what kimi said                      (to the main agent)
✻ Claude › Batched it into 5k-row chunks (edited migrate.rs)
◎ Sol → Claude
           (asked by Claude) Ran the tests: 214 passed
```

## The model

- **A guest is a session.** One rush-hosted session per (conversation,
  agent), started the first time it's named and reused after: it keeps
  its own history, so each turn needs only what's new. Its config is
  spawn_agent's for that agent (`SubAgents::config`, same folder, same
  checkout, the lead's trust) with Meta `spawnedBy=<lead>` (so it folds
  under the lead in the list, its asks surface on the lead's page and it
  stops with the lead), `guestOf=<lead>` and `guestName=Kimi`. No
  work prompt: it's a participant, not a worker.
- **The lead** is the conversation's own agent: the session you opened.
  Guests are kept in its folder (`sessions/<lead>/`):

| file | what |
|---|---|
| `guests.json` | the roster: each guest's name, handle (`kimi`), session id, agent and model; what of the chat it has seen; what the lead has seen; hops since you last spoke |
| `chat.jsonl` | what's said, a `rush_room::Entry` a line: your messages, the lead's and each guest's words, their tool calls one line each (`rush_room::doing`) |
| `lease.json` | the write lease: who holds it, for which turn |

  The lead's host writes `chat.jsonl` once `guests.json` exists; each
  guest's host writes its own words there. Appends are flocked; past
  512 KB it's cut to its last 256 KB.

## Asking a guest

- **In the box**, a message starting with `@name` (one or more:
  `@kimi @sol what do you think?`) goes to those guests, not the main
  agent. A name is a guest already in the chat (by handle), else any
  agent start_agent can start, resolved as spawn_agent resolves names
  (`agentmatch::matches`: `kimi`, `sol`, `codex astra`, `claude haiku`;
  a second word is taken as part of the name only when it narrows it).
  `@` completes guests and agents with session handles and files. A tag
  that names no agent, or any path (`@src/main.rs`), is sent to the main
  agent as before.
- **Agents ask** with rush's `ask_guest` tool (`name`, `prompt`,
  `wait`): the lead or a guest addresses any participant by name, the
  lead included (`ask_guest {name: "claude"}` from a guest reaches the
  lead, as a message in its queue). The asked one's reply card says
  who asked it. By default the call waits for the answer (as
  `agent_send`); `wait: false` returns at once.
- **Hops.** Agent-to-agent asks since you last spoke are counted in
  `guests.json`; the 6th is refused ("6 asks between agents since you
  last spoke: rush holds the rest until you say something"), and a card
  in the conversation says so. Anything you say sets it back to 0.

`rush_host::guests::ask` does both: resolves the names, starts or wakes
each guest, writes the ask to `chat.jsonl`, builds the turn's prompt and
delivers it as an exchange from the asker (`deliver_how`). The UI calls
it off its thread; the tool calls it in the host or in `rush mcp-tools`.

## What a guest's turn sees

Rooms' "what a turn sees", reused (`rush_room::seen`): everything said in
`chat.jsonl` since that guest's last turn: your messages, the lead's
words, other guests' replies, tool calls a line each; the newest kept
whole, the oldest dropped first past a budget of 60 KB, less for a model
with a small context window (about 1.5 bytes a token of it, less 4 KB).
The ask itself comes last, said as who asked it.

Its first turn carries a brief first: it's a guest in the user's
conversation with the lead, who else is in the chat, the folder, that it
can address anyone with `ask_guest`, and the write rule ("one writer at
a time: a file change may wait for another participant to finish").

## How guests show

A guest's turn is in the lead's conversation as a card, the way messages
between agents already are (an `Exchange`, phase `guest`): `you → ◐ Kimi`
for the ask, `◐ Kimi → you` (or `→ ✻ Claude` when an agent asked) for the
reply, the mark the agent's harness's (rush-convo's `guest_mark`, Settings'
marks without their colours; other message cards keep Go's ◇). The reply holds the guest's last words and a
line of what it did (`did: editing migrate.rs · running cargo test`);
folded to its start and end like any message card, space opens it whole.
The cards go through the lead's host (`record_exchange`) into its
exchange journal, so they're drawn live and come back, by time, when the
conversation is read again (`restore_exchanges`). When the lead's host
isn't running they're appended to the journal directly.

The guest itself is a hosted run of the lead (`spawnedBy`): a row under
the lead in the list while it works, its asks as the lead's subagent
cards, and its whole session (every step) one alt+o away. It is not a
step of the lead's, so nothing else draws it in the conversation.

## The lead hears guests

When the lead's next turn starts (`deliver_inline`, any harness), rush
puts what was said since its last turn by others (guests' words and
tools, your asks to guests) atop the message, in a
`<system-reminder>` (which the conversation doesn't draw as yours), and
only then marks it seen: a guest reply that lands while the note is
built waits for the turn after; nothing is dropped. The note is at the
end of the conversation, never in the cached system prompt.

## The write lease

One participant changes files at a time, per conversation.

- **What changes files:** edit, write, delete, move and notebook calls
  (`tool::Kind`), and shell commands `shellwrites::shell_writes` says
  write a file, or that start with `rm`, `rmdir`, `mkdir`, `patch` or a
  git command that changes the checkout (`commit`, `checkout`, `reset`,
  `merge`, `rebase`, `stash`, `apply`, …). Reads and read-only commands
  never wait.
- **Taking it:** a participant's first such call in a turn takes the
  lease (`lease.json`: holder, its name, its turn). It is held until that
  turn ends, and dies with its holder: a lease whose holder's host is
  gone, or whose holder is past that turn (stopped, crashed, ended), is
  free.
- **Waiting, every harness:** the host holds a file-changing approval
  (`Event::Approval`, the one point every adapter's permission asks go
  through) while another holds the lease: the ask isn't shown or answered
  yet, the session says `waiting for Kimi to finish editing`, and a look
  every second lets it through once the lease is free; then it goes as
  any ask goes (rules, you).
- **Waiting, Claude Code in any mode:** Claude Code asks nothing in
  `auto`, `acceptEdits` or `bypassPermissions`, so every hosted Claude
  Code session also gets a PreToolUse hook on edits and Bash
  (`rush guests hook <id>`; a shell test first, so a session with no
  guests starts no rush). It takes the lease or waits for it, up to 9
  minutes, then refuses the call with why.
- **Seen, not held:** another harness running in a mode that never asks
  can't be held; its file-changing calls still take the lease when seen,
  so the others wait for it.

## Limits

- A guest started by an agent before any chat was logged sees only what's
  in `chat.jsonl`: what the lead said before its first guest isn't there.
  The UI's `@name` sends the end of the conversation (as a hand-off tells
  it) with a new guest's first ask.
- The lead hears guests at its next turn, not mid-turn.
- The `@` picker offers the agents running now, as before; a model that
  isn't running (`@kimi` with no Kimi session) is still asked when typed,
  but isn't offered. Filling the picker from the registry off the UI's
  thread would make its rows change under the tests that pin them.
- Lease waits are looked at once a second; a hook waits at most 9
  minutes.

## Open questions

- Should a guest run in a mode that always asks before writing, so the
  lease holds it in every harness? Now it has the lead's trust, as a
  spawned agent does, and an agent that never asks is only seen.
- Guests' cards sit in the lead's turn they land in (exchanges attach to
  the turn under way or the last); a turn of their own, headed by the
  guest, would need the conversation model to take turns that aren't
  yours or the agent's.
