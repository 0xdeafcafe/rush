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
	"github.com/charmbracelet/x/ansi"
)

func TestSettingsWorkspaceFitsEveryPage(t *testing.T) {
	for _, size := range [][2]int{{44, 20}, {58, 24}, {80, 30}, {140, 40}, {200, 50}} {
		m, _ := benchModel(size[0], size[1])
		m.setView(placeSettings)
		for page, p := range m.settingsPages() {
			t.Run(fmt.Sprintf("%dx%d/%s", size[0], size[1], p.name), func(t *testing.T) {
				m.setSettingsPage(page)
				m.dialog.modelsRead = nil
				for _, last := range []bool{false, true} {
					if last {
						m.dialog.cursor = max(0, m.dialogLen()-1)
					}
					body := m.dialogBody(m.w - 6)
					if len(body) > m.settingsHeight() {
						t.Fatalf("%d rows exceed %d", len(body), m.settingsHeight())
					}
					for _, row := range body {
						if cellw.String(row) > m.w-6 {
							t.Fatalf("row overflows: %s", ansi.Strip(row))
						}
					}
					if m.curPage().form != nil && m.dialogLen() > 0 && !strings.Contains(strings.Join(body, "\n"), selBG) {
						t.Fatal("selected control is outside viewport")
					}
					if !last {
						if dir := os.Getenv("RUSH_SETTINGS_DUMP"); dir != "" {
							if err := os.MkdirAll(dir, 0700); err != nil {
								t.Fatal(err)
							}
							text := ansi.Strip(strings.Join(body, "\n")) + "\n"
							if err := os.WriteFile(filepath.Join(dir, fmt.Sprintf("%s-%dx%d.txt", p.name, size[0], size[1])), []byte(text), 0600); err != nil {
								t.Fatal(err)
							}
						}
					}
				}
			})
		}
	}
}

func TestSettingsFormPointerAndPaging(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	for _, width := range []int{58, 140, 200} {
		m, _ := benchModel(width, 30)
		m.setView(placeSettings)
		m.setSettingsPage(pageGeneral)
		m.dialogKey(tea.KeyPressMsg{}, "end")
		if m.dialog.cursor != m.dialogLen()-1 {
			t.Fatal("End missed last setting")
		}
		m.dialogBody(m.w - 6)
		g := m.dialog.formGeometry
		at := slices.Index(g.rows, m.dialog.cursor)
		if at < 0 {
			t.Fatal("last control not visible")
		}
		before := m.dialog.cursor
		m.Update(tea.MouseWheelMsg{X: g.x + g.railWidth + 4, Y: g.top + at, Button: tea.MouseWheelUp})
		if m.dialog.cursor >= before {
			t.Fatal("wheel did not move settings")
		}
		m.dialogBody(m.w - 6)
		g = m.dialog.formGeometry
		at = slices.IndexFunc(g.rows, func(i int) bool { return i >= 0 && i != m.dialog.cursor })
		if at < 0 {
			t.Fatal("no other visible row")
		}
		m.Update(tea.MouseClickMsg{X: g.x + g.railWidth + 4, Y: g.top + at, Button: tea.MouseLeft})
		if m.dialog.cursor != g.rows[at] {
			t.Fatal("pointer selected wrong row after scrolling")
		}
		m.dialogKey(tea.KeyPressMsg{}, "home")
		if m.dialog.cursor != 0 {
			t.Fatal("Home missed first setting")
		}
	}
}

func TestProvidersPagePointerAndBack(t *testing.T) {
	m, _ := benchModel(140, 35)
	m.setView(placeSettings)
	m.setSettingsPage(pageProviders)
	m.dialogBody(134)
	g := m.dialog.catalogGeometry
	at := slices.IndexFunc(g.listRows, func(i int) bool { return i > 0 })
	if at < 0 {
		t.Fatal("providers unavailable")
	}
	m.Update(tea.MouseClickMsg{X: g.x + 2, Y: g.listTop + at, Button: tea.MouseLeft})
	if m.dialog.cursor != g.listRows[at] || m.dialog.inside {
		t.Fatal("provider click should select its preview")
	}
	selected := m.dialog.cursor
	m.dialogKey(tea.KeyPressMsg{}, "enter")
	if !m.dialog.inside {
		t.Fatal("Enter did not open provider")
	}
	m.dialogKey(tea.KeyPressMsg{}, "esc")
	if m.dialog.inside || m.dialog.cursor != selected {
		t.Fatal("Escape lost selected provider")
	}
}

func TestSettingsHelpAndSectionNavigation(t *testing.T) {
	m, _ := benchModel(58, 24)
	m.setView(placeSettings)
	m.setSettingsPage(pageGeneral)
	secs := m.generalSections()
	m.dialogKey(tea.KeyPressMsg{}, "ctrl+down")
	if m.dialog.cursor != len(secs[0].rows) {
		t.Fatal("section shortcut did not reach next section")
	}
	m.dialogKey(tea.KeyPressMsg{}, "ctrl+up")
	if m.dialog.cursor != 0 {
		t.Fatal("section shortcut did not return")
	}
	m.dialogKey(tea.KeyPressMsg{}, "?")
	help, ok := m.sheet.(*settingHelpSheet)
	if !ok {
		t.Fatal("full explanation did not open")
	}
	body := help.body(m, 40, 8)
	if len(body) > 8 {
		t.Fatal("help overflows small terminal")
	}
	help.key(m, tea.KeyPressMsg{}, "end")
	body = help.body(m, 40, 8)
	if !strings.Contains(ansi.Strip(strings.Join(body, "\n")), "background") {
		t.Fatal("end of explanation unreachable")
	}
	help.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil || m.dialog.cursor != 0 {
		t.Fatal("closing help lost setting selection")
	}
}

func TestSettingsTabsKeepCurrentPageClickable(t *testing.T) {
	for _, w := range []int{44, 58, 80, 140} {
		m, _ := benchModel(w, 30)
		m.setView(placeSettings)
		for at, p := range m.settingsPages() {
			m.setSettingsPage(at)
			text := ansi.Strip(m.underHead()[0])
			before, _, found := strings.Cut(text, p.name)
			if !found {
				t.Fatalf("%d hides current page %s", w, p.name)
			}
			hit := m.headerTabAt(cellw.String(before)+1, m.headH())
			if lo, _ := placePages(m.view); !hit.valid || !hit.page || hit.index != at-lo {
				t.Fatalf("%d wrong target for %s: %+v", w, p.name, hit)
			}
		}
	}
}

func TestAppearanceSelectionKeepsLayoutStable(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.setView(placeSettings)
	m.setSettingsPage(pageGeneral)
	rows := flat(m.interfaceSections())
	m.dialogBody(194)
	before := m.dialog.formGeometry
	m.dialog.cursor = slices.IndexFunc(rows, func(st setting) bool { return st.preview == nil })
	m.dialogBody(194)
	after := m.dialog.formGeometry
	if before.controlWidth != after.controlWidth || before.railWidth != after.railWidth || len(before.rows) != len(after.rows) {
		t.Fatal("selecting a setting without a preview moved the panels")
	}
}
