#!/bin/sh
# Builds the plugin (macOS 26+, needs the macOS 26 SDK) into $RUSH_PLUGIN_DIR (by hand: $(rush plugin dir)/apple-intelligence).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dest="${RUSH_PLUGIN_DIR:-$(rush plugin dir)/apple-intelligence}"
mkdir -p "$dest"
swiftc -O -target arm64-apple-macos26.0 "$here/main.swift" -o "$dest/apple-intelligence"
cp "$here/plugin.json" "$dest/plugin.json"
echo "built $dest/apple-intelligence"
