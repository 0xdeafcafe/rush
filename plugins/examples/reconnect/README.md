# reconnect

rush's retry, rebuilt as a plugin. rush already continues a session an outage stopped; this plugin does the same through the [UI hooks](../../skills/write-rush-plugin/references/protocol.md#uievent-notification--needs-events-or-input), to show they're enough for it. rush's built-in retry stays as it is. When rush says it's retrying a session itself (`retrying` on the stop's error), this leaves it alone, unless *Retry what rush retries* is on; then a session may be continued by either, and whichever starts a turn first stops the other. Install it to read how it works, not because you need it.

What it does:

- When a session stops with an error of kind `offline` or `retryable`, it remembers it. A `limit`, `auth`, `too-long` or `other` error, or a stop with no error, is never retried: `continue` wouldn't help, and might spend what's left of a limit.
- When the network comes back (`network.up`), it sends `continue` to each session it remembers. After a `retryable` error with the network up, it waits first (*Wait before continuing*: 5s, 15s or 60s). An `offline` stop that came without a `network.down` gets the same wait, so it isn't stuck waiting for an event already past.
- If no turn starts, it tries again, waiting twice as long each time (15s, 30s, 60s … up to 10 minutes), up to *Tries before giving up* (3, 5 or 8). A turn starting, by its `continue` or yours, ends it. When the network goes down again, it stops counting until it's back.
- When it gives up, or rush refuses a send, it says so at the bottom of the screen, naming the session.

## What it's allowed to see

It hears what happens in rush's screen (`events`): sessions seen, opened and left, turns starting and ending, why a session stopped, and the network going and coming back, with what the agent list shows of each session. **Never what was said**, and nothing you type. It may show a notice (`notify`), and send a message to a session as if you'd typed it (`send`), but only to sessions it heard of, working in its `workspaces`, and in a permission mode that asks before acting (not `bypassPermissions` or `auto`), at most ten a minute. The only thing it sends is `continue`.

It keeps nothing on disk, has no network, and can't start programs.

## Install

Change `workspaces` in `plugin.json` to where your projects are: it's `~/Source` here. It's a Rust crate in rush's workspace, on rush's plugin crate (`plugins/sdk`), and needs nothing else of rush's. From the top of rush's repository:

```sh
cargo build --release -p reconnect   # on Apple Silicon, for arm64: an x86_64 plugin can't start in the sandbox
mkdir -p ~/.config/rush/plugins/reconnect
cp target/release/reconnect plugins/examples/reconnect/plugin.json ~/.config/rush/plugins/reconnect/
rush plugin check reconnect     # valid, and what approving it allows
rush plugin approve reconnect
```

## Reading it

- `src/retry.rs` is the state machine, with no I/O and no clock: `Retrier` takes `stopped`, `started`, `down` and `up`, and `due` says which sessions to continue or give up on now. `Config::backoff` is the backoff. Its tests are beside it.
- `src/main.rs` maps `ui.event`s onto it, and a loop sends `ui.send` and `ui.notify` when something's due.
- The protocol, length-prefixed JSON-RPC on fd 3, is `rush-plugin-sdk`'s.

```sh
cargo test -p reconnect
```
