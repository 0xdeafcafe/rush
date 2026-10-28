package claude

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestWriteNoteAndIndex(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "memory")
	p, err := WriteNote(dir, Note{Name: "Targeted Tests", Title: "Targeted tests", Description: "run: only what changed", Type: "feedback", Body: "Run only the tests you touched."})
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(p) != "targeted-tests.md" {
		t.Fatalf("path %s", p)
	}
	fm := FrontMatter(p)
	if fm["name"] != "targeted-tests" || fm["type"] != "feedback" || fm["description"] != "run: only what changed" {
		t.Fatalf("frontmatter %v", fm)
	}
	// Writing it again keeps one line.
	if _, err := WriteNote(dir, Note{Name: "targeted-tests", Description: "x", Type: "bogus", Body: "again"}); err != nil {
		t.Fatal(err)
	}
	ix, _ := os.ReadFile(filepath.Join(dir, "MEMORY.md"))
	if strings.Count(string(ix), "targeted-tests.md") != 1 || !strings.Contains(string(ix), "- [Targeted tests](targeted-tests.md) — run: only what changed") {
		t.Fatalf("index %q", ix)
	}
	if FrontMatter(p)["type"] != "project" {
		t.Fatal("an unknown type should fall back to project")
	}
}

func TestFixMemory(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "MEMORY.md"), []byte("- [Gone](gone.md) — x\n- [Here](here.md) — y\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "here.md"), []byte("---\nname: here\ndescription: y\n---\nhi\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "orphan.md"), []byte("---\nname: orphan\ndescription: lost\n---\nhi\n"), 0o644)
	did, err := FixMemory(dir)
	if err != nil || len(did) != 2 {
		t.Fatalf("did %v err %v", did, err)
	}
	ix, _ := os.ReadFile(filepath.Join(dir, "MEMORY.md"))
	if strings.Contains(string(ix), "gone.md") || !strings.Contains(string(ix), "(orphan.md) — lost") || !strings.Contains(string(ix), "here.md") {
		t.Fatalf("index %q", ix)
	}
	if did, _ := FixMemory(dir); len(did) != 0 {
		t.Fatalf("second fix did %v", did)
	}
}

func TestMergeAndGlobal(t *testing.T) {
	cfg := t.TempDir()
	dir := filepath.Join(cfg, "projects", "x", "memory")
	a, _ := WriteNote(dir, Note{Name: "a", Description: "first", Type: "feedback", Body: "A text"})
	b, _ := WriteNote(dir, Note{Name: "b", Description: "second", Type: "feedback", Body: "B text"})
	if err := MergeNotes(a, b); err != nil {
		t.Fatal(err)
	}
	got, _ := os.ReadFile(a)
	if !strings.Contains(string(got), "A text\n\nB text") {
		t.Fatalf("merged %q", got)
	}
	if _, err := os.Stat(b); err == nil {
		t.Fatal("b should be gone")
	}
	if err := MakeGlobal(cfg, a); err != nil {
		t.Fatal(err)
	}
	md, _ := os.ReadFile(filepath.Join(cfg, "CLAUDE.md"))
	if !strings.Contains(string(md), "## first\n\nA text") {
		t.Fatalf("CLAUDE.md %q", md)
	}
	ix, _ := os.ReadFile(filepath.Join(dir, "MEMORY.md"))
	if strings.TrimSpace(string(ix)) != "" {
		t.Fatalf("index left %q", ix)
	}
}

func TestNoteUses(t *testing.T) {
	projects := t.TempDir()
	slug := "-repo"
	dir := filepath.Join(projects, slug, "memory")
	read := func(p string) string {
		return `{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"` + p + `"}}]}}` + "\n"
	}
	os.MkdirAll(filepath.Join(projects, slug+"--claude-worktrees-1"), 0o755)
	os.MkdirAll(filepath.Join(projects, slug+"-other"), 0o755)
	os.MkdirAll(dir, 0o755)
	// Read through another account's folder, from a worktree's session.
	os.WriteFile(filepath.Join(projects, slug+"--claude-worktrees-1", "s.jsonl"),
		[]byte(read("/acct2/projects/-repo/memory/a.md")+read("/acct2/projects/-repo/memory/a.md")), 0o644)
	os.WriteFile(filepath.Join(projects, slug+"-other", "s.jsonl"), []byte(read("/x/projects/-repo/memory/a.md")), 0o644)
	f := filepath.Join(projects, slug, "s.jsonl")
	bash := func(cmd string) string {
		return `{"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"` + cmd + `"}}]}}` + "\n"
	}
	os.WriteFile(f, []byte(read("/x/projects/-repo/memory/b.md")+bash("cat ~/x/projects/-repo/memory/c.md; ls")+bash("rm /x/projects/-repo/memory/c.md")), 0o644)
	u := NoteUses(projects, slug, dir, 24*time.Hour)
	if u[filepath.Join(dir, "a.md")].Reads != 2 || u[filepath.Join(dir, "b.md")].Reads != 1 {
		t.Fatalf("uses %v", u)
	}
	if c := u[filepath.Join(dir, "c.md")]; c.Reads != 1 || c.Last.Day() != 2 {
		t.Fatalf("c %v: a cat reads it, an rm doesn't", c)
	}
	// More of the transcript is read from where it stopped.
	fh, _ := os.OpenFile(f, os.O_APPEND|os.O_WRONLY, 0)
	fh.WriteString(read("/x/projects/-repo/memory/b.md"))
	fh.Close()
	if n := NoteUses(projects, slug, dir, 24*time.Hour)[filepath.Join(dir, "b.md")].Reads; n != 2 {
		t.Fatalf("b reads %d", n)
	}
}
