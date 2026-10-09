package ui

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// What an agent runs besides talking: shells, monitors and workflows,
// in the background or with the turn waiting on them. The dock shows the
// ones running, the background view all of them; either lets you stop
// one, or send one the turn is waiting on into the background.

// dockJobsShown is how many running tasks the dock shows at once.
const dockJobsShown = 4

// dockJobs are the tasks the dock shows: everything running but subagents
// (they have their own block), a foreground one once it's run a while.
func (c *hostConn) dockJobs() []*convo.Job {
	var out []*convo.Job
	for _, j := range c.sess.RunningJobs() {
		// What the turn waits on shows in the conversation, its logs too.
		if c.jobKind(j) == "subagent" || !j.Background {
			continue
		}
		out = append(out, j)
	}
	return out
}

// jobIcon is a task's mark and colour, by what it is.
func jobIcon(kind string) (string, string) {
	switch kind {
	case "monitor":
		return "◎", cBlue
	case "workflow":
		return "⧉", cQueue
	case "subagent":
		return "⇉", cBlue
	}
	return "$", cSub
}

// jobLabel leads with the task description, falling back to its command.
func jobLabel(c *hostConn, j *convo.Job) string {
	l := j.Label
	if cmd := c.sess.JobCommand(j); l == "" && cmd != "" {
		l = cmd
	}
	if j.Agent != "" {
		l = j.Agent + "  " + l
	}
	return oneLine(tildify(convo.DropCd(l)))
}

// jobOwner is the subagent that started task j, when one did. Asking
// walks every run, so the answer is kept: a call is one run's for good,
// and one no run had made is asked again a second later, as runs are read.
func (c *hostConn) jobOwner(j *convo.Job) (convo.Subagent, bool) {
	if o, ok := c.owners[j.ToolUseID]; ok && (o.found && c.subTails[o.sa.ID] != nil || !o.found && time.Since(o.at) < time.Second) {
		return o.sa, o.found
	}
	o := jobOwnerSeen{at: time.Now()}
	for _, sa := range c.subs {
		if t := c.subTails[sa.ID]; t != nil && t.Sess.MadeCall(j.ToolUseID) {
			o.sa, o.found = sa, true
			break
		}
	}
	if c.owners == nil {
		c.owners = map[string]jobOwnerSeen{}
	}
	c.owners[j.ToolUseID] = o
	return o.sa, o.found
}

type jobOwnerSeen struct {
	sa    convo.Subagent
	found bool
	at    time.Time
}

// madeBy is whether subagent id started task j: jobOwner without walking
// every run, for a loop already over one run's tasks.
func (c *hostConn) madeBy(j *convo.Job, id string) bool {
	t := c.subTails[id]
	return t != nil && t.Sess.MadeCall(j.ToolUseID)
}

// jobWho is whose a task is and where it runs, after its command:
// "→ lane-opus(go-ports)" for a subagent's, "⎇ go-ports" for the
// session's own in another worktree. "" for the session's own, at home.
// Where is the worktree its command cds into, else its subagent's, when
// that isn't the session's own.
func jobWho(c *hostConn, j *convo.Job) string {
	sa, ok := c.jobOwner(j) // walks every run: once a row
	wt := convo.WorktreeIn(c.sess.JobCommand(j))
	if ok && wt == "" {
		wt = c.subTails[sa.ID].Sess.Worktree()
	}
	if wt == c.ownWorktree() {
		wt = ""
	}
	if ok {
		who := "  " + faint("→ ") + paint(cBlue, sa.Type)
		if wt != "" {
			who += faint("(") + dim(wt) + faint(")")
		}
		return who
	}
	if wt != "" {
		return "  " + faint("⎇ ") + dim(wt)
	}
	return ""
}

// looseJobs are the dock's tasks no running subagent started: those
// show under their subagent instead.
func (c *hostConn) looseJobs() []*convo.Job {
	running := c.runningSubs()
	var out []*convo.Job
	for _, j := range c.dockJobs() {
		if !slices.ContainsFunc(running, func(sa convo.Subagent) bool { return c.madeBy(j, sa.ID) }) {
			out = append(out, j)
		}
	}
	return out
}

// jobsOf are the dock's tasks subagent id started.
func (c *hostConn) jobsOf(id string) []*convo.Job {
	var out []*convo.Job
	for _, j := range c.dockJobs() {
		if c.madeBy(j, id) {
			out = append(out, j)
		}
	}
	return out
}

// jobHint is what the keys do on picked task j.
func (c *hostConn) jobHint(j *convo.Job) string {
	switch rp, ok := c.sess.RunningPart(j.ToolUseID); {
	case j.Background:
		return keys("x", "stop", "shift+x", "…and say why", "space", "output", "d", "command details")
	case ok:
		return keys("k", "kill "+firstWord(rp.Command), "p", pauseWord(j.ToolUseID), "b", "background", "x", "stop", "shift+x", "…and say why")
	}
	return keys("b", "background", "x", "stop", "shift+x", "…and say why", "space", "output", "d", "command details")
}

