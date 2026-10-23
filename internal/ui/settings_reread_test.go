package ui

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A settings file changed outside rush shows on the page within seconds,
// not only when it's next opened.
func TestSettingsFileReadAgainWhenItChanges(t *testing.T) {
	p := filepath.Join(t.TempDir(), "settings.json")
	if err := os.WriteFile(p, []byte(`{"model":"claude-sonnet-5"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	read := func() settingsLoad {
		for range 200 {
			if r, ok := settingsRead.get(p); ok {
				return r
			}
			time.Sleep(10 * time.Millisecond)
		}
		t.Fatal("never read")
		return settingsLoad{}
	}
	first := read()
	if first.file.String("model") != "claude-sonnet-5" {
		t.Fatalf("first read: %q", first.file.String("model"))
	}
	if err := os.WriteFile(p, []byte(`{"model":"opus[1m]"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	later := time.Now().Add(time.Minute)
	_ = os.Chtimes(p, later, later)
	deadline := time.Now().Add(6 * time.Second)
	for time.Now().Before(deadline) {
		if r := read(); !r.mod.Equal(first.mod) && r.file.String("model") == "opus[1m]" {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Fatal("the change never showed")
}
