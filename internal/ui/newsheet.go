package ui

import (
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// The New session sheet: the harness that leads the chat, then a model it
// may call. A chat has one leader harness, and it controls the models: the
// list is its row of Settings › Config, every provider it runs and the
// models each may call there, and its subagents and helper agents run on
// that row too (host.runners). It changes no default: Settings does. See
// docs/providers-harnesses.md.

// newPick is one model on the sheet's list: provider id's model ("" its
// default).
type newPick struct {
	id, model string
}

type newSheet struct {
	row     int        // 0 the harness, 1 the model
	harness agent.Kind // the leader
	pick    newPick    // the model chosen
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

// openComposer opens the sheet on what the next session would start as:
// its harness leading, on its model when that harness may call it.
func (m *Model) openComposer() tea.Cmd {
	o := m.nextStart(m.startDir())
	leaders := m.leaders()
	if len(leaders) == 0 {
		m.flash("rush can't run any harness here yet · Settings › Config", true)
		return nil
	}
	s := &newSheet{harness: agent.HarnessOf(agent.Kind(o.kind)), pick: newPick{id: startID(o), model: o.model}, row: 1}
	if !slices.Contains(leaders, s.harness) {
		s.harness = leaders[0]
	}
	s.keepPick(m)
	m.sheet = s
	return m.loadAllModels()
}

func (s *newSheet) width(m *Model) int { return max(64, min(96, m.w-6)) }

// leaders are the harnesses that can lead a chat now: on, with a provider
// it may run that's ready.
func (m *Model) leaders() []agent.Kind {
	cfg := &m.store.Config
	var out []agent.Kind
	for _, r := range m.routeRows() {
		if r.why == "" && cfg.CellOn(r.id, r.harness) && !slices.Contains(out, r.harness) {
			out = append(out, r.harness)
		}
	}
	return out
}

// newPicks are the models harness h may call: its row of Config, by
// provider, the ones set up first.
func (m *Model) newPicks(h agent.Kind) []newPick {
	cfg := &m.store.Config
	var out []newPick
	ready := m.routeRows()
	for _, id := range m.configProviders() {
		if !cfg.CellOn(id, h) || !slices.ContainsFunc(ready, func(r routeRow) bool { return r.id == id && r.harness == h && r.why == "" }) {
			continue
		}
		k, _, _ := agent.Compat(id, h)
		n := len(out)
		for _, c := range m.models(string(k)) {
			if p := (newPick{id, c.ID}); cfg.Calls(id, h, c.ID) && !slices.Contains(out, p) {
				out = append(out, p)
			}
		}
		if len(out) == n {
			out = append(out, newPick{id, ""}) // it lists none: its own default
		}
	}
	return out
}

// keepPick keeps the model when the leader may call it, else takes the
// one its first provider starts with.
func (s *newSheet) keepPick(m *Model) {
	picks := m.newPicks(s.harness)
	if slices.Contains(picks, s.pick) || len(picks) == 0 {
		return
	}
	id := picks[0].id
	if slices.ContainsFunc(picks, func(p newPick) bool { return p.id == s.pick.id }) {
		id = s.pick.id // same provider, another model
	}
	k, _, _ := agent.Compat(id, s.harness)
	s.pick = newPick{id, m.startOn(id, string(k)).model}
	if !slices.Contains(picks, s.pick) {
		i := slices.IndexFunc(picks, func(p newPick) bool { return p.id == id })
		s.pick = picks[max(0, i)]
	}
}

// start is what the sheet starts: the model in the leader, with what
// Settings gives that route for the rest.
func (s *newSheet) start(m *Model) startOver {
	k, _, _ := agent.Compat(s.pick.id, s.harness)
	o := m.startOn(s.pick.id, string(k))
	if s.pick.model != "" {
		o.model = s.pick.model
	}
	return o
}

// shown are the list's lines that match what's typed.
func (s *newSheet) shown(m *Model) []newPick {
	all := m.newPicks(s.harness)
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
	k, _, _ := agent.Compat(p.id, s.harness)
	return modelWord(string(k), p.model)
}

// cantFeatures are what a harness may lack that's worth saying before
// it starts.
var cantFeatures = []agent.Feature{agent.FeatureRewind, agent.FeatureImages, agent.FeatureResume, agent.FeatureSubagents}

func (s *newSheet) body(m *Model, w, h int) []string {
	s.hits = s.hits[:0]
	o := s.start(m)
	k := agent.Kind(o.kind)
	out := []string{sheetTitle("New session", "the harness that leads it, and a model it may call · Settings stay as they are", w), ""}

	harness := glyph(s.harness) + " " + paint(cText+bold, agent.HarnessLabel(s.harness)) + dim("  leads; its subagents run on its models too")
	if len(m.leaders()) > 1 {
		harness = faint("‹ ") + glyph(s.harness) + " " + paint(cText+bold, agent.HarnessLabel(s.harness)) + faint(" ›") + dim("  leads; its subagents run on its models too")
	}
	pr, _ := agent.Billed(s.pick.id)
	model := glyph(agent.Kind(pr)) + " " + paint(cText+bold, s.word(m, s.pick)) + dim("  "+provLabel(s.pick.id))
	s.hits = append(s.hits, newHit{len(out), 0, -1})
	out = append(out, sheetRow(dim(fit("harness", 10))+harness, s.row == 0, w))
	s.hits = append(s.hits, newHit{len(out), 1, -1})
	out = append(out, sheetRow(dim(fit("model", 10))+model, s.row == 1 && !s.open, w))

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
		if s.row == 1 {
			s.openList(m)
			return nil
		}
		leaders := m.leaders()
		i := max(0, slices.Index(leaders, s.harness))
		s.harness = leaders[roundMove(i, map[string]int{"left": -1, "right": 1}[key], len(leaders))]
		s.keepPick(m)
	case "space":
		if s.row == 1 {
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

// choose makes p the model.
func (s *newSheet) choose(p newPick) { s.pick = p }

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
		if s.row == h.row && h.row == 1 {
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
