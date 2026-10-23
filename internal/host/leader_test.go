package host

import (
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A chat's leader harness runs only what its row of Config lets it: its
// own providers with the cell on, in it, and no other harness's.
func TestLeaderCell(t *testing.T) {
	if _, ok := agent.Get("claude"); !ok {
		t.Skip("no Claude Code adapter")
	}
	cfg := state.Config{}
	if got := leaderCell(&cfg, "claude", "claude", "claude"); got != "claude" {
		t.Fatalf("Claude Code runs Anthropic's plan as %q", got)
	}
	if got := leaderCell(&cfg, "codex", "claude", "claude"); got != "" {
		t.Fatalf("Codex leads, yet runs Claude Code as %q", got)
	}
	cfg.SetHarnessOn("claude", false)
	if got := leaderCell(&cfg, "claude", "claude", "claude"); got != "" {
		t.Fatalf("Claude Code is off, yet runs %q", got)
	}
}
