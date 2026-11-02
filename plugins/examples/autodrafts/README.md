# autodrafts

rush's drafts, rebuilt as a plugin. rush already keeps what you type and don't send; this plugin does the same through the [UI hooks](../../skills/write-rush-plugin/references/protocol.md#uievent-notification--needs-events-or-input), to show they're enough for it. rush's own stash, its bundled `drafts` plugin, stays as it is. Install it to read how it works, not because you need it.

What it does:

- As you type in a message box (a Session's, or the Prompt's), it notes the text, and writes it down as that box's autosave once you stop typing for 1.5 seconds.
- When you leave the Session, or clear the box with text in it, it keeps that text as a draft for that box.
- When you send, it forgets the autosave: nothing is kept of what was sent.
- `plugin:autodrafts.restore` (`ctrl+]` then `b`, if it's free) puts the box's newest draft back, and takes it out of the list, so running it again goes one further back. With no draft left, it puts back the autosave, what you were typing when rush quit.
- `plugin:autodrafts.list` says how many drafts the box keeps.
- Settings, Plugins, *Drafts kept per box*: 10, 50 (the default) or 200. The oldest go first.

## What it's allowed to see

**Everything you type in rush's message boxes**, as you type it, and when you send or clear it. That's what `ui: ["input"]` means, and approval says so in capitals. It also hears which Session opened or closed (`events`), with what the agent list shows of it, and may show a notice (`notify`) and set a box's text (`input`).

It keeps what you typed only in its data folder, `~/.config/rush/plugin-data/autodrafts/drafts.json`, readable only by you, written whole each time (a temporary file renamed over it). It has no network, and can't start programs or read anything else: the sandbox stops it, whatever its code says. Delete that file to forget every draft; revoke the plugin to stop it hearing anything.

## Install

It's a Rust crate in rush's workspace, on rush's plugin crate (`plugins/sdk`), and needs nothing else of rush's. From the top of rush's repository:

```sh
cargo build --release -p autodrafts   # on Apple Silicon, for arm64: an x86_64 plugin can't start in the sandbox
mkdir -p ~/.config/rush/plugins/autodrafts
cp target/release/autodrafts plugins/examples/autodrafts/plugin.json ~/.config/rush/plugins/autodrafts/
rush plugin check autodrafts     # valid, and what approving it allows
rush plugin approve autodrafts
```

## Reading it

- `src/drafts.rs` is the logic, with no I/O and no clock: `Book` debounces a box's changes into its autosave (`changed`, `settle`), keeps drafts capped per box (`keep`, `set_keep`), forgets what was sent (`sent`), and walks back through them (`restore`). Its tests are beside it.
- `src/main.rs` maps `ui.event`s onto it, answers `ui.command` with `ui.input.set` and `ui.notify`, and writes the file off the thread that reads events.
- The protocol, length-prefixed JSON-RPC on fd 3, is `rush-plugin-sdk`'s.

```sh
cargo test -p autodrafts
```