// jobRow is a dock task's row, lead before its mark, whose it is after its
// command when who, and its latest line of output under it.
func (m *Model) jobRow(c *hostConn, j *convo.Job, i, w int, lead string, who bool, now time.Time) []string {
	kind := c.sess.JobKind(j)
	icon, col := jobIcon(kind)
	mark := paint(col, icon)
	if !j.Background {
		mark = paint(cOrange, spinner[(m.tick+i)%len(spinner)])
	}
	right, whose := m.jobRight(c, j, now), ""
	if who {
		whose = jobWho(c, j)
	}
	room := max(12, w-cellw.String(ansi.Strip(right))-cellw.String(ansi.Strip(whose))-12-cellw.String(lead))
	// Under a subagent (no who), a task reads as its, not as another run.
	name := paint(cText+bold, fmt.Sprintf("%-7s", kind))
	if !who {
		name = paint(cSub, fmt.Sprintf("%-7s", kind))
		lead += paint(cFaint, "↳") + " "
	}
	left := lead + mark + " " + name + " " +
		paint(cSub, cellw.Truncate(jobLabel(c, j), min(room, 40), "…")) + whose
	// Its latest line of output, on the same row, as far as there's room.
	if last := m.jobTail(c, j, 1); len(last) > 0 {
		left += "  " + paint(cOrange, "›") + " " + dim(last[0])
	}
	rows := []string{spread(cellw.Truncate(left, max(20, w-cellw.String(ansi.Strip(right))-2), "…"), right, w)}
	if c.sel == "job:"+j.ID {
		for k := range rows {
			rows[k] = picked1(rows[k], w, m.paneFocus)
		}
	}
	return rows
}

// jobState is how a task stands, in a word or two and its colour.
func jobState(j *convo.Job, now time.Time) string {
	took := ""
	end := j.End
	if j.Running() {
		end = now
	}
	if !j.Start.IsZero() {
		took = " · " + dur(end.Sub(j.Start).Round(time.Second))
	}
	switch {
	case j.Running() && j.Background:
		return dim("in background" + took)
	case j.Running():
		return paint(cOrange, "waiting on it") + dim(took)
	case j.Status == "completed":
		return paint(cGreen, "✓ done") + dim(took)
	case j.Status == "failed":
		return paint(cRed, "✗ failed") + dim(took)
	}
	return dim("⏹ "+j.Status) + dim(took)
}

// jobPID is the process a running shell task runs as: of those Claude
// started, the one that started with it.
// ponytail: matched on start time alone, so two shells started in the same
// second can swap; the task's own pid if Claude Code ever says it.
func (m *Model) jobPID(c *hostConn, j *convo.Job) int {
	if !j.Running() || c.sess.JobKind(j) != "shell" || m.snap == nil || m.snap.Table == nil || j.Start.IsZero() {
		return 0
	}
	tab, best, gap := m.snap.Table, 0, 3*time.Second
	for _, k := range tab.Children[c.sess.Info.ClaudePID] {
		if p := tab.Procs[k]; p != nil {
			if d := p.Start.Sub(j.Start).Abs(); d < gap {
				best, gap = k, d
			}
		}
	}
	return best
}

// crashRe is a line a program writes as it dies, or as it gives up and
// waits: a dev server whose app crashed keeps running, and Claude, told
// it runs, never hears.
var crashRe = regexp.MustCompile(`^panic: |Traceback \(most recent call last\)|\bFATAL\b|EADDRINUSE|address already in use|app crashed|exited with code [1-9]|Segmentation fault|npm ERR!|ELIFECYCLE|Cannot find module|command not found|^error(\[E\d+\])?: |^Killed$|^fatal error: `)

// crashQuiet suppresses transient error output while a program may restart.
// The warning reports output evidence, never an inferred process state.
const crashQuiet = 20 * time.Second

// jobCrashed is whether a running task's output ends on a crash and has
// stayed that way: still running as far as Claude knows, but likely dead.
func (m *Model) jobCrashed(c *hostConn, j *convo.Job, now time.Time) bool {
	if !j.Running() {
		return false
	}
	tail, from := m.jobTailFrom(c, j, 3)
	if e := c.tails[from]; e == nil || e.mod.IsZero() || now.Sub(e.mod) < crashQuiet {
		return false
	}
	return slices.ContainsFunc(tail, func(l string) bool { return crashRe.MatchString(strings.TrimSpace(l)) })
}

// jobRight is what ends a task's row: what it uses and how it's doing.
func (m *Model) jobRight(c *hostConn, j *convo.Job, now time.Time) string {
	if m.jobCrashed(c, j, now) {
		return paint(cRed, "⚠ error in recent output") + dim("  ·  ") + jobState(j, now) + "  "
	}
	return m.jobUsage(c, j) + jobState(j, now) + "  "
}

// jobUsage is a running task's CPU and memory, for the end of its row.
func (m *Model) jobUsage(c *hostConn, j *convo.Job) string {
	pid := m.jobPID(c, j)
	if pid == 0 {
		return ""
	}
	b, cpu, _ := m.snap.Table.Sum(pid, nil)
	return cpuColor(cpu, fmt.Sprintf("%.0f%%", cpu)) + dim(" · ") + memColor(b, mem(b)) + dim("  ·  ")
}

