package ui

import (
	"fmt"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// While the session's panel is open, its subagents and workflows show at
// its top rather than under the conversation: the one place to look for
// what's running. Each is two lines, its name and how long it's run,
// then what it's doing now (or what it said last, once done); a click
// watches it, as space does on its row in the dock. The agent doesn't
// draw them: rush does, from what the dock reads.

// subsFold is the key the subagents' heading folds by.
const subsFold = "§subagents"

// subsInPanel is whether the subagents show at the top of the session's
// panel, a pane w wide: it's open on one of the agent's views, and there
// are runs to show. The dock leaves them out then.
func (m *Model) subsInPanel(c *hostConn, w int) bool {
	side, _, width := m.viewsBeside(c, m.conversationWidth(w))
	if width <= viewsStrip || len(side) == 0 || viewsFolded(c, side) || c.views.past {
		return false
	}
	return len(c.dockSubs())+len(c.dockDone)+len(c.dockFlows()) > 0
}

// panelSubs are the session's workflows and subagents for the top of its
// panel, w wide: a heading, then two lines each, those working first and
// those just finished after.
func (m *Model) panelSubs(c *hostConn, w int) []viewLine {
	run, flows, done := c.dockSubs(), c.dockFlows(), c.doneShown()
	if len(run)+len(flows)+len(done) == 0 {
		return nil
	}
	working := len(run)
	for i := range flows {
		working += len(flows[i].agents)
	}
	how := fmt.Sprintf("%d working", working)
	if working == 0 {
		how = "all done"
	}
	folded := c.views.fold[subsFold]
	p := panelRuns{m: m, c: c, w: w, now: time.Now()}
	p.out = []viewLine{{viewHeading("Subagents", how, "", folded, w), viewAct{fold: subsFold, folded: folded}}}
	if folded {
		return p.out
	}
	for i := range flows {
		p.flow(&flows[i], i)
	}
	for i := range run {
		p.running(&run[i], "", i)
	}
	for i := range done {
		p.finished(&done[i])
	}
	return p.out
}

// panelRuns draws the runs at the panel's top.
type panelRuns struct {
	m   *Model
	c   *hostConn
	w   int
	now time.Time
	out []viewLine
}

// row adds a run's lines, lit when it's picked: a click watches it.
func (p *panelRuns) row(ref string, lines ...string) {
	for _, l := range lines {
		if l == "" {
			continue
		}
		if p.c.sel == ref {
			l = selBG + strings.ReplaceAll(fit(l, p.w), reset, reset+selBG) + reset
		}
		p.out = append(p.out, viewLine{l, viewAct{run: ref}})
	}
}

// name is a run's first line: its mark, its name, and on the right how
// long it has run or how it ended.
func (p *panelRuns) name(lead, mark, ink, right string, sa *convo.Subagent) string {
	room := p.w - cellw.String(lead) - 2 - cellw.String(right) - 2
	// What runs it only when the task shows whole beside it: the task
	// says which run this is.
	name := paint(ink, cellw.Truncate(subTask(*sa), max(8, room), "…"))
	if tag := p.c.subTag(*sa); tag != "" && cellw.String(subTask(*sa))+3+cellw.String(tag) <= room {
		name = p.c.subName(*sa, room, ink)
	}
	return spread(lead+mark+" "+name, dim(right), p.w)
}

// doing is a running run's second line: what it's doing now, else what
// it last did.
func (p *panelRuns) doing(lead string, sa *convo.Subagent) string {
	t := p.c.subTails[sa.ID]
	if t == nil {
		return lead + "  " + dim("starting…")
	}
	s := t.Sess
	var text string
	if d, _ := s.Doing(); d != "" {
		text = paint(cOrange, "› ") + paint(cText, d)
	} else if did := s.Did(); len(did) > 0 {
		text = dim("‹ " + did[0]) // newest first
	} else if lw := s.LastWords(); lw != "" {
		text = dim("‹ " + lw)
	} else {
		text = dim("thinking…")
	}
	return lead + "  " + cellw.Truncate(text, max(4, p.w-cellw.String(lead)-2), "…")
}

// ran is how long a run has gone, to end.
func (p *panelRuns) ran(sa *convo.Subagent) string {
	if t := p.c.subTails[sa.ID]; t != nil && !t.Sess.First.IsZero() {
		return dur(p.now.Sub(t.Sess.First).Round(time.Second))
	}
	return ""
}

func (p *panelRuns) running(sa *convo.Subagent, lead string, i int) {
	mark := spinOf(p.c.subKind(sa.ID), p.m.tick+i)
	p.row("run:"+sa.ID, p.name(lead, mark, cText, p.ran(sa), sa), p.doing(lead, sa))
}

// flow is a workflow: what it's for and the phase it's in, then its
// agents working under it.
func (p *panelRuns) flow(f *dockFlow, i int) {
	right := ""
	if !f.g.start.IsZero() {
		right = dur(p.now.Sub(f.g.start).Round(time.Second))
	}
	mark := paint(cOrange, spinner[(p.m.tick+i)%len(spinner)]) + " " + paint(cQueue, "⋔")
	top := spread(mark+" "+paint(cText, cellw.Truncate(f.g.title, max(8, p.w-cellw.String(right)-6), "…")), dim(right), p.w)
	under := ""
	if at := workflowNow(f.g.phases, p.m.tick+i); at != "" {
		under = "  " + cellw.Truncate(at, p.w-2, "…")
	}
	p.row(f.g.ref(), top, under)
	for k := range f.agents {
		p.running(&f.agents[k], "  ", i+k)
	}
}

// finished is a run that ended lately: ticked off, how it ended and
// when, and the last thing it said.
func (p *panelRuns) finished(d *doneRun) {
	status, _ := p.c.subStateIn(d.sa, nil)
	word := "done"
	if status == "failed" || status == "stopped" {
		word = status
	}
	right := word + " " + dur(p.now.Sub(d.at).Round(time.Second)) + " ago"
	under := ""
	if t := p.c.subTails[d.sa.ID]; t != nil {
		if lw := t.Sess.LastWords(); lw != "" {
			under = "  " + dim(cellw.Truncate("‹ "+lw, p.w-2, "…"))
		}
	}
	p.row("run:"+d.sa.ID, p.name("", subMark("", status, false, 0), cSub, right, &d.sa), under)
}
