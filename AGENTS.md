# Notes for agents working on the Rust port

- Read `DESIGN.md` first; update it when behaviour or measurements change.
- The Go code (`internal/`, `cmd/` at tag `go-final`) is the spec. Port behaviour,
  words and look exactly; cite the Go file:line you ported in a short
  comment at the top of each module.
- Commits: Conventional Commits, scope = the crate without `rush-`
  (`core`, `hl`, `convo`, `claude`, `draw`, `ui`, `rush` for the binary).
  Before committing: `cargo fmt --all`.
- `rush-core`, `rush-hl`, `rush-convo` stay free of terminal and platform
  code. Only `rush-draw` knows ratatui/crossterm.
- Performance claims need numbers: criterion benches, and
  `/usr/bin/footprint -p <pid>` for memory (not RSS).
- Tests go through public APIs, are table-driven for encodings, and never
  sleep; filesystem tests use temp dirs and never touch `~/.claude`.
- How work is checked is the user's choice, task by task: locally
  (`cargo check`, `cargo clippy -- -D warnings` and the tests of the
  crates you changed), on GitHub CI, or not by you at all, when a lead
  session or its CI will catch it. If they haven't said, ask once before
  the first check and keep to the answer, saying which you'd pick and
  why (a one-line fix: none; a wide change: a local check first, CI once
  at the end). A subagent checks the way its lead says, and never more.
- Never sit waiting on a check. A run on CI takes 5 to 15 minutes and a
  prompt cache lasts 5: an agent that waits wakes to read everything
  again at full price. Carry on with what's next while it runs, push in
  few, whole batches, fix all a run reports in one go, and wait only on
  the last run. Don't ask for a green run between the steps of one task.
- To hear from CI: `rush watch run <branch>` as a background command,
  one watch a push (stop the last before you start another). Never poll
  GitHub yourself.
- No pull requests: keep your branch rebased onto origin/main and green;
  the lead session puts it on main.
