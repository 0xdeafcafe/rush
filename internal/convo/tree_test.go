package convo

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestReadTree(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	root := t.TempDir()
	run := func(args ...string) {
		c := exec.Command("git", append([]string{"-C", root}, args...)...)
		c.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t")
		if out, err := c.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	write := func(p, s string) {
		_ = os.MkdirAll(filepath.Dir(filepath.Join(root, p)), 0o755)
		_ = os.WriteFile(filepath.Join(root, p), []byte(s), 0o644)
	}
	run("init", "-q")
	write("web/a.go", "a\n")
	write("old name.txt", "x\n")
	write("img.bin", "\x00\x01")
	run("add", ".")
	run("commit", "-qm", "init")
	run("mv", "old name.txt", "new name.txt")
	write("img.bin", "\x00\x02\x03")
	write("api/new.go", "n\n")
	tr := readTree(filepath.Join(root, "web"))
	if tr.Err != "" {
		t.Fatal(tr.Err)
	}
	got := map[string]TreeFile{}
	for _, f := range tr.Files {
		rel, _ := filepath.Rel(tr.Root, f.Path)
		got[rel] = f
	}
	if f, ok := got["api/new.go"]; !ok || !f.Untracked {
		t.Errorf("untracked outside the cwd's folder missing: %v", got)
	}
	if _, ok := got["new name.txt"]; !ok {
		t.Errorf("renamed file missing: %v", got)
	}
	if f := got["img.bin"]; !f.Binary {
		t.Errorf("binary not marked: %+v", f)
	}
	if nr := readTree(t.TempDir()); nr.Err == "" {
		t.Error("a folder outside git should say so")
	}
}

func TestTreeDiffFull(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	root := t.TempDir()
	run := func(args ...string) {
		c := exec.Command("git", append([]string{"-C", root}, args...)...)
		c.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t")
		if out, err := c.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	a, b := filepath.Join(root, "a.go"), filepath.Join(root, "b.go")
	_ = os.WriteFile(a, []byte("1\n2\n3\n4\n5\n6\n7\n8\n"), 0o644)
	_ = os.WriteFile(b, []byte("x\ny\n"), 0o644)
	run("init", "-q")
	run("add", ".")
	run("commit", "-qm", "init")
	_ = os.WriteFile(a, []byte("1\n2\n3\n4\nfive\n6\n7\n8\n"), 0o644)
	tr := readTree(root)
	// Read in the background: the first call says so.
	read := func(f TreeFile) string {
		for range 200 {
			if l := treeDiff(tr, f, true); len(l) != 1 || l[0] != "reading the diff…" {
				return strings.Join(l, "|")
			}
			time.Sleep(10 * time.Millisecond)
		}
		t.Fatal("never read")
		return ""
	}
	if got, want := read(TreeFile{Path: a}), "@@ -1,8 +1,8 @@| 1| 2| 3| 4|-5|+five| 6| 7| 8"; got != want {
		t.Errorf("every line, the diff laid over them:\n got %q\nwant %q", got, want)
	}
	if got := read(TreeFile{Path: b}); got != " x| y" {
		t.Errorf("no diff should be the file as it is: %q", got)
	}
}

// A changed file's turns, cut short once row would leave them out, draw
// the row as naming them all does.
func TestTurnsRightAsRow(t *testing.T) {
	counts := paint(cGreen, "+12") + " " + paint(cRed, "−3")
	for _, n := range []int{0, 1, 5, 20, 60, 400} {
		var turns []int
		var names []string
		for i := range n {
			turns = append(turns, i*7+1)
			names = append(names, fmt.Sprintf("#%d", i*7+1))
		}
		for _, w := range []int{20, 60, 124, 250} {
			want := row("", "  ▸ file.go", counts+"   "+dim(strings.Join(names, " ")), w, min(w, capRow))
			if got := row("", "  ▸ file.go", turnsRight(counts, turns, w, min(w, capRow)), w, min(w, capRow)); got != want {
				t.Errorf("%d turns at %d: %q, want %q", n, w, got, want)
			}
		}
	}
}
