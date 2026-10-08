package convo

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/charmbracelet/x/ansi"
)

const watcher = "cd /Users/lw/Source/github.com/langwatch/langwatch\nL=.claude/tmp/runs/r14/r14.log\nlast=0\nstill=0\n" +
	"while :; do if grep -qE \"^EXIT|stopping all|stop-all|exit 3\" $L; then echo EXITED; break; fi; n=$(wc -c < $L); " +
	"if [ \"$n\" = \"$last\" ]; then still=$((still+1)); else still=0; last=$n; fi; [ $still -ge 12 ] && { echo \"STALL 3m\"; break; }; " +
	"perl -e 'select(undef,undef,undef,15)'; done\ntail -25 $L"

func scriptRows(d *drawer) []string {
	var got []string
	for _, l := range d.lines {
		got = append(got, strings.TrimRight(ansi.Strip(l.Text), " "))
	}
	return got
}

// A script with a loop in it reads as one block, indented by how deep each
// statement is, with no time or mark beside each statement; Claude Code's
// notice that it runs in the background is a short note.
func TestScriptBlock(t *testing.T) {
	s := New()
	s.Info.Cwd = "/Users/lw/Source/github.com/langwatch/langwatch"
	now := time.Now()
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Now: now, Open: map[string]bool{}}, cw: 120}
	in, _ := jsonx.Marshal(map[string]string{"command": watcher})
	st := &Step{Tool: "Bash", Input: in, Status: OK, End: now,
		Output: "Command running in background with ID: b73s0uobp. Output is being written to: /tmp/tasks/b73s0uobp.output. " +
			"You will be notified when it completes. To check interim output, use Read on that file path.\n" +
			"Session cwd remains /Users/lw/Source/github.com/langwatch/langwatch",
		parts: map[int]*partRun{0: {start: now.Add(-2 * time.Second), end: now.Add(-time.Second)}, 5: {start: now.Add(-time.Second)}}}
	d.body(st, 4)
	all := strings.Join(scriptRows(d), "\n")
	want := strings.Join([]string{
		"    $ cd /Users/lw/Source/github.com/langwatch/langwatch",
		"      L=.claude/tmp/runs/r14/r14.log",
		"      last=0",
		"      still=0",
		"      while :; do",
		"        if grep -qE \"^EXIT|stopping all|stop-all|exit 3\" $L; then",
		"          echo EXITED",
		"          break",
		"        fi",
		"        n=$(wc -c < $L)",
		"        if [ \"$n\" = \"$last\" ]; then",
		"          still=$((still+1))",
		"        else",
		"          still=0",
		"          last=$n",
		"        fi",
		"        [ $still -ge 12 ] && {",
		"          echo \"STALL 3m\"",
		"          break",
		"        }",
		"        perl -e 'select(undef,undef,undef,15)'",
		"      done",
		"      tail -25 $L",
		"    ↳ running in the background · b73s0uobp",
		"      /tmp/tasks/b73s0uobp.output",
	}, "\n")
	if all != want {
		t.Errorf("got\n%s\nwant\n%s", all, want)
	}
	for _, not := range []string{"✓", "▸", "Command running", "Session cwd"} {
		if strings.Contains(all, not) {
			t.Errorf("shouldn't have %q:\n%s", not, all)
		}
	}
	// A folder other than the session's is said, briefly.
	d = &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}}, cw: 120}
	d.output("Session cwd remains /elsewhere", 4, false)
	if got := strings.Join(scriptRows(d), "\n"); got != "    cwd /elsewhere" {
		t.Errorf("cwd note = %q", got)
	}
}

// A long line of output wraps between words, each row on the rail at the
// same column and none with a rail inside its text.
func TestOutputWrapsOnWords(t *testing.T) {
	d := &drawer{s: New(), t: &Turn{}, o: Options{Width: 60, Verbose: true, Open: map[string]bool{}}, cw: 60}
	line := strings.Repeat("words that go on and on ", 8) + "-Users-lw-Source-github-com-langwatch-langwatch-and-more-of-it"
	d.output(line, 4, false)
	rows := scriptRows(d)
	if len(rows) < 3 {
		t.Fatalf("should wrap: %q", rows)
	}
	var text []string
	for _, r := range rows {
		if strings.Count(r, "▏") != 1 || strings.Index(r, "▏") != strings.Index(rows[0], "▏") {
			t.Errorf("row should have one rail at the rail's column: %q", r)
		}
		_, t, _ := strings.Cut(r, "▏")
		text = append(text, t)
	}
	joined := ""
	for _, t := range text {
		if joined != "" && !strings.HasSuffix(joined, "-") {
			joined += " "
		}
		joined += t
	}
	if joined != strings.TrimSpace(line) {
		t.Errorf("rows should break between words:\n%q\n%q", joined, line)
	}
}
