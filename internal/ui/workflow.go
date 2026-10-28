package ui

import (
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// A workflow runs agents in phases. Claude Code says how each stands as it
// goes; for one rush didn't hear run, its files on disk say. Its row shows
// the phases, and opened, each agent under its phase, one a click away
// from its conversation.

// workflowSeen is what a workflow's files said, read off the UI.
type workflowSeen struct {
	at   time.Time
	live time.Time // when it was last asked of while live
	w    headless.Workflow
	read offRead[headless.Workflow]
}

// workflowPhase is one phase of a workflow and the agents it ran.
type workflowPhase struct {
	title  string
	agents []event.WorkflowAgent
}

// workflowDir is the folder of workflow task j's run, from what the
// Workflow tool gave back; "" until it has.
func (c *hostConn) workflowDir(j *convo.Job) string {
	if d := c.workflowDirs[j.ID]; d != "" {
		return d
	}
	st := c.sess.Step(j.ToolUseID)
	if st == nil {
		return ""
	}
	d := headless.WorkflowDir(st.Output)
	if d != "" {
		if c.workflowDirs == nil {
			c.workflowDirs = map[string]string{}
		}
		c.workflowDirs[j.ID] = d
	}
	return d
}

// workflowFiles is what the files of the run in dir last said: read again
// every 2s while live, and once more after, till they say it ended.
func (c *hostConn) workflowFiles(dir string, live bool) headless.Workflow {
	if dir == "" {
		return headless.Workflow{}
	}
	if c.workflows == nil {
		c.workflows = map[string]*workflowSeen{}
	}
	r := c.workflows[dir]
	if r == nil {
		r = &workflowSeen{}
		c.workflows[dir] = r
	}
	if v, ok := r.read.take(); ok {
		r.w = v
	}
	now := time.Now()
	if live {
		r.live = now
	}
	if !r.w.Done && now.Sub(r.at) >= 2*time.Second && (r.at.IsZero() || now.Sub(r.live) < 30*time.Second) {
		r.at = now
		if r.read.start(func() headless.Workflow { return headless.ReadWorkflow(dir) }) {
			if v, ok := r.read.take(); ok { // tests read at once
				r.w = v
			}
		}
	}
	return r.w
}

// workflowOf is how workflow task j stands: its phases, each with its
// agents, as Claude Code last said, else as its files do.
func (c *hostConn) workflowOf(j *convo.Job) []workflowPhase {
	f := c.workflowFiles(c.workflowDir(j), j.Running())
	phases, agents := j.Phases, j.Agents
	if len(f.Phases) > len(phases) {
		phases = f.Phases
	}
	if len(agents) == 0 {
		agents = f.Agents
	}
	return phasesOf(phases, agents, !j.Running())
}

// phasesOf puts each agent under its phase, in the phases' order; ended,
// one not heard to finish stopped with its workflow.
func phasesOf(phases []string, agents []event.WorkflowAgent, ended bool) []workflowPhase {
	out := make([]workflowPhase, 0, len(phases))
	for _, p := range phases {
		out = append(out, workflowPhase{title: p})
	}
	for _, a := range agents {
		if ended && (a.State == "running" || a.State == "queued") {
			a.State = "stopped"
		}
		i := slices.IndexFunc(out, func(p workflowPhase) bool { return p.title == a.Phase })
		if i < 0 {
			i = len(out)
			out = append(out, workflowPhase{title: a.Phase})
		}
		out[i].agents = append(out[i].agents, a)
	}
	return out
}

// workflowJob is the task of the workflow run whose folder's name is run,
// or nil when rush didn't hear it start.
func (c *hostConn) workflowJob(run string) *convo.Job {
	for _, j := range c.sess.Jobs() {
		if c.sess.JobKind(j) == "workflow" && filepath.Base(c.workflowDir(j)) == run {
			return j
		}
	}
	return nil
}

// workflowAgent is the word of a workflow's agent sa: from its workflow's
// task, and whether that still runs; else from its files, once they say
// it finished. Not ok when neither knows: its transcript says, then.
func (c *hostConn) workflowAgent(sa convo.Subagent) (a event.WorkflowAgent, running, ok bool) {
	if j := c.workflowJob(sa.Workflow); j != nil {
		for _, p := range c.workflowOf(j) {
			for _, a := range p.agents {
				if a.ID == sa.ID {
					return a, j.Running(), true
				}
			}
		}
		return a, false, false
	}
	for _, a := range c.workflowFiles(filepath.Dir(sa.Path), false).Agents {
		if a.ID == sa.ID && (a.State == "done" || a.State == "failed") {
			return a, false, true
		}
	}
	return a, false, false
}

// workflowGroup is one workflow run as the subagents view and the dock
// show it: its phases, and its agents' runs.
type workflowGroup struct {
	run, title string
	job        *convo.Job // nil for one rush didn't hear start
	phases     []workflowPhase
	subs       map[string]convo.Subagent // its agents' runs, by agent id
	live       bool
	start, end time.Time
}

// ref is what picks the group's own row.
func (g workflowGroup) ref() string {
	if g.job != nil {
		return "job:" + g.job.ID
	}
	return "wf:" + g.run
}

// workflowGroups are the session's workflow runs, newest first: those
// rush heard start, and those whose agents' runs are on disk.
func (c *hostConn) workflowGroups(jobs map[string]*convo.Job) []workflowGroup {
	var out []workflowGroup
	at := map[string]int{}
	for _, j := range c.sess.Jobs() {
		if c.sess.JobKind(j) != "workflow" {
			continue
		}
		run := filepath.Base(c.workflowDir(j))
		if c.workflowDir(j) == "" {
			run = "task:" + j.ID
		}
		at[run] = len(out)
		out = append(out, workflowGroup{run: run, title: jobLabelPlain(j), job: j, phases: c.workflowOf(j),
			subs: map[string]convo.Subagent{}, live: j.Running(), start: j.Start, end: j.End})
	}
	for _, sa := range c.subs {
		if sa.Workflow == "" {
			continue
		}
		i, ok := at[sa.Workflow]
		if !ok {
			i, at[sa.Workflow] = len(out), len(out)
			f := c.workflowFiles(filepath.Dir(sa.Path), false)
			out = append(out, workflowGroup{run: sa.Workflow, title: firstNonEmpty(f.Title, sa.Workflow),
				phases: phasesOf(f.Phases, f.Agents, f.Done), subs: map[string]convo.Subagent{}, start: time.Unix(0, sa.Born)})
		}
		g := &out[i]
		g.subs[sa.ID] = sa
		if g.job == nil {
			if _, live := c.subStateIn(sa, jobs); live {
				g.live = true
			}
			if sa.Mod > 0 && time.Unix(0, sa.Mod).After(g.end) {
				g.end = time.Unix(0, sa.Mod)
			}
		}
	}
	slices.SortStableFunc(out, func(a, b workflowGroup) int { return b.start.Compare(a.start) })
	return out
}

// jobLabelPlain is a task's label in a line, without colour.
func jobLabelPlain(j *convo.Job) string {
	return oneLineUI(firstNonEmpty(j.Label, j.Summary, "workflow"))
}

// phaseMark is how a phase stands: not started, running, failed or done.
func phaseMark(p workflowPhase, tick int) string {
	run, done, failed := 0, 0, 0
	for _, a := range p.agents {
		switch a.State {
		case "running", "queued":
			run++
		case "done":
			done++
		case "failed":
			failed++
		}
	}
	switch {
	case len(p.agents) == 0:
		return paint(cFaint, "·")
	case run > 0:
		return paint(cOrange, spinner[tick%len(spinner)])
	case failed > 0:
		return paint(cRed, "✗")
	case done == len(p.agents):
		return paint(cGreen, "✓")
	}
	return dim("⏹")
}

// agentMark is how a workflow's agent stands.
func agentMark(a event.WorkflowAgent, tick int) string {
	switch a.State {
	case "running":
		return paint(cOrange, spinner[tick%len(spinner)])
	case "queued":
		return paint(cFaint, "◌")
	case "done":
		return paint(cGreen, "✓")
	case "failed":
		return paint(cRed, "✗")
	}
	return dim("⏹")
}

// workflowStrip is a workflow's phases on a line: each one's mark and
// title, and how many of its agents are done while it runs.
func workflowStrip(phases []workflowPhase, tick int) string {
	var parts []string
	for _, p := range phases {
		done := 0
		for _, a := range p.agents {
			if a.State == "done" {
				done++
			}
		}
		title := firstNonEmpty(p.title, "agents")
		mark := phaseMark(p, tick)
		switch {
		case len(p.agents) == 0:
			parts = append(parts, mark+" "+paint(cFaint, title))
		case done < len(p.agents):
			parts = append(parts, mark+" "+paint(cText+bold, title)+dim(fmt.Sprintf(" %d/%d", done, len(p.agents))))
		default:
			parts = append(parts, mark+" "+paint(cSub, title))
		}
	}
	return strings.Join(parts, paint(cFaint, "  ›  "))
}

// workflowRows are workflow j's rows under its own: its phases on a line;
// opened, each phase and the agents it ran, each agent's row its own to
// pick and open. refs are each row's.
func (m *Model) workflowRows(c *hostConn, j *convo.Job, open bool, w int, now time.Time) (rows, refs []string) {
	phases := c.workflowOf(j)
	if len(phases) == 0 {
		return nil, nil
	}
	ref := "job:" + j.ID
	if !open {
		return []string{cellw.Truncate("      "+workflowStrip(phases, m.tick), w-2, "…")}, []string{ref}
	}
	labelW := 8
	for _, p := range phases {
		for _, a := range p.agents {
			labelW = max(labelW, min(28, cellw.String(a.Label)))
		}
	}
	for pi, p := range phases {
		head := "      " + phaseMark(p, m.tick+pi) + " " + paint(cText+bold, firstNonEmpty(p.title, "agents"))
		if n := len(p.agents); n > 0 {
			head += dim(fmt.Sprintf("  %d agent%s", n, plural(n)))
		} else {
			head += dim("  not started")
		}
		rows, refs = append(rows, cellw.Truncate(head, w-2, "…")), append(refs, ref)
		for ai, a := range p.agents {
			rows = append(rows, m.workflowAgentRow(a, labelW, m.tick+ai, w, now))
			if a.ID != "" {
				refs = append(refs, "run:"+a.ID)
			} else {
				refs = append(refs, ref)
			}
		}
	}
	return rows, refs
}

// workflowAgentRow is one agent's row: its mark and label, its model, how
// long it ran and what it used, then what it's doing or what it gave back.
func (m *Model) workflowAgentRow(a event.WorkflowAgent, labelW, tick, w int, now time.Time) string {
	took := a.Took
	if took == 0 && !a.Start.IsZero() && a.State == "running" {
		took = now.Sub(a.Start)
	}
	var facts []string
	if a.Model != "" {
		facts = append(facts, convo.PrettyModel(a.Model))
	}
	if took > 0 {
		facts = append(facts, dur(took.Round(time.Second)))
	}
	if a.Tokens > 0 {
		facts = append(facts, convo.Tokens(a.Tokens))
	}
	var what string
	switch a.State {
	case "running":
		if a.LastTool != "" {
			what = paint(cOrange, "›") + " " + paint(cText, a.LastTool) + " " + dim(oneLineUI(a.Doing))
		}
	case "queued":
		what = dim("waiting to start")
	case "failed":
		what = paint(cRed, oneLineUI(a.Error))
	case "done":
		what = dim(oneLineUI(a.Result))
	case "stopped":
		what = dim("stopped with its workflow")
	}
	label := cellw.Truncate(a.Label, labelW, "…")
	row := "        " + agentMark(a, tick) + " " + paint(cText, label) + strings.Repeat(" ", labelW-cellw.String(label))
	if len(facts) > 0 {
		row += "  " + dim(strings.Join(facts, " · "))
	}
	if what != "" {
		row += "  " + what
	}
	return cellw.Truncate(row, w-2, "…")
}

// workflowNow is a running workflow's dock line: its phase now, and the
// agent last heard from.
func workflowNow(phases []workflowPhase, tick int) string {
	for _, p := range phases {
		done, running := 0, ""
		for _, a := range p.agents {
			switch a.State {
			case "done":
				done++
			case "running":
				running = a.Label
			}
		}
		if len(p.agents) == 0 || done == len(p.agents) {
			continue
		}
		s := phaseMark(p, tick) + " " + paint(cText, firstNonEmpty(p.title, "agents")) + dim(fmt.Sprintf(" %d/%d", done, len(p.agents)))
		if running != "" {
			s += dim(" · " + running)
		}
		return s
	}
	return ""
}

// workflowGroupLines adds workflow g to the subagents view's lines: its
// row (what it's for, its phases, how it stands), then each phase and
// under it its agents, each one's run drawn as any run is, a step in.
func (m *Model) workflowGroupLines(c *hostConn, g workflowGroup, runs map[string]subListRow,
	draw func(r subListRow, i int, indent string), emit func(convo.Line), o convo.Options) {
	w, now := o.Width, time.Now()
	end := g.end
	if g.live || end.IsZero() {
		end = now
	}
	right := ""
	switch {
	case g.live:
		right = paint(cOrange, "running")
	case g.job != nil && g.job.Status == "failed":
		right = paint(cRed, "failed")
	case g.job != nil && g.job.Status == "stopped":
		right = dim("stopped")
	default:
		right = dim("done")
	}
	if !g.start.IsZero() {
		right += "   " + dim(dur(end.Sub(g.start).Round(time.Second)))
	}
	right += "  "
	mark := paint(cQueue, "⋔")
	if g.live {
		mark = paint(cOrange, spinner[m.tick%len(spinner)])
	}
	room := w - cellw.String(ansi.Strip(right)) - 8
	title := cellw.Truncate(g.title, max(12, min(room/2, 60)), "…")
	strip := workflowStrip(g.phases, m.tick)
	left := "  " + mark + " " + paint(cText+bold, title) + "   " + strip
	top := spread(cellw.Truncate(left, max(20, room+4), "…"), right, w)
	ref := g.ref()
	switch ref {
	case o.Selected:
		top = picked1(top, w, o.Focused)
	case c.subHover:
		top = hoverLine(top, w)
	default:
		top = fit(top, w)
	}
	emit(convo.Line{Text: top, Ref: ref})
	if g.job != nil && (g.job.Tokens > 0 || g.job.ToolUses > 0) {
		var facts []string
		if g.job.ToolUses > 0 {
			facts = append(facts, fmt.Sprintf("%d steps", g.job.ToolUses))
		}
		if g.job.Tokens > 0 {
			facts = append(facts, convo.Tokens(g.job.Tokens)+" tokens")
		}
		row := fit("      "+dim(strings.Join(facts, " · ")), w)
		if ref == o.Selected {
			row = picked1(row, w, o.Focused)
		}
		emit(convo.Line{Text: row, Ref: ref})
	}
	shown := map[string]bool{}
	labelW := 8
	for _, p := range g.phases {
		for _, a := range p.agents {
			labelW = max(labelW, min(28, cellw.String(a.Label)))
		}
	}
	for _, p := range g.phases {
		head := "    " + paint(cSub+bold, firstNonEmpty(p.title, "agents"))
		if len(p.agents) == 0 {
			head += dim(" · not started")
		}
		emit(convo.Line{Text: fit(head, w)})
		for i, a := range p.agents {
			if r, ok := runs[a.ID]; ok && a.ID != "" {
				shown[a.ID] = true
				draw(r, i, "    ")
				continue
			}
			// Not started, or its run's files aren't there yet.
			emit(convo.Line{Text: fit(strings.TrimPrefix(m.workflowAgentRow(a, labelW, m.tick+i, w, now), "  "), w)})
		}
	}
	// Runs no phase lists: before its progress is heard of.
	i := 0
	for _, sa := range c.subs {
		if r, ok := runs[sa.ID]; ok && sa.Workflow == g.run && !shown[sa.ID] {
			draw(r, i, "    ")
			i++
		}
	}
}

// dockFlow is a workflow working in the background, as the dock shows
// it: its row, then its agents still working.
type dockFlow struct {
	g      workflowGroup
	agents []convo.Subagent
}

// dockFlows are the workflows working beside the turn, newest first. One
// the turn waits on shows under its step in the conversation instead.
// Asked several times a frame: kept for the second.
func (c *hostConn) dockFlows() []dockFlow {
	k := runsKey{sess: c.sess, applied: c.sess.Applied(), sec: time.Now().Unix()}
	if f := &c.flowMemo; f.ok && f.key == k {
		return f.out
	}
	var out []dockFlow
	running := c.runningSubs()
	for _, g := range c.workflowGroups(c.sess.SubagentJobs()) {
		if !g.live || g.job != nil && !g.job.Background {
			continue
		}
		f := dockFlow{g: g}
		for _, sa := range running {
			if sa.Workflow == g.run {
				f.agents = append(f.agents, sa)
			}
		}
		out = append(out, f)
	}
	c.flowMemo.key, c.flowMemo.out, c.flowMemo.ok = k, out, true
	return out
}

// flowRow is a workflow's row in the dock: what it's for, the phase it's
// in and its agent last heard from, how many of its agents are done.
func (m *Model) flowRow(c *hostConn, g workflowGroup, i, w int, now time.Time) string {
	done, all := 0, 0
	for _, p := range g.phases {
		for _, a := range p.agents {
			all++
			if a.State == "done" {
				done++
			}
		}
	}
	var facts []string
	if all > 0 {
		facts = append(facts, fmt.Sprintf("%d/%d agents done", done, all))
	}
	if !g.start.IsZero() {
		facts = append(facts, dur(now.Sub(g.start).Round(time.Second)))
	}
	right := dim(strings.Join(facts, " · ")) + "  "
	left := "  " + paint(cOrange, spinner[(m.tick+i)%len(spinner)]) + " " + paint(cQueue, "⋔") + " " +
		paint(cText+bold, cellw.Truncate(g.title, min(40, max(16, w/3)), "…"))
	if at := workflowNow(g.phases, m.tick+i); at != "" {
		left += "  " + paint(cOrange, "›") + " " + at
	}
	row := spread(cellw.Truncate(left, max(20, w-cellw.String(ansi.Strip(right))-2), "…"), right, w)
	if c.sel == g.ref() {
		row = picked1(row, w, m.paneFocus)
	}
	return row
}

// isWorkflowRef is whether ref picks a workflow: its task, or a run of
// one rush didn't hear start.
func (m *Model) isWorkflowRef(c *hostConn, ref string) bool {
	if strings.HasPrefix(ref, "wf:") {
		return true
	}
	id, ok := strings.CutPrefix(ref, "job:")
	if !ok {
		return false
	}
	j := c.sess.Job(id)
	return j != nil && c.sess.JobKind(j) == "workflow"
}

// openWorkflow shows the subagents view on the workflow ref picks: its
// phases and its agents' runs.
func (m *Model) openWorkflow(c *hostConn, ref string) {
	m.showView(c, "subagents")
	c.subOpen, c.subTail, c.subBack, c.subFrom, c.subFromView = "", nil, false, "", ""
	c.sel, c.selMoved = ref, true
}
