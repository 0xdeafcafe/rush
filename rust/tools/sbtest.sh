#!/bin/sh
# Runs a command (usually cargo test) under rust/tools/rush-tests.sb with a
# clean environment, so tests can't reach the user's rush:
#   rust/tools/sbtest.sh cargo test -p rush-ui
# CARGO_TARGET_DIR and SCRATCH (where builds and test homes may write;
# default $TMPDIR) are passed through.
here=$(cd "$(dirname "$0")" && pwd)
scratch=${SCRATCH:-${TMPDIR:-/tmp}}
# Claude Code's daemon for the real ~/.claude, which hosts its background
# sessions: /tmp/cc-daemon-<uid>/<sha256(config dir)[:8]> (daemon.rs).
# Claude Code hashes the folder as it's named, so ~/.claude reached through
# a link has a second daemon under the resolved name: both are denied.
hash8() { printf %s "$1" | shasum -a 256 | cut -c1-8; }
daemon=/private/tmp/cc-daemon-$(id -u)/$(hash8 "$HOME/.claude")
real=$(cd "$HOME/.claude" 2>/dev/null && pwd -P)
daemon_real=$daemon
[ -n "$real" ] && [ "$real" != "$HOME/.claude" ] && daemon_real=/private/tmp/cc-daemon-$(id -u)/$(hash8 "$real")
cmd=$1; shift
case $cmd in cargo) cmd=/opt/homebrew/bin/cargo ;; esac
exec env -i HOME="$HOME" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup" \
  RUSTC=/opt/homebrew/bin/rustc RUSTDOC=/opt/homebrew/bin/rustdoc \
  CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$scratch/t-sbtest}" \
  CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  TMPDIR="$scratch" \
  sandbox-exec -D HOME="$HOME" -D SCRATCH="$scratch" -D CLAUDE_DAEMON="$daemon" -D CLAUDE_DAEMON_REAL="$daemon_real" -f "$here/rush-tests.sb" "$cmd" "$@"
