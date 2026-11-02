# Notes for agents working on the Rust port

- Read `DESIGN.md` first; update it when behaviour or measurements change.
- The Go code (`internal/`, `cmd/` at tag `go-final`) is the spec. Port behaviour,
  words and look exactly; cite the Go file:line you ported in a short
  comment at the top of each module.
- Commits: Conventional Commits, scope = the crate without `rush-`
  (`core`, `hl`, `convo`, `claude`, `draw`, `ui`, `rush` for the binary).
  Before committing: `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and the tests of
  the crates you changed.
- `rush-core`, `rush-hl`, `rush-convo` stay free of terminal and platform
  code. Only `rush-draw` knows ratatui/crossterm.
- Performance claims need numbers: criterion benches, and
  `/usr/bin/footprint -p <pid>` for memory (not RSS).
- Tests go through public APIs, are table-driven for encodings, and never
  sleep; filesystem tests use temp dirs and never touch `~/.claude`.
- Builds and tests run on GitHub CI, not locally. Push your branch, then
  wait with `rush watch run <branch>` (as a background command): it
  shares one watcher with every other agent and keeps to GitHub's rate
  limit. If the installed rush has no `watch run`, use
  `gh run watch <id> --exit-status --interval 120`, never faster.
- No pull requests: keep your branch rebased onto origin/main and green;
  the lead session puts it on main.
