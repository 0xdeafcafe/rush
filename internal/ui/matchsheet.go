package ui

import (
	"cmp"
	"slices"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// The composer: what the next session starts as, picked as Halo's match
// composer picks a playlist. Providers and harnesses are tiles you tick;
// the routes they make between them are worked out beside them, each
// provider once and each harness once, and START takes the one chosen.
// See docs/match-composer.md.

// Its rows, top to bottom.
const (
	compStart = iota
	compProviders
	compHarnesses
	compRoutes
)

type matchSheet struct {
	provs []string     // the installed providers, as tiles
	harns []agent.Kind // every harness a provider here runs in, as tiles
	onP   map[string]bool
	onH   map[agent.Kind]bool
	row   int
	col   int // the tile in focus, in a tile row
	sel   int // the route in focus
	o     startOver
	model map[string]string // a route's model, by its kind, where ←→ changed it
	hits  []compHit         // where body drew what a click takes
}

// compHit is a part of the body a click takes: a tile, a route, START.
type compHit struct {
	x0, x1, y0, y1 int
	row, i         int
}

// openComposer opens the composer on what the next session would start
// as, the tiles its shelf uses ticked.
func (m *Model) openComposer() tea.Cmd {
	c := &matchSheet{onP: map[string]bool{}, onH: map[agent.Kind]bool{}, model: map[string]string{}, o: m.nextStart(m.startDir())}
	for _, r := range m.routeRows() {
		if !slices.Contains(c.provs, r.id) {
			c.provs = append(c.provs, r.id)
		}
		if !slices.Contains(c.harns, r.harness) {
			c.harns = append(c.harns, r.harness)
		}
	}
	if len(c.provs) == 0 {
		m.flash("rush can't run any harness here yet", true)
		return nil
	}
	for _, o := range append(m.shelf(m.startDir()), c.o) {
		c.onP[startID(o)], c.onH[agent.HarnessOf(agent.Kind(o.kind))] = true, true
	}
	m.sheet = c
	return m.loadAllModels()
}

func (c *matchSheet) width(m *Model) int { return max(80, min(128, m.w-6)) }

// routes are the ticked providers' routes into the ticked harnesses, in
// the provider tiles' order, each provider's default harness first.
func (c *matchSheet) routes(m *Model) []routeRow {
	var out []routeRow
	for _, id := range c.provs {
		for _, r := range m.routeRows() {
			if r.id == id && c.onP[id] && c.onH[r.harness] {
				out = append(out, r)
			}
		}
	}
	return out
}

// ready are the routes that can start now.
func (c *matchSheet) ready(m *Model) []routeRow {
	return slices.DeleteFunc(c.routes(m), func(r routeRow) bool { return r.why != "" })
}

// chosen is the route of o, or -1 when the ticks leave it out.
func (c *matchSheet) chosen(m *Model, rs []routeRow) int {
	return slices.IndexFunc(rs, func(r routeRow) bool {
		return r.id == startID(c.o) && string(r.kind) == c.o.kind
	})
}

// pick makes route r what START takes, as Settings starts it.
func (c *matchSheet) pick(m *Model, r routeRow) {
	c.o = m.startOn(r.id, string(r.kind))
	if v, ok := c.model[r.id+"/"+string(r.kind)]; ok {
		c.o.model = v
	}
}

// modelOf is the model route r starts on here.
func (c *matchSheet) modelOf(m *Model, r routeRow) string {
	if v, ok := c.model[r.id+"/"+string(r.kind)]; ok {
		return v
	}
	return m.startOn(r.id, string(r.kind)).model
}

// retick keeps o among the routes once a tile changes: the first that can
// start, when the ticks took its own away.
func (c *matchSheet) retick(m *Model) {
	if c.chosen(m, c.ready(m)) >= 0 {
		return
	}
	if rs := c.ready(m); len(rs) > 0 {
		c.pick(m, rs[0])
	}
}

func (c *matchSheet) body(m *Model, w, h int) []string {
	c.hits = c.hits[:0]
	out := []string{sheetTitle("Compose", "what the next session starts as · Settings stay as they are", w), ""}
	// START, Halo's PLAY: what enter takes, or why nothing can.
	rs, ready := c.routes(m), c.ready(m)
	start := paint(cOrange+bold, "▶ START") + "   " + m.setupChip(c.o)
	if _, warn, _ := agent.Compat(startID(c.o), agent.HarnessOf(agent.Kind(c.o.kind))); warn != "" {
		start += "  " + paint(cYellow, "⚠ "+warn)
	}
	if len(ready) == 0 {
		start = paint(cRed+bold, "✗ nothing runs") + "  " + dim("tick a provider and a harness that go together")
	}
	c.hits = append(c.hits, compHit{0, w, len(out), len(out) + 1, compStart, 0})
	out = append(out, sheetRow(start, c.row == compStart, w), "")

	lw := w * 60 / 100
	rw := w - lw - 3
	var left []string
	tiles := func(row int, n int, tile func(i int) [4]string) {
		const tw = 12
		per := max(1, lw/tw)
		for from := 0; from < n; from += per {
			lines := make([]string, 5)
			for i := from; i < min(n, from+per); i++ {
				t := tile(i)
				edge := faint
				switch {
				case c.row == row && c.col == i:
					edge = func(s string) string { return paint(cOrange+bold, s) }
				case t[3] == "on":
					edge = func(s string) string { return paint(cText, s) }
				}
				x := (i - from) * tw
				c.hits = append(c.hits, compHit{x, x + tw, len(out) + len(left), len(out) + len(left) + 5, row, i})
				lines[0] += edge("┌" + strings.Repeat("─", tw-2) + "┐")
				for j := range 3 {
					lines[j+1] += edge("│") + center(t[j], tw-2) + edge("│")
				}
				lines[4] += edge("└" + strings.Repeat("─", tw-2) + "┘")
			}
			left = append(left, lines...)
		}
	}
	head := func(row int, name string) string {
		if c.row == row {
			return paint(cOrange+bold, name)
		}
		return dim(name)
	}
	left = append(left, head(compProviders, "PROVIDERS INCLUDED"))
	tiles(compProviders, len(c.provs), func(i int) [4]string {
		id := c.provs[i]
		l := lookOf(agent.Kind(provOf(id)))
		name := ansi.Truncate(agent.ProviderLabel(provOf(id)), 10, "…")
		sub := billWord(id)
		if sub == "subscription" {
			sub = "plan"
		}
		mark, state := faint("○"), ""
		switch {
		case !slices.ContainsFunc(m.routeRows(), func(r routeRow) bool { return r.id == id && r.why == "" }):
			mark = paint(cRed, "✗")
		case c.onP[id] && !slices.ContainsFunc(rs, func(r routeRow) bool { return r.id == id }):
			mark, state = faint("░"), "on" // ticked, but no harness ticked runs it
		case c.onP[id]:
			mark, state = paint(cGreen, "●"), "on"
		}
		return [4]string{paint(l.colour(), l.glyph), paint(cText, name), dim(sub) + " " + mark, state}
	})
	left = append(left, head(compHarnesses, "HARNESSES INCLUDED"))
	tiles(compHarnesses, len(c.harns), func(i int) [4]string {
		hk := c.harns[i]
		name := ansi.Truncate(agent.HarnessLabel(hk), 10, "…")
		mark, state := faint("○"), ""
		switch {
		case !agent.Runs(hk) && !slices.ContainsFunc(m.routeRows(), func(r routeRow) bool { return r.harness == hk && r.why == "" }):
			mark = paint(cRed, "✗")
		case c.onH[hk] && !slices.ContainsFunc(rs, func(r routeRow) bool { return r.harness == hk }):
			mark, state = faint("░"), "on"
		case c.onH[hk]:
			mark, state = paint(cGreen, "●"), "on"
		}
		return [4]string{glyph(hk), paint(cText, name), mark, state}
	})

	// The routes the ticks make, as a tree under each provider, then what
	// every one of them does and what only some do.
	right := []string{head(compRoutes, "ROUTES") + dim("  "+strconv.Itoa(len(ready))+" can start")}
	on := c.chosen(m, rs)
	for i, r := range rs {
		if i == 0 || rs[i-1].id != r.id {
			l := lookOf(agent.Kind(provOf(r.id)))
			name := agent.ProviderLabel(provOf(r.id))
			if b := billWord(r.id); b != "" {
				name += " · " + b
			}
			right = append(right, paint(l.colour(), l.glyph)+" "+paint(cText+bold, fit(name, rw-2)))
		}
		branch := "├ "
		if i == len(rs)-1 || rs[i+1].id != r.id {
			branch = "└ "
		}
		mark := "  "
		switch {
		case r.why != "":
			mark = paint(cRed, "✗ ")
		case r.warn != "":
			mark = paint(cYellow, "⚠ ")
		case r.def:
			mark = paint(cOrange, "★ ")
		}
		note := cmp.Or(modelWord(string(r.kind), c.modelOf(m, r)), "default model")
		if c.row == compRoutes && c.sel == i && r.why == "" {
			note = "‹ " + note + " ›"
		}
		if r.why != "" {
			note = r.why
		}
		lead := " "
		if i == on {
			lead = paint(cOrange, "▸")
		}
		line := lead + faint(branch) + mark + paint(cText, fit(agent.HarnessLabel(r.harness), 13)) + " " + dim(note)
		if r.why != "" {
			line = " " + faint(ansi.Strip(line)[1:])
		}
		c.hits = append(c.hits, compHit{lw + 3, w, len(out) + len(right), len(out) + len(right) + 1, compRoutes, i})
		if c.row == compRoutes && c.sel == i {
			line = highlight(line, rw)
		}
		right = append(right, fit(line, rw))
	}
	if len(rs) == 0 {
		right = append(right, faint("none: tick a provider and a harness"))
	}
	right = append(right, "")
	right = append(right, c.featureLines(ready, rw)...)

	for i := range max(len(left), len(right)) {
		l, r := "", ""
		if i < len(left) {
			l = left[i]
		}
		if i < len(right) {
			r = right[i]
		}
		out = append(out, fit(l, lw)+faint(" │ ")+r)
	}
	out = append(out, "", "  "+dim("same as  ")+paint(cText, m.newWords(c.o)))
	do := "use it"
	if len(m.input) > 0 {
		do = "start it"
	}
	out = append(out, "")
	out = append(out, keysControls(w, "↑↓", "row", "←→", "tile or model", "space", "include", "enter", do, "esc", "cancel")...)
	if h > 0 && len(out) > h {
		out = out[:h]
	}
	for i := range out {
		out[i] = fit(out[i], w)
	}
	return out
}

// compFeatures are the features worth comparing routes by.
var compFeatures = []agent.Feature{agent.FeatureResume, agent.FeatureRewind, agent.FeatureImages,
	agent.FeatureSubagents, agent.FeatureQuestions, agent.FeaturePlan, agent.FeatureMCP, agent.FeatureFork}

// featureLines are what every ready route does and what only some do:
// Halo's playlist stats, for what you've ticked.
func (c *matchSheet) featureLines(rs []routeRow, w int) []string {
	if len(rs) == 0 {
		return nil
	}
	var all, some []string
	for _, f := range compFeatures {
		n := 0
		for _, r := range rs {
			if agent.Supports(r.kind, f) {
				n++
			}
		}
		switch {
		case n == len(rs):
			all = append(all, string(f))
		case n > 0:
			some = append(some, string(f))
		}
	}
	line := func(name string, fs []string) []string {
		if len(fs) == 0 {
			return nil
		}
		out := wrap(paint(cText, strings.Join(fs, " · ")), w-12)
		for i := range out {
			out[i] = dim(fit(map[bool]string{true: name}[i == 0], 12)) + out[i]
		}
		return out
	}
	out := line("EVERY ROUTE", all)
	return append(out, line("ONLY SOME", some)...)
}

func (c *matchSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	rs := c.routes(m)
	n := map[int]int{compProviders: len(c.provs), compHarnesses: len(c.harns)}
	switch k {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "up", "shift+tab":
		switch {
		case c.row == compRoutes && c.sel > 0 && k == "up":
			c.sel--
		case c.row > compStart:
			c.row--
			c.col = min(c.col, max(0, n[c.row]-1))
		}
	case "down", "tab":
		switch {
		case c.row == compRoutes && c.sel < len(rs)-1 && k == "down":
			c.sel++
		case c.row < compRoutes:
			c.row++
			c.col = min(c.col, max(0, n[c.row]-1))
			if c.row == compRoutes {
				c.sel = max(0, c.chosen(m, rs))
			}
		}
	case "left", "right":
		d := map[string]int{"left": -1, "right": 1}[k]
		switch {
		case c.row == compProviders || c.row == compHarnesses:
			c.col = max(0, min(n[c.row]-1, c.col+d))
		case c.row == compRoutes && c.sel < len(rs) && rs[c.sel].why == "":
			// A route's model, from what its harness lists.
			r := rs[c.sel]
			vals := []string{""}
			for _, ch := range m.models(string(r.kind)) {
				vals = append(vals, ch.ID)
			}
			now := c.modelOf(m, r)
			if !slices.Contains(vals, now) {
				vals = append(vals, now)
			}
			c.model[r.id+"/"+string(r.kind)] = vals[roundMove(max(0, slices.Index(vals, now)), d, len(vals))]
			c.pick(m, r)
		}
	case "space", " ":
		return c.act(m, false)
	case "enter":
		return c.act(m, true)
	}
	return nil
}

// act is space or enter on the part in focus: a tile ticks, a route is
// taken, and enter on START or a route uses it.
func (c *matchSheet) act(m *Model, enter bool) tea.Cmd {
	switch c.row {
	case compProviders:
		id := c.provs[c.col]
		c.onP[id] = !c.onP[id]
		c.retick(m)
	case compHarnesses:
		hk := c.harns[c.col]
		c.onH[hk] = !c.onH[hk]
		c.retick(m)
	case compRoutes:
		rs := c.routes(m)
		if c.sel >= len(rs) {
			return nil
		}
		if r := rs[c.sel]; r.why != "" {
			m.flash(agent.HarnessLabel(r.harness)+" on "+provLabel(r.id)+": "+r.why, true)
			return nil
		}
		c.pick(m, rs[c.sel])
		if enter {
			return c.use(m)
		}
	case compStart:
		return c.use(m)
	}
	return nil
}

// use closes the composer on o: the typed task starts as it, else the
// next session will.
func (c *matchSheet) use(m *Model) tea.Cmd {
	if len(c.ready(m)) == 0 {
		return nil
	}
	m.sheet = nil
	o := c.o
	if len(m.input) > 0 {
		msg := string(m.input)
		m.input, m.back = nil, 0
		return m.startAs(o, msg)
	}
	m.shelfPick(o)
	return nil
}

func (c *matchSheet) mouse(m *Model, ev mouseEv, x, y int) tea.Cmd {
	if ev != mousePress {
		return nil
	}
	for _, h := range c.hits {
		if x >= h.x0 && x < h.x1 && y >= h.y0 && y < h.y1 {
			c.row = h.row
			if h.row == compRoutes {
				c.sel = h.i
			} else {
				c.col = h.i
			}
			return c.act(m, h.row == compStart)
		}
	}
	return nil
}

// center puts s in the middle of w cells.
func center(s string, w int) string {
	pad := max(0, w-cellw.String(s))
	return strings.Repeat(" ", pad/2) + s + strings.Repeat(" ", pad-pad/2)
}
