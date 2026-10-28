package memory

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
)

func TestDraft(t *testing.T) {
	old := Asker
	defer func() { Asker = old }()
	var prompt string
	Asker = func(_ context.Context, _, p string) (string, error) {
		prompt = p
		return "Here:\n```json\n{\"name\":\"no-mocks\",\"title\":\"No mocks\",\"description\":\"tests hit a real db\",\"type\":\"feedback\",\"body\":\"Use the real db.\"}\n```", nil
	}
	n, err := Draft(context.Background(), "never mock the db", "")
	if err != nil || n.Name != "no-mocks" || n.Type != "feedback" {
		t.Fatalf("%+v %v", n, err)
	}
	if !strings.Contains(prompt, "never mock the db") || !strings.Contains(prompt, "Today is") {
		t.Fatalf("prompt %q", prompt)
	}
	Asker = func(context.Context, string, string) (string, error) { return "sorry", nil }
	if _, err := Draft(context.Background(), "x", ""); err == nil {
		t.Fatal("an answer that isn't a note should fail")
	}
	if _, err := Draft(context.Background(), " ", ""); err == nil {
		t.Fatal("nothing to remember should fail")
	}
}

func TestWhy(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Now()
	note := func(kind string, age time.Duration, warn string) agent.MemoryFile {
		return agent.MemoryFile{Group: "Auto memory", Name: "n", Path: filepath.Join(t.TempDir(), "n.md"), Kind: kind, Mod: now.Add(-age), Warn: warn}
	}
	if Why(note("feedback", 400*24*time.Hour, ""), now) != "" {
		t.Error("how to work doesn't go out of date by age")
	}
	if !strings.Contains(Why(note("project", 90*24*time.Hour, ""), now), "still true") {
		t.Error("an old project note is worth a look")
	}
	if Why(note("project", 10*24*time.Hour, ""), now) != "" {
		t.Error("a recent project note isn't")
	}
	f := note("user", time.Hour, "repeats x")
	if Why(f, now) != "repeats x" {
		t.Error("what's untidy is why")
	}
	os.WriteFile(f.Path, []byte("x"), 0o644)
	os.Chtimes(f.Path, f.Mod, f.Mod)
	if err := Keep(f.Path); err != nil {
		t.Fatal(err)
	}
	if Why(f, now) != "" {
		t.Error("a kept note isn't asked about again")
	}
	f.Mod = f.Mod.Add(time.Minute)
	if Why(f, now) == "" {
		t.Error("until it changes")
	}
}

func TestParseDraftAround(t *testing.T) {
	n, err := parseDraft(`Sure! {"name":"a","title":"A","description":"d","type":"opinion","body":"uses {x}"} Hope that helps {:}`)
	if err != nil || n.Body != "uses {x}" || n.Type != "project" {
		t.Fatalf("%+v %v", n, err)
	}
}
