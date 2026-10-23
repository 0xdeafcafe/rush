package ui

import (
	"slices"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Providers, Harnesses and Config are pages of their own: whose models,
// the programs running them, and which may run which.
func TestProvidersHarnessesConfigArePages(t *testing.T) {
	m, _ := benchModel(180, 60)
	m.setView(placeSettings)
	pages := m.settingsPages()
	if pages[pageProviders].name != "Providers" || pages[pageHarnesses].name != "Harnesses" || pages[pageConfig].name != "Config" {
		t.Fatalf("pages: %s, %s, %s", pages[0].name, pages[1].name, pages[2].name)
	}
	for _, p := range pages {
		if p.name == "Models" || p.name == "Accounts" || p.name == "Presets" {
			t.Fatalf("%s is gone", p.name)
		}
	}
	m.setSettingsPage(pageProviders)
	for _, it := range m.provItems() {
		if it.provider == "" {
			t.Fatalf("Providers lists %+v", it)
		}
	}
}

// A provider shows its account, links to Config for where it runs, and
// keeps what rush can do with it at the bottom, as it was.
func TestProviderShowsWhatItCanDo(t *testing.T) {
	if !agent.Runs("codex") {
		t.Skip("codex isn't installed here")
	}
	m, _ := benchModel(180, 60)
	m.setView(placeSettings)
	m.setSettingsPage(pageProviders)
	m.dialog.cursor = slices.Index(m.provItems(), provItem{provider: "codex"})
	body := ansi.Strip(strings.Join(m.dialogBody(170), "\n"))
	for _, want := range []string{"Where it runs, and its models", "What it can do", "OpenAI"} {
		if !strings.Contains(body, want) {
			t.Fatalf("missing %q:\n%s", want, body)
		}
	}
	if i, j := strings.Index(body, "Where it runs"), strings.Index(body, "What it can do"); i > j {
		t.Fatal("what it can do isn't at the bottom")
	}
}

// Harnesses starts with Every harness; a harness shows what it changes
// from it, and a change there wins over it at start.
func TestHarnessChangesFromEvery(t *testing.T) {
	if !agent.Runs("codex") {
		t.Skip("codex isn't installed here")
	}
	m, _ := benchModel(180, 60)
	m.setView(placeSettings)
	m.setSettingsPage(pageHarnesses)
	rows := flat(m.harnessCatalogSections())
	if rows[0].label != "Every harness" {
		t.Fatalf("first line %q", rows[0].label)
	}
	m.dialog.cursor = 0
	m.dialogKey(tea.KeyPressMsg{}, "enter")
	if !m.dialog.harnessInside || m.dialog.modelKind != "" {
		t.Fatal("enter didn't go into Every harness")
	}
	d := &m.store.Config.Dispatch
	d.Every.Effort = "high"
	d.SetHarnessStart("codex", state.Start{Effort: "low"})
	if st := d.Resolved("codex", "codex"); st.Effort != "low" {
		t.Fatalf("codex starts with %+v", st)
	}
	m.dialogKey(tea.KeyPressMsg{}, "esc")
	at := slices.IndexFunc(flat(m.harnessCatalogSections()), func(r setting) bool { return r.label == "Codex" })
	m.dialog.cursor = at
	m.dialogKey(tea.KeyPressMsg{}, "enter")
	if !m.dialog.harnessInside || agent.HarnessOf(m.dialog.modelKind) != "codex" {
		t.Fatal("enter didn't go into Codex")
	}
	body := ansi.Strip(strings.Join(m.dialogBody(170), "\n"))
	if !strings.Contains(body, "low · was high") {
		t.Fatalf("Codex doesn't say what it changes:\n%s", body)
	}
}

// Each worktree of a repository has the same definitions: they're listed
// once, the other places counted.
func TestSameDefsOnce(t *testing.T) {
	defs := []agent.AgentDef{
		{Name: "claude"},
		{Name: "lane", Scope: "langwatch", Model: "opus", Desc: "a lane"},
		{Name: "lane", Scope: "feat-a", Model: "opus", Desc: "a lane"},
		{Name: "lane", Scope: "feat-b", Model: "opus", Desc: "a lane"},
		{Name: "lane", Scope: "feat-c", Model: "sonnet", Desc: "changed here"},
	}
	got := sameDefsOnce(defs)
	if len(got) != 3 || got[1].Scope != "langwatch +2" || got[2].Scope != "feat-c" {
		t.Fatalf("got %+v", got)
	}
}
