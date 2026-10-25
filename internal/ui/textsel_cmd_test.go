package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// shellSession draws one opened Bash step that ran cmd and printed out, in
// a session at /home/me/src/wt, home at /home/me, w wide.
func shellSession(t *testing.T, cmd, out string, w int) []convo.Line {
	t.Helper()
	t.Setenv("HOME", "/home/me")
	s := convo.New()
	s.Info.Cwd = "/home/me/src/wt"
	now := time.Now()
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	res, _ := jsonx.Marshal(map[string]string{"stdout": out, "stderr": ""})
	s.Apply(host.Sent{Text: "check"}, now)
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "Bash", Input: in}}}, now)
	s.Apply(headless.Message{Role: "user", ToolResult: res, Blocks: []headless.Block{{Type: "tool_result", ToolUseID: "b1", Text: out}}}, now)
	s.Apply(headless.Result{Subtype: "success"}, now)
	return s.Render(convo.Options{Width: w, Now: now, Open: map[string]bool{"t1": true, "t1:s:b1": true}})
}

// rowsWith is the first and last rows whose text has from and to.
func rowsWith(t *testing.T, lines []convo.Line, from, to string) (int, int) {
	t.Helper()
	a, b := -1, -1
	for i, l := range lines {
		p := ansi.Strip(l.Text)
		if a < 0 && strings.Contains(p, from) {
			a = i
		}
		if a >= 0 && strings.Contains(p, to) {
			b = i
		}
	}
	if a < 0 || b < a {
		t.Fatalf("rows %q…%q not drawn:\n%s", from, to, dump(lines))
	}
	return a, b
}

const wrappedCmd = "cd /home/me/src/saas && git fetch -q origin && git grep -n \"EVAL_V3\" origin/main -- . | head\n" +
	"git log -1 --format='%h %cd' origin/main\n" +
	"cd /home/me/src/wt && sed -n 50,90p /home/me/src/wt/modules/evaluations/server/services/experiment-workflow-evaluation.service.ts"

// A copy of the whole command block is the command as the agent sent it,
// byte for byte: no $, no shortened paths, no breaks from wrapping.
func TestCopyShellBlockIsTheCommand(t *testing.T) {
	const w = 80
	lines := shellSession(t, wrappedCmd, "5d5a1b0 Fri\n", w)
	a, b := rowsWith(t, lines, "$  cd ~/src/saas", "experiment-workflow")
	if got := selectedText(lines, cell{row: a, col: 0}, cell{row: b, col: w - 1}, w); got != wrappedCmd {
		t.Errorf("got\n%q\nwant\n%q\ndrawn\n%s", got, wrappedCmd, dump(lines))
	}
}

// Part of one wrapped command, even one of the rows it wraps to, copies
// that command as written.
func TestCopyPartOfWrappedCommand(t *testing.T) {
	const w = 80
	lines := shellSession(t, wrappedCmd, "5d5a1b0 Fri\n", w)
	_, b := rowsWith(t, lines, "$  cd ~/src/saas", "experiment-workflow")
	if !lines[b].Wrap {
		t.Fatalf("the sed should wrap at %d:\n%s", w, dump(lines))
	}
	want := "sed -n 50,90p /home/me/src/wt/modules/evaluations/server/services/experiment-workflow-evaluation.service.ts"
	if got := selectedText(lines, cell{row: b, col: 12}, cell{row: b, col: 30}, w); got != want {
		t.Errorf("got %q\nwant %q", got, want)
	}
	// From the git log down: from it to the end, the newline and cd too.
	a, _ := rowsWith(t, lines, "git log -1", "git log -1")
	want = "git log -1 --format='%h %cd' origin/main\ncd /home/me/src/wt && " + want
	if got := selectedText(lines, cell{row: a, col: 0}, cell{row: b, col: w - 1}, w); got != want {
		t.Errorf("got %q\nwant %q", got, want)
	}
}

// A cd shown shortened is copied as written, and what the command printed
// follows it as it always did.
func TestCopyShortenedCdWithOutput(t *testing.T) {
	const w = 100
	cmd := "cd /home/me/src/saas && git status --short\ncd /home/me/src/wt/sub && ls /home/me/src/wt/sub/x"
	lines := shellSession(t, cmd, "M a.go\nx\n", w)
	a, _ := rowsWith(t, lines, "cd ~/src/saas", "cd ~/src/saas")
	_, b := rowsWith(t, lines, "M a.go", "x")
	shown := ansi.Strip(lines[a+2].Text) + ansi.Strip(lines[a+3].Text)
	if !strings.Contains(shown, "cd ../wt/sub") || !strings.Contains(shown, "ls x") {
		t.Fatalf("the paths should show shortened:\n%s", dump(lines))
	}
	if got, want := selectedText(lines, cell{row: a, col: 0}, cell{row: b, col: w - 1}, w), cmd+"\nM a.go\nx"; got != want {
		t.Errorf("got\n%q\nwant\n%q\ndrawn\n%s", got, want, dump(lines))
	}
}

// A long line of output wraps onto a dotted rail; a copy joins it again
// without the rail.
func TestCopyWrappedOutputDropsItsRail(t *testing.T) {
	const w = 60
	long := strings.TrimSpace(strings.Repeat("word ", 20))
	lines := shellSession(t, "./check.sh", long+"\n", w)
	a, b := rowsWith(t, lines, "▏word", "┊")
	if got := selectedText(lines, cell{row: a, col: 0}, cell{row: b, col: w - 1}, w); got != long {
		t.Errorf("got %q\nwant %q\ndrawn\n%s", got, long, dump(lines))
	}
}
