package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// The Projects place and the Agents place's Wall page: what is happening,
// over time, read from the transcripts while one of them is open. Projects
// (projects.go) shows each repository whole, its agents' sessions each at
// the state workSession draws. Wall (wall.go) shows every agent at once.

// workSince is how far back these pages look.
const workSince = 24 * time.Hour

// agentsPages are the Agents place's own pages: the plain list, and every
// agent at once on the Wall.
var agentsPages = []string{"Agents", "Wall"}

const (
	agentsList = iota
	agentsWall
)

// setAgentsPage shows one of the Agents place's pages; the Wall has a mode
// of its own, so rendering and keys dispatch on it.
func (m *Model) setAgentsPage(p int) {
	m.work.page = (p + len(agentsPages)) % len(agentsPages)
	switch m.work.page {
	case agentsWall:
		m.mode = modeWall
	default:
		m.mode = modeList
	}
}

type workState struct {
	page int // which of agentsPages
	// projPos and projSel are the Projects page's row picked, and its id
	// so new rows above it keep it picked.
	projPos int
	projSel string
	// projIn is set once enter has gone into what the list picked, inPos
	// and inSel its row picked there.
	projIn bool
	inPos  int
	inSel  string
	// peeks are folders' tops as last looked at, sizes what each thing in
	// an opened temp folder measured, opened the sessions whose temp
	// folders Temporary shows thing by thing, and lists what's in those
	// folders (projects_paths.go).
	peeks  map[string]pathInfo
	sizes  map[string]int64
	opened map[string]bool
	lists  map[string]tempList

	tls     map[string]agent.Timeline    // by transcript path; the loader's alone
	views   map[string][]agent.Happening // by agent key
	loading bool
	loaded  time.Time
}

type workTimelinesMsg struct {
	tls   map[string]agent.Timeline
	views map[string][]agent.Happening
}

// workLoad reads what the open sessions' transcripts have gained.
func (m *Model) workLoad() tea.Cmd {
	w := &m.work
	if w.loading {
		return nil
	}
	w.loading = true
	tls := w.tls
	paths := map[string]string{}
	kinds := map[string]agent.Kind{}
	for _, a := range m.workAgents() {
		if a.TranscriptPath != "" {
			paths[a.Key], kinds[a.Key] = a.TranscriptPath, a.Acct.Kind
		}
	}
	return func() tea.Msg {
		if tls == nil {
			tls = map[string]agent.Timeline{}
		}
		since := time.Now().Add(-workSince)
		views := map[string][]agent.Happening{}
		want := make(map[string]bool, len(paths))
		for _, p := range paths {
			want[p] = true
		}
		for p := range tls {
			if !want[p] {
				delete(tls, p) // a session that's aged out of the overview
			}
		}
		for key, p := range paths {
			tl := tls[p]
			if tl == nil {
				tr, ok := agent.As[agent.Timeliner](kinds[key])
				if !ok {
					continue
				}
				tl = tr.NewTimeline()
				tls[p] = tl
			}
			tl.Update(p, since)
			views[key] = tl.Events(since)
		}
		return workTimelinesMsg{tls: tls, views: views}
	}
}

func (m *Model) onWorkTimelines(msg workTimelinesMsg) {
	w := &m.work
	w.loading, w.loaded = false, time.Now()
	w.tls, w.views = msg.tls, msg.views
}

// workTick reads again every few seconds while Projects is open.
func (m *Model) workTick() tea.Cmd {
	if m.mode != modeProjects || time.Since(m.work.loaded) < 4*time.Second {
		return nil
	}
	return m.workLoad()
}

// workAgents are the sessions worth reading: open, busy or at work within
// the window, and not put away.
func (m *Model) workAgents() []*fleet.Agent {
	now := m.snap.At
	var out []*fleet.Agent
	for _, a := range m.snap.Agents {
		if a.Done || a.Past {
			continue
		}
		if a.Live() || a.Busy() || a.PID != 0 || now.Sub(a.UpdatedAt) < workSince {
			out = append(out, a)
		}
	}
	return out
}

// workRow is one line of the Projects page. A project's head, an agent, a
// worktree, temp work or a System process can be picked; the rest is text.
type workRow struct {
	id    string
	line  string
	proj  *project
	a     *fleet.Agent
	wt    *fleet.Worktree
	proc  *procRow
	tmp   bool         // the /tmp row
	temp  *fleet.Agent // a finished agent's temp work
	pane  bool         // Temporary or System in the Projects list
	path  *pathRow     // a thing on disk x deletes on its own
	owner string       // the project an agent or worktree row is inside
}

