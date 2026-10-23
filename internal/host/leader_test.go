package host

import (
	"os"
	"path/filepath"
	"slices"
	"testing"

	_ "github.com/0xdeafcafe/rush/internal/adapters/codex"
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

// A chat Claude Code leads hands work to Codex's models, in Codex, by
// default; switching Codex off in Config takes them away, and a cell's
// models keep it to those.
func TestTakesOtherHarness(t *testing.T) {
	cfg := state.Config{}
	if cell, ok := takes(&cfg, "claude", "codex", "codex"); !ok || cell != "codex" {
		t.Fatalf("Claude Code leads: Codex taken %v as %q, want its own cell", ok, cell)
	}
	if cell, ok := takes(&cfg, "codex", "claude", "claude"); !ok || cell != "claude" {
		t.Fatalf("Codex leads: Claude Code taken %v as %q", ok, cell)
	}
	cfg.SetCellModels("codex", "codex", []string{"gpt-6-astra"})
	if !cfg.Calls("codex", "codex", "gpt-6-astra") || cfg.Calls("codex", "codex", "gpt-6.1-sol") {
		t.Fatal("Codex's cell keeps it to astra, yet calls aren't kept to it")
	}
	cfg.SetHarnessOn("codex", false)
	if _, ok := takes(&cfg, "claude", "codex", "codex"); ok {
		t.Fatal("Codex is off, yet a Claude Code chat hands it work")
	}
}

// Config is read on each call, so a change in Settings reaches a session
// already running: its next spawn_agent sees it.
func TestRunnersReadConfigEachCall(t *testing.T) {
	if a, ok := agent.Get("codex"); !ok || !slices.ContainsFunc(Installed(), func(b agent.Adapter) bool { return b.Kind() == a.Kind() }) {
		t.Skip("Codex isn't installed here")
	}
	home := t.TempDir()
	t.Setenv("RUSH_HOME", home)
	write := func(json string) {
		if err := os.WriteFile(filepath.Join(home, "config.json"), []byte(json), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	has := func() bool {
		return slices.ContainsFunc(runners("claude"), func(r runner) bool { return r.k == "codex" })
	}
	write(`{"harnessesOff":["codex"]}`)
	if has() {
		t.Fatal("Codex is off, yet listed")
	}
	write(`{}`)
	if !has() {
		t.Fatal("Codex switched on, yet the next call doesn't list it")
	}
}
