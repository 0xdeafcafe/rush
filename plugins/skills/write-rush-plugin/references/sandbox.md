# The sandbox

Each plugin runs as `taskpolicy -c utility sandbox-exec -p <profile> <program>`: at utility QoS, so it yields the CPU to the user and their agents, and in a macOS sandbox profile that starts from `(deny default)`.

## Allowed

- **Execute:** its program only. For a framework Python (Homebrew's or python.org's), the interpreter `bin/python3` hands over to is allowed as well.
- **Read:**
  - its plugin folder and its data folder;
  - the system's libraries (`/usr/lib`, `/usr/share`, `/System`, the dyld cache, time zones, `/private/etc/ssl`);
  - for a program under `/opt/homebrew` or `/usr/local`: that prefix's `Cellar`, `opt`, `lib`, `etc/openssl@3` and `etc/ca-certificates`;
  - the paths in `read` and `write`.
- **Look up metadata (stat) anywhere.** Loaders need it. The plugin can learn that a file exists and how big it is, never what's in it.
- **Write:** its data folder, the paths in `write`, and `/dev/null`.
- **Connect:** `127.0.0.1:<its proxy's port>`, and only when it has `network`.
- A little more: sysctl reads, and the one system service libSystem looks up at start.

## Denied

Everything else, including:

- starting any other program (`fork`/`exec`, `posix_spawn`, shells, `git`, `curl`). To drive another tool's CLI, name it in the manifest's `exec` and call rush's `exec`: rush runs it, outside the sandbox;
- reading the user's files or listing their folders;
- writing outside the data folder and `write` paths, including the plugin's own folder;
- any other socket: the internet directly, other localhost ports, unix sockets (rush's included), DNS;
- the user's environment.

## The proxy

With `network`, `HTTPS_PROXY` and friends point at an HTTP CONNECT proxy the broker runs for this plugin alone. It tunnels only to the exact `host:port` pairs approved. It resolves the name itself and refuses loopback, private, link-local and CGNAT addresses, so an approved name can't be pointed at the user's machine or network. Plain-HTTP proxying isn't offered; use HTTPS.

Most HTTP clients honour `HTTPS_PROXY`: Go's `net/http`, Python's `urllib` and `requests`, `curl`-based libraries. So does Node 24+ with `NODE_USE_ENV_PROXY=1`, which rush sets; that covers `fetch` and `https`. A client that ignores proxy variables can't connect at all, which is the right failure.

## Troubleshooting

Start with `rush plugin list` (state and last error), `rush plugin logs <name>` (the paths of the plugin's own log and the broker's), and `python3 scripts/call.py status`.

| Symptom | Cause | Fix |
|---|---|---|
| `list` says *not approved* or *changed since approval* | Not approved, or a file in its folder changed | The user runs `rush plugin approve <name>` |
| `broken: … unknown field` | A typo in `plugin.json` | Fix the field name: see `manifest.md` |
| `waiting … did not start: connection closed`, restarting | The program died before answering `initialize`, or never read fd 3 | Read the plugin's log; run the program's syntax check locally |
| `dyld: Library not loaded … (file system sandbox blocked open())` | The interpreter's libraries are outside the readable paths | Add their folder to `read`, or use Homebrew's interpreter |
| `posix_spawn … Undefined error: 0`, `fork/exec … operation not permitted` | The program, or its launcher, started another program | Don't. Point `command` at the real interpreter. Do subprocess work inside the plugin, name the program in `exec` and ask rush to run it, or give Claude a tool instead. |
| `OpenSSL configuration error … fopen(…openssl.cnf)` | An interpreter from an unusual prefix | Add its `etc/openssl@3` to `read` |
| `operation not permitted` writing a file | Writing outside `$RUSH_PLUGIN_DATA` (often `~/.cache` or `~/.something` from a library that ignores `HOME`) | Point the library at `$RUSH_PLUGIN_DATA`, via `env` with `${DATA}` if it reads a variable |
| `connect: operation not permitted` | Connecting directly instead of via the proxy, or no `network` at all | Add `network`; use a client that honours `HTTPS_PROXY` |
| Proxy answers `403` | The `host:port` isn't approved: a different subdomain, port or redirect target | Add the exact pair |
| Proxy answers `502` | DNS failed, or the name resolves only to private addresses | Check the host; internal hosts can't be reached |
| `ending it: using N MB, over its M MB limit` in the broker log | Its memory footprint passed `memoryMB` | Raise `memoryMB`, or stream instead of loading everything |
| A tool call fails with `<name> is not running: …` | The plugin is restarting or refused | Fix what the error says; the call fails fast rather than wait out the restart |
| A tool isn't in Claude's list | The session's Claude Code started before approval | New sessions get it; a running session gets it when Claude Code restarts after resting |
| `broker never listened` / nothing starts under a custom `RUSH_HOME` | The unix socket path is over macOS's 104-byte limit | Use a shorter `RUSH_HOME` |