func (r workRow) pickable() bool {
	return r.id != "" && (r.proj != nil || r.a != nil || r.wt != nil || r.proc != nil || r.tmp || r.temp != nil || r.pane || r.path != nil)
}

// workSession is a session in Projects: its state, then how far through
// its tasks it is and how full its context.
func (m *Model) workSession(a *fleet.Agent, w int, now time.Time) string {
	marker := dim("◦")
	state := dim("idle")
	switch {
	case a.Halted():
		marker, state = paint(cRed, "✗"), paint(cRed, "stopped · "+oneLine(a.HaltReason()))
	case a.NeedsYou() || a.Waiting():
		q := oneLine(a.Needs)
		if q == "" {
			q = oneLine(a.Detail)
		}
		marker, state = paint(cYellow, "?"), paint(cYellow, "asks: ")+paint(cText, q)
	case a.YourTurn(now):
		marker, state = paint(cGreen, "◆"), paint(cGreen, "your turn · ")+paint(cSub, m.workSaid(a))
	case a.Live():
		marker = paint(cOrange, convo.Spin(a.Kind, m.tick+len(a.ID)))
		state = m.workSaid(a)
		if p := m.previews[a.Key].p; p.Doing != "" {
			state = oneLine(p.Doing)
		}
		if state == "" {
			state = "working"
		}
		state = paint(cText, state)
	case a.Busy():
		marker, state = paint(cBlue, "◎"), paint(cBlue, lanesLine(a))
	}
	var tail []string
	planned, ticked := 0, 0
	for _, e := range m.work.views[a.Key] {
		if e.Run == "" && e.Kind == agent.EvPlan {
			planned += e.N
		}
		if e.Run == "" && e.Kind == agent.EvTick {
			ticked++
		}
	}
	if planned > 0 {
		tail = append(tail, paint(cGreen, fmt.Sprintf("✓ %d/%d", min(ticked, planned), planned)))
	}
	if a.Spend.Context > 0 {
		pc := int(ctxFill(a, a.Spend.Context, agent.ContextWindow(agent.Kind(a.Kind), a.Spend.Model)).Pct())
		c := dim
		if pc >= 80 {
			c = func(s string) string { return paint(cYellow, s) }
		}
		tail = append(tail, c(fmt.Sprintf("ctx %d%%", pc)))
	}
	tail = append(tail, faint(right(age(now.Sub(a.UpdatedAt)), 5)))
	t := strings.Join(tail, "  ")
	left := " " + marker + " " + paint(cText+bold, fit(oneLine(a.DisplayName), 26)) + " "
	return left + fit(state, w-cellw.String(left)-cellw.String(t)-2) + "  " + t
}

// workSaid is what a session last said: Claude Code's summary of it, or
// its last turn's words when that summary is only a synthetic reply.
func (m *Model) workSaid(a *fleet.Agent) string {
	d := oneLine(a.Detail)
	if d != "" && d != "No response requested." {
		return d
	}
	ev := m.work.views[a.Key]
	for i := len(ev) - 1; i >= 0; i-- {
		if ev[i].Kind == agent.EvTurn && ev[i].Run == "" {
			return oneLine(ev[i].Text)
		}
	}
	return ""
}

// pickRow finds the row picked, by its id at *sel, following it if rows
// moved, else the nearest row that can be picked to *pos.
func pickRow(rows []workRow, pos *int, sel *string) int {
	if *pos < len(rows) && rows[*pos].id == *sel && rows[*pos].pickable() {
		return *pos
	}
	for i, r := range rows {
		if r.id == *sel && r.pickable() {
			*pos = i
			return i
		}
	}
	for d := 0; d < len(rows); d++ {
		for _, i := range []int{*pos + d, *pos - d} {
			if i >= 0 && i < len(rows) && rows[i].pickable() {
				*pos, *sel = i, rows[i].id
				return i
			}
		}
	}
	return -1
}

func sign(d int) int {
	if d < 0 {
		return -1
	}
	return 1
}

// lanesLine is a finished turn still waiting on its background work.
func lanesLine(a *fleet.Agent) string {
	agents, other := 0, 0
	for _, b := range a.Background {
		if strings.HasPrefix(b, "agent\x00") {
			agents++
		} else {
			other++
		}
	}
	var parts []string
	if agents > 0 {
		parts = append(parts, fmt.Sprintf("%d subagent%s running", agents, plural(agents)))
	}
	if other > 0 {
		parts = append(parts, fmt.Sprintf("%d task%s running", other, plural(other)))
	}
	if len(parts) == 0 {
		return "background work running"
	}
	return strings.Join(parts, ", ")
}
