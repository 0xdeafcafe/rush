package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/bundled/prwatch"
	"github.com/0xdeafcafe/rush/internal/plugin"
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

// Waiting on a pull request's checks is rush's: gh pr checks --watch comes
// back as rush watch pr, fitted to the call's timeout, and a sleep before
// asking GitHub is turned back, while the watch plugin is on; off, both go
// as they are.
func TestWatchHook(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	var out strings.Builder
	gateHook(strings.NewReader(`{"tool_name":"Bash","tool_use_id":"toolu_5","tool_input":{"command":"sleep 60 && gh pr checks 42"}}`), &out)
	if out.Len() != 0 {
		t.Fatalf("watch off, still turned back: %s", out.String())
	}
	if err := plugin.SetBundled(prwatch.Name, true); err != nil {
		t.Fatal(err)
	}
	gateHook(strings.NewReader(`{"tool_name":"Bash","tool_use_id":"toolu_3","tool_input":{"command":"gh pr checks 42 --watch","timeout":600000}}`), &out)
	if !strings.Contains(out.String(), `"updatedInput"`) || !strings.Contains(out.String(), "watch pr 42 --until done --timeout 9m45s") {
		t.Fatalf("checks --watch: %s", out.String())
	}
	out.Reset()
	gateHook(strings.NewReader(`{"tool_name":"Bash","tool_use_id":"toolu_4","tool_input":{"command":"sleep 60 && gh pr checks 42"}}`), &out)
	if !strings.Contains(out.String(), `"permissionDecision":"deny"`) || !strings.Contains(out.String(), "rush watch pr") {
		t.Fatalf("sleep then poll: %s", out.String())
	}
}
