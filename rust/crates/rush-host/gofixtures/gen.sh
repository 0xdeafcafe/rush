#!/bin/sh
# Writes ../tests/fixtures from the Go rush: copies the fixture test into
# internal/host, runs it, and takes it out again. Nothing it runs starts a
# host or an agent: it only marshals what Go writes and sends.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
out="$here/../tests/fixtures"
mkdir -p "$out"
cp "$here/host_fixtures_test.go.txt" "$root/internal/host/zz_rust_fixtures_test.go"
trap 'rm -f "$root/internal/host/zz_rust_fixtures_test.go"' EXIT
cd "$root"
RUSH_CACHE=/rush-cache RUSH_FIXTURES="$out" go test -count=1 -run TestWriteRustFixtures ./internal/host
