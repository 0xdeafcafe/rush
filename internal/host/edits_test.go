package host

import (
	"strings"
	"testing"
)

func TestEditedIn(t *testing.T) {
	root := func(d string) string {
		for _, r := range []string{"/lw/.worktrees/ds", "/lw", "/rush"} {
			if d == r || strings.HasPrefix(d, r+"/") {
				return r
			}
		}
		return ""
	}
	for _, c := range []struct {
		dirs []string
		want string
		move bool
	}{
		{[]string{"/lw/a", "/lw/b", "/lw"}, "/lw", true},                                                           // another checkout
		{[]string{"/lw/.worktrees/ds/a", "/lw/.worktrees/ds/b/c", "/lw/.worktrees/ds"}, "/lw/.worktrees/ds", true}, // its worktree
		{[]string{"/lw/a", "/rush/b", "/lw/a"}, "", false},                                                         // back home between
		{[]string{"/lw/a", "/lw/.worktrees/ds/a", "/lw/a"}, "", false},                                             // two checkouts
		{[]string{"/rush/a", "/rush/b", "/rush"}, "", false},                                                       // its own
		{[]string{"/home/.claude/memory", "/lw/a", "/lw/b"}, "", false},                                            // no checkout
	} {
		got, move := editedIn("/rush/sub", c.dirs, root)
		if got != c.want || move != c.move {
			t.Errorf("%v: got %q %v, want %q %v", c.dirs, got, move, c.want, c.move)
		}
	}
}
