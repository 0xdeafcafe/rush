package claude

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// A home links to everything of ~/.claude's but the sign-in and state
// file, makes the folders sessions write in ~/.claude first so they land
// there, and leaves alone what the home has of its own.
func TestLinkHome(t *testing.T) {
	root := Account{ConfigDir: filepath.Join(t.TempDir(), ".claude")}
	home := Account{ConfigDir: filepath.Join(t.TempDir(), "home")}
	for _, f := range []string{"settings.json", ".credentials.json", ".claude.json"} {
		if err := os.MkdirAll(root.ConfigDir, 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(root.ConfigDir, f), []byte("{}"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.MkdirAll(home.ConfigDir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home.ConfigDir, "CLAUDE.md"), []byte("own"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root.ConfigDir, "CLAUDE.md"), []byte("root's"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := LinkHome(home, root); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"settings.json", "projects", "todos"} {
		to, err := os.Readlink(filepath.Join(home.ConfigDir, name))
		if err != nil || to != filepath.Join(root.ConfigDir, name) {
			t.Errorf("%s should link to ~/.claude's: %q, %v", name, to, err)
		}
	}
	for _, name := range []string{".credentials.json", ".claude.json"} {
		if _, err := os.Lstat(filepath.Join(home.ConfigDir, name)); err == nil {
			t.Errorf("%s is the home's own, not a link", name)
		}
	}
	if b, _ := os.ReadFile(filepath.Join(home.ConfigDir, "CLAUDE.md")); string(b) != "own" {
		t.Errorf("the home's own CLAUDE.md was replaced: %q", b)
	}
	// A session in the home writes its transcript into ~/.claude.
	if err := os.WriteFile(filepath.Join(home.ConfigDir, "projects", "t.jsonl"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(root.ConfigDir, "projects", "t.jsonl")); err != nil {
		t.Error("a transcript written in the home isn't in ~/.claude")
	}
	// Its state file keeps up with ~/.claude's MCP servers, and keeps
	// who it's signed in as.
	if err := os.WriteFile(home.StatePath(), []byte(`{"oauthAccount":{"accountUuid":"me"}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(root.StatePath(), []byte(`{"oauthAccount":{"accountUuid":"other"},"mcpServers":{"x":{}}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	// Linking again, once ~/.claude has something new, adds it.
	if err := os.WriteFile(filepath.Join(root.ConfigDir, "keybindings.json"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := LinkHome(home, root); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Readlink(filepath.Join(home.ConfigDir, "keybindings.json")); err != nil {
		t.Error("what ~/.claude gains isn't linked the next time")
	}
	if b, _ := os.ReadFile(home.StatePath()); !strings.Contains(string(b), `"x"`) || SignedInAs(home) != "me" {
		t.Errorf("the home's state file should have ~/.claude's MCP servers and still be me: %s", b)
	}
}

// A home whose projects folder is a real one, made before it was linked,
// gives its transcripts to ~/.claude and becomes the link. A transcript in
// both keeps all of both: the longer when one starts with the other, else
// the home's carried on after ~/.claude's. A file still open by its old
// path is the same file as ~/.claude's.
func TestLinkHomeAdoptsStray(t *testing.T) {
	root := Account{ConfigDir: filepath.Join(t.TempDir(), ".claude")}
	home := Account{ConfigDir: filepath.Join(t.TempDir(), "home")}
	write := func(path, body string, age time.Duration) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
		at := time.Now().Add(-age)
		if err := os.Chtimes(path, at, at); err != nil {
			t.Fatal(err)
		}
	}
	write(filepath.Join(root.ConfigDir, "projects", "p", "moved.jsonl"), "old", time.Hour)
	write(filepath.Join(root.ConfigDir, "projects", "p", "kept.jsonl"), "a\nb\n", 0)
	write(filepath.Join(root.ConfigDir, "projects", "p", "carried.jsonl"), "before\n", time.Hour)
	write(filepath.Join(home.ConfigDir, "projects", "p", "moved.jsonl"), "old+more", time.Minute)
	write(filepath.Join(home.ConfigDir, "projects", "p", "kept.jsonl"), "a\n", time.Hour)
	write(filepath.Join(home.ConfigDir, "projects", "p", "carried.jsonl"), "after\n", 0)
	write(filepath.Join(home.ConfigDir, "projects", "p", "new.jsonl"), "only here", 0)
	open, err := os.OpenFile(filepath.Join(home.ConfigDir, "projects", "p", "new.jsonl"), os.O_APPEND|os.O_WRONLY, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer open.Close()

	if err := LinkHome(home, root); err != nil {
		t.Fatal(err)
	}
	if to, err := os.Readlink(filepath.Join(home.ConfigDir, "projects")); err != nil || to != filepath.Join(root.ConfigDir, "projects") {
		t.Fatalf("projects should now link to ~/.claude's: %q, %v", to, err)
	}
	if _, err := open.WriteString(", still"); err != nil {
		t.Fatal(err)
	}
	for name, want := range map[string]string{
		"moved.jsonl": "old+more", "kept.jsonl": "a\nb\n", "carried.jsonl": "before\nafter\n", "new.jsonl": "only here, still",
	} {
		if b, _ := os.ReadFile(filepath.Join(root.ConfigDir, "projects", "p", name)); string(b) != want {
			t.Errorf("%s in ~/.claude: %q, want %q", name, b, want)
		}
	}
	if _, err := os.Lstat(filepath.Join(home.ConfigDir, "projects.adopted")); err == nil {
		t.Error("the adopted folder was left behind")
	}
}
