package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A diff cut short by head, from a command rush can't tell prints one,
// and followed by grep's hits, still reads as a diff: its lines added and
// removed, highlighted as TypeScript, and the hits after it left as they are.
func TestDiffFoundInOutput(t *testing.T) {
	s := New()
	s.Info.Cwd = "/w"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}}, cw: 120}
	cmd := `git -C /w diff main -- src/a.ts | head -50; grep -n Scenario specs/a.feature | head`
	out := "diff --git a/src/a.ts b/src/a.ts\nindex 1..2 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,40 +1,40 @@\n" +
		"-import { a } from \"x\";\n+import { a, b } from \"x\";\n const y = 1;\n248:  Scenario: foo\n"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var raw, plain []string
	for _, l := range d.lines {
		raw, plain = append(raw, l.Text), append(plain, stripANSI(l.Text))
	}
	all, txt := strings.Join(raw, "\n"), strings.Join(plain, "\n")
	for _, want := range []string{plusSign(), minusSign(), hlKw + "import", hlKw + "const", paint(cText+bold, "a.ts")} {
		if !strings.Contains(all, want) {
			t.Errorf("output should have %q:\n%q", want, all)
		}
	}
	if strings.Contains(all, paint(cOut, `-import { a } from "x";`)) {
		t.Errorf("the removed line shouldn't be plain output:\n%q", all)
	}
	for _, not := range []string{"+++ b/src/a.ts", "@@"} {
		if strings.Contains(txt, not) {
			t.Errorf("output shouldn't have %q:\n%s", not, txt)
		}
	}
	if !strings.Contains(txt, "248:  Scenario: foo") {
		t.Errorf("grep's hit after the diff should stay:\n%s", txt)
	}
}
