package ui

import (
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"time"

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
	dir  string // the run's folder
	at   time.Time
	w    headless.Workflow
	read offRead[headless.Workflow]
}

// workflowPhase is one phase of a workflow and the agents it ran.
type workflowPhase struct {
	title  string
	agents []event.WorkflowAgent
}

// workflowOf is how workflow task j stands: its phases, each with its
// agents, as Claude Code last said, else as its files do.
func (c *hostConn) workflowOf(j *convo.Job) []workflowPhase {
	if c.workflows == nil {
		c.workflows = map[string]*workflowSeen{}
	}
	r := c.workflows[j.ID]
	if r == nil {
		r = &workflowSeen{}
		c.workflows[j.ID] = r
	}
	if r.dir == "" {
		if st := c.sess.Step(j.ToolUseID); st != nil {
			r.dir = headless.WorkflowDir(st.Output)
		}
	}
	if v, ok := r.read.take(); ok {
		r.w = v
	}
	// Read while it runs, and for a while after, till it says it ended.
	now := time.Now()
	if r.dir != "" && !r.w.Done && now.Sub(r.at) >= 2*time.Second && (j.Running() || now.Sub(j.End) < 30*time.Second || r.at.IsZero()) {
		r.at = now
		dir := r.dir
		if r.read.start(func() headless.Workflow { return headless.ReadWorkflow(dir) }) {
			if v, ok := r.read.take(); ok { // tests read at once
				r.w = v
			}
		}
	}
	phases, agents := j.Phases, j.Agents
	if len(r.w.Phases) > len(phases) {
		phases = r.w.Phases
	}
	if len(agents) == 0 {
		agents = r.w.Agents
	}
	out := make([]workflowPhase, 0, len(phases))
	for _, p := range phases {
		out = append(out, workflowPhase{title: p})
	}
	for _, a := range agents {
		if !j.Running() && (a.State == "running" || a.State == "queued") {
			a.State = "stopped" // its workflow ended without word of it
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

// workflowRun is the run id of workflow task j, from its folder.
func (c *hostConn) workflowRun(j *convo.Job) string {
	c.workflowOf(j)
	if r := c.workflows[j.ID]; r != nil && r.dir != "" {
		return filepath.Base(r.dir)
	}
	return ""
}

// workflowAgent is the word of a workflow's agent sa from its workflow's
// task, and whether that task still runs.
func (c *hostConn) workflowAgent(sa convo.Subagent) (a event.WorkflowAgent, running, ok bool) {
	for _, j := range c.sess.Jobs() {
		if c.sess.JobKind(j) != "workflow" || c.workflowRun(j) != sa.Workflow {
			continue
		}
		for _, p := range c.workflowOf(j) {
			for _, a := range p.agents {
				if a.ID == sa.ID {
					return a, j.Running(), true
				}
			}
		}
	}
	return a, false, false
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
