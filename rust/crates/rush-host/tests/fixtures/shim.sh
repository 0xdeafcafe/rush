#!/bin/sh
# rush's stand-in: rush hosts the run when it can.
a='/usr/local/bin/rush'\''s'
[ -x "$a" ] && exec "$a" spawn 'codex' "$@"
p=; IFS=:; for x in $PATH; do [ "$x" = '/Users/lw/.config/agtop/tmp/rush-7e5ebafc/e57fb8f4/rush-cache3553687442/shims' ] || p="$p${p:+:}$x"; done; unset IFS
PATH=$p; export PATH
exec 'codex' "$@"