// jobsPreview is the dock's block of running tasks: a heading, then a row
// each with its latest line of output under it.
func (m *Model) jobsPreview(c *hostConn, jobs []*convo.Job, w int) []string {
	picked := -1
	for i, j := range jobs {
		if c.sel == "job:"+j.ID {
			picked = i
		}
	}
	fg := 0
	for _, j := range jobs {
		if !j.Background {
			fg++
		}
	}
	head := "running"
	if bg := len(jobs) - fg; bg > 0 && fg > 0 {
		head += fmt.Sprintf(" · %d in the background", bg)
	} else if fg == 0 {
		head = "in the background"
	}
	hint := ""
	switch {
	case picked >= 0:
		hint = c.jobHint(jobs[picked])
	case m.paneFocus && fg > 0:
		hint = keys("ctrl+b", "background", "↑", "pick")
	case m.paneFocus && len(c.input) == 0 && len(m.queueOf(c).items) == 0:
		hint = keys("↑", "pick one to stop")
	}
	out := []string{spread(" "+paint(cOrange+bold, "shell")+paint(cFaint, fmt.Sprintf(" ·%d", len(jobs)))+dim("  "+head), hint+"  ", w)}
	start := 0
	if picked >= dockJobsShown {
		start = picked - dockJobsShown + 1
	}
	if start > 0 {
		out = append(out, dim(fmt.Sprintf("  … %d before", start)))
	}
	now := time.Now()
	for i := start; i < min(len(jobs), start+dockJobsShown); i++ {
		if c.panelRefs != nil {
			c.panelRefs[c.previewBase+len(out)] = "job:" + jobs[i].ID
		}
		out = append(out, m.jobRow(c, jobs[i], i, w, "  ", true, now)...)
	}
	if rest := len(jobs) - start - dockJobsShown; rest > 0 {
		out = append(out, dim(fmt.Sprintf("  + %d more in the background view", rest)))
	}
	return out
}

