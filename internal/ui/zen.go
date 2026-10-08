package ui

import (
	"fmt"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// Zen shows nothing until an agent needs you, then only that agent: what it
// last said and what it needs, answered in its dock. Answer it and it stays,
// to carry on with; mark it done and the next one takes its place. No header or strip: one bar with where you are in
// the queue and the keys to skip (ctrl+n) or leave (ctrl+z).

// zenQueue is every agent waiting on you, oldest first.
func (m *Model) zenQueue() []*fleet.Agent {
	var out []*fleet.Agent
	for _, a := range m.snap.Agents {
		if t, ok := m.openFailed[a.Key]; ok && time.Since(t) < 30*time.Second {
			continue // couldn't be shown; try it again in a bit
		}
		if a.NeedsYou() || a.Waiting() {
			out = append(out, a)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].UpdatedAt.Before(out[j].UpdatedAt) })
	return out
}

// zenHeld is the agent zen has on screen: the one it put there, answered
// or not, until it's marked done or gone.
func (m *Model) zenHeld() *fleet.Agent {
	for _, a := range m.snap.Agents {
		if a.Key == m.sel && a.Key == m.zenAt && !a.Done {
			return a
		}
	}
	return nil
}

// zenPick keeps the selection on the agent zen showed you, answered or not,
// so you can carry on with it; once it's marked done, the oldest one
// waiting takes its place.
func (m *Model) zenPick() {
	if !m.zen {
		return
	}
	if m.zenHeld() != nil {
		m.paneFocus = true
		return
	}
	if q := m.zenQueue(); len(q) > 0 {
		m.sel, m.zenAt = q[0].Key, q[0].Key
		m.paneFocus = true
	}
}

// zenSkip moves to the next agent waiting on you: from an answered one,
// the oldest.
func (m *Model) zenSkip() {
	q := m.zenQueue()
	next := 0
	for i, a := range q {
		if a.Key == m.sel {
			next = (i + 1) % len(q)
		}
	}
	if len(q) > 0 && q[next].Key != m.sel {
		m.sel, m.zenAt = q[next].Key, q[next].Key
	}
}

// zenQuiet is the screen when nothing needs you.
func (m *Model) zenQuiet(w, h int) []string {
	t := m.tally()
	out := make([]string, max(0, h/3))
	center := func(s string) string {
		pad := max(0, (w-len([]rune(stripAnsi(s))))/2)
		return strings.Repeat(" ", pad) + s
	}
	out = append(out, center(paint(cGreen, "✓ nothing needs you")), "")
	var counts []string
	if t.working > 0 {
		counts = append(counts, paint(cOrange, fmt.Sprintf("✻ %d working", t.working)))
	}
	if t.busy > 0 {
		counts = append(counts, dim(fmt.Sprintf("◌ %d in background", t.busy)))
	}
	counts = append(counts, dim(fmt.Sprintf("%d finished", t.done)))
	out = append(out, center(strings.Join(counts, dim(" · "))), "", "")
	return append(out, center(paint(cYellow, "◇ zen")+dim(" shows an agent here when one needs you")),
		"", center(keys("hold tab", "what's working", "ctrl+z", "leave zen")))
}

func stripAnsi(s string) string { return ansi.Strip(s) }

// zenBar is the row pinned above zen's agent: that it's zen, which of the
// waiting agents this is and where, and the keys to move on or leave, so a
// long reply never scrolls them away.
func (m *Model) zenBar(a *fleet.Agent, w int) string {
	q := m.zenQueue()
	pos := 0
	for i, x := range q {
		if x.Key == a.Key {
			pos = i + 1
		}
	}
	left := paint(cYellow+bold, "◇ zen") + "   "
	if pos == 0 {
		// Answered, or never waiting: say so rather than "needs you".
		left += paint(cGreen, "✓ answered") + "  " + dim(fmt.Sprintf("%d waiting", len(q)))
	} else {
		left += paint(cYellow+bold, "● needs you") + "  " + paint(cSub, fmt.Sprintf("%d of %d", pos, len(q)))
		if !a.UpdatedAt.IsZero() {
			left += dim(" · waiting " + dur(time.Since(a.UpdatedAt).Round(time.Second)))
		}
	}
	where := tildify(a.Cwd)
	if a.Branch != "" {
		where += " · " + a.Branch
	}
	pairs := []string{"hold tab", "what's working", "ctrl+z", "leave zen"}
	next := ""
	if len(q) > 1 || pos == 0 && len(q) > 0 {
		next = nextKey() + faint("  ·  ")
	}
	right := next + keysFit(max(20, w-cellw.String(left)-cellw.String(next)-4), pairs...)
	if room := w - cellw.String(left) - cellw.String(right) - 6; room > 12 {
		left += "   " + dim(fit(where, min(room, cellw.String(where))))
	}
	return spread(left, right, w)
}

