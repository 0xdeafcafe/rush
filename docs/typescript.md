# One TypeScript language server per checkout, shared and capped

Alex, 2026-10-23. Why the `ts` plugin exists, instead of each agent
running its own.

## What happened

On 2026-10-23 the machine hung for minutes and the kernel watchdog
panicked. Three `tsgo` language servers held 9.7, 4.2 and 2.8 GiB of
18. tslsp-cli started them: each worktree's daemon detached to launchd
with its `tsgo`, out of any agent's process tree, and nothing capped
them. The gate never saw them, and its pause waited for a pressure level
the kernel never reached.

## Measured

On the langwatch monorepo (54 modules), one server rooted at the
checkout, one file opened in each module, physical footprint:

| | tsgo | tsrs |
|---|---|---|
| first project loaded | 976 MiB | 392 MiB |
| a file open in each of 54 modules | 4.6 GiB, 30s | 1.9 GiB, 5s |
| then `workspace/symbol` | past 6 GiB | past 6 GiB |

A symbol search with no file loads every project, whichever engine.

## Decisions

1. **One server per checkout**, keyed by the folder with the `.git`
   (a worktree's is a file), not one per tsconfig: opened files find
   their own projects inside it and share what they load.
2. **In rush, not in each agent.** rush sees every session and harness;
   its tools reach every rush-mode agent over MCP and `rush ts` reaches
   any with a shell. Claude Code's own LSP tool would start a server per
   session, uncapped.
3. **Bundled**: a sandboxed plugin may start no programs.
4. **tsrs first**, the platform binary run directly (never the package's
   Node shim), so the process watched is the server.
5. **Admitted by the gate** like `tsc`, holding its ticket while it
   lives and noting what it holds, so its reservation shrinks to what
   it's yet to take. Inside a gated run it isn't gated again.
6. **Capped** (default 4 GiB): past it the server is killed, the call
   told to give a file and line, and the next call starts a fresh one.
   Closed after 10 minutes idle. The guard names `tsrs` and `tsgo`, so
   it stops a server too when the machine runs out.

## Not yet

Rename and moving a file with its imports rewritten: they need LSP's
workspace edits applied to files, which tslsp-cli still does.