// jobLines is the background view: every task this session has run but
// its subagents (the subagents view has those), what's running first, each
// with its output under it when opened.
func (m *Model) jobLines(c *hostConn, o convo.Options) []convo.Line {
	w := o.Width
	var run, done []*convo.Job
	for _, j := range c.sess.WorkJobs() {
		if c.spawnJob(j) || !j.Background {
			continue // a spawned agent (the subagents view), or the turn's own (the conversation)
		}
		if j.Running() {
			run = append(run, j)
		} else {
			done = append(done, j)
		}
	}
	slices.Reverse(done) // the latest finished first
	head := dim(fmt.Sprintf("%d tasks", len(run)+len(done)))
	if len(run) > 0 {
		head = paint(cOrange, fmt.Sprintf("%d running", len(run))) + dim(" · ") + head
	}
	how := "space or click: output · d: command details · x: stop"
	lines := []convo.Line{{Text: fit("  "+head+dim(" · "+how), w)}, {Text: ""}}
	if len(run)+len(done) == 0 {
		return append(lines, convo.Line{Text: dim("  nothing running · shells, monitors and workflows your agent starts show here")})
	}
	now := o.Now
	if now.IsZero() {
		now = time.Now()
	}
	// edge runs down the left of an opened task, beside every part of it,
	// so its processes, command and output read as inside it: the pick's
	// bar when it's picked.
	edge := func(ref, r string) string {
		if !c.open[ref] {
			return r
		}
		bar := paint(cEdge, "│")
		switch {
		case ref == o.Selected && o.Focused:
			bar = paint(cOrange, "▍")
		case ref == o.Selected:
			bar = faint("▍")
		}
		return bar + ansi.Cut(r, 1, w)
	}
	emit := func(ref string, rows []string, pick bool) {
		for _, r := range rows {
			switch {
			case pick && ref == o.Selected:
				r = picked1(r, w, o.Focused)
			case pick && ref == c.subHover:
				r = edge(ref, hoverLine(r, w))
			default:
				r = edge(ref, fit(r, w))
			}
			lines = append(lines, convo.Line{Text: r, Ref: ref})
		}
	}
	section := func(title string, jobs []*convo.Job) {
		if len(jobs) == 0 {
			return
		}
		lines = append(lines, convo.Line{Text: fit("  "+paint(cSub+bold, title), w)})
		for i, j := range jobs {
			ref := "job:" + j.ID
			kind := c.sess.JobKind(j)
			icon, col := jobIcon(kind)
			mark := paint(col, icon)
			fold := faint("▸") // every task opens, whatever it is
			if c.open[ref] {
				fold = paint(cText, "▾")
			}
			if j.Running() && !j.Background {
				mark = paint(cOrange, spinner[(m.tick+i)%len(spinner)])
			}
			n := 1
			if j.Running() {
				n = 3
			}
			if c.open[ref] {
				n = jobTailMost
			}
			tail, from := m.jobTailFrom(c, j, n)
			// A finished task's rows stay as they were drawn until what
			// they're drawn from changes: a long session has hundreds.
			var key jobRowsKey
			if memo := !j.Running() && !c.open[ref]; memo {
				key = jobRowsKey{w: w, pal: cText + cSub, status: j.Status, end: j.End, from: from, tail: len(tail),
					steps: j.ToolUses, tokens: j.Tokens, errorText: j.Error, summary: j.Summary, label: j.Label, agent: j.Agent}
				if len(tail) > 0 {
					key.last = tail[len(tail)-1]
				}
				if e, ok := c.jobRows[j.ID]; ok && e.key == key {
					emit(ref, e.rows, true)
					continue
				}
			}
			right := m.jobRight(c, j, now)
			label := jobLabel(c, j)
			who := jobWho(c, j)
			left := "  " + fold + " " + mark + " " + paint(cText+bold, fmt.Sprintf("%-8s", kind)) + " " +
				who + paint(cSub, cellw.Truncate(label, max(12, w-cellw.String(ansi.Strip(right))-cellw.String(ansi.Strip(who))-18), "…"))
			rows := []string{spread(left, right, w)}
			var facts []string
			if rp, ok := c.sess.RunningPart(j.ToolUseID); ok {
				facts = append(facts, paint(cOrange, fmt.Sprintf("▸ %d/%d", rp.At, rp.Of))+" "+paint(cText, oneLine(rp.Command)))
			}
			if j.ToolUses > 0 {
				facts = append(facts, fmt.Sprintf("%d steps", j.ToolUses))
			}
			if j.Tokens > 0 {
				facts = append(facts, convo.Tokens(j.Tokens)+" tokens")
			}
			if s := firstNonEmpty(j.Error, j.Summary); s != "" && s != j.Label {
				facts = append(facts, oneLine(s))
			}
			if len(facts) > 0 {
				rows = append(rows, cellw.Truncate("      "+paint(cSub, strings.Join(facts, dim(" · "))), w-2, "…"))
			}
			if activity := c.jobActivity(j, from, now); activity != "" {
				rows = append(rows, "      "+activity)
			}
			// The launch script is secondary to progress and recent output.
			var body []convo.Line
			if c.open[ref] && c.open["job-details:"+j.ID] {
				body = c.sess.JobLines(j, o, 6)
			}
			var processes []string
			var followed []string // what a tail -f under it follows
			if pid := m.jobPID(c, j); pid != 0 && c.open[ref] {
				// Opened, what it runs: the processes under it, busiest first.
				nodes := m.snap.Table.Tree(pid)
				for _, n := range nodes {
					followed = append(followed, tailFollows(m.shortCmd(n.PID, n.Comm), c.sess.Info.Cwd)...)
				}
				sort.SliceStable(nodes, func(a, b int) bool { return nodes[a].CPU > nodes[b].CPU })
				for _, n := range nodes[:min(len(nodes), 5)] {
					name := filepath.Base(n.Comm)
					if c.open["job-details:"+j.ID] {
						name = m.shortCmd(n.PID, n.Comm)
					}
					processes = append(processes, "      "+paint(cSub, fit(name, max(10, w-24)))+
						cpuColor(n.CPU, right1(fmt.Sprintf("%.0f%%", n.CPU), 6))+memColor(n.Footprint, right1(mem(n.Footprint), 8)))
				}
			}
			switch {
			case len(tail) > 0 && from != c.jobOutput(j):
				rows = append(rows, "      "+faint("from "+c.shownPath(from))+c.tailWhen(from, now))
			case len(tail) > 0 && c.open[ref]:
				rows = append(rows, "      "+paint(cSub+bold, "Recent output")+c.tailWhen(from, now))
			}
			if len(tail) == 0 && c.open[ref] && !j.Background {
				// Claude Code keeps no file for a call the turn waited on:
				// what it got back is its output.
				if st := c.sess.Step(j.ToolUseID); st != nil {
					tail = lastLines(st.Output, n)
				}
			}
			if from != c.jobOutput(j) && from != "" {
				rows = append(rows, c.fileRows(from, tail, 6, w-2)...) // a file it writes, as the document it is
			} else {
				for _, l := range tail {
					rows = append(rows, cellw.Truncate("      "+paint(cFaint, "│ ")+paint(cText, l), w-2, "…"))
				}
			}
			if len(tail) == 0 && c.open[ref] {
				none := "no output"
				if j.Running() {
					none = "no output yet"
				}
				rows = append(rows, "      "+paint(cSub, none))
			}
			if c.open[ref] {
				// Opened: every other file its command writes, and those what
				// it runs follows, too; one that's there but empty says so.
				var seen []string
				for _, f := range append(c.jobWrites(j), followed...) {
					if f == from || slices.Contains(seen, f) {
						continue
					}
					seen = append(seen, f)
					more := c.tailOf(f, j.Running(), jobFileLines)
					if len(more) == 0 {
						if e := c.tails[f]; e != nil && e.size == 0 {
							rows = append(rows, "      "+faint("from "+c.shownPath(f)+" · empty so far")+c.tailWhen(f, now))
						}
						continue
					}
					rows = append(rows, "      "+faint("from "+c.shownPath(f))+c.tailWhen(f, now))
					rows = append(rows, c.fileRows(f, more, 6, w-2)...)
				}
			}
			if key.w != 0 {
				if c.jobRows == nil {
					c.jobRows = map[string]jobRowsMemo{}
				}
				c.jobRows[j.ID] = jobRowsMemo{key: key, rows: rows}
			}
			emit(ref, rows, true)
			if c.open[ref] {
				if len(processes) > 0 {
					emit(ref, []string{"      " + dim("Processes · busiest first")}, false)
					emit(ref, processes, false)
				}
				details := "▸ command details · d to show"
				if c.open["job-details:"+j.ID] {
					details = "▾ command details · d to hide"
				}
				if c.sess.JobCommand(j) != "" || len(processes) > 0 {
					emit(ref, []string{"      " + paint(cSub, details)}, false)
				}
				for _, l := range body {
					lines = append(lines, convo.Line{Text: edge(ref, l.Text), Ref: ref})
				}
			}
		}
		lines = append(lines, convo.Line{Text: ""})
	}
	section("Running", run)
	section("Finished", done)
	return lines
}

