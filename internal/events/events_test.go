package events

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLogWritesRushAndSessionLogs(t *testing.T) {
	home := t.TempDir()
	t.Setenv("RUSH_HOME", home)
	SetProcess("test")
	sess := filepath.Join(home, "s1.jsonl")
	Log("restarted", map[string]any{"why": "stale"})
	Session("s1", sess, "check-in", nil)
	Flush()
	all, _ := os.ReadFile(Path())
	own, _ := os.ReadFile(sess)
	if n := strings.Count(string(all), "\n"); n != 2 || !strings.Contains(string(all), `"session":"s1"`) || !strings.Contains(string(all), `"proc":"test"`) {
		t.Fatalf("rush's log:\n%s", all)
	}
	if !strings.Contains(string(own), `"kind":"check-in"`) || strings.Contains(string(own), "restarted") {
		t.Fatalf("the session's log:\n%s", own)
	}
}
