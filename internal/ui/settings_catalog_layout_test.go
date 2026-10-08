package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/charmbracelet/x/ansi"
)

func catalogTestModel(t *testing.T, width, height, page int) *Model {
	t.Helper()
	m, _ := benchModel(width, height)
	m.setView(placeSettings)
	m.setSettingsPage(page)
	m.dialog.modelsRead = nil
	return m
}

func TestCatalogShowsProviderBeforeEnteringControls(t *testing.T) {
	for _, width := range []int{80, 140, 220} {
		t.Run(fmt.Sprint(width), func(t *testing.T) {
			m := catalogTestModel(t, width, 42, pageProviders)
			at := slices.Index(m.provItems(), provItem{provider: "claude"})
			if at < 0 {
				t.Skip("claude isn't installed here")
			}
			m.dialog.cursor = at
			body := m.dialogBody(width - 6)
			plain := ansi.Strip(strings.Join(body, "\n"))
			for _, want := range []string{"Anthropic", "Harnesses", "enter open"} {
				if !strings.Contains(plain, want) {
					t.Fatalf("width %d hides automatic preview %q:\n%s", width, want, plain)
				}
			}
			if m.dialog.inside {
				t.Fatal("preview unexpectedly moved keyboard focus")
			}
			assertCatalogFits(t, m, body, width-6)
			dumpCatalog(t, "providers", width, plain)
			m.dialogKey(tea.KeyPressMsg{}, "right")
			if !m.dialog.inside || m.dialog.pick != at {
				t.Fatal("Right must focus displayed controls")
			}
			m.dialogKey(tea.KeyPressMsg{}, "esc")
			if m.dialog.inside || m.dialog.cursor != at {
				t.Fatal("Escape must return to same provider")
			}
		})
	}
}

func TestHarnessCatalogShowsDetailsAndDenseFeatures(t *testing.T) {
	for _, width := range []int{80, 140, 220} {
		t.Run(fmt.Sprint(width), func(t *testing.T) {
			m := catalogTestModel(t, width, 42, pageHarnesses)
			rows := flat(m.harnessCatalogSections())
			at := slices.IndexFunc(rows, func(r setting) bool { return r.label == "Claude Code" })
			m.dialog.cursor = at
			body := m.dialogBody(width - 6)
			plain := ansi.Strip(strings.Join(body, "\n"))
			if !strings.Contains(plain, "Provider") || !strings.Contains(plain, "New sessions start with") {
				t.Fatalf("automatic harness preview absent:\n%s", plain)
			}
			if m.dialog.harnessInside {
				t.Fatal("preview unexpectedly took focus")
			}
			assertCatalogFits(t, m, body, width-6)
			dumpCatalog(t, "harnesses", width, plain)
			grid := m.pairingGrid("claude", "opus", width-6)
			if width >= 140 && len(grid) >= len(agent.AllFeatures())/2 {
				t.Fatal("capabilities are a vertical list instead of a grid")
			}
			if len(flat(m.harnessDetailSections())) >= len(agent.AllFeatures()) {
				t.Fatal("features duplicated as form rows")
			}
		})
	}
}

func TestCatalogShortTerminalCanReachAllDetails(t *testing.T) {
	for _, width := range []int{58, 80, 140} {
		m := catalogTestModel(t, width, 20, pageHarnesses)
		rows := flat(m.harnessCatalogSections())
		m.dialog.cursor = slices.IndexFunc(rows, func(r setting) bool { return r.label == "Claude Code" })
		m.dialogKey(tea.KeyPressMsg{}, "right")
		body := m.dialogBody(width - 6)
		assertCatalogFits(t, m, body, width-6)
		seenMemory := false
		for range 120 {
			m.dialogKey(tea.KeyPressMsg{}, "pgdown")
			body = m.dialogBody(width - 6)
			seenMemory = seenMemory || strings.Contains(ansi.Strip(strings.Join(body, "\n")), "Memory")
		}
		plain := ansi.Strip(strings.Join(body, "\n"))
		if !seenMemory {
			t.Fatalf("width %d cannot reach last capabilities:\n%s", width, plain)
		}
		if m.dialog.catalogScroll == 0 {
			t.Fatal("detail did not scroll")
		}
		m.dialogKey(tea.KeyPressMsg{}, "down")
		body = m.dialogBody(width - 6)
		if !strings.Contains(strings.Join(body, "\n"), selBG) {
			t.Fatal("moving control cursor must bring it back into view")
		}
	}
}

func TestCatalogFeatureGridPreservesPairingSupport(t *testing.T) {
	m := catalogTestModel(t, 160, 40, pageProviders)
	grid := ansi.Strip(strings.Join(m.pairingGrid("claude", "opus", 150), "\n"))
	for _, f := range agent.AllFeatures() {
		if !strings.Contains(grid, f.Label) {
			t.Fatalf("feature omitted: %s", f.Label)
		}
	}
	if got := pairingSupport("definitely-missing", "", agent.FeatureRun); got.Is == agent.StateYes {
		t.Fatal("unknown harness advertised as supported")
	}
}

func assertCatalogFits(t *testing.T, m *Model, body []string, w int) {
	t.Helper()
	maxRows := m.h - len(m.header()) - 4
	if len(body) > maxRows {
		t.Fatalf("catalog has %d rows but frame only holds %d", len(body), maxRows)
	}
	for _, line := range body {
		if cellw.String(line) > w {
			t.Fatalf("catalog row exceeds %d cells: %q", w, ansi.Strip(line))
		}
	}
}

