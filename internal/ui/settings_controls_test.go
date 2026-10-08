package ui

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/charmbracelet/x/ansi"
)

func TestSettingsActionsAndFactsAreNotSelectors(t *testing.T) {
	m, _ := benchModel(140, 50)
	called := false
	action := setting{label: "Sign in", keys: []string{"enter", "sign in"}, key: func(s string) (tea.Cmd, bool) {
		if s == "enter" {
			called = true
			return nil, true
		}
		return nil, false
	}}
	fact := setting{label: "Status", value: "not checked"}
	fixed := choiceSetting("Provider", "one", "", [][2]string{{"one", "Only provider"}}, func(string) { t.Fatal("fixed choice must not mutate config") })
	fixed.names = map[string]string{"one": "Only provider"}
	for _, st := range []setting{action, fact, fixed} {
		row := ansi.Strip(m.settingRow(false, st, 24, 18, 100))
		if strings.ContainsAny(row, "‹›") || strings.Contains(row, "default") {
			t.Fatalf("fake selector: %q", row)
		}
	}
	if row := ansi.Strip(m.settingRow(false, action, 24, 18, 100)); !strings.Contains(row, "[enter]") {
		t.Fatalf("action has no visible affordance: %q", row)
	}
	m.setView(placeSettings)
	m.formKey([]section{{rows: []setting{fixed}}}, "right")
	m.formKey([]section{{rows: []setting{action}}}, "right")
	if called {
		t.Fatal("sideways selection triggered action")
	}
	m.formKey([]section{{rows: []setting{action}}}, "enter")
	if !called {
		t.Fatal("Enter failed to activate action")
	}
	choice := choiceSetting("Effort", "high", "", [][2]string{{"high", "high effort"}, {"low", "low effort"}}, func(string) {})
	if row := ansi.Strip(m.settingRow(false, choice, 24, 18, 100)); !strings.Contains(row, "‹") {
		t.Fatalf("real choice lost arrows: %q", row)
	}
}

func TestSettingsRowsStayWithinWidthAndHintsAreUnique(t *testing.T) {
	m, _ := benchModel(140, 50)
	for _, w := range []int{24, 60, 120} {
		for _, st := range []setting{
			{label: "Long status label", value: strings.Repeat("x", 200)},
			{line: func(int) string { return strings.Repeat("界", 200) }},
			{label: "Run action", key: func(string) (tea.Cmd, bool) { return nil, false }, keys: []string{"enter", "run"}},
		} {
			for _, on := range []bool{false, true} {
				row := m.settingRow(on, st, 32, 20, w)
				if cellw.String(row) > w {
					t.Fatalf("row width %d exceeds %d", cellw.String(row), w)
				}
			}
		}
	}
	hint := ansi.Strip(m.formKeys(setting{keys: []string{"enter", "restart idle sessions"}}, []string{"enter", "update", "r", "refresh"}, 160))
	if strings.Count(hint, "enter") != 1 || !strings.Contains(hint, "restart idle sessions") {
		t.Fatalf("incorrect duplicate key hints: %q", hint)
	}
	if got := (setting{label: "Sign in"}).now(); got != "" {
		t.Fatalf("action describes invented current value: %q", got)
	}
}
