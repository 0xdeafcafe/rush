package ui

import (
	"fmt"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/charmbracelet/x/ansi"
)

// maxSubRows is how many rows an agent's subagents take in the list at
// most: past it, the last says how many more.
const maxSubRows = 4

// subagentsShown is how many of a's running subagents the list draws under
// it, and how many more a last row counts: none when ← tucked them away.
func (m *Model) subagentsShown(a *fleet.Agent) (shown, more int) {
	n := len(a.Subagents)
	if n == 0 || m.subsFolded[a.Key] {
		return 0, 0
	}
	if n > maxSubRows {
		return maxSubRows - 1, n - (maxSubRows - 1)
	}
	return n, 0
}

// subagentRows adds the rows of d's agent's running subagents after its own.
func (m *Model) subagentRows(d rowDraw, sel bool, emit func(rowDraw, bool), cont func()) {
	shown, more := m.subagentsShown(d.a)
	if more > 0 {
		shown++
	}
	d.part, d.tag = rowSubagent, ""
	for i := range shown {
		d.sub = i
		emit(d, sel)
		cont()
	}
}

// subagentLine is the i'th of a's subagents in w cells: what it was asked,
// what kind it is, and how long it has run, in the list's time column
// (listW wide, as the agent's row reads it).
func (m *Model) subagentLine(a *fleet.Agent, i, w, listW int) string {
	shown, more := m.subagentsShown(a)
	branch := "├ "
	if i == shown-1 && more == 0 || i >= shown {
		branch = "└ "
	}
	if i >= shown {
		return fit(faint(branch)+dim(fmt.Sprintf("%d more subagents", more)), w)
	}
	s := a.Subagents[i]
	_, _, _, _, _, wTime := m.colWidths(listW)
	right := " "
	if wTime > 0 && !s.Born.IsZero() {
		right = dim(right1(dur(m.snap.At.Sub(s.Born)), wTime)) + " "
	}
	glyph := paint(cOrange, spinner[(m.tick+len(s.ID))%len(spinner)])
	typ := firstNonEmpty(s.Type, "subagent")
	// What it was asked leads, as the run's label does everywhere else.
	left := faint(branch) + glyph + " " + paint(cSub, typ)
	if what := oneLine(s.Description); what != "" {
		left = faint(branch) + glyph + " " + paint(cText, what) + faint(" · ") + dim(typ)
	}
	return spread(ansi.Truncate(left, max(1, w-cellw.String(right)-2), "…"), right, w)
}

// toggleSubagents is ← on an agent: its subagents' rows tuck away, or come
// back.
func (m *Model) toggleSubagents(a *fleet.Agent) {
	if m.subsFolded == nil {
		m.subsFolded = map[string]bool{}
	}
	m.subsFolded[a.Key] = !m.subsFolded[a.Key]
	if m.subsFolded[a.Key] {
		m.flash("subagents tucked away · ← shows them", false)
	} else {
		delete(m.subsFolded, a.Key)
	}
}
