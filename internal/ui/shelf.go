package ui

import (
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The shelf: what a new agent can start as, on the Prompt's top border.
// The one it will start as is the chip; the others are a word each, ⌥1…9
// or a click away, for the next session only. It's the profile's
// providers in order, then setups the fleet ran lately.

// shelfHit is a setup on the Prompt's top border, by column.
type shelfHit struct {
	x0, x1 int
	o      startOver
}

// shelfMax is how many setups the shelf holds: one per ⌥ digit.
const shelfMax = 9

// shelf is what a new session in dir can start as, each once.
func (m *Model) shelf(dir string) []startOver {
	first := m.byProfile(dir)
	out := []startOver{first}
	seen := []string{m.setupKey(first)}
	add := func(o startOver) {
		if k := m.setupKey(o); len(out) < shelfMax && !slices.Contains(seen, k) {
			seen, out = append(seen, k), append(out, o)
		}
	}
	if inst := m.startProfile(dir).Installed(); len(inst) > 1 {
		for _, k := range inst[1:] {
			add(m.startDefaults(k))
		}
	}
	for _, o := range m.recentSetups(first) {
		add(o)
	}
	return out
}

// shelfWord is o as the shelf names it: whose model, in which harness
// when not its own, which model.
func shelfWord(o startOver) string {
	k := agent.Kind(o.kind)
	word := agent.ProviderLabel(agent.ProviderOf(k))
	if h := agent.HarnessOf(k); h != k {
		word += "·" + strings.ToLower(agent.HarnessLabel(h))
	}
	if o.billing == state.BillingKey {
		word += "·key"
	}
	if o.model != "" {
		word += "·" + modelWord(o.kind, o.model)
	}
	return word
}

// setupKey is o as words, for telling setups apart.
func (m *Model) setupKey(o startOver) string { return strings.Join(m.startWords(o), " · ") }

// shelfLine is the shelf as the Prompt's border shows it, at most w wide:
// the next session's setup as a chip, the others numbered. Its hits are
// from the line's left edge.
func (m *Model) shelfLine(dir string, w int) (string, []shelfHit) {
	cur := m.nextStart(dir)
	tiles := m.shelf(dir)
	chip := m.setupChip(cur)
	on := slices.IndexFunc(tiles, func(o startOver) bool { return m.setupKey(o) == m.setupKey(cur) })
	// Each tile's text, the chosen one as the chip; one off the shelf
	// (from the start sheet, or #new) goes first, unnumbered.
	type seg struct {
		s string
		o startOver
	}
	var segs []seg
	if on < 0 {
		segs = append(segs, seg{chip, cur})
	}
	for i, o := range tiles {
		s := dim("⌥"+strconv.Itoa(i+1)) + " " + paint(cText, shelfWord(o))
		if i == on {
			s = chip
		}
		segs = append(segs, seg{s, o})
	}
	// Too wide: the others go from the end, the chosen one stays.
	width := func() int {
		n := 0
		for _, s := range segs {
			n += cellw.String(s.s) + 1
		}
		return n - 1
	}
	for len(segs) > 1 && width() > w {
		at := len(segs) - 1
		if segs[at].s == chip {
			at--
		}
		segs = slices.Delete(segs, at, at+1)
	}
	var line string
	var hits []shelfHit
	for i, s := range segs {
		if i > 0 {
			line += " "
		}
		x := cellw.String(line)
		line += s.s
		hits = append(hits, shelfHit{x, cellw.String(line), s.o})
	}
	return line, hits
}

// shelfPick starts the next session as o.
func (m *Model) shelfPick(o startOver) {
	if m.setupKey(o) == m.setupKey(m.byProfile(m.startDir())) {
		m.startOver = nil // the profile's own again
	} else {
		m.startOver = &o
	}
	m.flash("the next session starts as "+m.startWith(m.startDir(), true), false)
}

// shelfKey takes ⌥1…9: the shelf's setup of that number.
func (m *Model) shelfKey(s string) bool {
	if len(s) != 5 || !strings.HasPrefix(s, "alt+") || s[4] < '1' || s[4] > '9' {
		return false
	}
	if tiles := m.shelf(m.startDir()); int(s[4]-'1') < len(tiles) {
		m.shelfPick(tiles[s[4]-'1'])
	}
	return true
}

// shelfClick takes the shelf's setup at screen x, y, if the click is on one.
func (m *Model) shelfClick(x, y int) bool {
	if y != m.promptBoxY {
		return false
	}
	for _, h := range m.shelfHits {
		if x >= h.x0 && x < h.x1 {
			m.shelfPick(h.o)
			return true
		}
	}
	return false
}
