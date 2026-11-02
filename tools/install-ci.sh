#!/bin/sh
# Installs the rush CI built for a commit of main (default: origin/main's
# newest), waiting for its run when it's still going:
#   tools/install-ci.sh [commit]
# Installs to ~/.local/bin/rush (RUSH_INSTALL_DIR moves where); the rush it
# replaces is kept as ~/.local/share/rush/rush.prev.
set -eu
repo=0xdeafcafe/rush
dir=${RUSH_INSTALL_DIR:-$HOME/.local/bin}
prev=$HOME/.local/share/rush
here=$(cd "$(dirname "$0")" && pwd)
if [ $# -gt 0 ]; then
  sha=$(git -C "$here" rev-parse "$1")
else
  git -C "$here" fetch -q origin main
  sha=$(git -C "$here" rev-parse origin/main)
fi
run=$(gh run list -R "$repo" -w ci.yml -b main -c "$sha" -L 1 --json databaseId -q '.[0].databaseId')
if [ -z "$run" ]; then
  echo "no CI run for $sha on main (pushed yet?)" >&2
  exit 1
fi
echo "CI run $run for ${sha%"${sha#????????????}"}"
gh run watch -R "$repo" "$run" --exit-status --interval 20 >/dev/null || {
  echo "CI run $run failed: gh run view -R $repo $run --log-failed" >&2
  exit 1
}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/rush-ci.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
gh run download -R "$repo" "$run" -n rush-main -D "$tmp"
chmod +x "$tmp/rush"
"$tmp/rush" --version
mkdir -p "$dir" "$prev"
[ -f "$dir/rush" ] && cp "$dir/rush" "$prev/rush.prev"
# Copied beside it, then renamed over it: a running rush keeps its file.
cp "$tmp/rush" "$dir/rush.new"
mv "$dir/rush.new" "$dir/rush"
echo "installed $("$dir/rush" --version) in $dir"