func dumpCatalog(t *testing.T, name string, width int, body string) {
	t.Helper()
	if dir := os.Getenv("RUSH_CATALOG_DUMP"); dir != "" {
		if err := os.MkdirAll(dir, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, fmt.Sprintf("%s-%d.txt", name, width)), []byte(body+"\n"), 0600); err != nil {
			t.Fatal(err)
		}
	}
}

func TestCatalogMouseUsesRenderedRows(t *testing.T) {
	m := catalogTestModel(t, 140, 42, pageProviders)
	m.dialogBody(134)
	g := m.dialog.catalogGeometry
	picked := -1
	y := 0
	for i, row := range g.listRows {
		if row >= 0 && row != m.dialog.cursor {
			picked = row
			y = g.listTop + i
			break
		}
	}
	if picked < 0 {
		t.Skip("one provider installed here")
	}
	m.Update(tea.MouseClickMsg{X: g.x + 3, Y: y, Button: tea.MouseLeft})
	if m.dialog.cursor != picked || m.dialog.inside {
		t.Fatal("left click did not select preview without opening action")
	}
	m.dialogBody(134)
	g = m.dialog.catalogGeometry
	y = -1
	for i, row := range g.detailRows {
		if row == 0 {
			y = g.detailTop + i
			break
		}
	}
	if y < 0 {
		t.Fatal("first provider control is not visible")
	}
	m.Update(tea.MouseClickMsg{X: g.detailX + 4, Y: y, Button: tea.MouseLeft})
	if !m.dialog.inside || m.dialog.pick != picked || m.dialog.cursor != 0 {
		t.Fatal("right click did not focus visible control")
	}
	m.dialog.asking = "keep this prompt"
	before := m.dialog.cursor
	m.Update(tea.MouseClickMsg{X: g.x + 3, Y: g.listTop + 1, Button: tea.MouseLeft})
	if m.dialog.cursor != before || !m.dialog.inside {
		t.Fatal("click changed catalog while an input prompt was active")
	}
}

func TestCatalogWheelScrollsTargetColumn(t *testing.T) {
	m := catalogTestModel(t, 80, 25, pageProviders)
	m.dialogBody(74)
	g := m.dialog.catalogGeometry
	before := m.dialog.cursor
	m.Update(tea.MouseWheelMsg{X: g.detailX + 3, Y: g.detailTop + 3, Button: tea.MouseWheelDown})
	if m.dialog.catalogScroll <= 0 || m.dialog.cursor != before {
		t.Fatal("detail wheel moved list instead of detail")
	}
	m.Update(tea.MouseWheelMsg{X: g.x + 3, Y: g.listTop + 2, Button: tea.MouseWheelDown})
	if m.dialog.cursor <= before {
		t.Fatal("list wheel did not change selection")
	}
}

func TestCatalogIncludesAccountsAndSupportedCaveats(t *testing.T) {
	m := catalogTestModel(t, 160, 50, pageHarnesses)
	m.dialog.modelKind = "claude"
	secs := m.harnessDetailSections()
	if _, ok := m.nativeAuthSection("claude"); !ok && !slices.ContainsFunc(secs, func(s section) bool { return s.title == "Account" }) {
		t.Fatal("Claude account hidden from harness detail")
	}
	body, _ := m.catalogDetail(secs, "claude", "opus", false, 130)
	plain := ansi.Strip(strings.Join(body, "\n"))
	for _, f := range agent.AllFeatures() {
		support := pairingSupport("claude", "opus", f.Feature)
		if support.Is == agent.StateYes && support.Note != "" && !strings.Contains(plain, support.Note) {
			t.Fatalf("supported caveat missing: %s", support.Note)
		}
	}
}

func TestCatalogClampsStaleSelections(t *testing.T) {
	m := catalogTestModel(t, 100, 30, pageProviders)
	m.dialog.inside = true
	m.dialog.pick = 100000
	m.dialog.cursor = 100000
	m.dialogBody(94)
	if m.provPicked() != m.provItems()[len(m.provItems())-1] {
		t.Fatal("stale provider selection not clamped")
	}
	m.setSettingsPage(pageHarnesses)
	m.dialog.harnessInside = true
	m.dialog.harnessReturn = 100000
	m.dialog.cursor = 100000
	m.dialogBody(94)
	if m.dialog.harnessReturn >= len(catalogHarnesses()) || m.dialog.cursor >= len(flat(m.harnessDetailSections())) {
		t.Fatal("stale harness or control cursor not clamped")
	}
}

func TestHarnessPreviewAppliesCatalogWithoutActivation(t *testing.T) {
	m := catalogTestModel(t, 140, 42, pageHarnesses)
	rows := flat(m.harnessCatalogSections())
	m.dialog.cursor = slices.IndexFunc(rows, func(r setting) bool { return r.label == "Claude Code" })
	models := map[string][]agent.Choice{"claude": {{ID: "fresh-model", Note: "freshly discovered"}}}
	m.dialog.modelsRead = &pending[map[string][]agent.Choice]{ch: make(chan map[string][]agent.Choice, 1)}
	m.dialog.modelsRead.ch <- models
	m.harnessesBody(134)
	if !m.dialog.modelsApplied || len(m.listed["claude"]) != 1 || m.listed["claude"][0].ID != "fresh-model" {
		t.Fatal("Harnesses preview did not apply discovery")
	}
	if m.dialog.harnessInside {
		t.Fatal("catalog discovery changed focus")
	}
}
