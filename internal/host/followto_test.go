package host

import (
	"strings"
	"testing"
)

func TestFollowTo(t *testing.T) {
	root := func(d string) string {
		for _, r := range []string{"/repo/.worktrees/b", "/repo"} {
			if d == r || strings.HasPrefix(d, r+"/") {
				return r
			}
		}
		return ""
	}
	for _, c := range []struct {
		was, cwd, want string
		move           bool
	}{
		{"/repo", "/repo/modules/auth/process", "", false},            // a cd inside it
		{"/repo", "/repo/.worktrees/b/x", "/repo/.worktrees/b", true}, // into a worktree
		{"/repo/modules/auth/process", "/repo", "/repo", true},        // back up
		{"/repo", "/tmp/x", "", false},                                // out of git
		{"/repo", "/home/.claude/projects/repo/memory", "", false},    // its memory
		{"/home/notes", "/home", "/home", true},                       // up, out of git
		{"/repo", "/repo", "", false},
	} {
		got, move := followTo(c.was, c.cwd, root)
		if got != c.want || move != c.move {
			t.Errorf("followTo(%q, %q) = %q, %v; want %q, %v", c.was, c.cwd, got, move, c.want, c.move)
		}
	}
}