// jobKey acts on a picked task, and ctrl+b on whatever the turn waits on.
func (m *Model) jobKey(c *hostConn, s string, empty bool) (tea.Cmd, bool) {
	if s == "ctrl+b" && c.client != nil {
		// Only while the turn waits on a command; otherwise ctrl+b is /btw's.
		if !slices.ContainsFunc(c.sess.RunningJobs(), func(j *convo.Job) bool { return !j.Background && c.jobKind(j) != "subagent" }) {
			return nil, false
		}
		if j := m.pickedJob(c); j != nil && !j.Background {
			return m.backgroundJob(c, j), true
		}
		return m.backgroundJob(c, nil), true
	}
	if cmd, used := m.shellKey(c, s, empty); used {
		return cmd, true
	}
	j := m.pickedJob(c)
	if j == nil {
		return nil, false
	}
	if strings.HasPrefix(c.sel, "run:") && s != "b" {
		return nil, false // the subagent's own keys
	}
	switch {
	case s == "ctrl+x" || s == "x" && empty:
		return m.stopJob(c, j), true
	case s == "b" && empty:
		if j.Background {
			m.flash("that's already in the background", false)
			return nil, true
		}
		return m.backgroundJob(c, j), true
	case s == "d" && empty:
		if c.sess.JobCommand(j) == "" && m.jobPID(c, j) == 0 {
			m.flash("this task did not report a launch command or process details", false)
			return nil, true
		}
		ref := "job-details:" + j.ID
		c.open[ref] = !c.open[ref]
		c.open["job:"+j.ID] = true
		for i, v := range m.views(c) {
			if v == "background" {
				c.view = i
			}
		}
		c.sel, c.selMoved = "job:"+j.ID, true
		return nil, true
	case s == "space" || s == "right":
		ref := "job:" + j.ID
		if m.viewName(c) != "background" {
			// From the dock: the background view, on it, opened.
			for i, v := range m.views(c) {
				if v == "background" {
					c.view = i
				}
			}
			c.open[ref], c.sel, c.selMoved = true, ref, true
			return nil, true
		}
		c.open[ref] = s == "right" || !c.open[ref]
		return nil, true
	}
	return nil, false
}

// pickedJob is the task picked, in the dock or the background view; a
// subagent picked in the dock counts as its task.
func (m *Model) pickedJob(c *hostConn) *convo.Job {
	if id, ok := strings.CutPrefix(c.sel, "job:"); ok {
		return c.sess.Job(id)
	}
	if id, ok := strings.CutPrefix(c.sel, "run:"); ok {
		for _, sa := range c.subs {
			if sa.ID != id {
				continue
			}
			for _, j := range c.sess.RunningJobs() {
				if j.ToolUseID != "" && j.ToolUseID == sa.ToolUseID || j.ID == sa.ID {
					return j
				}
			}
		}
	}
	return nil
}

func (m *Model) stopJob(c *hostConn, j *convo.Job) tea.Cmd {
	switch {
	case !j.Running():
		m.flash("that has already finished", false)
		return nil
	case c.client == nil:
		m.flash("rush can stop a task only in a session it runs; this one is Claude Code's", true)
		return nil
	}
	what := c.sess.JobKind(j)
	if j.Background {
		m.flash("stopping the "+what+" · the rest carries on", false)
	} else {
		m.flash("stopping the "+what+" · Claude hears it was stopped and carries on", false)
	}
	cl, id := c.client, j.ID
	return hostCmd(func() error { return cl.StopTask(id) })
}

// backgroundJob sends a task the turn waits on into the background (nil:
// every one), so the turn carries on and Claude hears when it's done.
func (m *Model) backgroundJob(c *hostConn, j *convo.Job) tea.Cmd {
	if c.client == nil {
		return nil
	}
	if c.sess.Info.Proto < 3 {
		m.flash("this session's host is older than this rush · /restart it to move tasks to the background", true)
		return nil
	}
	id := ""
	if j != nil {
		id = j.ToolUseID
		m.flash("moved the "+c.sess.JobKind(j)+" to the background · the turn carries on", false)
	} else {
		m.flash("moved what the turn was waiting on to the background", false)
	}
	cl := c.client
	return hostCmd(func() error { return cl.Background(id) })
}

// jobTail is a task's last n lines of output (at most jobTailMost), if
// it writes any where rush can find it. The background view draws every
// task's tail each frame, so each file is read once and then only looked
// at again, at most every tailEvery, while its task runs: a stat, and a
// read only when it has grown. Both happen in the background; a frame
// draws what was last read.
func (m *Model) jobTail(c *hostConn, j *convo.Job, n int) []string {
	lines, _ := m.jobTailFrom(c, j, n)
	return lines
}

// jobTailFrom is jobTail and the file it's from: Claude Code's own, or,
// when that has nothing, a file the command sends its output to.
func (m *Model) jobTailFrom(c *hostConn, j *convo.Job, n int) ([]string, string) {
	p := c.jobOutput(j)
	if lines := c.tailOf(p, j.Running(), n); len(lines) > 0 || c.sess.JobKind(j) != "shell" {
		return lines, p
	}
	// Else the file it writes that changed last; while it runs, only one
	// still changing: a file written at its start and left (what it
	// fetched to read) isn't its output, and would read as done.
	var best []string
	from, at := "", time.Time{}
	for _, w := range c.jobWrites(j) {
		lines := c.tailOf(w, j.Running(), n)
		e := c.tails[w]
		if len(lines) == 0 || e == nil || j.Running() && time.Since(e.mod) > liveFor || !e.mod.After(at) && from != "" {
			continue
		}
		best, from, at = lines, w, e.mod
	}
	return best, from
}

// writesFor is how long after a shell step ends its opened row still
// shows what the files its command wrote hold.
const writesFor = 5 * time.Minute

// withWrites puts under each opened shell step in the conversation what
// the files its command writes to hold, as the background view does: its
// log, what it redirected, what it tees. Only for one running, in the
// background or lately ended, so a long session doesn't read them all.
// body is left as it is; the rows go in a copy.
func (m *Model) withWrites(c *hostConn, body []convo.Line, w int) []convo.Line {
	return m.withWritesFrom(c, body, w, 0)
}

