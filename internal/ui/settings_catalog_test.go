package ui

import (
	"slices"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// Providers, Harnesses and Profiles are pages of their own: whose models,
// the programs running them, and how work is routed between them.
func TestProvidersHarnessesProfilesArePages(t *testing.T) {
	m, _ := benchModel(180, 60)
	m.setView(placeSettings)
	pages := m.settingsPages()
	if pages[pageProviders].name != "Providers" || pages[pageHarnesses].name != "Harnesses" || pages[pageProfiles].name != "Presets" {
		t.Fatalf("pages: %s, %s, %s", pages[0].name, pages[1].name, pages[2].name)
	}
	for _, p := range pages {
		if p.name == "Models" || p.name == "Accounts" {
			t.Fatalf("%s is folded into Providers", p.name)
		}
	}
	m.setSettingsPage(pageProviders)
	for _, it := range m.provItems() {
		if it.provider == "" {
			t.Fatalf("Providers lists %+v", it)
		}
	}
	m.setSettingsPage(pageProfiles)
	items := m.provItems()
	if !slices.Contains(items, provItem{add: "profile"}) || slices.ContainsFunc(items, func(it provItem) bool { return it.provider != "" }) {
		t.Fatalf("Profiles lists %+v", items)
	}
}

// A provider shows the harnesses it runs in, ★ the default, and keeps
// what rush can do with it at the bottom, as it was.
func TestProviderShowsHarnessesAndWhatItCanDo(t *testing.T) {
	if !agent.Runs("codex") {
		t.Skip("codex isn't installed here")
	}
	m, _ := benchModel(180, 60)
	m.setView(placeSettings)
	m.setSettingsPage(pageProviders)
	m.dialog.cursor = slices.Index(m.provItems(), provItem{provider: "codex"})
	body := ansi.Strip(strings.Join(m.dialogBody(170), "\n"))
	for _, want := range []string{"Harnesses", "★ ", "What it can do", "OpenAI"} {
		if !strings.Contains(body, want) {
			t.Fatalf("missing %q:\n%s", want, body)
		}
	}
	if i, j := strings.Index(body, "Harnesses"), strings.Index(body, "What it can do"); i > j {
		t.Fatal("what it can do isn't at the bottom")
	}
}

// A harness says every provider it runs and why not the rest; a plan
// run through another's harness is allowed, with a warning.
func TestHarnessRunsOn(t *testing.T) {
	if _, ok := agent.Get("pi"); !ok {
		t.Skip("no Pi adapter")
	}
	m, _ := benchModel(180, 60)
	m.setView(placeSettings)
	m.setSettingsPage(pageHarnesses)
	sec := m.runsOnSection("pi")
	if len(sec.rows) != len(agent.ProviderIDs()) {
		t.Fatalf("%d rows for %d providers", len(sec.rows), len(agent.ProviderIDs()))
	}
	var lines []string
	for _, r := range sec.rows {
		lines = append(lines, ansi.Strip(r.line(120)))
	}
	if all := strings.Join(lines, "\n"); agent.Runs("pi") && !strings.Contains(all, "runs your Anthropic plan through Pi") {
		t.Fatalf("no plan warning:\n%s", all)
	}
	at := slices.IndexFunc(flat(m.harnessCatalogSections()), func(r setting) bool { return r.label == "Codex" })
	m.dialog.cursor = at
	m.dialogKey(tea.KeyPressMsg{}, "enter")
	if !m.dialog.harnessInside {
		t.Fatal("enter didn't go into the harness")
	}
	if !slices.ContainsFunc(m.harnessDetailSections(), func(s section) bool { return s.title == "Runs on" }) {
		t.Fatal("no Runs on")
	}
}
