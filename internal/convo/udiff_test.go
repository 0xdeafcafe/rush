package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// git log -p: each commit's header is its own, each file's lines are
// numbered from its hunks, and a removed line pairs with the added one.
func TestParseDiff(t *testing.T) {
	out := "commit 0f73310319a2fee6cb46bc0adc5faa56d6ee12de\n" +
		"Author: A <a@b>\n\n    fix: x\n\n" +
		"diff --git a/old.go b/new.go\nsimilarity index 90%\nrename from old.go\nrename to new.go\n" +
		"index 1..2 100644\n--- a/old.go\n+++ b/new.go\n@@ -10,3 +10,3 @@ func f() {\n" +
		" \ta := 1\n-\treturn a\n+\treturn a + 1\n }\n" +
		"commit 1111111111111111111111111111111111111111\n"
	lines := strings.Split(out, "\n")
	u := parseDiff(lines, func(int) bool { return true })
	if u == nil {
		t.Fatal("no diff found")
	}
	var kinds strings.Builder
	for _, x := range u.ls {
		if x.kind == 0 {
			x.kind = '.'
		}
		kinds.WriteByte(x.kind)
	}
	if want := ".....fssssss" + "h" + " -+ " + ".."; kinds.String() != want {
		t.Fatalf("kinds %q, want %q", kinds.String(), want)
	}
	f := u.ls[5]
	if f.path != "new.go" || f.note != "renamed from old.go" || f.add != 1 || f.del != 1 {
		t.Errorf("file header %+v", f)
	}
	if u.ls[13].n != 10 || u.ls[14].n != 11 || u.ls[15].n != 11 || u.ls[16].n != 12 {
		t.Errorf("line numbers %d %d %d %d", u.ls[13].n, u.ls[14].n, u.ls[15].n, u.ls[16].n)
	}
	if u.ls[14].pair != 15 || u.ls[15].pair != 14 {
		t.Errorf("the removed and added lines should pair: %d %d", u.ls[14].pair, u.ls[15].pair)
	}
	if u.ls[17].kind != 0 || u.ls[17].file != -1 {
		t.Errorf("the next commit isn't the diff's: %+v", u.ls[17])
	}
}

// A git diff in a step draws each file's header and its lines numbered,
// with no raw headers left.
func TestGitDiffOutput(t *testing.T) {
	s := New()
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}}, cw: 120}
	in, _ := jsonx.Marshal(map[string]string{"command": "git diff"})
	out := "diff --git a/a.go b/a.go\nindex 1..2 100644\n--- a/a.go\n+++ b/a.go\n@@ -1,2 +1,2 @@\n" +
		" package a\n-var x = 1\n+var x = 2\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	got := make([]string, 0, len(d.lines))
	for _, l := range d.lines {
		got = append(got, stripANSI(l.Text))
	}
	all := strings.Join(got, "\n")
	for _, want := range []string{"a.go  +1 −1", "    1   package a", "    2 − var x = 1", "    2 + var x = 2"} {
		if !strings.Contains(all, want) {
			t.Errorf("output should have %q:\n%s", want, all)
		}
	}
	for _, not := range []string{"index 1..2", "+++ b/a.go", "@@"} {
		if strings.Contains(all, not) {
			t.Errorf("output shouldn't have %q:\n%s", not, all)
		}
	}
}
