# Notes for agents working on the Rust port

- Read `DESIGN.md` first; update it when behaviour or measurements change.
- The Go code in `../internal` and `../cmd` is the spec. Port behaviour,
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
