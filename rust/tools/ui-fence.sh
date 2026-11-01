#!/bin/sh
# Prints each reference from a rush-ui module to a screen group it may not
# reach, and exits 1 if there are any:
#   rust/tools/ui-fence.sh <module path> [allowed group ...]
#   rust/tools/ui-fence.sh crates/rush-ui/src/screens/bars bars
#   rust/tools/ui-fence.sh crates/rush-ui/src/hosted.rs       # none allowed
# A group is a folder of crates/rush-ui/src/screens. Looks for
# `crate::screens::<group>`, `crate::screens::{..<group>..}` and
# `super::super::<group>` (the last only means a sibling group from a file
# one folder down, so a group's own deeper files may name it falsely).
# A module's own group has to be listed to be allowed.
[ $# -ge 1 ] || { sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
screens=$here/../crates/rush-ui/src/screens
target=$1; shift
[ -e "$target" ] || target=$here/../$target
[ -e "$target" ] || { echo "ui-fence: no such module: $1" >&2; exit 2; }
groups=$(cd "$screens" && for d in */; do printf '%s ' "${d%/}"; done)
bad=0
for g in $groups; do
  case " $* " in *" $g "*) continue ;; esac
  found=$(rg -nH --no-heading \
    -e "crate::screens::$g\\b" \
    -e "crate::screens::\\{[^}]*\\b$g\\b" \
    -e "super::super::$g\\b" \
    --glob '*.rs' "$target" 2>/dev/null | sed "s/^/[$g] /")
  if [ -n "$found" ]; then
    printf '%s\n' "$found"
    bad=1
  fi
done
exit $bad
