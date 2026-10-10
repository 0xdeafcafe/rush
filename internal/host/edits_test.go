package host

import (
	"os"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
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

func TestWorkedIn(t *testing.T) {
	home, _ := os.UserHomeDir()
	s := &server{cfg: Config{Cwd: "/lw"}, startCwd: "/rush"}
	for _, c := range []struct {
		call *tool.Call
		want string
	}{
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "cd /lw/.worktrees/ds && pnpm test"}}, "/lw/.worktrees/ds"},
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: `cd "/lw/a b"; ls`}}, "/lw/a b"},
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "cd ../lw&&ls"}}, "/lw"}, // from where the shell was put back
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "pushd ~/x >/dev/null"}}, home + "/x"},
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "git status"}}, ""},
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "ls", Cwd: "/lw/m"}}, "/lw/m"},
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "cd - && ls"}}, ""},
		{&tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "echo cd /x"}}, ""},
		{&tool.Call{Kind: tool.Edit, Input: tool.Input{Path: "/lw/src/a.ts"}}, "/lw/src"},
		{&tool.Call{Kind: tool.Read, Input: tool.Input{Path: "/lw/src/a.ts"}}, ""},
		{nil, ""},
	} {
		if got := s.workedIn(c.call); got != c.want {
			t.Errorf("%+v: got %q, want %q", c.call, got, c.want)
		}
	}
}
