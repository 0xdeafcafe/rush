package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/script"
)

// A Bash call of more than three commands over more than one line comes back from the hook as a
// script run, its command kept as the script; a short one goes as it is.
func TestScriptHook(t *testing.T) {
	if script.Bash() == "" {
		t.Skip("no bash 4 or later")
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	var out strings.Builder
	gateHook(strings.NewReader(`{"tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{"command":"a && b\nc && d","description":"x"}}`), &out)
	if !strings.Contains(out.String(), `"updatedInput"`) || !strings.Contains(out.String(), "rush-script") || !strings.Contains(out.String(), `"description":"x"`) {
		t.Fatalf("long call: %s", out.String())
	}
	if b, err := os.ReadFile(filepath.Join(script.Dir(), "toolu_1.sh")); err != nil || string(b) != "a && b\nc && d\n" {
		t.Fatalf("script: %q %v", b, err)
	}
	for _, cmd := range []string{"go test ./...", "a && b && c && d"} {
		out.Reset()
		gateHook(strings.NewReader(`{"tool_name":"Bash","tool_use_id":"toolu_2","tool_input":{"command":"`+cmd+`"}}`), &out)
		if out.Len() != 0 {
			t.Fatalf("short call or one-liner rewritten: %s", out.String())
		}
	}
}
