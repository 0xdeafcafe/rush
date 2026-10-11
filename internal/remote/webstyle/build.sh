#!/bin/sh
# Builds web/app.css from webstyle/app.css with Tailwind 4, installed for
# the build in a scratch folder: go generate ./internal/remote
set -e
d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT
npm i --prefix "$d" --silent --no-audit --no-fund @tailwindcss/cli@4
NODE_PATH="$d/node_modules" "$d/node_modules/.bin/tailwindcss" -i webstyle/app.css -o web/app.css --minify
