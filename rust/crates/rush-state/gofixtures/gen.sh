#!/bin/sh
# Writes ../tests/fixtures from the Go rush: copies the fixture tests into
# the Go packages, runs them, and takes them out again.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
out="$here/../tests/fixtures"
mkdir -p "$out"
for p in state health settingsfile; do
	cp "$here/${p}_fixtures_test.go.txt" "$root/internal/$p/zz_rust_fixtures_test.go"
done
trap 'for p in state health settingsfile; do rm -f "$root/internal/$p/zz_rust_fixtures_test.go"; done' EXIT
cd "$root"
RUSH_FIXTURES="$out" go test -count=1 -run TestWriteRustFixtures ./internal/state ./internal/health ./internal/settingsfile