// withWritesFrom is withWrites reading body from row from on: the shell
// steps above it finished long ago.
func (m *Model) withWritesFrom(c *hostConn, body []convo.Line, w, from int) []convo.Line {
	now := time.Now()
	var out []convo.Line
	for i := max(0, from); i < len(body); i++ {
		if out != nil {
			out = append(out, body[i])
		}
		ref := body[i].Ref
		_, id, ok := strings.Cut(ref, ":s:")
		if !ok {
			continue // nothing's, or not a step
		}
		// The cheap tests first, every frame: finding the step's block below
		// strips every row of its output.
		st := c.sess.Step(id)
		if st == nil || st.Tool != "Bash" {
			continue
		}
		// Running, it shows what it writes whether opened or not.
		running := st.Status == convo.Running || c.sess.JobRunning(id)
		if !running && (st.End.IsZero() || now.Sub(st.End) > writesFor) {
			continue
		}
		// A step's body rows carry no ref but sit deeper than its row: its
		// block runs on over them, bar the blank rows after it.
		end, last, depth := i, true, -1
		for j := i + 1; j < len(body); j++ {
			if r := body[j].Ref; r != "" {
				last = stepOfRow(r) != ref
				break
			}
			rail := railOf(body[j].Text)
			if len(rail) == len(ansi.Strip(body[j].Text)) {
				continue // blank
			}
			if depth < 0 {
				depth = cellw.String(railOf(body[i].Text))
			}
			if cellw.String(rail) <= depth {
				break // not its: the turn's text after it
			}
			end = j
		}
		if !last {
			continue // not its last row
		}
		// isOpen may draw the step's turn: last.
		if !running && !m.isOpen(c, ref) {
			continue
		}
		rows := c.writeRows(id, st, running, now, m.followedBy(c, id))
		if len(rows) == 0 {
			continue
		}
		if out == nil {
			out = append(make([]convo.Line, 0, len(body)+len(rows)), body[:i+1]...)
		}
		out = append(out, body[i+1:end+1]...)
		pad := faint(railOf(body[i].Text))
		for _, r := range rows {
			out = append(out, convo.Line{Text: cellw.Truncate(pad+r, w, "…"), Ref: ref})
		}
		i = end
	}
	if out == nil {
		return body
	}
	return out
}

// stepOfRow is the step a row's ref belongs to: a script line's is its
// step's, so a script drawn in a step doesn't end the step's rows.
func stepOfRow(ref string) string {
	if step, _, ok := convo.ScriptLine(ref); ok {
		return step
	}
	return ref
}

// followedBy are the files a tail -f under step id's running task
// follows, as the background view finds them.
func (m *Model) followedBy(c *hostConn, id string) []string {
	var out []string
	for _, j := range c.sess.RunningJobs() {
		if j.ToolUseID != id {
			continue
		}
		if pid := m.jobPID(c, j); pid != 0 {
			for _, n := range m.snap.Table.Tree(pid) {
				out = append(out, tailFollows(m.shortCmd(n.PID, n.Comm), c.sess.Info.Cwd)...)
			}
		}
	}
	return out
}

// writeRows are the files step id's command writes to, and those what it
// runs follows, each named with its last lines, as the background view
// shows them.
func (c *hostConn) writeRows(id string, st *convo.Step, running bool, now time.Time, followed []string) []string {
	files, ok := c.writes[id]
	if !ok {
		cmd := strings.TrimSpace(st.Call().Input.Command)
		files = c.sess.Writes(cmd)
		if cmd != "" { // its call may not be read yet
			if c.writes == nil {
				c.writes = map[string][]string{}
			}
			c.writes[id] = files
		}
	}
	var rows, seen []string
	for _, f := range append(slices.Clone(files), followed...) {
		if slices.Contains(seen, f) {
			continue
		}
		seen = append(seen, f)
		lines := c.tailOf(f, running, jobFileLines)
		if len(lines) == 0 {
			continue
		}
		rows = append(rows, "  "+faint("from "+c.shownPath(f))+c.tailWhen(f, now))
		rows = append(rows, c.fileRows(f, lines, 2, 1<<10)...) // cut to the pane where they're put
	}
	return rows
}

// railOf is the rail and indent a conversation row starts with, so rows
// put under it line up with it.
func railOf(row string) string {
	s := ansi.Strip(row)
	n := 0
	for _, r := range s {
		if r != ' ' && r != '▏' && r != '│' && r != '▍' {
			break
		}
		n += len(string(r))
	}
	return s[:n]
}

// liveFor is how recently a file a running task writes must have changed
// to stand for its output.
const liveFor = 30 * time.Second

// jobWrites is the files a task's command writes to, worked out once: a
// frame asks for every task's.
func (c *hostConn) jobWrites(j *convo.Job) []string {
	if w, ok := c.writes[j.ToolUseID]; ok {
		return w
	}
	w := c.sess.JobWrites(j)
	if c.sess.JobCommand(j) != "" { // its call may not be read yet
		if c.writes == nil {
			c.writes = map[string][]string{}
		}
		c.writes[j.ToolUseID] = w
	}
	return w
}

// jobRowsKey is what a finished task's rows are drawn from.
type jobRowsKey struct {
	w                                int
	pal, status, from, last          string
	errorText, summary, label, agent string
	steps, tokens                    int
	end                              time.Time
	tail                             int
}

