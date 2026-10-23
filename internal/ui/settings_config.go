package ui

import (
	"slices"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Config is one table: harnesses down the side, providers across. A
// harness switches on or off as a whole; a cell is where that provider
// starts by default
// provider (★), one it may run in (●), one it may but doesn't (○), or
// one it can't (·). enter on a cell picks the models it may call there.
// Under the table, what rush does at a usage limit. See
// docs/providers-harnesses.md.

// cellPick is the models list of one cell, open under the table.
type cellPick struct {
	id     string
	h      agent.Kind
	kind   agent.Kind // the agent that runs id in h
	cursor int        // 0 is "every model", then the models
}

func configPage() page {
	return page{
		name: "Config",
		pre: func(m *Model, s string) (tea.Cmd, bool) {
			if m.dialog.cellPick != nil {
				return m.cellPickKey(s), true
			}
			return nil, false
		},
		body: (*Model).configBody,
		key:  (*Model).configKey,
		rows: func(m *Model) int { return len(m.configHarnesses()) + 1 },
	}
}

// configHarnesses are the table's rows: every harness some provider can
// run in.
func (m *Model) configHarnesses() []agent.Kind {
	var out []agent.Kind
	for _, h := range agent.HarnessKinds() {
		for _, id := range m.configProviders() {
			if k, _, _ := agent.Compat(id, h); k != "" {
				out = append(out, h)
				break
			}
		}
	}
	return out
}

// configProviders are the table's columns: every installed provider, the
// ones set up first.
func (m *Model) configProviders() []string {
	items := m.provItems()
	out := make([]string, 0, len(items))
	for _, it := range items {
		out = append(out, it.provider)
	}
	return out
}

// cellMark is a cell as the table shows it, 2 cells wide, and how many
// models it's kept to; why is set when the two can't go together.
func (m *Model) cellMark(id string, h agent.Kind) (mark, why string) {
	cfg := &m.store.Config
	k, warn, why := m.fits(id, h)
	switch {
	case k == "":
		return faint("·"), "it doesn't run there"
	case why != "":
		return faint("·"), why
	}
	col := cGreen
	if warn != "" {
		col = cYellow
	}
	on := cfg.CellOn(id, h)
	switch {
	case !cfg.HarnessOn(h):
		mark = faint("○")
	case cfg.HarnessFor(id) == h && on:
		mark = paint(cOrange, "★")
	case on:
		mark = paint(col, "●")
	default:
		mark = faint("○")
	}
	if n := len(cfg.CellModelsOf(id, h)); n > 0 {
		mark += dim(" " + strconv.Itoa(n))
	}
	return mark, ""
}

const (
	configNameW = 16 // the harness column
	configColW  = 9  // each provider's
)

// configHead is the table's two heading lines: each provider shown, and
// how it's paid for, × when it isn't set up; then how many more there are.
func (m *Model) configHead(ids []string, from, to int, inTable bool, w int) []string {
	var head1, head2 strings.Builder
	head1.WriteString(fit("", configNameW+2))
	head2.WriteString(fit("", configNameW+2))
	for i := from; i < to; i++ {
		id := ids[i]
		pr, _ := agent.Billed(id)
		name := ansi.Truncate(agent.ProviderLabel(pr), configColW-1, "…")
		sub := billWord(id)
		if sub == "subscription" {
			sub = "plan"
		}
		if !m.provSetUp(id) {
			sub = strings.TrimSpace(sub + " ×")
		}
		style := dim
		if inTable && m.dialog.configCol == i+1 {
			style = func(s string) string { return paint(cOrange+bold, s) }
		}
		head1.WriteString(style(fit(name, configColW)))
		head2.WriteString(faint(fit(sub, configColW)))
	}
	more := ""
	if from > 0 {
		more = "‹ " + strconv.Itoa(from) + " more  "
	}
	if to < len(ids) {
		more += strconv.Itoa(len(ids)-to) + " more ›"
	}
	return []string{fit(head1.String()+faint(more), w), head2.String()}
}

// configRows are the table's rows: each harness, on or off, then its
// cell for each provider shown.
func (m *Model) configRows(hs []agent.Kind, ids []string, from, to int) []string {
	d := m.dialog
	cfg := &m.store.Config
	out := make([]string, 0, len(hs))
	for r, h := range hs {
		dot, name := paint(cGreen, "●"), paint(cText, fit(agent.HarnessLabel(h), configNameW-2))
		switch {
		case !cfg.HarnessOn(h):
			dot, name = faint("○"), faint(fit(agent.HarnessLabel(h), configNameW-2))
		case !agent.Runs(h):
			name = dim(fit(agent.HarnessLabel(h), configNameW-2))
		}
		var line strings.Builder
		if r == d.cursor && d.configCol == 0 {
			line.WriteString(paint(cOrange, "▍") + " ")
		} else {
			line.WriteString("  ")
		}
		line.WriteString(dot + " " + name)
		for i := from; i < to; i++ {
			mark, _ := m.cellMark(ids[i], h)
			if r == d.cursor && d.configCol == i+1 {
				line.WriteString(paint(cOrange, "▸") + fit(" "+mark, configColW-1))
			} else {
				line.WriteString(fit("  "+mark, configColW))
			}
		}
		out = append(out, line.String())
	}
	return out
}

func (m *Model) configBody(w int) []string {
	d := m.dialog
	cfg := &m.store.Config
	hs, ids := m.configHarnesses(), m.configProviders()
	if len(hs) == 0 || len(ids) == 0 {
		return []string{dim("No provider is installed yet: Settings › Providers says how to add one.")}
	}
	d.cursor = max(0, min(d.cursor, len(hs)))
	d.configCol = max(0, min(d.configCol, len(ids)))
	// Columns that fit, around the one in focus.
	fits := max(1, (w-configNameW-4-16)/configColW) // 16 for "‹ n more  n more ›"
	from, to := window(len(ids), max(0, d.configCol-1), fits)

	out := m.configHead(ids, from, to, d.cursor < len(hs), w)
	table := m.configRows(hs, ids, from, to)
	legend := []string{""}
	for _, l := range wrap("● on   ○ off   ★ its default harness   · can't   ● yellow: on the harness's own sign-in   a number: only that many models", w-2) {
		legend = append(legend, faint(l))
	}
	// What rush does at a limit.
	limit := map[string]string{
		state.AtLimitAccount: "another account of the same provider",
		state.AtLimitWait:    "wait for the reset",
		state.AtLimitNext:    "move on to the next provider with room",
	}
	row := fit(dim("At a usage limit  ")+faint("‹ ")+paint(cText, limit[cfg.AtLimit])+faint(" ›"), w-2)
	tail := []string{"", "  " + row}
	if d.cursor == len(hs) {
		tail[1] = highlight(paint(cOrange, "▍")+" "+row, w)
	}

	// What's under it: the cell's models, or what's in focus. The page
	// fits the screen: the legend goes first, then the table scrolls.
	avail := max(6, m.settingsHeight()-len(m.settingsHeading(w)))
	var under []string
	if p := d.cellPick; p != nil {
		under = m.cellPickLines(p, w, max(3, avail-len(out)-min(len(table), 4)-len(tail)-4))
	} else {
		under = m.configAbout(hs, ids, w)
	}
	under = append([]string{""}, under...)
	if len(out)+len(table)+len(legend)+len(tail)+len(under) > avail {
		legend = nil
	}
	if extra := len(out) + len(table) + len(tail) + len(under) - avail; extra > 0 {
		keep := max(1, len(table)-extra)
		from, to := window(len(table), min(d.cursor, len(table)-1), keep)
		table = table[from:to]
	}
	out = append(append(append(out, table...), legend...), tail...)
	out = append(out, under...)
	if len(out) > avail {
		out = out[:avail]
	}
	return out
}

// configAbout says what the part in focus is, and its keys.
func (m *Model) configAbout(hs []agent.Kind, ids []string, w int) []string {
	d := m.dialog
	cfg := &m.store.Config
	var title, what string
	keys := make([]string, 0, 6)
	switch {
	case d.cursor >= len(hs):
		title = "At a usage limit"
		what = "What rush does when the account a session runs on is nearly out. Moving on hands the conversation, with what it was doing, to the next provider set up here that has room, and new sessions start there until the first resets."
		keys = []string{"←→", "change"}
	case d.configCol == 0:
		h := hs[d.cursor]
		title = agent.HarnessLabel(h)
		what = title + " is on: rush may start sessions in it."
		if !cfg.HarnessOn(h) {
			what = title + " is off: rush starts no session in it, nor runs a subagent in it; its providers start in their next harness that's on."
		}
		if !agent.Runs(h) {
			what += " It isn't installed here."
		}
		keys = []string{"space", "on or off", "enter", "its settings", "→", "cells"}
	default:
		h, id := hs[d.cursor], ids[d.configCol-1]
		title = provLabel(id) + " in " + agent.HarnessLabel(h)
		_, warn, _ := m.fits(id, h)
		_, why := m.cellMark(id, h)
		switch {
		case why != "":
			what = "It can't: " + why + "."
		case !cfg.HarnessOn(h):
			what = agent.HarnessLabel(h) + " is off."
		case cfg.HarnessFor(id) == h && cfg.CellOn(id, h):
			what = "★ " + provLabel(id) + " starts here by default, and " + agent.HarnessLabel(h) + " may call its models for the chats it leads and their subagents."
		case cfg.CellOn(id, h):
			what = "On: " + agent.HarnessLabel(h) + " may call " + provLabel(id) + "'s models, for the chats it leads and their subagents; by default " + provLabel(id) + " starts in " + agent.HarnessLabel(cfg.HarnessFor(id)) + "."
		default:
			what = "Off: it could run here; space switches it on."
		}
		if warn != "" && why == "" {
			what += " It " + warn + "."
		}
		if why == "" {
			if ms := cfg.CellModelsOf(id, h); len(ms) > 0 {
				what += " It may call " + strings.Join(ms, ", ") + "."
			} else {
				what += " It may call every model."
			}
			keys = []string{"space", "on or off", "*", "its default harness", "enter", "models"}
		}
	}
	out := []string{rule("About "+title, "", w)}
	for _, l := range wrap(what, w-4) {
		out = append(out, "  "+paint(cSub, l))
	}
	out = append(out, "")
	return append(out, keysControls(w, append(keys, "[ ]", "page", "esc", "back")...)...)
}

func (m *Model) configKey(s string) tea.Cmd {
	d := m.dialog
	cfg := &m.store.Config
	hs, ids := m.configHarnesses(), m.configProviders()
	if len(hs) == 0 || len(ids) == 0 {
		return nil
	}
	save := func() { _ = m.store.SaveConfig(); m.spillTo() }
	if d.cursor >= len(hs) {
		vals := []string{state.AtLimitAccount, state.AtLimitWait, state.AtLimitNext}
		dir := 0
		switch s {
		case "right", "l", "enter", "space":
			dir = 1
		case "left", "h":
			dir = -1
		}
		if dir != 0 {
			cfg.AtLimit = vals[roundMove(max(0, slices.Index(vals, cfg.AtLimit)), dir, len(vals))]
			cfg.SyncLegacy()
			save()
		}
		return nil
	}
	h := hs[d.cursor]
	switch s {
	case "left", "h":
		d.configCol = max(0, d.configCol-1)
		return nil
	case "right", "l":
		d.configCol = min(len(ids), d.configCol+1)
		return nil
	}
	if d.configCol == 0 {
		switch s {
		case "space":
			cfg.SetHarnessOn(h, !cfg.HarnessOn(h))
			save()
		case "enter":
			m.openDialog(pageHarnesses)
			nd := m.dialog
			nd.harnessReturn = max(0, slices.Index(catalogHarnesses(), h)+1)
			nd.harnessInside, nd.cursor, nd.modelKind = true, 0, h
			return m.loadModels(string(h))
		}
		return nil
	}
	id := ids[d.configCol-1]
	k, _, why := m.fits(id, h)
	if s != "space" && s != "*" && s != "enter" {
		return nil
	}
	if k == "" || why != "" {
		_, why = m.cellMark(id, h)
		m.flash(provLabel(id)+" in "+agent.HarnessLabel(h)+": "+why, true)
		return nil
	}
	switch s {
	case "space":
		if cfg.StarHarness(id) == h {
			m.flash(provLabel(id)+" starts here by default · * on another cell moves that first", false)
			return nil
		}
		cfg.SetUses(id, string(h), !cfg.Uses(id, string(h)))
		save()
	case "*":
		cfg.SetRunsIn(id, string(h))
		cfg.SetUses(id, string(h), false) // the ★ is on by being it
		if !cfg.HarnessOn(h) {
			cfg.SetHarnessOn(h, true)
		}
		save()
		m.flash(provLabel(id)+" starts in "+agent.HarnessLabel(h)+" by default", false)
	case "enter":
		d.cellPick = &cellPick{id: id, h: h, kind: k}
		return m.loadModels(string(k))
	}
	return nil
}

// cellModels are what cell p's agent offers, then any it's kept to that
// the list lacks.
func (m *Model) cellModels(p *cellPick) []agent.Choice {
	out := m.agentModels(p.kind)
	for _, v := range m.store.Config.CellModelsOf(p.id, p.h) {
		if !slices.ContainsFunc(out, func(c agent.Choice) bool { return c.ID == v }) {
			out = append(out, agent.Choice{ID: v})
		}
	}
	return out
}

func (m *Model) cellPickLines(p *cellPick, w, h int) []string {
	cfg := &m.store.Config
	kept := cfg.CellModelsOf(p.id, p.h)
	def := cfg.Dispatch.StartOn(p.id, p.kind).Model
	out := []string{rule("Models · "+provLabel(p.id)+" in "+agent.HarnessLabel(p.h), "the ones it may call; ★ starts with", w)}
	rows := []string{}
	all := faint("○")
	if len(kept) == 0 {
		all = paint(cGreen, "●")
	}
	rows = append(rows, all+" "+paint(cText, "every model"))
	models := m.cellModels(p)
	for _, c := range models {
		mark := faint("○")
		if len(kept) == 0 || slices.Contains(kept, c.ID) {
			mark = paint(cGreen, "●")
		}
		star := "  "
		if c.ID == def {
			star = paint(cOrange, "★ ")
		}
		rows = append(rows, mark+" "+star+paint(cText, fit(modelWord(string(p.kind), c.ID), 28))+" "+faint(c.Note))
	}
	if len(models) == 0 {
		rows = append(rows, faint("  "+agent.HarnessLabel(p.h)+" lists no models here; it starts on its own default"))
	}
	p.cursor = max(0, min(p.cursor, len(models)))
	from, to := window(len(rows), p.cursor, h)
	for i := from; i < to; i++ {
		line := rows[i]
		if i == p.cursor {
			out = append(out, highlight(paint(cOrange, "▍")+" "+line, w))
		} else {
			out = append(out, "  "+line)
		}
	}
	out = append(out, "")
	return append(out, keysControls(w, "↑↓", "model", "space", "may call it or not", "*", "starts with it", "esc", "done")...)
}

func (m *Model) cellPickKey(s string) tea.Cmd {
	d := m.dialog
	p := d.cellPick
	cfg := &m.store.Config
	models := m.cellModels(p)
	switch s {
	case "esc", "enter", "q", "backspace", "left":
		d.cellPick = nil
	case "up", "k":
		p.cursor = max(0, p.cursor-1)
	case "down", "j":
		p.cursor = min(len(models), p.cursor+1)
	case "space", " ":
		if p.cursor == 0 {
			cfg.SetCellModels(p.id, p.h, nil)
			break
		}
		v := models[p.cursor-1].ID
		kept := slices.Clone(cfg.CellModelsOf(p.id, p.h))
		switch {
		case len(kept) == 0:
			// From every model to every one but this.
			for _, c := range models {
				if c.ID != v {
					kept = append(kept, c.ID)
				}
			}
		case slices.Contains(kept, v):
			kept = slices.DeleteFunc(kept, func(x string) bool { return x == v })
			if len(kept) == 0 {
				m.flash("it has to call one: space on every model gives it all again", true)
				return nil
			}
		default:
			kept = append(kept, v)
			if len(kept) == len(models) {
				kept = nil
			}
		}
		cfg.SetCellModels(p.id, p.h, kept)
	case "*":
		if p.cursor == 0 {
			return nil
		}
		v := models[p.cursor-1].ID
		st := cfg.Dispatch.StartOn(p.id, p.kind)
		if st.Model == v {
			v = ""
		}
		st.Model = v
		cfg.Dispatch.SetStartOn(p.id, p.kind, st)
		if v != "" && !cfg.Calls(p.id, p.h, v) {
			cfg.SetCellModels(p.id, p.h, append(slices.Clone(cfg.CellModelsOf(p.id, p.h)), v))
		}
	default:
		return nil
	}
	_ = m.store.SaveConfig()
	return nil
}
