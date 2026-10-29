#!/bin/sh
# Writes ../tests/parity: the cases (cases.py), then what Go's rush pr and
# rush watch say for each (*.want.json), by copying the parity test into
# cmd/rush, running it, and taking it out again. gh is faked; nothing
# asks GitHub, and RUSH_HOME is a temp folder per case.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
out="$here/../tests/parity"
python3 "$here/cases.py" "$out"
cp "$here/parity_test.go.txt" "$root/cmd/rush/zz_rust_parity_test.go"
trap 'rm -f "$root/cmd/rush/zz_rust_parity_test.go"' EXIT
cd "$root"
RUSH_PARITY="$out" go test -count=1 -run '^TestWriteRustParity$' ./cmd/rush
