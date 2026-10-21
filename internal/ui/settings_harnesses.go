package ui

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"slices"
)

func harnessesPage() page {
	return page{name: "Harnesses", pre: func(m *Model, key string) (tea.Cmd, bool) {
		d := m.dialog
		if d.harnessInside && (key == "esc" || key == "backspace") {
			d.harnessInside = false
			d.cursor = d.harnessReturn
			d.catalogScroll = 0
			return nil, true
		}
		return m.catalogKey(key, d.harnessInside)
	}, rows: func(m *Model) int { return len(flat(m.harnessesSections())) }, key: func(m *Model, key string) tea.Cmd {
		if key == "right" || key == "enter" {
			m.dialog.catalogScroll = 0
		}
		return m.formKey(m.harnessesSections(), key)
	}, body: (*Model).harnessesBody}
}

func (m *Model) harnessesBody(w int) []string {
	m.applySettingsModels()
	d := m.dialog
	catalog := m.harnessCatalogSections()
	picked := d.cursor
	if d.harnessInside {
		picked = d.harnessReturn
	}
	kinds := catalogHarnesses()
	if len(kinds) == 0 {
		return []string{dim("No harnesses available")}
	}
	picked = max(0, min(picked, len(kinds)-1))
	if d.harnessInside {
		d.harnessReturn = picked
	} else {
		d.cursor = picked
	}
	h := kinds[picked]
	if agent.HarnessOf(d.modelKind) != agent.HarnessOf(h) {
		d.modelKind = h
	}
	return m.catalogBody(catalog, picked, d.harnessInside, false, w, func(dw int) ([]string, int) {
		return m.catalogDetail(m.harnessDetailSections(), d.modelKind, m.store.Config.Dispatch.StartFor(string(d.modelKind)).Model, d.harnessInside, dw)
	})
}

func (m *Model) harnessesSections() []section {
	if m.dialog.harnessInside {
		return m.harnessDetailSections()
	}
	return m.harnessCatalogSections()
}

func catalogHarnesses() []agent.Kind {
	var out []agent.Kind
	var seen []agent.Kind
	for _, a := range agent.All() {
		if agent.CurrentKind(a.Kind()) != a.Kind() {
			continue
		}
		k, h := a.Kind(), agent.HarnessOf(a.Kind())
		if slices.Contains(seen, h) {
			continue
		}
		seen = append(seen, h)
		if _, ok := agent.Get(h); ok {
			k = h
		}
		out = append(out, k)
	}
	return out
}

func (m *Model) harnessDetailSections() []section {
	d := m.dialog

	h := agent.HarnessOf(d.modelKind)
	var choices [][2]string
	for _, a := range agent.All() {
		if agent.HarnessOf(a.Kind()) == h {
			id, _ := agent.RouteOf(a.Kind(), false)
			choices = append(choices, [2]string{string(a.Kind()), provLabel(id)})
		}
	}
	row := choiceSetting("Provider", string(d.modelKind), "Only providers supported by this harness appear here. Changing this preview does not change running sessions.", choices, func(v string) { d.modelKind = agent.Kind(v) })
	row.names = map[string]string{}
	for _, c := range choices {
		row.names[c[0]] = c[1]
	}
	row.run = func(v string) tea.Cmd { d.modelKind = agent.Kind(v); return m.loadModels(v) }
	secs := []section{{title: agent.HarnessLabel(d.modelKind), rows: []setting{row}}}
	if auth, ok := m.nativeAuthSection(d.modelKind); ok {
		secs = append(secs, auth)
	} else if !agent.KeyOnly(d.modelKind) {
		if accounts, ok := m.accountSection(d.modelKind); ok {
			secs = append(secs, accounts)
		}
	}
	// Defaults and permission controls belong to the exact provider/harness
	// adapter, never the first installed adapter for the provider.
	secs = append(secs, m.agentSections(d.modelKind)...)
	return append(secs, m.runsOnSection(h))
}

func (m *Model) harnessCatalogSections() []section {
	d := m.dialog
	var rows []setting
	for _, k := range catalogHarnesses() {
		name := agent.HarnessLabel(k)
		rows = append(rows, setting{label: name, line: func(int) string {
			status := dim("installed")
			if !agent.Runs(k) {
				status = faint("not installed")
			}
			return paint(cText, fit(name, 24)) + status
		}, what: name + " runs the model. Its provider, defaults and features are shown beside the list.", key: func(s string) (tea.Cmd, bool) {
			if s != "enter" && s != "right" {
				return nil, false
			}
			d.harnessInside = true
			d.harnessReturn = d.cursor
			d.cursor = 0
			d.modelKind = k
			return m.loadModels(string(k)), true
		}, keys: []string{"right", "controls"}})
	}
	return []section{{rows: rows}}
}

// runsOnSection is every provider and how it runs in harness h: ★ where
// it's the provider's default, a warning where it runs on a plan through
// h's own sign-in, and why where it can't.
func (m *Model) runsOnSection(h agent.Kind) section {
	cfg := &m.store.Config
	hn := agent.HarnessLabel(h)
	sec := section{title: "Runs on", note: "★ where it's the provider's default; enter makes it"}
	var cant []string // providers it can't run, and why: one row says them
	for _, id := range agent.ProviderIDs() {
		k, warn, why := agent.Compat(id, h)
		def := k != "" && agent.HarnessOf(m.provKind(id)) == h
		name := provLabel(id)
		if why != "" {
			cant = append(cant, name+": "+why)
			continue
		}
		sec.rows = append(sec.rows, setting{
			label: name,
			line: func(int) string {
				mark, now := "  ", ""
				switch {
				case def:
					mark, now = paint(cOrange, "★ "), dim("the default")
				case warn != "":
					now = paint(cYellow, "⚠ ") + dim(warn)
				}
				return mark + paint(cText, fit(name, 26)) + now
			},
			key: func(s string) (tea.Cmd, bool) {
				if s != "*" && s != "enter" && s != "space" {
					return nil, false
				}
				if !def {
					cfg.SetRunsIn(id, string(h))
					_ = m.store.SaveConfig()
				}
				return nil, true
			},
			keys: []string{"enter", "make default"},
			about: func() (string, string, string) {
				what := name + " in " + hn + ": #new " + agent.HarnessWord(h) + "@" + agent.ProviderWord(id) + " starts one here once."
				switch {
				case def:
					return name, what, "The default: new " + name + " sessions start in " + hn + "."
				case warn != "":
					return name, what, "It " + warn + ", on " + hn + "'s own sign-in. enter makes it the default."
				}
				return name, what, "enter makes " + hn + " the default for " + name + "."
			},
		})
	}
	if len(cant) > 0 {
		n := fmt.Sprintf("%d more can't run %s", len(cant), hn)
		sec.rows = append(sec.rows, setting{label: n, line: func(int) string { return "  " + faint(n) },
			about: func() (string, string, string) { return n, strings.Join(cant, ". ") + ".", "" }})
	}
	return sec
}