// zenBody is the middle of zen's Session: the last thing the agent said,
// drawn as it is in the conversation, then what it needs.
func (m *Model) zenBody(a *fleet.Agent, c *hostConn, w int) []convo.Line {
	said := ""
	if c != nil {
		for i := len(c.sess.Turns) - 1; i >= 0 && said == ""; i-- {
			items := c.sess.Turns[i].Items
			for j := len(items) - 1; j >= 0; j-- {
				if items[j].Kind == convo.KText && strings.TrimSpace(items[j].Text) != "" {
					said = items[j].Text
					break
				}
			}
		}
	}
	if said == "" {
		said = firstNonEmpty(a.Needs, a.Detail)
	}
	lines := []convo.Line{{Text: ""}, {Text: "    " + dim("it said")}}
	if c != nil {
		lines = append(lines, c.sess.Answer(said, w)...)
	} else {
		for _, l := range wrap(said, min(w-8, 100)) {
			lines = append(lines, convo.Line{Text: "    " + paint(cText, l)})
		}
	}
	if a.Needs != "" && c != nil && len(c.sess.Pending()) == 0 {
		lines = append(lines, convo.Line{Text: ""}, convo.Line{Text: "    " + dim("it needs  ") + paint(cYellow, oneLine(a.Needs))})
	}
	return lines
}

// zenPeek is a look at what's working, for as long as tab is held.
// Terminals don't say when a key is let go, so a hold is known by its
// repeats: once they stop coming, tab is up. A tap with no repeats keeps
// the peek until the next key.
type zenPeek struct {
	on   bool
	held bool          // repeats have come: it ends when they stop
	at   time.Time     // the last tab
	gap  time.Duration // between the last two repeats
}

type peekCheckMsg struct{}

// peekLet is how long after the last repeat tab counts as let go.
func (p zenPeek) peekLet() time.Duration {
	if p.gap == 0 {
		return 300 * time.Millisecond
	}
	return min(600*time.Millisecond, max(150*time.Millisecond, 3*p.gap))
}

// peekKey takes the keys while peeking. Repeats of tab keep it open; any
// other key, or tab pressed afresh, closes it, and goes no further: the
// agent's box isn't on screen to type into.
func (m *Model) peekKey(s string) tea.Cmd {
	p := &m.peek
	since := time.Since(p.at)
	// The first repeat comes after the keyboard's repeat delay; the rest
	// much quicker.
	repeat := s == "tab" && (p.held && since < p.peekLet() || !p.held && since < time.Second)
	if !repeat {
		m.peek = zenPeek{}
		return nil
	}
	first := !p.held
	if p.held {
		p.gap = since
	}
	p.held, p.at = true, time.Now()
	if first {
		return m.peekTick()
	}
	return nil
}

func (m *Model) peekTick() tea.Cmd {
	return tea.Tick(m.peek.peekLet(), func(time.Time) tea.Msg { return peekCheckMsg{} })
}

// peekCheck ends a held peek once the repeats have stopped.
func (m *Model) peekCheck() tea.Cmd {
	if !m.peek.on || !m.peek.held {
		return nil
	}
	if time.Since(m.peek.at) >= m.peek.peekLet() {
		m.peek = zenPeek{}
		return nil
	}
	return m.peekTick()
}

// zenPeekLines is what's working, row by row as in Agents, the busiest
// first.
func (m *Model) zenPeekLines(w, h int) []string {
	var busy []*fleet.Agent
	for _, a := range m.snap.Agents {
		if !a.NeedsYou() && (a.Live() || a.Busy()) {
			busy = append(busy, a)
		}
	}
	sort.SliceStable(busy, func(i, j int) bool {
		if li, lj := busy[i].Live(), busy[j].Live(); li != lj {
			return li
		}
		return busy[i].UpdatedAt.After(busy[j].UpdatedAt)
	})
	back := keys("tab", "again to go back")
	if m.peek.held {
		back = faint("let go of tab to go back")
	}
	out := []string{spread(paint(cYellow+bold, "◇ zen")+"   "+paint(cText+bold, "what's working"), back, w), ""}
	if len(busy) == 0 {
		return append(out, "", "   "+dim("nothing is working right now"))
	}
	nameCol := m.nameColumn(w)
	for _, a := range busy {
		if len(out) >= h {
			break
		}
		out = append(out, m.agentLine(a, w, w, false, nameCol, false, ""))
	}
	return out
}