// jobRowsMemo is a finished task's rows as last drawn, unpicked.
type jobRowsMemo struct {
	key  jobRowsKey
	rows []string
}

// tailOf is the last n lines of the file at p, read as jobTail says.
func (c *hostConn) tailOf(p string, running bool, n int) []string {
	if p == "" {
		return nil
	}
	if c.tails == nil {
		c.tails = map[string]*jobTailed{}
	}
	e := c.tails[p]
	if e == nil {
		e = &jobTailed{size: -2}
		c.tails[p] = e
	}
	e.poll()
	// A file read final by a task that had ended is read again for one
	// still running: tasks can share a file.
	if now := time.Now(); (!e.final || running) && now.Sub(e.at) >= tailEvery {
		e.at = now
		size, mod := e.size, e.mod
		if e.read.start(func() tailRead { return readTail(p, size, mod, running) }) {
			e.poll() // tests read at once
		}
	}
	return e.lines[max(0, len(e.lines)-n):]
}

// fileRows is what tailOf last found of a file a command writes: a log as
// it is, anything else as a document, its lines numbered as they are in it, coloured as its kind of
// file has them, indent cells in and w wide.
func (c *hostConn) fileRows(p string, lines []string, indent, w int) []string {
	if !convo.IsCode(p) {
		rows := make([]string, len(lines))
		for i, l := range lines {
			rows[i] = cellw.Truncate(blanks(indent)+paint(cFaint, "│ ")+paint(cText, l), w, "…")
		}
		return rows // a log, not a document
	}
	var nos []int
	if e := c.tails[p]; e != nil && len(e.nos) == len(e.lines) {
		nos = e.nos[len(e.nos)-len(lines):]
	}
	rows := convo.CodeRows(p, lines, nos, max(10, w-indent))
	for i, r := range rows {
		rows[i] = blanks(indent) + r
	}
	return rows
}

// tailRead is a task's output as a read in the background found it.
type tailRead struct {
	size  int64 // -1 when there was nothing there
	mod   time.Time
	lines []string
	nos   []int // each line's number in the file, when it was counted
	same  bool  // unchanged: nothing was read
	final bool  // read after its task ended: it won't change
}

// readTail looks at a task's output, reading it when it isn't the size
// and time it was.
func readTail(p string, size int64, mod time.Time, running bool) tailRead {
	st, err := os.Stat(p)
	switch {
	case err != nil:
		return tailRead{size: -1}
	case st.Size() == size && st.ModTime().Equal(mod):
		return tailRead{same: true, size: size, final: !running}
	}
	// Read once its task had ended, it won't change again.
	lines, nos := tailLines(p, jobTailMost)
	return tailRead{size: st.Size(), mod: st.ModTime(), lines: lines, nos: nos, final: !running}
}

// poll takes in a read that's done.
func (e *jobTailed) poll() {
	r, ok := e.read.take()
	if !ok {
		return
	}
	if !r.same {
		e.size, e.mod, e.lines, e.nos = r.size, r.mod, r.lines, r.nos
	}
	e.final = r.final && e.size >= 0
}

// shownPath is a path as a task's rows name it: from the agent's folder
// when it's in it.
func (c *hostConn) shownPath(p string) string {
	if cwd := c.sess.Info.Cwd; cwd != "" {
		if rel, ok := strings.CutPrefix(p, strings.TrimSuffix(cwd, "/")+"/"); ok {
			return rel
		}
	}
	return tildify(p)
}

// tailWhen is when the file at p last changed, as tailOf last found it:
// " · updated 12s ago", so output that's stopped coming reads as such.
func (c *hostConn) tailWhen(p string, now time.Time) string {
	e := c.tails[p]
	if e == nil || e.mod.IsZero() {
		return ""
	}
	return faint(" · updated " + age(now.Sub(e.mod)) + " ago")
}

// tailFollows are the files a tail -f or -F command follows, made absolute
// from dir; none for any other command.
func tailFollows(cmd, dir string) []string {
	w := strings.Fields(cmd)
	if len(w) < 2 || w[0] != "tail" {
		return nil
	}
	follow := false
	var files []string
	for i := 1; i < len(w); i++ {
		switch a := w[i]; {
		case a == "-n" || a == "-c":
			i++ // its count
		case strings.HasPrefix(a, "-"):
			follow = follow || strings.ContainsAny(a[1:], "fF") && !strings.HasPrefix(a, "--") || a == "--follow" || strings.HasPrefix(a, "--follow=")
		case filepath.IsAbs(a):
			files = append(files, filepath.Clean(a))
		case dir != "":
			files = append(files, filepath.Join(dir, a))
		}
	}
	if !follow {
		return nil
	}
	return files
}

// jobFileLines is how much of each further file an opened task writes is
// shown.
const jobFileLines = 10

// jobTailMost is the most of a task's output shown: an opened one's.
const jobTailMost = 30

// tailEvery is how often a running task's output is looked at again.
const tailEvery = 500 * time.Millisecond

// jobTailed is a task's output as last read.
type jobTailed struct {
	at    time.Time // when it was last looked at
	size  int64     // -1 when there was nothing there
	mod   time.Time
	lines []string
	nos   []int
	final bool // read after its task ended: it won't change
	read  offRead[tailRead]
}

