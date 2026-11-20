# Projects, with three keys

Projects today has about 25 actions on 9 kinds of row; `x` means six things,
and enter on a worktree opens Delete, whose enter force-removes it even when
commits exist only there (`place.rs:364`, `delete.rs:621`, `:529`). This
rebuilds it around three keys that mean the same on every row.

## What it looks like

```
╭─ Projects ─────────────────────────────────────── 14.2G on disk · 6.1G can go ─╮
│ ▾ rush                                                    9.8G                 │
│     ✱ e72ca09c       claude is working                    1.2G                 │
│     ✓ codex-clean    merged, done 2h                      0.8G   can go        │
│     ⚠ scratch-root   2 commits not pushed                 1.1G   keep          │
│     ✓ 9 subagent worktrees                                4.4G   2.3G can go   │
│ ▸ alex-forbes-red                                         1.6G   0.4G can go   │
│ ▸ This Mac   caches, /tmp, old Codex sessions, processes  6.2G   2.4G can go   │
│ ▸ Pull requests   3, 1 wants your review                                       │
╰────────────────────────────────────────────────────────────────────────────────╯
 on ⚠ scratch-root:   enter details   x clean up…   c clean up everything that can go
```

## The rules

- **enter opens.** A repo or group folds and unfolds; a worktree shows its
  details; an agent opens its chat; a PR its sheet. Enter never deletes,
  anywhere in the place or its sheets.
- **x cleans up this row.** Always the one Clean up sheet, scoped to the row
  (a worktree, a repo and all under it, This Mac). What loses nothing comes
  ticked; ⚠ rows never do. Only `y` cleans up; ticking a ⚠ row asks again
  and says what's lost.
- **c is x for everything** that can go.
- **One mark per row:** ✱ working, ✓ can go and loses nothing, ⚠ keep
  (would lose work, said why in words), blank for kept for now. One verb:
  "clean up". "can go, by hand", "remove", "clear", "empty", "delete all of
  it" go.
- **The rare actions live in the details pane** with their key written
  beside each: push, open in Finder or on GitHub, end a process or its
  tree, close an agent, review memory notes.
- **The footer shows only what works on the row you're on.**
- `/` filters the tree by repo, branch, agent or PR; `r` rescans; esc and ←
  go back; `q` no longer leaves.

## Tasks

Each lands green on its own, behind nothing; snapshot tests in
`crates/rush-ui/src/screens_tests/projects_machine` change with each.

- [ ] **1. One worktree truth.** The tree reads `Data.wts` (the cleanup
  scan's verdict) and the details pane `Folder.trees` (GitState); they can
  disagree. Make the verdict (`rush-fleet/src/verdict.rs:106`) the one
  source both draw from, and map its outcomes to the four marks.
- [ ] **2. Every repo with worktrees.** `data.rs:352 projects()` lists only
  folders agents worked in recently (`work_agents`). Add the repos
  `rush_fleet::find_worktrees` knows of, so old, big worktrees show.
- [ ] **3. Kit findings as rows.** Give `CleanupReport`
  (`rush-plugin/src/services.rs:29`) optional structured items (id, title,
  size, can go, comes back by itself, what cleaning does) and a
  `cleanup.clean` service that cleans one by id. The bundled kits
  (`rush-bundled/src/clean`) answer both; the cleanup agent keeps reading
  the text. Projects puts them under This Mac and, for worktree builds
  (target/, node_modules), under their worktree.
- [ ] **4. The tree, drawn.** One look: drop the classic table
  (`view.rs:130`) and `list_look`. Rows as above; Temporary, System and the
  kits fold into This Mac; Pull requests stays its own group. Top line:
  on disk and can go. Transcripts show as kept, compressed (they're
  squeezed in place already; nothing deletes them).
- [ ] **5. Keys.** enter, x, c, /, r, esc as above (`place.rs:331-449`);
  the footer per row (`:216-328`). Delete's enter does nothing
  (`delete.rs:621`). Remove the per-kind x meanings, `t`, `X`, `!`, `A`.
- [ ] **6. One Clean up sheet, scoped.** `clean.rs` takes a scope (row,
  repo, This Mac, everything). Ticked: what loses nothing. ⚠ asks again in
  words. y cleans up; force-removal only after that second ask. It replaces
  the Delete sheet's direct use.
- [ ] **7. Details pane.** Worktree: why it's kept or can go, its unpushed
  commits, agents in it, disk by part (source, builds, deps), and its rare
  actions with keys. Repo, This Mac and agent rows likewise.
- [ ] **8. Filter.** `/` narrows the tree in place; esc clears it.
- [ ] **9. DESIGN.md.** Rewrite "The Projects tree" (l.1225) to this.

## Not in this

- Transcripts: kept as they are; they're stored compressed already
  (`rush-sys/src/squeeze.rs`).
- Search: its own plan (one bar, scope as chips, filter errors, n/N next).
