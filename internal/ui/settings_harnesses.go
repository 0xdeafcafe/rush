package ui

import (
	"slices"
	"strconv"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Harnesses is Every harness, then each harness: what every one starts
// with, said once, then what one changes from it, with its sign-in and
// its own settings. Which providers each runs, and their models, are
// Settings › Config.

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

// harnessItems are the list's lines: Every harness (""), then each harness.
func harnessItems() []agent.Kind { return append([]agent.Kind{""}, catalogHarnesses()...) }

func (m *Model) harnessesBody(w int) []string {
	m.applySettingsModels()
	d := m.dialog
	catalog := m.harnessCatalogSections()
	picked := d.cursor
	if d.harnessInside {
		picked = d.harnessReturn
	}
	items := harnessItems()
	picked = max(0, min(picked, len(items)-1))
	if d.harnessInside {
		d.harnessReturn = picked
	} else {
		d.cursor = picked
	}
	if h := items[picked]; h == "" || agent.HarnessOf(d.modelKind) != agent.HarnessOf(h) {
		d.modelKind = h
	}
	return m.catalogBody(catalog, picked, d.harnessInside, false, w, func(dw int) ([]string, int) {
		if d.modelKind == "" {
			return m.everyDetail(dw)
		}
		return m.catalogDetail(m.harnessDetailSections(), d.modelKind, "", d.harnessInside, dw)
	})
}

// everyDetail is Every harness's side of the page.
func (m *Model) everyDetail(w int) ([]string, int) {
	d := m.dialog
	secs := m.everySections()
	cur := -1
	if d.harnessInside {
		cur = max(0, min(d.cursor, len(flat(secs))-1))
		d.cursor = cur
	}
	rows := []string{paint(cText+bold, "Every harness"), dim("What every harness starts with; each one shows only what it changes.")}
	rows = append(rows, m.formRows(secs, cur, w)...)
	if d.harnessInside {
		rows = append(rows, m.about(rowAt(secs, cur), w)...)
	}
	return rows, m.frameCursor(rows)
}

func (m *Model) harnessesSections() []section {
	if !m.dialog.harnessInside {
		return m.harnessCatalogSections()
	}
	if m.dialog.modelKind == "" {
		return m.everySections()
	}
	return m.harnessDetailSections()
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

// everyEfforts are the efforts Every harness can name: each harness that
// offers one takes it, the rest keep their own.
var everyEfforts = []string{"low", "medium", "high", "xhigh", "max"}

// everySections are Every harness's rows: effort and permissions, then
// attribution.
func (m *Model) everySections() []section {
	d := &m.store.Config.Dispatch
	set := func(f func(*state.Start, string)) func(string) {
		return func(v string) { f(&d.Every, v) }
	}
	effort := make([][2]string, 0, 1+len(everyEfforts))
	effort = append(effort, [2]string{"", "each harness thinks as its own settings say."})
	for _, e := range everyEfforts {
		effort = append(effort, [2]string{e, "every harness that offers " + e + " starts with it; the rest keep their own."})
	}
	er := choiceSetting("Effort", d.Every.Effort, "How hard a new session thinks before acting: more is slower and spends more.", effort, set(func(s *state.Start, v string) { s.Effort = v }))
	er.unset = "each harness's own"
	pr := choiceSetting("Permissions", d.Every.Mode, "What a new session may do without asking you.",
		[][2]string{
			{"", "each harness asks as its own settings say."},
			{state.ModeApproveAll, "every harness that can approves every tool without asking: bypassPermissions, full-access, yolo, as each calls it."},
		}, set(func(s *state.Start, v string) { s.Mode = v }))
	pr.unset = "each harness's own"
	pr.names = map[string]string{state.ModeApproveAll: "approve everything"}
	return []section{
		{title: "New sessions start with", note: "sessions rush starts; running ones keep theirs", rows: []setting{er, pr}},
		{title: "Every agent", note: "set once here, kept in each agent's own settings", rows: []setting{m.attributionSetting()}},
	}
}

// harnessChanges is how many of Every harness's settings harness h changes.
func (m *Model) harnessChanges(h agent.Kind) int {
	st, n := m.store.Config.Dispatch.HarnessStart(agent.HarnessOf(h)), 0
	if st.Effort != "" {
		n++
	}
	if st.Mode != "" {
		n++
	}
	return n
}

func (m *Model) harnessDetailSections() []section {
	k := m.dialog.modelKind
	secs := []section{m.changesSection(k)}
	if auth, ok := m.nativeAuthSection(k); ok {
		secs = append(secs, auth)
	} else if !agent.KeyOnly(k) {
		if accounts, ok := m.accountSection(k); ok {
			secs = append(secs, accounts)
		}
	}
	secs = append(secs, m.agentSections(k)...)
	return append(secs, m.harnessConfigLink(k))
}

// changesSection is what harness k's sessions start with where it
// differs from Every harness: unset is Every harness's.
func (m *Model) changesSection(k agent.Kind) section {
	name := agent.HarnessLabel(k)
	h := agent.HarnessOf(k)
	d := &m.store.Config.Dispatch
	sec := section{title: name, note: "what it changes from every harness"}
	if !agent.Supports(k, agent.FeatureRun) {
		sec.rows = append(sec.rows, setting{label: "rush can't start its sessions yet", line: func(int) string {
			return faint("rush can't start " + name + " sessions yet, so there's nothing to choose here.")
		}})
		return sec
	}
	ch, _ := agent.ChoicesOf(k)
	change := func(f func(*state.Start, string)) func(string) {
		return func(v string) {
			s := d.HarnessStart(h)
			f(&s, v)
			d.SetHarnessStart(h, s)
		}
	}
	was := func(every string) string {
		if every == "" {
			return "as every harness"
		}
		return "as every harness · " + every
	}
	st := d.HarnessStart(h)
	row := func(label, value, every, what string, list []agent.Choice, set func(*state.Start, string)) setting {
		pairs := make([][2]string, 0, 1+len(list))
		pairs = append(pairs, [2]string{"", "as every harness says."})
		for _, c := range list {
			pairs = append(pairs, [2]string{c.ID, c.Note})
		}
		r := choiceSetting(label, value, what, pairs, change(set))
		r.unset = was(every)
		if value != "" && every != "" {
			r.names = map[string]string{value: value + " · was " + every}
		}
		return r
	}
	if agent.Supports(k, agent.FeatureEffort) {
		r := row("Effort", st.Effort, d.Every.Effort, "How hard a new "+name+" session thinks before acting.", ch.Efforts,
			func(s *state.Start, v string) { s.Effort = v })
		r.typed = len(ch.Efforts) == 0
		sec.rows = append(sec.rows, r)
	}
	if agent.Supports(k, agent.FeatureModes) {
		every := d.Every.Mode
		if every == state.ModeApproveAll {
			every = "approve everything"
		}
		r := row("Permissions", st.Mode, every, "What a new "+name+" session may do without asking you.", ch.Modes,
			func(s *state.Start, v string) { s.Mode = v })
		r.typed = len(ch.Modes) == 0
		sec.rows = append(sec.rows, r)
	}
	if len(sec.rows) == 0 {
		sec.rows = append(sec.rows, setting{label: "nothing to change", line: func(int) string {
			return faint(name + " has no effort or permissions of its own to set.")
		}})
	}
	return sec
}

// harnessConfigLink opens Settings › Config on harness k's row.
func (m *Model) harnessConfigLink(k agent.Kind) section {
	label := "Providers it runs, and their models"
	return section{rows: []setting{{
		label: label,
		line:  func(int) string { return dim("→ ") + paint(cText, label) + faint(" · on Config") },
		what:  "Which providers " + agent.HarnessLabel(k) + " runs, whether it's on, and the models each may call in it: Settings › Config.",
		keys:  []string{"enter", "open"},
		key: func(s string) (tea.Cmd, bool) {
			if s != "enter" && s != "right" {
				return nil, false
			}
			m.openDialog(pageConfig)
			m.dialog.cursor = max(0, slices.Index(m.configHarnesses(), agent.HarnessOf(k)))
			return nil, true
		},
	}}}
}

func (m *Model) harnessCatalogSections() []section {
	d := m.dialog
	enter := func(i int, k agent.Kind) func(string) (tea.Cmd, bool) {
		return func(s string) (tea.Cmd, bool) {
			if s != "enter" && s != "right" {
				return nil, false
			}
			d.harnessInside = true
			d.harnessReturn = i
			d.cursor = 0
			d.modelKind = k
			if k == "" {
				return nil, true
			}
			return m.loadModels(string(k)), true
		}
	}
	harnesses := catalogHarnesses()
	rows := make([]setting, 0, 1+len(harnesses))
	rows = append(rows, setting{label: "Every harness", line: func(int) string {
		return paint(cText+bold, fit("Every harness", 24)) + dim("what they all start with")
	}, what: "What every harness starts with: effort, permissions, credit in commits.", key: enter(0, ""), keys: []string{"right", "open"}})
	for i, k := range harnesses {
		name := agent.HarnessLabel(k)
		rows = append(rows, setting{label: name, line: func(int) string {
			status := dim("as every harness")
			switch n := m.harnessChanges(k); {
			case !agent.Runs(k):
				status = faint("not installed")
			case !m.store.Config.HarnessOn(agent.HarnessOf(k)):
				status = faint("off")
			case n == 1:
				status = paint(cOrange, "1 change")
			case n > 1:
				status = paint(cOrange, strconv.Itoa(n)+" changes")
			}
			return paint(cText, fit(name, 24)) + status
		}, what: name + " runs the model. What it changes from every harness, its sign-in and its own settings are beside the list.", key: enter(i+1, k), keys: []string{"right", "open"}})
	}
	return []section{{rows: rows}}
}
