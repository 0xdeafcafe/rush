package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// openShell draws an opened Bash step at width w in a session at cwd, home
// at /home/me, and gives its rows as plain text.
func openShell(t *testing.T, cwd, cmd, out string, w int) []string {
	t.Helper()
	t.Setenv("HOME", "/home/me")
	s := New()
	s.Info.Cwd = cwd
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: w, Verbose: true, Open: map[string]bool{}}, cw: w}
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var rows []string
	for _, l := range d.lines {
		rows = append(rows, strings.TrimRight(stripANSI(l.Text), " "))
	}
	return rows
}

// Every command of a chain starts in one column after a $ or the && that
// joins it, whether a newline or && parts it from the last; paths read
// relative to where the command is at that point, or from ~; and a rule
// parts the command from what it printed.
func TestShellBodyLayout(t *testing.T) {
	cmd := "cd /home/me/src/saas && git fetch -q origin && git grep -n X origin/main -- . | head\n" +
		"git log -1 origin/main\n" +
		"cd /home/me/src/wt && sed -n 50,90p /home/me/src/wt/modules/a.ts"
	rows := openShell(t, "/home/me/src/wt", cmd, "5d5a1b0 Fri\norigin/main:modules/a.ts:17:  run: 4", 100)
	want := []string{
		"    $  cd ~/src/saas",
		"    && git fetch -q origin",
		"    && git grep -n X origin/main -- . | head",
		"    $  git log -1 origin/main",
		"    $  cd ../wt",
		"    && sed -n 50,90p modules/a.ts",
		"    ────────────────────────────────────────",
		"    ▏5d5a1b0 Fri",
		"    ▏origin/main:modules/a.ts:17:  run: 4",
	}
	if got := strings.Join(rows, "\n"); got != strings.Join(want, "\n") {
		t.Errorf("got\n%s\nwant\n%s", got, strings.Join(want, "\n"))
	}
}

// A command too long for its row breaks between words, never inside a
// path, the rows it carries on to hanging under its start.
func TestShellBodyWrapsOnWords(t *testing.T) {
	long := "/opt/x/modules/evaluations/server/services/experiment-workflow-evaluation.service.ts"
	rows := openShell(t, "/work", "go build ./... && sed -n 50,90p "+long, "", 60)
	want := []string{
		"    $  go build ./...",
		"    && sed -n 50,90p",
		"         /opt/x/modules/evaluations/server/services/",
		"         experiment-workflow-evaluation.service.ts",
	}
	if got := strings.Join(rows[:len(want)], "\n"); got != strings.Join(want, "\n") {
		t.Errorf("got\n%s\nwant\n%s", got, strings.Join(want, "\n"))
	}
}

// A cd inside a subshell leaves where later words are unknown: their
// paths are only shortened to ~.
func TestShortPathsSubshell(t *testing.T) {
	cur := cwdAt{dir: "/home/me/wt"}
	if got := shortPaths("(cd /tmp && cat /home/me/wt/a.go)", "/home/me", &cur, true); got != "(cd /tmp && cat ~/wt/a.go)" {
		t.Errorf("got %q", got)
	}
	if cur.dir != "" {
		t.Errorf("cur should be unknown after a subshell's cd: %+v", cur)
	}
	cur = cwdAt{dir: "/home/me/wt"}
	if got := shortPaths("cat /home/me/wt/a.go /home/me/wt", "/home/me", &cur, true); got != "cat a.go ." {
		t.Errorf("got %q", got)
	}
	// A cd back to the session's folder says where, not ".".
	if got := shortPaths("cd /home/me/wt", "/home/me", &cur, true); got != "cd ~/wt" {
		t.Errorf("got %q", got)
	}
}

// Output rows a long line wraps to hang two cells in, and a grep match's
// place stays in front of what it found.
func TestShellOutputHangs(t *testing.T) {
	out := "x.go:12: " + strings.Repeat("word ", 20)
	rows := openShell(t, "/work", "./check.sh", out, 60)
	var body []string
	for _, r := range rows {
		if _, s, ok := strings.Cut(r, "▏"); ok {
			body = append(body, s)
		} else if _, s, ok := strings.Cut(r, "┊"); ok {
			body = append(body, s) // the dotted rail of a row it wrapped to
		}
	}
	if len(body) < 2 || !strings.HasPrefix(body[0], "x.go:12:") || !strings.HasPrefix(body[1], "          word") {
		t.Errorf("rows %q", body)
	}
}
