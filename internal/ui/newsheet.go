package ui

import (
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// The New session sheet: a model, and the harness that runs it, "auto"
// unless you pick one. The models are every provider's that has a
// harness on in Settings › Config, as many as each may call there; auto
// is the provider's ★ harness, or its next one on that calls the model.
// It changes no default: Settings does. See docs/providers-harnesses.md.

// newPick is one model on the sheet's list: provider id's model ("" its
// default).
type newPick struct {
	id, model string
}

type newSheet struct {
	row     int        // 0 the model, 1 the harness
	pick    newPick    // the model chosen
	harness agent.Kind // the harness chosen; "" is auto
	open    bool       // the model list is open
	sel     int        // the line of the list in focus
	filter  []rune     // what's typed to narrow the list
	hits    []newHit   // where body drew what a click takes
}

// newHit is a line of the body a click takes: a row, or a model.
type newHit struct {
	y    int
	row  int
	pick int // the list's line, or -1
}

// openComposer opens the sheet on what the next session would start as.
func (m *Model) openComposer() tea.Cmd {
	o := m.nextStart(m.startDir())
	s := &newSheet{pick: newPick{id: startID(o), model: o.model}}
	if h := agent.HarnessOf(agent.Kind(o.kind)); h != s.auto(m) {
		s.harness = h
	}
	if len(m.newPicks()) == 0 {
		m.flash("rush can't run any harness here yet · Settings › Config", true)
		return nil
	}
	if !slices.Contains(m.newPicks(), s.pick) {
		s.pick = m.newPicks()[0]
		s.harness = ""
	}
	m.sheet = s
	return m.loadAllModels()
}

func (s *newSheet) width(m *Model) int { return max(64, min(96, m.w-6)) }

// newCells are the harnesses provider id may run in now, the ★ first.
func (m *Model) newCells(id string) []agent.Kind {
	cfg := &m.store.Config
	var out []agent.Kind
	for _, r := range m.routeRows() {
		if r.id == id && r.why == "" && cfg.CellOn(id, r.harness) {
			out = append(out, r.harness)
		}
	}
	if star := cfg.HarnessFor(id); slices.Contains(out, star) {
		out = slices.DeleteFunc(out, func(h agent.Kind) bool { return h == star })
		out = append([]agent.Kind{star}, out...)
	}
	return out
}

// newPicks are the sheet's models, by provider, set up ones first: each
// a harness that's on may call, once.
func (m *Model) newPicks() []newPick {
	cfg := &m.store.Config
	var out []newPick
	for _, id := range m.configProviders() {
		cells := m.newCells(id)
		if len(cells) == 0 {
			continue
		}
		var mine []newPick
		for _, h := range cells {
			k, _, _ := agent.Compat(id, h)
			for _, c := range m.models(string(k)) {
				p := newPick{id, c.ID}
				if cfg.Calls(id, h, c.ID) && !slices.Contains(mine, p) {
					mine = append(mine, p)
				}
			}
		}
		if len(mine) == 0 {
			mine = []newPick{{id, ""}} // its harness lists none: its own default
		}
		out = append(out, mine...)
	}
	return out
}

// harnesses are the harnesses that can run the model picked, the one
// auto takes first.
func (s *newSheet) harnesses(m *Model) []agent.Kind {
	var out []agent.Kind
	for _, h := range m.newCells(s.pick.id) {
		if m.store.Config.Calls(s.pick.id, h, s.pick.model) {
			out = append(out, h)
		}
	}
	return out
}

// auto is the harness auto runs the model picked in.
func (s *newSheet) auto(m *Model) agent.Kind {
	if hs := s.harnesses(m); len(hs) > 0 {
		return hs[0]
	}
	return m.store.Config.HarnessFor(s.pick.id)
}

// start is what the sheet starts: the model in its harness, with what
// Settings gives that route for the rest.
func (s *newSheet) start(m *Model) startOver {
	h := s.harness
	if h == "" || !slices.Contains(s.harnesses(m), h) {
		h = s.auto(m)
	}
	k, _, _ := agent.Compat(s.pick.id, h)
	o := m.startOn(s.pick.id, string(k))
	if s.pick.model != "" {
		o.model = s.pick.model
	}
	return o
}

// shown are the list's lines that match what's typed.
func (s *newSheet) shown(m *Model) []newPick {
	all := m.newPicks()
	q := strings.ToLower(strings.TrimSpace(string(s.filter)))
	if q == "" {
		return all
	}
	var out []newPick
	for _, p := range all {
		if strings.Contains(strings.ToLower(s.word(m, p)+" "+provLabel(p.id)), q) {
			out = append(out, p)
		}
	}
	return out
}

// word is model p in words.
func (s *newSheet) word(m *Model, p newPick) string {
	if p.model == "" {
		return "its default model"
	}
	h := m.store.Config.HarnessFor(p.id)
	k, _, _ := agent.Compat(p.id, h)
	return modelWord(string(k), p.model)
}

// cantFeatures are what a harness may lack that's worth saying before
// it starts.
var cantFeatures = []agent.Feature{agent.FeatureRewind, agent.FeatureImages, agent.FeatureResume, agent.FeatureSubagents}

func (s *newSheet) body(m *Model, w, h int) []string {
	s.hits = s.hits[:0]
	o := s.start(m)
	k := agent.Kind(o.kind)
	out := []string{sheetTitle("New session", "pick a model; rush runs it · Settings stay as they are", w), ""}

	pr, _ := agent.Billed(s.pick.id)
	model := glyph(agent.Kind(pr)) + " " + paint(cText+bold, s.word(m, s.pick)) + dim("  "+provLabel(s.pick.id))
	harness := paint(cText+bold, "auto") + dim(" · "+agent.HarnessLabel(k))
	if s.harness != "" {
		harness = paint(cText+bold, agent.HarnessLabel(k))
	}
	if len(s.harnesses(m)) > 1 {
		harness = faint("‹ ") + harness + faint(" ›")
	}
	s.hits = append(s.hits, newHit{len(out), 0, -1})
	out = append(out, sheetRow(dim(fit("model", 10))+model, s.row == 0 && !s.open, w))
	s.hits = append(s.hits, newHit{len(out), 1, -1})
	out = append(out, sheetRow(dim(fit("harness", 10))+harness, s.row == 1, w))

	// Only when it matters: what this one can't do.
	var cant []string
	for _, f := range cantFeatures {
		if !agent.Supports(k, f) {
			cant = append(cant, string(f))
		}
	}
	_, warn, _ := agent.Compat(startID(o), agent.HarnessOf(k))
	switch {
	case warn != "":
		out = append(out, "  "+paint(cYellow, "⚠ "+warn))
	case len(cant) > 0:
		out = append(out, "  "+paint(cYellow, "⚠ ")+dim(agent.HarnessLabel(k)+" can't "+strings.Join(cant, " · ")))
	}

	if s.open {
		out = s.listLines(m, out, w, h)
	}

	do := "use it"
	if len(m.input) > 0 {
		do = "start it"
	}
	out = append(out, "")
	if s.open {
		out = append(out, keysControls(w, "type", "find", "↑↓", "model", "enter", "pick", "esc", "back")...)
	} else {
		out = append(out, keysControls(w, "↑↓", "row", "←→", "change", "enter", do, "esc", "cancel")...)
	}
	if h > 0 && len(out) > h {
		out = out[:h]
	}
	for i := range out {
		out[i] = fit(out[i], w)
	}
	return out
}

func (s *newSheet) key(m *Model, k tea.KeyPressMsg, key string) tea.Cmd {
	if s.open {
		list := s.shown(m)
		switch key {
		case "esc":
			s.open, s.filter = false, nil
		case "up":
			s.sel = max(0, s.sel-1)
		case "down":
			s.sel = min(len(list)-1, s.sel+1)
		case "enter", "tab":
			if s.sel < len(list) {
				s.choose(list[s.sel])
			}
			s.open, s.filter = false, nil
		case "backspace":
			if len(s.filter) > 0 {
				s.filter = s.filter[:len(s.filter)-1]
				s.sel = 0
			}
		default:
			if k.Text != "" && k.Mod&^tea.ModShift == 0 {
				s.filter = append(s.filter, []rune(k.Text)...)
				s.sel = 0
			}
		}
		return nil
	}
	switch key {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "up", "down", "tab", "shift+tab":
		s.row = 1 - s.row
	case "left", "right":
		if s.row == 0 {
			s.openList(m)
			return nil
		}
		// auto, then every other harness that can run it.
		opts := []agent.Kind{""}
		if hs := s.harnesses(m); len(hs) > 1 {
			opts = append(opts, hs[1:]...)
		}
		i := max(0, slices.Index(opts, s.harness))
		s.harness = opts[roundMove(i, map[string]int{"left": -1, "right": 1}[key], len(opts))]
	case "space":
		if s.row == 0 {
			s.openList(m)
		}
	case "enter":
		return s.use(m)
	}
	return nil
}

// openList opens the model list on the model picked.
func (s *newSheet) openList(m *Model) {
	s.open, s.filter = true, nil
	s.sel = max(0, slices.Index(s.shown(m), s.pick))
}

// choose makes p the model, keeping the harness picked when it can run
// it, else auto.
func (s *newSheet) choose(p newPick) {
	if p.id != s.pick.id {
		s.harness = ""
	}
	s.pick = p
}

// use closes the sheet on what it starts: the typed task starts as it,
// else the next session will.
func (s *newSheet) use(m *Model) tea.Cmd {
	m.sheet = nil
	o := s.start(m)
	if len(m.input) > 0 {
		msg := string(m.input)
		m.input, m.back = nil, 0
		return m.startAs(o, msg)
	}
	m.shelfPick(o)
	return nil
}

func (s *newSheet) mouse(m *Model, ev mouseEv, x, y int) tea.Cmd {
	switch ev {
	case mouseWheelUp:
		return s.key(m, tea.KeyPressMsg{}, "up")
	case mouseWheelDown:
		return s.key(m, tea.KeyPressMsg{}, "down")
	case mousePress:
	default:
		return nil
	}
	for _, h := range s.hits {
		if h.y != y {
			continue
		}
		if h.pick >= 0 {
			if list := s.shown(m); h.pick < len(list) {
				s.choose(list[h.pick])
			}
			s.open, s.filter = false, nil
			return nil
		}
		if s.row == h.row && h.row == 0 {
			s.openList(m)
			return nil
		}
		if s.row == h.row {
			return s.key(m, tea.KeyPressMsg{}, "right")
		}
		s.row = h.row
		return nil
	}
	return nil
}

// listLines are out with the model list after it, grouped by provider,
// as many lines as fit in h.
func (s *newSheet) listLines(m *Model, out []string, w, h int) []string {
	out = append(out, "", "  "+dim("find ")+paint(cText, string(s.filter))+paint(cOrange, "▏"))
	list := s.shown(m)
	s.sel = max(0, min(s.sel, len(list)-1))
	room := 1 << 20
	if h > 0 {
		room = max(3, h-len(out)-4)
	}
	var lines []string
	var at []int // the list's line each drawn line is, or -1 for a heading
	for i, p := range list {
		if i == 0 || list[i-1].id != p.id {
			lines = append(lines, "  "+rule(provLabel(p.id), "", w-4))
			at = append(at, -1)
		}
		id := "" // two of one name: their ids tell them apart
		if slices.ContainsFunc(list, func(q newPick) bool { return q != p && q.id == p.id && s.word(m, q) == s.word(m, p) }) {
			id = faint("  " + p.model)
		}
		lines = append(lines, sheetRow("  "+paint(cText, s.word(m, p))+id, i == s.sel, w))
		at = append(at, i)
	}
	if len(list) == 0 {
		lines, at = []string{"  " + faint("no model matches")}, []int{-1}
	}
	from, to := window(len(lines), slices.Index(at, s.sel), room)
	for i := from; i < to; i++ {
		if at[i] >= 0 {
			s.hits = append(s.hits, newHit{len(out), 0, at[i]})
		}
		out = append(out, lines[i])
	}
	return out
}