// jobOutput is the file a task writes its output to: Claude Code says
// where once it's done; while it runs, it's in the session's tasks folder
// under its own temp folder, found once, by the task's id.
func (c *hostConn) jobOutput(j *convo.Job) string {
	if j.OutputFile != "" {
		return j.OutputFile
	}
	if c.client == nil || c.id == "" || c.sess.Info.SessionID == "" {
		return ""
	}
	if c.taskDirFor != c.sess.Info.SessionID {
		c.taskDir, c.taskDirAt, c.taskDirFor = "", time.Time{}, c.sess.Info.SessionID
	}
	if d, ok := taskDirs.take(c); ok && d.sid == c.taskDirFor {
		c.taskDir = d.dir
	}
	if c.taskDir == "" && time.Since(c.taskDirAt) >= 2*time.Second {
		c.taskDirAt = time.Now()
		sid := c.sess.Info.SessionID
		id := c.id
		taskDirs.start(c, func() taskDir {
			d := taskDir{sid: sid}
			if found, _ := filepath.Glob(filepath.Join(host.TempDir(id), "claude-*", "*", sid, "tasks")); len(found) > 0 {
				d.dir = found[0]
			}
			return d
		})
		if d, ok := taskDirs.take(c); ok && d.sid == c.taskDirFor { // tests look at once
			c.taskDir = d.dir
		}
	}
	if c.taskDir == "" {
		return ""
	}
	return filepath.Join(c.taskDir, j.ID+".output")
}

// taskDir is a session's tasks folder, looked for in the background.
type taskDir struct{ sid, dir string }

var taskDirs = offReads[*hostConn, taskDir]{}

// tailWidest is the most of a line of output kept, in bytes.
const tailWidest = 1024

// tailCounted is the most of a file before its tail that's read to number
// the tail's lines; past it they go unnumbered.
const tailCounted = 32 << 20

// tailLines is the last n non-blank lines of a file, from its last 16KB,
// and each one's number in the file (none for a file too big to count).
func tailLines(path string, n int) ([]string, []int) {
	f, err := os.Open(path)
	if err != nil {
		return nil, nil
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil || st.Size() == 0 {
		return nil, nil
	}
	const most = 16 << 10
	off := max(0, st.Size()-most)
	buf := make([]byte, st.Size()-off)
	if _, err := f.ReadAt(buf, off); err != nil && err != io.EOF {
		return nil, nil
	}
	first := -1 // the number of the buffer's first line
	if off <= tailCounted {
		first = 1 + newlines(io.NewSectionReader(f, 0, off))
	}
	var out []string
	var nos []int
	for k, l := range strings.Split(string(buf), "\n") {
		// A progress bar redraws itself with \r: its latest state counts.
		if i := strings.LastIndexByte(strings.TrimRight(l, "\r"), '\r'); i >= 0 {
			l = l[i+1:]
		}
		if l = strings.TrimRight(ansi.Strip(l), " \t\r"); l != "" {
			// No row is wider than this: the rest would only be cut off
			// again, every frame.
			if len(l) > tailWidest {
				l = strings.ToValidUTF8(l[:tailWidest], "")
			}
			if off > 0 && k == 0 {
				continue // cut mid-line
			}
			out = append(out, strings.ReplaceAll(l, "\t", "  "))
			nos = append(nos, first+k)
		}
	}
	if first < 0 {
		nos = nil
	} else {
		nos = nos[max(0, len(nos)-n):]
	}
	return out[max(0, len(out)-n):], nos
}

// newlines counts the line ends r reads.
func newlines(r io.Reader) int {
	buf := make([]byte, 64<<10)
	n := 0
	for {
		k, err := r.Read(buf)
		n += bytes.Count(buf[:k], []byte{'\n'})
		if err != nil {
			return n
		}
	}
}

// lastLines is the last n non-blank lines of s.
func lastLines(s string, n int) []string {
	var out []string
	for l := range strings.SplitSeq(ansi.Strip(s), "\n") {
		if l = strings.TrimRight(l, " \t\r"); l != "" {
			out = append(out, strings.ReplaceAll(l, "\t", "  "))
		}
	}
	return out[max(0, len(out)-n):]
}

// firstWord is a command's program.
func firstWord(cmd string) string {
	for _, f := range strings.Fields(cmd) {
		if !strings.Contains(f, "=") {
			return f
		}
	}
	return cmd
}

// subUsage is what subagent sa's processes hold now: the shells it runs,
// or all of a spawned agent's own session. A Claude subagent runs inside
// the main process, so what it thinks with isn't counted apart.
func (m *Model) subUsage(c *hostConn, sa convo.Subagent) (mem uint64, cpu float64, n int) {
	if m.snap == nil || m.snap.Table == nil {
		return 0, 0, 0
	}
	t := m.snap.Table
	if id, ok := strings.CutPrefix(sa.ID, spawnPrefix); ok {
		if r := c.spawns[id]; r != nil && r.hosted != "" {
			for _, a := range m.order {
				if a.ID == r.hosted && a.PID != 0 {
					return t.Sum(a.PID, nil)
				}
			}
		}
	}
	for _, j := range c.sess.RunningJobs() {
		if !c.madeBy(j, sa.ID) {
			continue
		}
		if pid := m.jobPID(c, j); pid != 0 {
			jm, jc, jn := t.Sum(pid, nil)
			mem, cpu, n = mem+jm, cpu+jc, n+jn
		}
	}
	return mem, cpu, n
}

// usageText is b of memory, cpu and n processes, coloured as the list has them.
func usageText(b uint64, cpu float64, n int) string {
	if n == 0 {
		return ""
	}
	return cpuColor(cpu, fmt.Sprintf("%.0f%% cpu", cpu)) + dim(" · ") + memColor(b, mem(b)) + dim(fmt.Sprintf(" · %d procs", n))
}
