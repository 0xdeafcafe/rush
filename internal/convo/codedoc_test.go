package convo

import (
	"fmt"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestStdinFault(t *testing.T) {
	for out, want := range map[string]int{
		"Traceback (most recent call last):\n  File \"<stdin>\", line 3, in <module>\n  File \"<stdin>\", line 9, in f\nKeyError: 'x'": 9,
		"-:4:in `<main>': undefined method":  4,
		"Died at - line 7.":                  7,
		"[stdin]:2\n  throw new Error('no')": 2,
		"exit status 1":                      0,
	} {
		if got, _ := stdinFault(out); got != want {
			t.Errorf("%q: line %d, want %d", out, got, want)
		}
	}
	if _, says := stdinFault("  File \"<stdin>\", line 1\nKeyError: 'x'\n"); says != "KeyError: 'x'" {
		t.Errorf("says %q", says)
	}
}

// A long script fed on stdin that failed keeps its start, the lines round
// where it stopped and its end word in view, numbered as Python counts.
func TestScriptFaultInView(t *testing.T) {
	var src []string
	for i := 1; i <= 20; i++ {
		src = append(src, fmt.Sprintf("x%d = %d", i, i))
	}
	cmd := "python3 - <<'EOF'\n" + strings.Join(src, "\n") + "\nEOF"
	tb := "Traceback (most recent call last):\n  File \"<stdin>\", line 15, in <module>\nNameError: name 'y' is not defined"
	s := New()
	s.Apply(host.Sent{Text: "run it"}, at(0))
	s.Apply(toolUse("a", "Bash", map[string]any{"command": cmd}), at(1))
	s.Apply(toolResult("a", "Exit code 1\n"+tb, true, map[string]any{"stdout": "", "stderr": tb}), at(2))
	s.Apply(headless.Result{Subtype: "success"}, at(3))
	ref := ""
	for _, l := range s.Render(Options{Width: 110, Now: at(4)}) {
		if strings.Contains(l.Ref, ":s:a") {
			ref = l.Ref
			break
		}
	}
	out := plain(s.Render(Options{Width: 110, Now: at(4), Open: map[string]bool{ref: true}}))
	for _, w := range []string{" 6  x6 = 6", "… 7 lines", "14  x14 = 14", "15▸ x15 = 15", "✗ NameError: name 'y' is not defined", "16  x16 = 16", "… 4 lines", "EOF"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if strings.Contains(out, "x10 = 10") || strings.Contains(out, "x18 = 18") {
		t.Errorf("the lines between should fold:\n%s", out)
	}
}
