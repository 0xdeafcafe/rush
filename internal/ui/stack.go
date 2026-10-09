package ui

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// The stack view: the agent and its subagents in bands, top to bottom, each
// the bottom of its conversation. Pulled up from the subagents panel, it
// grows over stackFrames frames from where the panel was.
const stackFrames = 6

// stackMax is how many subagents get a band at most.
const stackMax = 4

// stackOrder is the subagents, the most recently active first.
func (c *hostConn) stackOrder() []convo.Subagent {
	subs := slices.Clone(c.subs)
	slices.SortStableFunc(subs, func(a, b convo.Subagent) int {
		return int(max(b.Mod, b.Born)/1e6 - max(a.Mod, a.Born)/1e6)
	})
	return subs
}

// stackLines draws the bands into h rows. Each band's rows carry its ref,
// "stack:" for the main agent and "stack:<id>" for a subagent, so hover and
// clicks find it.
func (m *Model) stackLines(c *hostConn, o convo.Options, h int) []convo.Line {
	full := h
	if c.stackGrow < stackFrames {
		c.stackGrow++
		from := min(h, max(c.stackFrom, 2))
		h = from + (h-from)*c.stackGrow/stackFrames
	}
	jobs := c.sess.SubagentJobs()
	// Only the main agent and the live ones: the finished are under the
	// subagents tab.
	var live []convo.Subagent
	for _, sa := range c.stackOrder() {
		if _, on := c.subStateIn(sa, jobs); on {
			live = append(live, sa)
		}
	}
	avail := h
	// At most stackMax live bands; the rest of the live ones are their
	// title row alone, over the finished ones.
	n := min(len(live)+1, stackMax+1, max(1, avail/4))
	rest := live[n-1:]
	live = live[:n-1]
	// As many title rows as leave each band its title and a row: dozens
	// live overran the pane.
	rest = rest[:min(len(rest), max(0, avail-2*n))]
	avail -= len(rest)
	// The main agent on top, a bigger share; the live ones evenly under it.
	heights := []int{avail}
	if n > 1 {
		mainH := max(avail/n, avail*2/5)
		heights = append([]int{mainH}, split(avail-mainH, n-1)...)
	}
	out := make([]convo.Line, full-h, full)
	now := time.Now()
	so := o
	so.Selected, so.Focused = "", false
	so.Width = o.Width - 2 // the band's rule on the left
	facts := func(s *convo.Session) string {
		var f []string
		if s != nil {
			if k := s.Totals(now).ToolCalls; k > 0 {
				f = append(f, fmt.Sprintf("%d steps", k))
			}
			if !s.First.IsZero() {
				f = append(f, dur(now.Sub(s.First).Round(time.Second)))
			}
		}
		return strings.Join(f, " · ")
	}
	head := func(ref, glyph, name, desc string, s *convo.Session) convo.Line {
		l := spread(" "+glyph+" "+paint(cText+bold, name)+"  "+paint(cSub, oneLine(desc)), dim(facts(s))+"  ", o.Width)
		return convo.Line{Text: onBg(selBG, l, o.Width), Ref: ref}
	}
	band := func(ref, glyph, name, desc string, s *convo.Session, rows int) {
		out = append(out, head(ref, glyph, name, desc, s))
		var tail []convo.Line
		if s != nil {
			tail = s.Tail(so, rows-1) // only the rows that show, however long it is
		}
		tail = tail[max(0, len(tail)-(rows-1)):]
		rule := faint("│ ")
		for _, l := range tail {
			t := l.Text
			if strings.HasPrefix(ansi.Strip(t), "▏") {
				t = ansi.TruncateLeft(t, 2, "") // the band's rule stands for the turn's
			}
			out = append(out, convo.Line{Text: rule + t, Ref: ref})
		}
		for range rows - 1 - len(tail) {
			out = append(out, convo.Line{Text: rule, Ref: ref})
		}
	}
	sessOf := func(id string) *convo.Session {
		if t := c.subTails[id]; t != nil {
			return t.Sess
		}
		return nil
	}
	main := "main agent"
	if a := m.focused(); a != nil {
		main = oneLine(a.DisplayName)
	}
	band("stack:", paint(cBlue, "◆"), main, "", c.sess, heights[0])
	for i, sa := range live {
		band("stack:"+sa.ID, spinOf(c.subKind(sa.ID), m.tick+i), sa.Type, sa.Description, sessOf(sa.ID), heights[i+1])
	}
	for i, sa := range rest {
		out = append(out, head("stack:"+sa.ID, spinOf(c.subKind(sa.ID), m.tick+len(live)+i), sa.Type, sa.Description, sessOf(sa.ID)))
	}
	return out
}

// split shares rows between n bands, the first ones a row more when it
// doesn't divide.
func split(rows, n int) []int {
	out := make([]int, n)
	for i := range out {
		out[i] = rows / n
		if i < rows%n {
			out[i]++
		}
	}
	return out
}

// openStack switches to the stack view, growing from `from` rows high.
func (m *Model) openStack(c *hostConn, from int) {
	if i := slices.Index(m.views(c), "stack"); i >= 0 {
		c.view, c.scroll, c.scrollOnly, c.sel = i, 0, false, ""
		c.stackGrow, c.stackFrom, c.pullY = 0, from, 0
	}
}

// toggleStack is session.stack: the stack, or back to the conversation.
func (m *Model) toggleStack(c *hostConn) {
	if m.viewName(c) == "stack" {
		c.view, c.scroll, c.scrollOnly = 0, 0, false
		return
	}
	if c.liveSubs() < 2 {
		m.flash("the stack needs 2 subagents working", false)
		return
	}
	m.openStack(c, 0)
}

// liveSubs is how many subagents are working.
func (c *hostConn) liveSubs() int {
	jobs, n := c.sess.SubagentJobs(), 0
	for _, sa := range c.subs {
		if _, on := c.subStateIn(sa, jobs); on {
			n++
		}
	}
	return n
}

// clickStack opens the band clicked full-size: the main agent's in the
// conversation, a subagent's under subagents. A click on the frame around
// them (the header, a row in no band) puts the stack away.
func (m *Model) clickStack(c *hostConn, y int) bool {
	i := y - m.paneTop
	if m.viewName(c) != "stack" || i < 0 || i >= c.bodyTop+c.bodyRows {
		return false
	}
	ref := ""
	if i < len(c.rowRefs) {
		ref = c.rowRefs[i]
	}
	id, ok := strings.CutPrefix(ref, "stack:")
	if !ok || id == "" {
		c.view, c.scroll, c.scrollOnly = 0, 0, false
		return true
	}
	c.view = slices.Index(m.views(c), "subagents")
	m.openSub(c, id)
	c.scrollOnly = false
	return true
}

// onSubsTitle is whether y is the conversation's subagents panel title.
func (m *Model) onSubsTitle(c *hostConn, y int) bool {
	i := y - m.paneTop
	return m.viewName(c) == "conversation" && i >= 0 && i < len(c.rowRefs) && c.rowRefs[i] == "subs:title"
}

// stackFromY is how high the bands start, pulled up from row y.
func (m *Model) stackFromY(c *hostConn, y int) int {
	return max(0, c.bodyTop+c.bodyRows-(y-m.paneTop))
}
