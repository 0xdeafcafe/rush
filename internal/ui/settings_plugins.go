package ui

import (
	"cmp"
	"os/exec"
	"runtime"
	"slices"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/keymap"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// Settings, Plugins lists rush's plugins, the bundled ones and those
// installed in plugin.Root(): which run, which don't and why, and what
// each needs to run here. enter opens one: on or off, its settings, and
// its commands' keys. rush keeps the values, and the broker hands them
// to the plugin.

// pluginsPage is a func, not a var: a command's row goes to Keys, which
// goes through the pages.
func pluginsPage() page {
	return page{
		name: "Plugins",
		head: func(m *Model, w int) []string {
			if m.dialog.plugin != "" {
				return nil
			}
			return []string{faint(ansi.Truncate("Installed in "+tildify(plugin.Root())+" · rush plugin approve <name> runs a new one", w, "…"))}
		},
		form: (*Model).pluginSections,
		pre: func(m *Model, s string) (tea.Cmd, bool) {
			d := m.dialog
			if d.plugin != "" && (s == "esc" || s == "q") {
				d.plugin, d.cursor = "", 0
				return nil, true
			}
			return nil, false
		},
	}
}

// pluginRow is one plugin as Plugins lists it, read off the UI.
type pluginRow struct {
	m       plugin.Manifest
	bundled bool
	runs    bool     // on, and able to run here
	status  string   // how it stands: on, off, not approved, why it can't run
	missing []string // the programs it requires that aren't on PATH
}

// readPlugins reads every plugin and how it stands. It reads their
// folders, so never on the UI.
func readPlugins() []pluginRow {
	var out []pluginRow
	on := plugin.BundlesOn()
	bundles := plugin.Bundles()
	for i := range bundles {
		b := &bundles[i]
		r := pluginRow{m: b.Manifest, bundled: true, runs: true, status: "bundled, on", missing: notOnPath(b.Manifest.Requires.Bin)}
		switch why := b.Manifest.Unmet(); {
		case why != "":
			r.runs, r.status = false, "won't run: "+why
		case !on[b.Manifest.Name]:
			r.runs, r.status = false, "bundled, off"
		}
		out = append(out, r)
	}
	installed, bad := plugin.Installed()
	approved := plugin.Approvals()
	for i := range installed {
		p := &installed[i]
		if _, ok := plugin.BundleNamed(p.Name); ok {
			continue // rush's own of that name runs instead
		}
		r := pluginRow{m: p.Manifest, runs: true, status: "on", missing: notOnPath(p.Requires.Bin)}
		a, ok := approved[p.Name]
		d, _ := plugin.Digest(p.Dir)
		switch why := p.Unmet(); {
		case why != "":
			r.runs, r.status = false, "won't run: "+why
		case !ok:
			r.runs, r.status = false, "not approved"
		case d != a.Digest:
			r.runs, r.status = false, "changed since approved"
		}
		out = append(out, r)
	}
	names := make([]string, 0, len(bad))
	for n := range bad {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		out = append(out, pluginRow{m: plugin.Manifest{Name: n, Description: bad[n].Error()}, status: "not a valid plugin"})
	}
	return out
}

// notOnPath are those of bins not found on PATH. It looks, so never on
// the UI.
func notOnPath(bins []string) []string {
	var out []string
	for _, b := range bins {
		if _, err := exec.LookPath(b); err != nil {
			out = append(out, b)
		}
	}
	return out
}

// needCells are what a plugin requires, a cell each: ✓ met here, – not.
// Its programs are met unless missing says otherwise.
func needCells(r plugin.Requires, missing []string) string {
	var cells []string
	cell := func(words string, met bool) {
		if met {
			cells = append(cells, paint(cGreen, "✓ ")+paint(cText, words))
		} else {
			cells = append(cells, faint("– "+words))
		}
	}
	for _, o := range r.OS {
		cell(cmp.Or(plugin.OSNames[o], o), o == runtime.GOOS)
	}
	for _, a := range r.Arch {
		cell(a, a == runtime.GOARCH)
	}
	for _, b := range r.Bin {
		cell(b+" on PATH", !slices.Contains(missing, b))
	}
	if len(cells) == 0 {
		return faint("nothing; it runs anywhere")
	}
	return strings.Join(cells, "  ")
}

// askCells are what a plugin asks rush for, a plain cell each: what it
// does in rush's screen, whether it offers tools, and what it may do with
// sessions.
func askCells(p plugin.UIPlugin, mf *plugin.Manifest) string {
	var cells []string
	for _, c := range p.UI {
		cells = append(cells, uiCapWords[c])
	}
	if mf.HasTools() {
		cells = append(cells, "offers tools")
	}
	for _, c := range mf.Sessions {
		cells = append(cells, "sessions: "+c)
	}
	if len(cells) == 0 {
		return faint("commands and settings only")
	}
	return dim(strings.Join(cells, "   "))
}

// pluginNeeds is what a plugin needs to run, and where it starts sessions.
func pluginNeeds(mf *plugin.Manifest) string {
	var parts []string
	if n := mf.Requires.Needs(); n != "" {
		parts = append(parts, "needs "+n)
	}
	if len(mf.Workspaces) > 0 {
		ws := make([]string, len(mf.Workspaces))
		for i, w := range mf.Workspaces {
			ws[i] = tildify(w)
		}
		parts = append(parts, "sessions in "+strings.Join(ws, ", "))
	}
	if len(parts) == 0 {
		return "runs anywhere"
	}
	return strings.Join(parts, " · ")
}

// uiPlugin is the running plugin called name, as the broker last said.
func (m *Model) uiPlugin(name string) (plugin.UIPlugin, bool) {
	if m.hooks == nil {
		return plugin.UIPlugin{}, false
	}
	for _, p := range m.hooks.State().Plugins {
		if p.Name == name {
			return p, true
		}
	}
	return plugin.UIPlugin{}, false
}

func (m *Model) pluginSections() []section {
	d := m.dialog
	if ps, ok := d.pluginsRead.take(); ok {
		d.plugins, d.pluginsRead = ps, nil
	}
	if d.plugin != "" {
		return []section{m.pluginPage(d.plugin)}
	}
	if d.plugins == nil && d.pluginsRead != nil {
		return []section{{rows: []setting{{label: "reading", line: func(int) string { return dim("reading plugins…") }}}}}
	}
	rows := d.plugins
	// One the broker runs that wasn't read from disk still shows.
	if m.hooks != nil {
		for _, p := range m.hooks.State().Plugins {
			if !slices.ContainsFunc(rows, func(r pluginRow) bool { return r.m.Name == p.Name }) {
				rows = append(rows, pluginRow{m: plugin.Manifest{Name: p.Name, UI: p.UI}, runs: true, status: "on"})
			}
		}
	}
	// A section for each kind of plugin, by what they do; in each, the
	// ones running first.
	rows = slices.Clone(rows)
	slices.SortStableFunc(rows, func(a, b pluginRow) int {
		if a.runs != b.runs {
			if a.runs {
				return -1
			}
			return 1
		}
		return strings.Compare(a.m.Name, b.m.Name)
	})
	var out []section
	for _, c := range plugin.Categories {
		s := section{title: c.Title, note: c.About}
		for i := range rows {
			if rows[i].m.CategoryOf().ID == c.ID {
				s.rows = append(s.rows, m.pluginListRow(&rows[i]))
			}
		}
		if len(s.rows) > 0 {
			out = append(out, s)
		}
	}
	if len(out) == 0 {
		return []section{{title: "Plugins", rows: []setting{{
			label: "none",
			line: func(w int) string {
				return dim("No plugins. ") + faint("Put one in "+tildify(plugin.Root())+"/<name>, then rush plugin approve <name>.")
			},
			about: func() (string, string, string) {
				return "Plugins", "rush's own plugins, not your agent's: those are under a Session's Settings tab.", "None installed."
			},
		}}}}
	}
	return out
}

// pluginListRow is a plugin on the list; enter opens it.
func (m *Model) pluginListRow(r *pluginRow) setting {
	name, needs := r.m.Name, pluginNeeds(&r.m)
	mark := paint(cGreen, "●")
	if !r.runs {
		mark = dim("○")
	}
	return setting{
		label: name,
		line: func(w int) string {
			return mark + " " + paint(cText+bold, fit(name, 22)) + " " + dim(fit(r.status, 30)) + " " + faint(ansi.Truncate(needs, max(0, w-56), "…"))
		},
		about: func() (string, string, string) {
			return name, firstNonEmpty(r.m.Description, "no description"), r.status + " · " + needs
		},
		key: func(s string) (tea.Cmd, bool) {
			if s == "enter" || s == "right" || s == "l" {
				m.dialog.plugin, m.dialog.cursor = name, 0
				return nil, true
			}
			return nil, false
		},
		keys: []string{"enter", "open"},
	}
}

// pluginPage is one plugin open: on or off, what it needs, then its
// settings and commands while it runs.
func (m *Model) pluginPage(name string) section {
	r := &pluginRow{}
	for i := range m.dialog.plugins {
		if m.dialog.plugins[i].m.Name == name {
			r = &m.dialog.plugins[i]
		}
	}
	up, running := m.uiPlugin(name)
	if !running {
		up.UI = r.m.UI
	}
	sec := section{title: name, note: firstNonEmpty(r.status, "on")}
	if r.bundled {
		b, _ := plugin.BundleNamed(name)
		on, ok := m.bundledOn[name]
		if !ok {
			on = !b.Optional
		}
		v, what := "off", "Comes with rush, and is on until you turn it off."
		if on {
			v = "on"
		}
		if b.Optional {
			what = "Comes with rush, and is off until you turn it on."
		}
		sec.rows = append(sec.rows, setting{
			label: "Runs", value: v, choices: []string{"on", "off"},
			what:  firstNonEmpty(r.m.Description, what),
			means: map[string]string{"on": "it runs, and " + capWords(up), "off": "it doesn't run"},
			run:   func(v string) tea.Cmd { return m.setBundled(name, v == "on") },
		})
	}
	needs := pluginNeeds(&r.m)
	sec.rows = append(sec.rows, setting{
		label: "requires",
		line: func(w int) string {
			return fit("Requires", 16) + ansi.Truncate(needCells(r.m.Requires, r.missing), max(0, w-16), "…")
		},
		about: func() (string, string, string) {
			return "Requires", "What the system must offer for it to run, and the folders sessions it starts may run in: ✓ met here, – not.", needs
		},
	}, setting{
		label: "asks for",
		line: func(w int) string {
			return fit("Asks for", 16) + ansi.Truncate(askCells(up, &r.m), max(0, w-16), "…")
		},
		about: func() (string, string, string) {
			return "Asks for", "What it may do in rush's screen and with sessions, as its manifest asks.", capWords(up)
		},
	})
	switch r.status {
	case "not approved", "changed since approved":
		sec.rows = append(sec.rows, setting{
			label: "approve",
			line: func(int) string {
				return paint(cYellow, r.status+": ") + faint("rush plugin approve "+name+" shows what it may do, and runs it")
			},
			what: "A plugin runs only once you've approved it as it is: approving again is asked for whenever its files change.",
		})
	case "not a valid plugin":
		sec.rows = append(sec.rows, setting{
			label: "error",
			line:  func(w int) string { return paint(cRed, ansi.Truncate(r.m.Description, w, "…")) },
			what:  r.m.Description,
		})
	}
	for _, spec := range up.Settings {
		sec.rows = append(sec.rows, m.pluginSetting(up, spec))
	}
	for _, c := range up.Commands {
		id := keymap.PluginID(name, c.Name)
		sec.rows = append(sec.rows, setting{
			label: "#" + c.Name,
			line: func(w int) string {
				k := m.keyMap().KeyText(id)
				if k == "" {
					k = "no key"
				}
				return fit("command "+c.Name, 32) + paint(cText, k) + faint("  "+c.Description)
			},
			what: c.Description + " Settings, Keys changes its key.",
			key: func(s string) (tea.Cmd, bool) {
				if s == "enter" {
					m.setSettingsPage(pageKeys)
					for i, a := range m.keyRows() {
						if a.ID == id {
							m.dialog.cursor = i
						}
					}
					return nil, true
				}
				return nil, false
			},
			keys: []string{"enter", "its key"},
		})
	}
	if up.Skipped != "" {
		sec.rows = append(sec.rows, setting{label: "skipped", line: func(int) string { return paint(cYellow, "intercepts skipped: "+up.Skipped) }, what: "It stopped answering in time, so messages go without asking it until it restarts."})
	}
	if !running && r.runs && (len(r.m.Settings) > 0 || len(r.m.Commands) > 0) {
		sec.rows = append(sec.rows, setting{label: "starting", line: func(int) string { return faint("its settings and commands show once it's running") }, what: "The broker starts it in the background."})
	}
	return sec
}

func capWords(p plugin.UIPlugin) string {
	w := make([]string, 0, len(p.UI))
	for _, c := range p.UI {
		w = append(w, uiCapWords[c])
	}
	if len(w) == 0 {
		return "commands and settings only"
	}
	return joinAnd(w)
}

var uiCapWords = map[string]string{
	plugin.UIEvents:    "hears what happens",
	plugin.UIInput:     "sees what you type",
	plugin.UIIntercept: "is asked before a message goes",
	plugin.UIOverview:  "adds to overviews",
	plugin.UINotify:    "shows notices",
	plugin.UISend:      "sends to sessions",
}

func joinAnd(xs []string) string {
	switch len(xs) {
	case 0:
		return ""
	case 1:
		return xs[0]
	}
	return strings.Join(xs[:len(xs)-1], ", ") + " and " + xs[len(xs)-1]
}

// pluginSetting is one of a plugin's settings as a row; changing it asks
// the broker, off the UI.
func (m *Model) pluginSetting(p plugin.UIPlugin, spec plugin.SettingSpec) setting {
	v := p.Values[spec.Key]
	st := setting{label: spec.Title, value: v, what: spec.Description, unset: spec.Default, means: map[string]string{}}
	switch spec.Type {
	case "bool":
		st.choices = []string{"true", "false"}
		st.means = map[string]string{"true": "on", "false": "off"}
	case "choice":
		st.choices = spec.Choices
	case "text":
		st.typed = true
	}
	name, key := p.Name, spec.Key
	st.run = func(v string) tea.Cmd {
		return m.hooks.SetSetting(name, key, v, func(err error) tea.Msg {
			return sheetMsg{apply: func(m *Model) tea.Cmd {
				if err != nil {
					m.flash(name+": "+err.Error(), true)
				}
				return nil
			}}
		})
	}
	return st
}
