package ui

import (
	"cmp"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/fswait"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// Pane views, cycled with [ and ]: every session has a conversation and an
// overview; a running Claude Code session also has its live screen.
var paneViews = []string{"conversation", "overview", "changes"}

func (m *Model) views(c *hostConn) []string {
	v := append([]string{}, paneViews...)
	if len(c.sess.Tasks) > 0 {
		v = append(v, "tasks")
	}
	if len(c.subs) > 0 {
		v = append(v, "subagents")
	}
	if slices.ContainsFunc(c.sess.Jobs(), func(j *convo.Job) bool { return j.Background && c.jobKind(j) != "subagent" }) {
		v = append(v, "background")
	}
	if canScreen(c, "memory") {
		v = append(v, "memory")
	}
	if c.client == nil {
		if a := m.focused(); a != nil && liveCapable(a) {
			v = append(v, "screen")
		}
	}
	if len(c.subs) > 0 {
		// Last, and no tab: pulled up from the subagents panel or ctrl+] g.
		v = append(v, "stack")
	}
	return v
}

// refreshSubs reads what the pane's transcripts have gained: the
// session's own, the runs beside it and the one opened or picked, and the
// runs' rows' numbers while the subagents view is on screen (the running
// ones' while the conversation is). All of it is read off the UI's
// goroutine, by the command it returns, and taken in by onPane; one read
// is out at a time, and the tails it reads aren't read here meanwhile.
func (m *Model) refreshSubs() tea.Cmd {
	c := m.host
	if c == nil || c.paneReading {
		return nil
	}
	m.followSessionID(c)
	if c.path == "" {
		// No transcript to list runs beside: only the agents the shell ran.
		c.subs = c.withJobSubs(append(slices.DeleteFunc(c.subs, func(sa convo.Subagent) bool { return strings.HasPrefix(sa.ID, spawnPrefix) }), c.spawnSubs()...))
		c.runMemo.ok = false
	}
	var tails []*convo.Tail
	add := func(t *convo.Tail) {
		if t != nil && !slices.Contains(tails, t) {
			tails = append(tails, t)
		}
	}
	add(c.tail)
	// A run the shell started is read as it's drawn (refreshSpawns).
	if c.subTail != nil && c.spawnRunFor(c.subOpen) == nil {
		add(c.subTail)
	}
	if c.subPeek != nil && c.spawnRunFor(c.subPeekID) == nil {
		add(c.subPeek)
	}
	for _, sa := range m.followedSubs(c) {
		if t := c.subTails[sa.ID]; t != nil && c.spawnRunFor(sa.ID) == nil && t.Size() != sa.Size {
			add(t)
		}
	}
	hist := c.hist
	if hist != nil && time.Since(hist.at) < historyEvery {
		hist = nil
	}
	// The agents the session's shell ran, once found, are read as they grow.
	var spawnHists []spawnHist
	for _, r := range c.spawns {
		switch {
		case r.path == "":
		case r.tail != nil:
			add(r.tail)
		case r.hist != nil && (r.sess == nil || time.Since(r.hist.at) >= historyEvery):
			spawnHists = append(spawnHists, spawnHist{r: r, first: r.sess == nil})
		}
	}
	if c.path == "" && len(tails) == 0 && hist == nil && len(spawnHists) == 0 {
		return m.readUnread(c)
	}
	if c.subReader == nil {
		c.subReader = c.newRuns()
	}
	c.paneReading = true
	msg := paneMsg{owner: c, key: c.key, path: c.path, hist: hist}
	// Which checkout each run worked in, for those not known yet.
	var wtFor []convo.Subagent
	cwd := ""
	if c.sess != nil {
		cwd = firstNonEmpty(c.sess.Info.Cwd, c.sess.Cwd)
	}
	for _, sa := range c.subs {
		if _, ok := c.subWT[sa.ID]; !ok && sa.Path != "" {
			wtFor = append(wtFor, sa)
		}
	}
	reader, list, gone := c.subReader, &c.subList, m.sessionGone(c)
	if !c.listed || c.tail != nil && c.sess.Partial {
		reader = nil // the runs listed first, quickly; what they say with the whole transcript
	}
	o, closed := m.warmOpts(), &c.closed // a history read again is drawn here, as warmed does
	return func() tea.Msg {
		if msg.path != "" {
			msg.subs = list.List(msg.path)
			if len(msg.subs) > 0 && reader != nil {
				reader.SetGone(gone)
				reader.UpdateRuns(msg.path, subRuns(msg.subs))
				msg.runs = reader.Clone()
			}
		}
		if h := msg.hist; h != nil {
			h.at = time.Now()
			if h.stat() {
				if msg.histSess = h.read(closed); msg.histSess != nil {
					warm(msg.histSess, o) // else the frame that takes it draws it whole
				}
			}
		}
		msg.got = make([]fetched, len(tails))
		for i, t := range tails {
			f, err := t.Fetch()
			msg.got[i] = fetched{t: t, f: f, err: err}
		}
		paths := make([]string, len(wtFor))
		for i, sa := range wtFor {
			paths[i] = sa.Path
		}
		wts := fleet.SubWorktrees(paths, cwd)
		for _, sa := range wtFor {
			if wt, ok := wts[sa.Path]; ok {
				if msg.wt == nil {
					msg.wt = map[string]string{}
				}
				msg.wt[sa.ID] = wt
			}
		}
		for _, sh := range spawnHists {
			if h := sh.r.hist; sh.first || h.stat() {
				h.at = time.Now()
				if sh.sess = h.read(closed); sh.sess != nil {
					msg.spawnHists = append(msg.spawnHists, sh)
				}
			}
		}
		return msg
	}
}

// paneMsg brings what refreshSubs read in the background.
type paneMsg struct {
	owner    *hostConn
	key      string
	path     string
	subs     []convo.Subagent
	runs     agent.SubagentRuns
	hist     *history
	histSess *convo.Session // hist read again, when it had grown
	got      []fetched
	// spawnHists are another agent's sessions the shell ran, read again.
	spawnHists []spawnHist
	wt         map[string]string // runs' checkouts: c.subWT's
}

type spawnHist struct {
	r     *spawnRun
	first bool // never read yet
	sess  *convo.Session
}

type fetched struct {
	t   *convo.Tail
	f   convo.Fresh
	err error
}

// onPane takes in what refreshSubs read.
func (m *Model) onPane(msg paneMsg) tea.Cmd {
	c := m.host
	if c == nil || c.key != msg.key || msg.owner != nil && msg.owner != c {
		return nil
	}
	c.paneReading = false
	c.runMemo.ok = false // the runs, and what they say, as read now
	for id, wt := range msg.wt {
		if c.subWT == nil {
			c.subWT = map[string]string{}
		}
		c.subWT[id] = wt
	}
	grew := map[*convo.Tail]bool{}
	for _, g := range msg.got {
		if g.err == nil && g.t.Take(g.f) {
			grew[g.t] = true
		}
	}
	m.takeSpawns(c, grew, msg.spawnHists)
	if msg.path != "" && msg.path == c.path && !c.listed {
		c.listed, c.paneKick = true, len(msg.subs) > 0
	}
	if msg.histSess != nil && c.hist == msg.hist {
		if c.sleeping {
			msg.histSess.Info = c.sess.Info
		}
		if c.sess.Partial {
			m.takeWhole(c, msg.histSess)
		} else {
			c.sess = msg.histSess
		}
	}
	if msg.path != "" && msg.path == c.path {
		c.subs = c.withJobSubs(append(msg.subs, c.spawnSubs()...))
		if len(msg.subs) > 0 {
			c.subRuns = msg.runs
		}
	}
	m.followTail()
	m.readSub()
	return m.readUnread(c)
}

// followedSubs are the runs whose rows' numbers are followed: every run
// while the subagents view is on screen, the running ones while the
// conversation is.
func (m *Model) followedSubs(c *hostConn) []convo.Subagent {
	switch m.viewName(c) {
	case "subagents", "stack":
		return c.subs
	case "conversation":
		return c.runningSubs()
	}
	return nil
}

// readUnread reads, in the background, the numbers of followed runs never
// read yet.
func (m *Model) readUnread(c *hostConn) tea.Cmd {
	follow := m.followedSubs(c)
	if len(follow) == 0 {
		return nil
	}
	if c.subTails == nil {
		c.subTails = map[string]*convo.Tail{}
	}
	var unread []convo.Subagent
	for _, sa := range follow {
		if r := c.spawnRunFor(sa.ID); r != nil {
			c.subTails[sa.ID] = r.view() // read as it's drawn: refreshSpawns
			continue
		}
		if c.subTails[sa.ID] == nil {
			unread = append(unread, sa)
		}
	}
	if len(unread) == 0 || c.subReading {
		return nil
	}
	c.subReading = true
	key := c.key
	return func() tea.Msg {
		// Hundreds of runs took seconds: a few at once, the newest first,
		// brought a batch a frame or so; onSubStats asks for the rest.
		tails := make([]*convo.Tail, len(unread))
		var wg sync.WaitGroup
		sem := make(chan struct{}, max(2, runtime.NumCPU()/2))
		until := time.Now().Add(subBatch)
		for i := len(unread) - 1; i >= 0 && time.Now().Before(until); i-- {
			sem <- struct{}{}
			wg.Go(func() {
				defer func() { <-sem }()
				t := convo.SubagentStats(unread[i].Path)
				_, _ = t.Read()
				tails[i] = t
			})
		}
		wg.Wait()
		read := make(map[string]*convo.Tail, len(unread))
		for i, sa := range unread {
			if tails[i] != nil {
				read[sa.ID] = tails[i]
			}
		}
		return subStatsMsg{key: key, tails: read}
	}
}

// followSessionID keeps a hosted session's transcript path on the
// conversation Claude Code is writing now. The id it was opened on can
// change under it: a fork gets its own once Claude Code starts, and a
// rewind or /clear starts another; the old path would never see the new
// conversation's subagents. Entering a worktree moves the file itself:
// the fleet finds it again, off the UI, and the pane follows.
func (m *Model) followSessionID(c *hostConn) {
	i := c.sess.Info
	if c.client == nil || i.SessionID == "" || i.Cwd == "" || !agent.ReadsAsClaude(sessionAgent(c)) {
		return
	}
	a := m.agentByKey(c.key)
	if a == nil {
		return
	}
	if p := a.TranscriptPath; filepath.Base(p) == i.SessionID+".jsonl" {
		c.path = p
		return
	}
	if !strings.HasSuffix(c.path, string(filepath.Separator)+i.SessionID+".jsonl") {
		// A new id the fleet hasn't read yet.
		c.path = agent.TranscriptPath(sessionAgent(c), a.Acct, i.Cwd, i.SessionID)
	}
}

// sessionGone is whether the session's Claude Code is known to have
// exited: a hosted one's host says it isn't running and isn't working; a
// terminal one's process is gone, or it's a conversation nothing has open.
// A background job's process isn't always known, so it never is.
func (m *Model) sessionGone(c *hostConn) bool {
	if c.client != nil {
		i := c.sess.Info
		return i.Proto >= 3 && i.ClaudePID == 0 && i.State != "working"
	}
	a := m.agentByKey(c.key)
	return a != nil && (a.Past || a.Interactive && a.PID == 0)
}

// subStatsMsg brings subagent runs' numbers read in the background.
type subStatsMsg struct {
	key   string
	tails map[string]*convo.Tail
}

func (m *Model) onSubStats(msg subStatsMsg) tea.Cmd {
	c := m.host
	if c == nil || c.key != msg.key {
		return nil
	}
	c.subReading = false
	for id, t := range msg.tails {
		if c.subTails[id] == nil {
			c.subTails[id] = t
		}
	}
	return m.readUnread(c) // the next batch
}

// subBatch is about how long each batch of runs' numbers reads for.
const subBatch = 50 * time.Millisecond

// subDetail is the whole conversation of a run, for showing it beside the
// list: the opened run's, or one read now and kept while it's the one picked.
func (c *hostConn) subDetail(id string) *convo.Tail {
	if c.subTail != nil && c.subOpen == id {
		return c.subTail
	}
	if c.subPeek != nil && c.subPeekID == id {
		return c.subPeek
	}
	if r := c.spawnRunFor(id); r != nil {
		c.subPeek, c.subPeekID = r.view(), id
		return c.subPeek
	}
	for _, sa := range c.subs {
		if sa.ID == id && sa.Path != "" {
			// Read by refreshSubs, not here in the middle of a frame.
			c.subPeek, c.subPeekID, c.paneKick = convo.SubagentTail(sa.Path), id, true
			return c.subPeek
		}
	}
	return nil
}

// readSub closes the opened subagent's last turn once its step has
// finished; refreshSubs reads what it writes.
func (m *Model) readSub() {
	c := m.host
	if c == nil || c.subTail == nil {
		return
	}
	// One run in the background returned its call at once; it's done when
	// its task is, or each line it writes would be a turn of its own.
	id := c.subToolUse()
	if j := c.sess.SubagentJob(c.subOpen, id); j != nil && j.Running() {
		return
	}
	if st := c.sess.Step(id); st != nil && st.Status != convo.Running {
		if live := c.subTail.Sess.Live(); live != nil {
			c.subTail.Sess.Apply(event.TurnEnd{Reason: "done"}, time.Now())
		}
	}
}

// growMsg says a transcript the pane follows has grown.
type growMsg struct{ key string }

type watched struct {
	path string
	size int64
}

// syncWatch keeps a watch on the transcripts the pane follows: a Claude
// Code session's own, and the subagent opened. The kernel says when one is
// written, so what Claude Code writes shows at once, and a quiet one costs
// nothing.
func (m *Model) syncWatch() tea.Cmd {
	c := m.host
	if c == nil {
		return nil
	}
	var want []watched
	if c.tail != nil {
		want = append(want, watched{c.tail.Path, c.tail.Size()})
	}
	if c.subTail != nil && c.subTail.Path != "" {
		want = append(want, watched{c.subTail.Path, c.subTail.Size()})
	}
	if c.stopWatch != nil && slices.Equal(want, c.watching) {
		return nil
	}
	c.unwatch()
	if len(want) == 0 {
		return nil
	}
	stop, key := make(chan struct{}), c.key
	c.watching, c.stopWatch = want, stop
	files := make([]fswait.File, len(want))
	for i, w := range want {
		files[i] = fswait.File{Path: w.path, Size: w.size}
	}
	return func() tea.Msg {
		if fswait.Grown(stop, files) {
			return growMsg{key}
		}
		return nil
	}
}

func (c *hostConn) unwatch() {
	if c.stopWatch != nil {
		close(c.stopWatch)
	}
	c.stopWatch, c.watching = nil, nil
}

func (m *Model) onGrow(msg growMsg) tea.Cmd {
	if c := m.host; c != nil && c.key == msg.key {
		c.unwatch() // that watch is over; the next update starts another
		return m.refreshSubs()
	}
	return nil
}

// subState is how a subagent run stands: the status its task-finished
// notice gave (or failed), and whether it is still working.
func (c *hostConn) subState(sa convo.Subagent) (status string, live bool) {
	return c.subStateIn(sa, nil)
}

// subStateIn is subState with the session's SubagentJobs, for asking of
// many runs at once; nil looks each one up.
func (c *hostConn) subStateIn(sa convo.Subagent, jobs map[string]*convo.Job) (status string, live bool) {
	if strings.HasPrefix(sa.ID, spawnPrefix) {
		return c.spawnState(sa)
	}
	if strings.HasPrefix(sa.ID, jobSubPrefix) {
		return c.jobSubState(sa)
	}
	// When its transcript last grew: known without reading it.
	var last time.Time
	if sa.Mod > 0 {
		last = time.Unix(0, sa.Mod)
	}
	status = c.sess.TaskStatus[sa.ID]
	st := c.sess.Step(sa.ToolUseID)
	if st != nil && st.Status == convo.Failed && status == "" {
		status = "failed"
	}
	// Claude Code says, in a session rush runs: its task runs until it
	// hears otherwise, however long the run goes quiet (a long command, a
	// long think), and one launched in the background has no step still
	// running to say so.
	var j *convo.Job
	if jobs == nil {
		j = c.sess.SubagentJob(sa.ID, sa.ToolUseID)
	} else if j = jobs[sa.ID]; j == nil && sa.ToolUseID != "" && len(jobs) > 0 {
		j = jobs["call:"+sa.ToolUseID]
	}
	if j != nil {
		if !j.Running() && status == "" && j.Status != "ended" {
			status = j.Status
		}
		return status, j.Running()
	}
	// Otherwise the transcripts say: its call without a result, or a
	// background launch with no word yet that it finished, is working
	// however quiet it is; with no word of it, it's working while it writes.
	runs := c.runs()
	if runs == nil {
		return status, false
	}
	if rs, _, _ := runs.State(sa.ID, sa.ToolUseID); status != "" && rs != agent.RunRunning {
		return status, false // Claude Code said how it ended, and nothing woke it since
	}
	live, ended := runs.Going(sa.ID, sa.ToolUseID, last, time.Now())
	if st != nil && st.Status == convo.Running && !runs.Gone() {
		live = true
	}
	if live {
		return "", true
	}
	return firstNonEmpty(status, ended), false
}

// newRuns is a new follower of the session's subagent runs: its agent's,
// else the native agent's, which follows those its shell ran; nil when
// neither can.
func (c *hostConn) newRuns() agent.SubagentRuns {
	if r, ok := agent.FollowRuns(sessionAgent(c)); ok {
		return r
	}
	if n, ok := agent.NativeAgent(); ok {
		if f, ok := n.(agent.RunFollower); ok {
			return f.SubagentRuns()
		}
	}
	return nil
}

// runs is subRuns, a follower that has read nothing until the pane has.
func (c *hostConn) runs() agent.SubagentRuns {
	if c.subRuns == nil {
		c.subRuns = c.newRuns()
	}
	return c.subRuns
}

// runningSubs are the subagent runs still working, the latest started
// first: one writing doesn't move it, so the dock's rows stay put.
// dockSubs are the running subagents the dock shows: those in the
// background. One the turn waits on shows in the conversation, under its
// call.
func (c *hostConn) dockSubs() []convo.Subagent {
	var out []convo.Subagent
	for _, sa := range c.runningSubs() {
		if st := c.sess.Step(sa.ToolUseID); st != nil && (st.Status == convo.Running || st.Status == convo.Waiting) {
			continue
		}
		out = append(out, sa)
	}
	return out
}

// sideRefs is where panelRefs keeps the rows drawn right of the
// conversation: past any row of the dock.
const sideRefs = 1 << 20

// sideWidth is how wide the subagents and background tasks are drawn right
// of the conversation's text in a pane w wide; 0 when there isn't room.
func sideWidth(w int) int {
	const text = 124 // convo's capRow: its rows never run further right
	if sw := w - text - 3; sw >= 44 {
		return min(sw, 80)
	}
	return 0
}

func (c *hostConn) runningSubs() []convo.Subagent {
	// Asked several times a frame, of hundreds of runs: kept for the second
	// unless the session changes, or onPane reads the runs (and drops it).
	k := runsKey{sess: c.sess, applied: c.sess.Applied(), sec: time.Now().Unix()}
	if m := &c.runMemo; m.ok && m.key == k {
		return m.out
	}
	var out []convo.Subagent
	jobs := c.sess.SubagentJobs()
	for i := len(c.subs) - 1; i >= 0; i-- {
		if _, live := c.subStateIn(c.subs[i], jobs); live {
			out = append(out, c.subs[i])
		}
	}
	slices.SortStableFunc(out, func(a, b convo.Subagent) int { return cmp.Compare(b.Born, a.Born) })
	out = out[:len(out):len(out)] // a caller's append copies
	c.runMemo.key, c.runMemo.out, c.runMemo.ok = k, out, true
	return out
}

// runsKey is what runningSubs's answer is kept for.
type runsKey struct {
	sess    *convo.Session
	applied int
	sec     int64
}

// cycleSub steps through the conversation and its subagent runs (running
// ones first, then the rest, newest first), each shown in place.
func (m *Model) cycleSub(c *hostConn, dir int) {
	order := c.runningSubs()
	seen := map[string]bool{}
	for _, sa := range order {
		seen[sa.ID] = true
	}
	for i := len(c.subs) - 1; i >= 0; i-- {
		if !seen[c.subs[i].ID] {
			order = append(order, c.subs[i])
		}
	}
	if len(order) == 0 {
		return
	}
	// Position 0 is the main conversation.
	cur := 0
	if m.viewName(c) == "subagents" && c.subOpen != "" {
		for i, sa := range order {
			if sa.ID == c.subOpen {
				cur = i + 1
			}
		}
	}
	next := (cur + dir + len(order) + 1) % (len(order) + 1)
	if next == 0 {
		c.subOpen, c.subTail = "", nil
		c.view, c.sel, c.scroll = 0, "", 0
		return
	}
	for i, v := range m.views(c) {
		if v == "subagents" {
			c.view = i
		}
	}
	back := c.subBack
	m.openSub(c, order[next-1].ID)
	c.subBack = back // a sibling is still watched from where this one was
}

func (c *hostConn) subToolUse() string {
	for _, sa := range c.subs {
		if sa.ID == c.subOpen {
			return sa.ToolUseID
		}
	}
	return ""
}

// openSub drills into one subagent's own conversation.
func (m *Model) openSub(c *hostConn, id string) {
	if r := c.spawnRunFor(id); r != nil {
		c.subTail, c.subOpen, c.subSel, c.subBack = r.view(), id, "", false
		c.sel, c.scroll = "", 0
		return
	}
	if strings.HasPrefix(id, jobSubPrefix) {
		m.flash("this agent keeps that subagent's conversation to itself; its step is in the conversation", false)
		return
	}
	for _, sa := range c.subs {
		if sa.ID == id {
			// Read by refreshSubs, at once rather than on the next tick.
			c.subTail, c.subOpen, c.subSel, c.subBack = convo.SubagentTail(sa.Path), id, "", false
			c.sel, c.scroll, c.paneKick = "", 0, true
			return
		}
	}
}

// watchingSub is whether the pane shows one subagent's own conversation.
func (m *Model) watchingSub(c *hostConn) bool {
	return c.subOpen != "" && c.subTail != nil && m.viewName(c) == "subagents"
}

// closeSub leaves an opened subagent for wherever it was opened from.
func (m *Model) closeSub(c *hostConn) {
	if c.subBack {
		// Watched from the dock: back to the conversation, on its row.
		c.view, c.sel = 0, "run:"+c.subOpen
	} else {
		c.sel = ""
	}
	c.subOpen, c.subTail, c.subBack = "", nil, false
}

// subBanner is the strip pinned under the header while you watch a
// subagent: a breadcrumb back to the main session, then its siblings as
// chips (icon, name, status), the one watched lit. The body below is the
// ordinary conversation view.
func (m *Model) subBanner(c *hostConn, w int) string {
	c.stripHits = c.stripHits[:0]
	if !m.watchingSub(c) {
		return ""
	}
	t := c.subTail.Sess
	jobs := c.sess.SubagentJobs()
	var cur convo.Subagent
	for _, x := range c.subs {
		if x.ID == c.subOpen {
			cur = x
		}
	}
	status, live := c.subStateIn(cur, jobs)
	now := time.Now()
	state := paint(cGreen, "✓ done")
	switch {
	case live:
		state = paint(cOrange, spinner[m.tick%len(spinner)]+" running")
	case status == "stopped":
		state = dim("⏹ " + status)
	case status == "failed":
		state = paint(cRed, "✗ failed")
	case status != "":
		state = dim(status)
	}
	end := t.Last
	if live {
		end = now
	}
	if !t.First.IsZero() && !end.IsZero() {
		state += dim(" " + dur(end.Sub(t.First).Round(time.Second)))
	}
	state += dim(fmt.Sprintf(" · %d steps", t.Totals(now).ToolCalls))
	back := "main session"
	if !c.subBack {
		back = "subagents"
	}
	crumb := paint(cBlue, "▍") + paint(cText, back) + dim(" › ") + paint(cBright+bold, cur.Type)
	if on := c.subRunsOn(cur, t); on != "" {
		crumb += dim(" · " + on)
	}
	crumb += "  " + state
	hint := keysFit(w, "shift+↑↓", "siblings", "esc", "up")
	room := w - cellw.String(crumb) - 2
	if room < 0 {
		return onBg(bgSub, cellw.Truncate(crumb, w, "…"), w)
	}
	// Siblings in the order they started, as many as fit around the one
	// watched; each a click target.
	type chipT struct {
		text, id string
		w        int
	}
	var all []chipT
	at := 0
	for _, sa := range c.subs {
		st, lv := c.subStateIn(sa, jobs)
		dot := paint(cGreen, "✓")
		switch {
		case lv:
			dot = paint(cOrange, "●")
		case st == "failed":
			dot = paint(cRed, "✗")
		case st == "stopped":
			dot = dim("⏹")
		}
		l := lookOf(c.subKind(sa.ID))
		chip := paint(l.colour(), l.glyph) + " " + cellw.Truncate(sa.Type, 14, "…") + " " + dot
		if sa.ID == c.subOpen {
			at = len(all)
			chip = paint(cOrange, "▸") + paint(cBright+bold, ansi.Strip(chip))
		} else {
			chip = dim("[") + chip + dim("]")
		}
		all = append(all, chipT{chip, sa.ID, cellw.String(chip) + 1})
	}
	avail := room - cellw.String(hint) - 1
	lo, hi, used := at, at, 0
	if len(all) > 0 {
		used = all[at].w
	}
	for grew := true; grew && len(all) > 0; {
		grew = false
		if hi+1 < len(all) && used+all[hi+1].w <= avail {
			hi++
			used += all[hi].w
			grew = true
		}
		if lo > 0 && used+all[lo-1].w <= avail {
			lo--
			used += all[lo].w
			grew = true
		}
	}
	x := cellw.String(crumb) + 2
	chips := ""
	for k := lo; len(all) > 0 && k <= hi; k++ {
		chips += all[k].text + " "
		c.stripHits = append(c.stripHits, stripHit{from: x, to: x + all[k].w - 1, id: all[k].id})
		x += all[k].w
		room -= all[k].w
	}
	left := crumb + "  " + chips
	right := ""
	if room >= cellw.String(hint)+1 {
		right = hint + " "
	}
	if c.subHover == "subback" {
		left = paint(cBright+bold, ansi.Strip(crumb)) + "  " + chips
	}
	return onBg(bgSub, spread(left, right, w), w)
}

// stripHit is a sibling's chip on the strip, in pane columns.
type stripHit struct {
	from, to int
	id       string
}

// stripClick opens the sibling whose chip is at (x, y), whatever the
// strip's other cells do (they go back).
func (m *Model) stripClick(c *hostConn, x, y int) bool {
	i := y - m.paneTop
	if !m.watchingSub(c) || i < 0 || i >= len(c.rowRefs) || c.rowRefs[i] != "subback" {
		return false
	}
	col := x - m.paneX()
	for _, h := range c.stripHits {
		if col >= h.from && col < h.to && h.id != c.subOpen {
			back := c.subBack
			m.openSub(c, h.id)
			c.subBack = back
			return true
		}
	}
	return false
}

// subPeekAfter is how long the pointer rests on a run before the wide
// subagents view shows its conversation beside the list.
const subPeekAfter = 300 * time.Millisecond

type subHoverMsg struct{}

// subHoverAt is what of the subagents view is under the pointer: a run's
// rows, or the banner that goes back from one being watched.
func (m *Model) subHoverAt(x, y int) string {
	c := m.host
	if c == nil || m.mode != modeList || m.dialog != nil || m.picker != nil || m.zen || m.embedded ||
		c.txt.drag || (m.listW > 0 && x <= m.listW+1) {
		return ""
	}
	if b := m.cardBtnAt(c, x, y); b != "" {
		return "btn:" + b // a card's button: the pointer says it's one
	}
	view := m.viewName(c)
	i := y - m.paneTop
	if view != "subagents" && view != "background" || i < 0 || i >= len(c.rowRefs) {
		return ""
	}
	r := c.rowRefs[i]
	if view == "background" {
		if strings.HasPrefix(r, "job:") {
			return r // a task's rows, to open with a click
		}
		return ""
	}
	// Wide, a run's conversation sits beside the list on its rows; only
	// the list itself is the run's.
	if strings.HasPrefix(r, "sub:") && c.subOpen == "" && c.paneW >= 150 && x-m.paneX() >= min(72, c.paneW*2/5) {
		return ""
	}
	if strings.HasPrefix(r, "sub:") || r == "subback" {
		return r
	}
	return ""
}

// subMouseMove follows the pointer over the subagents view. It says whether
// that changed what shows, and asks for a redraw once it has rested long
// enough for the run's conversation to show beside the list.
func (m *Model) subMouseMove(x, y int) (bool, tea.Cmd) {
	c := m.host
	if c == nil {
		return false, nil
	}
	r := m.subHoverAt(x, y)
	if r == c.subHover {
		return false, nil
	}
	c.subHover, c.subHoverAt = r, time.Now()
	if !strings.HasPrefix(r, "sub:") || c.subOpen != "" {
		return true, nil
	}
	return true, tea.Tick(subPeekAfter, func(time.Time) tea.Msg { return subHoverMsg{} })
}

// subagentLines is the subagents view: one row per run, or the opened
// run's own conversation under a breadcrumb.
func (m *Model) subagentLines(c *hostConn, o convo.Options) []convo.Line {
	w := o.Width
	if c.subOpen != "" && c.subTail != nil {
		// The banner pinned above the body says whose this is.
		so := o
		so.Selected = c.sel
		return append([]convo.Line{{Text: ""}}, c.subTail.Sess.Render(so)...)
	}
	// Keep the entire list available to navigation. The preview is composed
	// beside the visible slice later, using the actual body height.
	if wideSubagents(c, w) {
		o.Width = subListWidth(w)
	}
	return m.subagentList(c, o)
}

// noSession stands in for a run not read yet; it's only ever read.
var noSession = convo.New()

// subagentList is the runs, one two-line row each, newest first.
func (m *Model) subagentList(c *hostConn, o convo.Options) []convo.Line {
	w := o.Width
	running := 0
	type row struct {
		sa     convo.Subagent
		t      *convo.Session
		status string
		live   bool
	}
	var rows []row
	jobs := c.sess.SubagentJobs()
	for i := len(c.subs) - 1; i >= 0; i-- { // newest first
		sa := c.subs[i]
		r := row{sa: sa, t: noSession}
		if t := c.subTails[sa.ID]; t != nil {
			r.t = t.Sess
		}
		r.status, r.live = c.subStateIn(sa, jobs)
		if r.live {
			running++
		}
		rows = append(rows, r)
	}
	// Laid out as the background view is: a count, then what's running
	// and what's finished under their own headings.
	head := dim(fmt.Sprintf("%d runs", len(c.subs)))
	if running > 0 {
		head = paint(cOrange, fmt.Sprintf("%d running", running)) + dim(" · ") + head
	}
	lines := []convo.Line{{Text: fit("  "+head+dim(" · space opens one · → opens its conversation"), w)}, {Text: ""}}
	slices.SortStableFunc(rows, func(a, b row) int {
		switch {
		case a.live == b.live:
			return 0
		case a.live:
			return -1
		}
		return 1
	})
	for i, r := range rows {
		if i == 0 || r.live != rows[i-1].live {
			if i > 0 {
				lines = append(lines, convo.Line{Text: ""})
			}
			title := "Finished"
			if r.live {
				title = "Running"
			}
			lines = append(lines, convo.Line{Text: fit("  "+paint(cSub+bold, title), w)})
		}
		sa := r.sa
		ref := "sub:" + sa.ID
		key := subRowKey{sa: sa, t: r.t, status: r.status, where: c.subWhere(sa.ID), w: w, pal: convo.Palette()}
		if tl := c.subTails[sa.ID]; tl != nil {
			key.size = tl.Size()
		}
		// A finished run's row changes only as its transcript is read or
		// the pane changes: 239 of them cost milliseconds a frame.
		if memo, ok := c.subRows[sa.ID]; ok && !r.live && memo.key == key {
			lines = memo.appendTo(lines, c, o, ref, w)
			continue
		}
		now := time.Now()
		end := r.t.Last
		if r.live {
			end = now
		}
		took := ""
		if !r.t.First.IsZero() && !end.IsZero() {
			took = dur(end.Sub(r.t.First).Round(time.Second))
		}
		mark, state := paint(cGreen, "✓"), dim("done")
		switch {
		case r.live:
			mark, state = spinOf(c.subKind(sa.ID), m.tick+i), paint(cOrange, "running")
		case r.status == "stopped":
			mark, state = dim("⏹"), dim(r.status)
		case r.status == "failed":
			mark, state = paint(cRed, "✗"), paint(cRed, "failed")
		case r.status != "":
			state = dim(r.status)
		}
		where := key.where
		left := "  " + mark + " " + paint(cBlue, "⇉") + " " + paint(cText+bold, sa.Type) + where + "  " + paint(cSub, oneLine(sa.Description))
		right := state + "   " + dim(took) + "  "
		top := spread(left, right, w)
		tot := r.t.Totals(now)
		var facts []string
		if tot.ToolCalls > 0 { // one that only answered has none to count
			facts = append(facts, fmt.Sprintf("%d steps", tot.ToolCalls))
		}
		if tot.Requests > 0 {
			facts = append(facts, "in "+convo.Tokens(tot.In+tot.CacheRead+tot.CacheOut), "out "+convo.Tokens(tot.Out))
			if c := r.t.Cost(); c > 0 {
				facts = append(facts, money(c))
			}
		}
		if on := c.subRunsOn(sa, r.t); on != "" {
			facts = append(facts, on)
		}
		second := "      " + dim(strings.Join(facts, " · "))
		if r.live { // subUsage walks every task and run: only for one working
			if u := usageText(m.subUsage(c, sa)); u != "" {
				second += dim(" · ") + u
			}
		}
		if lw := r.t.LastWords(); lw != "" {
			second += dim("  ·  ") + faint(cellw.Truncate(lw, max(10, w-cellw.String(second)-8), "…"))
		}
		sr := subRow{key: key, top: top, second: second, topFit: fit(top, w), secondFit: fit(second, w)}
		if !r.live {
			if c.subRows == nil {
				c.subRows = map[string]subRow{}
			}
			c.subRows[sa.ID] = sr
		}
		lines = sr.appendTo(lines, c, o, ref, w)
	}
	return lines
}

// subRowKey is everything a finished run's row is drawn from.
type subRowKey struct {
	sa            convo.Subagent
	t             *convo.Session
	size          int64 // how far its tail has read
	status, where string
	w, pal        int
}

// subRow is a run's two lines as drawn, before they're picked or hovered.
type subRow struct {
	key                            subRowKey
	top, second, topFit, secondFit string
}

func (r subRow) appendTo(dst []convo.Line, c *hostConn, o convo.Options, ref string, w int) []convo.Line {
	switch ref {
	case o.Selected:
		bar := faint("▍")
		if o.Focused {
			bar = paint(cOrange, "▍")
		}
		return append(dst,
			convo.Line{Text: selBG + strings.ReplaceAll(bar+r.topFit[1:], reset, reset+selBG) + reset, Ref: ref},
			convo.Line{Text: selBG + strings.ReplaceAll(r.secondFit, reset, reset+selBG) + reset, Ref: ref})
	case c.subHover:
		return append(dst, convo.Line{Text: hoverLine(r.top, w), Ref: ref}, convo.Line{Text: hoverLine(r.second, w), Ref: ref})
	}
	return append(dst, convo.Line{Text: r.topFit, Ref: ref}, convo.Line{Text: r.secondFit, Ref: ref})
}

// subRunsOn is what a subagent run runs on, as far as its transcript has
// said: the provider when it isn't Claude Code (a spawned agent's own),
// the model it last asked, the effort its last turn ran at.
func (c *hostConn) subRunsOn(sa convo.Subagent, t *convo.Session) string {
	k := c.kind
	if id, ok := strings.CutPrefix(sa.ID, spawnPrefix); ok {
		if r := c.spawns[id]; r != nil {
			k = r.kind
		}
	}
	model := firstNonEmpty(t.Model, sa.Model)
	if n := len(t.Requests); n > 0 && t.Requests[n-1].Model != "" {
		model = t.Requests[n-1].Model
	}
	effort := t.Info.Effort
	for _, tn := range slices.Backward(t.Turns) {
		if tn.Effort != "" {
			effort = tn.Effort
			break
		}
	}
	// A spawned agent's type is its provider's name already.
	return strings.TrimPrefix(runsOn(k, model, effort), sa.Type+" · ")
}

// dockRunsShown is how many running subagents the dock shows at once; ↑↓
// scroll through the rest.
const dockRunsShown = 5

// keepRunPick keeps the pick on the dock's running subagents where it was:
// on the same run while it's working, and when it finishes, on whichever
// run now sits in its place.
func (c *hostConn) keepRunPick(run []convo.Subagent) {
	id, ok := strings.CutPrefix(c.sel, "run:")
	if !ok {
		return
	}
	if i := slices.IndexFunc(run, func(sa convo.Subagent) bool { return sa.ID == id }); i >= 0 {
		c.runPick = i
		return
	}
	c.sel = ""
	if len(run) > 0 {
		c.sel = "run:" + run[min(c.runPick, len(run)-1)].ID
	}
}

// runningPreview is what the conversation shows of the subagents still
// working: a heading, then a line each (a few runs at a time, keeping
// the picked one in sight): what it was asked, and what it's doing right
// now after what it just did.
func (m *Model) runningPreview(c *hostConn, run []convo.Subagent, w int) []string {
	what := "subagents"
	c.keepRunPick(run)
	picked := strings.HasPrefix(c.sel, "run:")
	start := 0
	if picked && c.runPick >= dockRunsShown {
		start = c.runPick - dockRunsShown + 1
	}
	shown := run[start:min(len(run), start+dockRunsShown)]
	hint := ""
	switch {
	case picked:
		hint = keys("space", "watch it")
	case strings.HasPrefix(c.sel, "job:"):
		if j := c.sess.Job(strings.TrimPrefix(c.sel, "job:")); j != nil {
			if sa, ok := c.jobOwner(j); ok && slices.ContainsFunc(run, func(r convo.Subagent) bool { return r.ID == sa.ID }) {
				hint = c.jobHint(j)
			}
		}
	case m.paneFocus && len(c.input) == 0 && len(m.queueOf(c).items) == 0:
		hint = keys("↑", "pick one to watch")
	}
	title := paint(cBlue+bold, what) + paint(cFaint, fmt.Sprintf(" ·%d", len(run)))
	if len(run) >= 2 {
		title += "  " + paint(cBlue, "⇡") + dim(" drag up to see them side by side")
	}
	out := []string{spread(" "+title, hint+"  ", w)}
	if c.panelRefs != nil && len(run) >= 2 {
		c.panelRefs[c.previewBase] = "subs:title" // pulled up, or wheeled up on, it opens the stack
	}
	if start > 0 {
		out = append(out, dim(fmt.Sprintf("  … %d newer", start)))
	}
	now := time.Now()
	for i, sa := range shown {
		var doing, trail string
		var facts []string
		if t := c.subTails[sa.ID]; t != nil {
			s := t.Sess
			if d, since := s.Doing(); d != "" {
				doing = paint(cText, d)
				// A history read back has no times: no start, no elapsed.
				if el := now.Sub(since); !since.IsZero() && el >= 5*time.Second {
					doing += dim(" " + dur(el.Round(time.Second)))
				}
			} else {
				doing = dim("thinking…")
			}
			for _, d := range s.Did() {
				trail += dim("  ‹ " + d)
			}
			if trail == "" {
				if lw := s.LastWords(); lw != "" {
					trail = dim("  ‹ " + lw)
				}
			}
			if n := s.Totals(now).ToolCalls; n > 0 {
				facts = append(facts, strconv.Itoa(n))
			}
			if !s.First.IsZero() {
				facts = append(facts, dur(now.Sub(s.First).Round(time.Second)))
			}
			if on := c.subRunsOn(sa, s); on != "" {
				facts = append(facts, on)
			}
		} else {
			doing = dim("starting…")
		}
		right := dim(strings.Join(facts, " · ")) + "  "
		where := c.subWhere(sa.ID)
		if q := m.localQ[subQKey(c.key, sa.ID)]; q != nil && len(q.items) > 0 {
			where += " " + paint(cQueue, fmt.Sprintf("✉%d", len(q.items)))
		}
		// One line a run: who, what it was asked, then what it's doing
		// after what it just did, the facts set right.
		desc := fmt.Sprintf("%-24s", cellw.Truncate(oneLine(sa.Description), 24, "…"))
		left := "  " + spinOf(c.subKind(sa.ID), m.tick+i) + " " + paint(cText+bold, fmt.Sprintf("%-8s", sa.Type)) + where +
			" " + paint(cSub, desc) + "  " + paint(cOrange, "›") + " " + doing + trail
		top := spread(cellw.Truncate(left, max(20, w-cellw.String(ansi.Strip(right))-2), "…"), right, w)
		if c.sel == "run:"+sa.ID {
			top = picked1(top, w, m.paneFocus)
		}
		if c.panelRefs != nil {
			c.panelRefs[c.previewBase+len(out)] = "run:" + sa.ID
		}
		out = append(out, top)
		// What it's running itself, hung under it.
		for k, j := range c.jobsOf(sa.ID) {
			if c.panelRefs != nil {
				c.panelRefs[c.previewBase+len(out)] = "job:" + j.ID
			}
			out = append(out, m.jobRow(c, j, i+k, w, "    ", false, now)...)
		}
	}
	if rest := len(run) - start - len(shown); rest > 0 {
		out = append(out, dim(fmt.Sprintf("  + %d older", rest)))
	}
	return out
}

// otherViews names the views [ ] reaches from this one, those only some
// sessions have first, since they say there is something to see.
func (m *Model) otherViews(c *hostConn) string {
	cur := m.viewName(c)
	all := m.views(c)
	var names []string
	for _, v := range append(all[len(paneViews):], paneViews...) {
		if v != cur {
			names = append(names, v)
		}
	}
	if len(names) > 3 {
		return strings.Join(names[:3], ", ") + "…"
	}
	return strings.Join(names, ", ")
}

func (m *Model) viewName(c *hostConn) string {
	v := m.views(c)
	return v[c.view%len(v)]
}

// tabStat is what a tab says beside its name, as the list's git bits look:
// the Overview that it has artifacts, Changes what this session adds and
// deletes, else everything uncommitted: one pair, the tab says the rest.
func tabStat(c *hostConn, v string) string {
	// Small: thousands as k, and a side that's nothing left out.
	num := func(n int) string {
		if n >= 1000 {
			return strconv.FormatFloat(float64(n)/1000, 'f', 1, 64) + "k"
		}
		return strconv.Itoa(n)
	}
	switch v {
	case "overview":
		if n := len(c.artifactsOf()); n > 0 {
			return paint(cBlue, fmt.Sprintf("◆%d", n))
		}
	case "changes":
		if c.sess.Partial {
			return dim("counting…")
		}
		a, d := 0, 0
		for _, fc := range c.sess.Changes() {
			a, d = a+fc.Add, d+fc.Del
		}
		if dir := firstNonEmpty(c.sess.Info.Cwd, c.sess.Cwd); a+d == 0 && dir != "" {
			a, d = convo.WorkingTree(dir).Stat()
		}
		var p []string
		if a > 0 {
			p = append(p, paint(cGreen, "+"+num(a)))
		}
		if d > 0 {
			p = append(p, paint(cRed, "−"+num(d)))
		}
		return strings.Join(p, " ")
	}
	return ""
}

// hostConn is the open connection to the selected rush-mode session: its
// host client, the conversation built from what the host sends, and how the
// pane is being looked at.
type hostConn struct {
	contextLabel [2]int
	paneTabs     []paneTab
	pointerHover paneHover
	// label is where the header's agent label is drawn: its column in the
	// pane and its width, 0 when it isn't.
	label [2]int

	// intercepting is a message plugins are looking at before it goes;
	// intercepted is it going, after they have.
	intercepting, intercepted bool

	key              string
	id               string
	kind             agent.Kind   // the agent it runs, as its row says
	client           *host.Client // a rush session's host; nil when read from a transcript
	sleeping, waking bool
	sleepDraft       *sleepDraft
	picked           map[string]string // what /model and /effort last set here, by command: the host doesn't say
	tail             *convo.Tail       // a Claude Code session's transcript, followed as it grows
	// hist is another agent's session read from its history, read again
	// as it grows: see followHistory.
	hist  *history
	sess  *convo.Session
	ready bool // the replay has been drawn at least once

	// clearedAt is when the box was last wiped: its hints say how to
	// bring that back for a while after.
	clearedAt time.Time

	view        int
	sel         string
	open        map[string]bool
	looks       map[string]string // the view each step's output was switched to
	verbose     bool
	depth       convo.Depth // ctrl+] 0…3: prose, runs folded, as usual; 3 is verbose
	historyMode convo.HistoryMode
	scroll      int // rows up from the bottom; 0 follows the latest output

	input     []rune
	back      int
	anchor    int       // selection start + 1; 0 when nothing is selected
	imgs      imageRefs // images in the box, each [Image #N] in its text
	box       box       // the message box as last drawn, and where
	boxIdx    int
	boxY      int
	taskW     int
	tasksShut bool                                        // the task rows folded to one
	taskRows  int                                         // the task rows drawn just above the box
	editQ     int                                         // queued message being edited in the box, +1; 0 when none
	editWas   string                                      // its text before editing
	editHeld  bool                                        // editing held the queue, to let go once it's saved
	slashSel  int                                         // the slash-command picker's selection
	sendRaw   bool                                        // send a / command rush doesn't know as it is
	asks      map[string]func(*Model, host.Reply) tea.Cmd // control requests out (askClaude)
	askN      int
	pastes    pastes // long pastes shown as chips
	undo      undoStack
	arts      []*artifact
	marks     map[string]bool // files marked reviewed in the changes view
	full      string          // the changed file shown in full over the changes view
	bodyBuf   []convo.Line    // the conversation\'s lines, reused frame to frame
	artsKey   string
	mem       []memFile // the memory view's files, and when they were read
	memAt     time.Time
	memInfo   *agent.Memory // the memory view's checks, read with its files
	memTop    int           // the first of them in view
	memEd     *docEditor    // the picked file, open in the editor below them
	memEdit   bool          // the editor has the keys
	// scrollOnly is that the message since the last frame only scrolled:
	// the body drawn then (shown, of shownView) is drawn again as it was.
	scrollOnly bool
	shownView  string
	// shown is rows [shownBase, shownBase+len(shown)) of shownTotal: in a
	// conversation, only the turns around what's on screen are drawn, with
	// shownIns rows put in under shell steps.
	shownBase, shownTotal, shownIns int
	endShown                        bool            // the conversation's last screenful is in view: fast frames are for it
	local                           []event.Command // custom commands and skills on disk
	skills                          map[string]bool
	// cardFocus is set when ↑ has moved the keys from the box onto a card
	// waiting for an answer; only then do plain letters and digits answer.
	cardFocus  bool
	limitUsage limitUsageState
	cardAgain  string // the card just answered from the card, to hand the keys on to the next
	signinSent string // the sign-in error a continue was sent for, so its card goes at once
	signinPick int    // the account picked on the signed-out card
	// The card as a modal (modal.go): the one last seen and when it came,
	// whether you were typing then, the one set aside for later, and when
	// you last pressed a key.
	cardShown, cardLater string
	memPick              int // the memory card's button enter presses
	cardAt, lastKeyAt    time.Time
	cardTyping           bool
	inModal              bool   // the card is being drawn in its modal
	verdictShown         string // the room whose verdict last came to this chat's dock
	// A memory card's buttons as last drawn, for clicks: from the card's
	// top, which is cardTop rows into the dock, which starts at dockY.
	btns                                 []cardBtn
	cardTop, dockY                       int
	panelClip                            [2]int
	panelFocus                           bool
	auxRows, transcriptRows, previewBase int
	panelRefs                            map[int]string
	// The subagents and background tasks drawn right of the conversation,
	// when there's room: their rows, width, and where they landed.
	side                []string
	sideW, sideX, sideY int
	// The queue card's rows as last drawn: which message is on each row
	// from its top, qTop rows into the dock; qHover is the one under the
	// pointer, plus one.
	qAt          map[int]int
	qTop, qHover int
	peekOf       string  // the approval and note peek was read for
	peek         memPeek // the memory card's note as it is on disk
	editAfter    string  // the step whose note to open in the editor once written

	// Answering Claude's questions, one at a time.
	qFor    string
	qIdx    int
	qCursor int                  // the option ↑↓ is on while the card has the keys
	qPicks  map[int]map[int]bool // ticked options, by question
	qAnswer map[string]string
	qTyped  map[int][]rune // typed in the box on each question and not yet sent
	qFull   bool           // the option under the cursor shown whole, its preview full size
	// The question card as last drawn, for clicks: its rows, qHitTop rows
	// into the card, and what a click on each presses ("@n" for option n).
	qRows     []string
	qHit      map[int]string
	qHitTop   int
	qHeld     *heldBox // what was in the box when a question came, back once it's answered
	stopArmed time.Time
	lastSend  time.Time
	sending   []sending          // sent, and not yet seen to arrive
	landed    map[string]landing // where each text sent was last seen to arrive
	coldOK    time.Time          // the cold cache you said to send to anyway
	flushed   time.Time          // when the last batch of host lines was taken in
	watching  []watched          // the transcripts being watched for growth
	drawn     convo.Options      // how the conversation was last drawn
	drewConvo bool
	seenRef   map[string]bool
	stopWatch chan struct{}
	// selMoved asks the next draw to scroll the selection into view;
	// rowRefs is what each drawn row of the pane belongs to, for clicks.
	selMoved bool
	pins     promptPins // the first and latest prompts, as pinned atop the conversation
	brief    string     // what the session's start added to its agent's prompt, read once for briefFor
	briefFor string
	rowRefs  []string
	// top is the row at the top of the window when scrolled up, so output
	// arriving below (a replay, an agent at work) doesn't carry you down.
	top struct {
		ref, view   string
		off, scroll int
	}
	// reveal is a row just opened, for the next draw to show what opened
	// under it; revealTotal the rows there were before.
	reveal      string
	revealTotal int
	// A report (the overview, changes, a list) opens at its top rather
	// than its end, and stays there until you scroll. pinFor is the view
	// it was opened for.
	pinTop   bool
	pinFor   string
	bodyRefs []string // every selectable row drawn of the current view, in order
	// Text dragged over with the mouse. rowBody is which row of shown each
	// drawn row of the pane is (-1 for the header, pill and dock), and
	// bodyTop and bodyRows where the body sits among them.
	txt      textSel
	shown    []convo.Line
	rowBody  []int
	bodyTop  int
	bodyRows int
	minimap  conversationMap
	paneW    int

	// Subagents: every run found beside the transcript, and the one opened.
	exchangeID string // stopped Rush session journal for full transcript reads
	path       string
	subs       []convo.Subagent
	subTail    *convo.Tail
	subTails   map[string]*convo.Tail // every run, followed for its row's numbers only
	subWT      map[string]string      // each run's checkout, when not the session's, once known
	stopNote   *stopNote              // a stop waiting on the message that goes with it
	subBack    bool                   // the open run was picked in the dock: ← goes back there
	subPeek    *convo.Tail            // the run picked in the list, in full, shown beside it
	subPeekID  string
	subPreview subPreview
	subReading bool // runs' numbers are being read in the background
	// paneReading: refreshSubs is reading the pane's transcripts in the
	// background, with subReader and subList, which only it touches.
	paneReading bool
	paneKick    bool // a tail was opened: read it now, not on the next tick
	subReader   agent.SubagentRuns
	subOpen     string
	stackGrow   int                // frames the stack view has grown for, to stackFrames
	stackFrom   int                // rows high the stack starts growing from
	pullY       int                // the subagents panel title's row + 1, pressed to pull up
	subList     convo.Subagents    // finds the runs, reading each one's meta once
	subRuns     agent.SubagentRuns // which runs the transcripts say are still working: subReader's, as last read; see runs
	subSel      string             // selection inside the opened subagent
	subHover    string             // the run or task under the pointer, or "subback" for the banner
	stripHits   []stripHit         // the sibling chips on the subagent strip, as last drawn
	subRows     map[string]subRow  // each finished run's row as last drawn, by its id
	runMemo     struct {
		key runsKey
		out []convo.Subagent
		ok  bool
	}
	runPick    int                    // where the pick last was among the dock's running subagents
	taskDir    string                 // the session's tasks folder, once found
	taskDirAt  time.Time              // when it was last looked for
	taskDirFor string                 // the conversation it was found for
	tails      map[string]*jobTailed  // each task's output as last read, by file
	scripts    map[string]*scriptSeen // shell calls run as scripts, by tool call ID
	writes     map[string][]string    // the files each task's command writes, by its tool call
	jobRows    map[string]jobRowsMemo // a finished task's rows in the background view, by task
	subHoverAt time.Time
	// Agents the session's shell ran: those rush hosted by their rush
	// session, the rest by the step that ran each. spawnLooking is while
	// any are looked for, with hostList, which only that touches; hostedAt
	// is when those rush hosted were last listed, hostNone the ones no
	// step ran, and spawnsOn the conversation last given them.
	spawns       map[string]*spawnRun
	spawnLooking bool
	hostList     host.Lister
	hostedAt     time.Time
	hostNone     map[string]bool
	spawnsOn     *convo.Session

	// stale is whether the conversation drawn last ran out of time and
	// shows some of it as drawn before, at another width perhaps: relayout
	// draws again soon.
	stale bool

	listed bool        // its runs have been listed once, quickly: the next read is all of it
	closed atomic.Bool // let go: what still reads for it gives up

	// A hosted session's transcript before its replay (hostPre), and the
	// host's lines kept while the whole of it is read (keep), to apply after.
	pre   *hostPre
	since [][]byte
	keep  bool
}

type hostOpenMsg struct {
	key string
	c   *hostConn
	err error
}

type hostLinesMsg struct {
	owner  *hostConn
	key    string
	lines  [][]byte
	closed bool
}

var cBright string

// frame is how long a burst of output collects before the pane redraws;
// the replay on connecting gathers for a little longer so it draws whole.
const (
	frame  = 16 * time.Millisecond
	replay = 33 * time.Millisecond
)

func openHost(a *fleet.Agent) tea.Cmd {
	key, id, path, acct, kind, sessionID := a.Key, a.ID, firstNonEmpty(a.TranscriptPath, a.History), a.Acct, agent.Kind(a.Kind), a.SessionID
	return func() tea.Msg {
		info, infoErr := host.ReadInfo(id)
		if infoErr == nil && info.Sleeping {
			return openSavedHost(a)()
		}
		cl, err := host.Dial(id)
		if err != nil {
			return hostOpenMsg{key: key, err: err}
		}
		// The header from the host's info at once; the transcript up to
		// where its replay begins, then the replay, follow (hostPre): a
		// session that took over an existing conversation shows it, and a
		// long one whose replay was trimmed shows all of it.
		cfg, cfgErr := host.ReadConfig(id)
		if infoErr == nil && info.SessionID != sessionID {
			path = "" // the fleet may still point at a branch left by rewind
		}
		pre, path := preFor(info, infoErr, cfg, cfgErr, path, acct)
		sess := pre.base()
		return hostOpenMsg{key: key, c: &hostConn{key: key, id: id, kind: kind, client: cl, sess: sess, open: map[string]bool{}, path: path, pre: pre}}
	}
}

// replayMost is the longest the pane waits, on connecting, for the host's
// replay to come in whole before it draws what it has.
const replayMost = 3 * time.Second

// takeReplay takes in what the host replays on connecting, here rather
// than on the UI's thread a batch a frame, up to the info the host sends
// once the replay's done: the pane then draws the whole conversation, at
// its end, rather than its start growing towards it. It says whether the
// replay came in whole before most passed; keep, when set, gets its lines.
func takeReplay(lines <-chan []byte, sess *convo.Session, most time.Duration, keep *[][]byte) bool {
	done, _ := takeReplayState(lines, sess, most, keep)
	return done
}

func takeReplayState(lines <-chan []byte, sess *convo.Session, most time.Duration, keep *[][]byte) (done, closed bool) {
	t := time.NewTimer(most)
	defer t.Stop()
	now := time.Now()
	for {
		var l []byte
		var ok bool
		select {
		case l, ok = <-lines:
		case <-t.C:
			return false, false
		}
		if !ok {
			return false, true
		}
		if keep != nil {
			*keep = append(*keep, l)
		}
		if applyHostLine(sess, l, &now) {
			return true, false
		}
	}
}

// applyHostLine applies one of a host's lines as a replay is, and says
// whether it was the info that ends one.
func applyHostLine(sess *convo.Session, l []byte, now *time.Time) bool {
	ev, err := host.Decode(l)
	if err != nil || ev == nil {
		return false
	}
	switch ev := ev.(type) {
	case host.Stamp:
		*now = ev.At
		return false
	case host.Reply:
		return false // nothing's been asked on this connection yet
	}
	sess.Apply(ev, *now)
	_, done := ev.(host.InfoEvent)
	return done
}

// next waits for output and hands it over at once when the pane last drew
// a while ago; in a burst it gathers lines until a frame has passed since
// the last batch, so a busy session costs one redraw per frame, not one per
// line, and a delta never waits more than a frame.
func (c *hostConn) next() tea.Cmd {
	lines, key, last := c.client.Lines, c.key, c.flushed
	return func() tea.Msg {
		first, ok := <-lines
		if !ok {
			return hostLinesMsg{owner: c, key: key, closed: true}
		}
		batch := [][]byte{first}
		wait := replay
		if !last.IsZero() {
			wait = frame - time.Since(last)
		}
		var due <-chan time.Time
		if wait > 0 {
			t := time.NewTimer(wait)
			defer t.Stop()
			due = t.C
		}
		for len(batch) < 4096 {
			var l []byte
			ok := true
			if due == nil { // only what is already here comes along
				select {
				case l, ok = <-lines:
				default:
					return hostLinesMsg{owner: c, key: key, lines: batch}
				}
			} else {
				select {
				case l, ok = <-lines:
				case <-due:
					return hostLinesMsg{owner: c, key: key, lines: batch}
				}
			}
			if !ok {
				return hostLinesMsg{owner: c, key: key, lines: batch, closed: true}
			}
			batch = append(batch, l)
		}
		return hostLinesMsg{owner: c, key: key, lines: batch}
	}
}

// syncHost keeps one connection open, to the rush-mode agent the pane is
// showing, and closes it when the pane moves on.
func (m *Model) syncHost() tea.Cmd {
	a := m.focused()
	_, paneW, _ := m.layout()
	showing := a != nil && paneW > 0 && m.mode == modeList
	hosted := showing && a.Rush && a.PID != 0
	if showing && isRoomRow(a) {
		return m.syncRoom(a) // a room, drawn as its agents' conversation
	}
	if showing && a.Rush && m.host != nil && m.host.key == a.Key && (m.host.sleeping || m.host.waking) {
		c := m.host
		if c.waking || !hosted || a.PID == c.sess.Info.HostPID || m.hostOpening == a.Key {
			return nil
		}
		// Another explicit sender woke a different process. Dial only: viewing
		// a session must never create a host.
		m.hostOpening = a.Key
		return warmed(openHost(a), m.warmOpts())
	}
	if showing && a.Rush && m.host != nil && m.host.key == a.Key && m.host.client != nil {
		return nil
	}
	// A stopped rush session of another agent is read from its history.
	fromFile := showing && !hosted && (a.TranscriptPath != "" || a.History != "" || a.Rush)
	if !hosted && !fromFile {
		m.gridPark()
		m.dropHost()
		if a == nil {
			m.paneFocus = false
		}
		return nil
	}
	if m.host != nil && m.host.key == a.Key && (m.host.client != nil) == hosted {
		return nil
	}
	if m.hostOpening == a.Key {
		return nil
	}
	if m.gridAdopt(a.Key) {
		return nil // a tile's session, kept open: in at once
	}
	m.gridPark()
	m.dropHost()
	m.hostOpening = a.Key
	o := convo.Options{Width: m.conversationWidth(paneW - 3), Open: map[string]bool{}, Focused: m.paneFocus, Wide: m.hostedAlone(), NoRule: m.besideList()}
	if fromFile {
		if a.Rush {
			return warmed(openSavedHost(a), o)
		}
		return warmed(openTail(a), o)
	}
	return warmed(openHost(a), o)
}

// warmRows is how much of a session's end is drawn ahead of its first frame.
const warmRows = 400

// warmed has a session just opened drawn once as the pane will draw it,
// still off the UI's thread and before the pane has it: the pane's first
// frame then finds each turn, and what each step's command and output
// say, already drawn, rather than drawing a long conversation whole while
// you wait on the switch.
func warmed(open tea.Cmd, o convo.Options) tea.Cmd {
	return func() tea.Msg {
		msg := open()
		if hm, ok := msg.(hostOpenMsg); ok && hm.c != nil && hm.c.sess != nil {
			hm.c.sess.Spawns() // each shell step read for an agent it runs, once, here and not on a tick
			if o.Width > 0 {
				o.Now = time.Now()
				hm.c.sess.Tail(o, warmRows)
			}
		}
		return msg
	}
}

func (m *Model) dropHost() {
	if m.host != nil {
		m.host.unwatch()
		m.host.closed.Store(true)
	}
	if m.host != nil && m.host.client != nil {
		go m.host.client.Close() //nolint:errcheck // a socket let go: nothing waits on its close
	}
	// Dropping references lets the normal collector reclaim this session.
	// Forcing a full GC and scavenging after every switch competes with the
	// next conversation's read/render work and stalls interactive frames.
	m.host, m.hostOpening = nil, ""
}

// openTail reads a Claude Code session's transcript in the background the
// first time; after that a watch takes in what is new as it is written.
func openTail(a *fleet.Agent) tea.Cmd {
	key, id, path, rush, kind := a.Key, a.ID, a.TranscriptPath, a.Rush, agent.Kind(a.Kind)
	if path == "" && (a.History != "" || rush) {
		return openHistory(a)
	}
	return func() tea.Msg {
		t := convo.NewTailFrom(path, tailBytes)
		// A stopped rush session that never got a message has no
		// transcript yet: it opens empty, and a message resumes it.
		if _, err := t.Read(); err != nil && (!rush || !errors.Is(err, fs.ErrNotExist)) {
			return hostOpenMsg{key: key, err: err}
		}
		exchangeID := ""
		if rush {
			exchangeID = id
			t.Sess.RestoreExchanges(host.ReadExchanges(id))
		}
		return hostOpenMsg{key: key, c: &hostConn{key: key, id: id, kind: kind, tail: t, sess: t.Sess, open: map[string]bool{}, ready: true, path: path, exchangeID: exchangeID}}
	}
}

// followTail closes the last turn once the agent has stopped working
// (transcripts don't always mark it); refreshSubs reads the transcript.
func (m *Model) followTail() {
	c := m.host
	if c == nil || c.tail == nil {
		return
	}
	a := m.agentByKey(c.key)
	if a == nil {
		return
	}
	if live := c.sess.Live(); live != nil && !a.Live() {
		c.sess.Apply(event.TurnEnd{Reason: "done"}, time.Now())
	}
	c.sess.Info.Cwd = a.Cwd
	c.sess.Info.CostUSD = a.Spend.Cost
	if c.sess.Info.Model == "" {
		c.sess.Info.Model = a.Spend.Model
	}
}

func (m *Model) onHostOpen(msg hostOpenMsg) tea.Cmd {
	if msg.key != m.hostOpening {
		if msg.c != nil && msg.c.client != nil {
			go msg.c.client.Close() //nolint:errcheck // one opened too late: nothing waits on its close
		}
		return nil
	}
	m.hostOpening = ""
	if msg.err != nil {
		m.flash("couldn't open the session: "+msg.err.Error(), true)
		if m.openFailed == nil {
			m.openFailed = map[string]time.Time{}
		}
		m.openFailed[msg.key] = time.Now() // zen moves past it for a while
		return nil
	}
	if old := m.host; old != nil && old.key == msg.key && old.sleeping {
		carryHostView(old, msg.c)
	}
	m.host = msg.c
	if d, ok := m.rewound[msg.key]; ok && m.host.client != nil {
		delete(m.rewound, msg.key)
		m.host.input, m.host.back = []rune(d), 0
		m.paneFocus = true
	}
	if c := m.host; c.client == nil {
		m.followTail()
		c.paneKick = true // its subagents and jobs, now rather than on the next tick
		if c.tail != nil && c.sess.Partial {
			return c.readWhole(m.warmOpts())
		}
		return nil
	}
	return m.host.readPre(m.warmOpts())
}

// besideList is whether the list sits left of the pane, its border just
// left of the conversation.
func (m *Model) besideList() bool {
	l, _ := m.widths()
	return l > 0
}

// warmOpts is how the pane draws a conversation, for one read in the
// background to be drawn once there as warmed does.
func (m *Model) warmOpts() convo.Options {
	_, paneW, _ := m.layout()
	return convo.Options{Width: m.conversationWidth(paneW - 3), Open: map[string]bool{}, Focused: m.paneFocus, Wide: m.hostedAlone(), NoRule: m.besideList()}
}

func (m *Model) onHostLines(msg hostLinesMsg) tea.Cmd {
	if t := m.grid.conns[msg.key]; t != nil && msg.owner == t && m.host != t {
		// A tile's session reads on as the open one does.
		var cmd tea.Cmd
		m.withHost(t, func() { cmd = m.onHostLines(msg) })
		if t.closed.Load() {
			delete(m.grid.conns, msg.key)
		}
		return cmd
	}
	c := m.host
	if c == nil || c.key != msg.key || msg.owner != nil && msg.owner != c {
		return nil
	}
	now := time.Now()
	var cmds []tea.Cmd
	if c.keep { // for the whole transcript being read to take in too
		c.since = append(c.since, msg.lines...)
	}
	for _, l := range msg.lines {
		ev, err := host.Decode(l)
		if err != nil || ev == nil {
			continue
		}
		if st, ok := ev.(host.Stamp); ok {
			now = st.At
			continue
		}
		if r, ok := ev.(host.Reply); ok {
			if cmd := m.onReply(c, r); cmd != nil {
				cmds = append(cmds, cmd)
			}
			continue
		}
		c.sess.Apply(ev, now)
		if e, ok := ev.(host.ErrorEvent); ok {
			m.flash(e.Error, true)
		}
	}
	m.holdForQuestion(c)
	if c.editAfter != "" {
		m.openWritten(c)
	}
	c.ready, c.flushed = true, time.Now()
	if c.sess.Info.Sleeping {
		m.parkHost(c)
		return tea.Batch(cmds...)
	}
	if msg.closed {
		if c.sess.Info.Sleeping {
			m.parkHost(c)
			return tea.Batch(cmds...)
		}
		if c.id != "" {
			return tea.Batch(append(cmds, m.resolveHostClose(c))...)
		}
		opening := m.hostOpening
		m.dropHost() // its transcript watch ends too, and a big one's memory goes back
		m.hostOpening = opening
		return tea.Batch(cmds...)
	}
	return tea.Batch(append(cmds, c.next())...)
}

// --- drawing ---

var (
	bgChrome string // L3: pane header and dock
	bgTabOn  string // the active view opens into the body
	bgSub    string // watching a subagent: its own, cooler ground
	bgRuns   string // the dock's running subagents
	bgQueue  string // the dock's queued messages

	cQueue string // what's queued: neither working nor needing you
)

// dockCard sets rows w-1 wide apart on a ground of their own, edged down
// the left in the colour that says what they are.
func dockCard(bg, edge string, rows []string, w int) []string {
	out := make([]string, len(rows))
	for i, r := range rows {
		out[i] = onBg(bg, paint(edge, "▌")+r, w)
	}
	return out
}

func onBg(bg, s string, w int) string {
	s = fit(s, w)
	return bg + strings.ReplaceAll(s, reset, reset+bg) + reset
}

func spread(left, right string, w int) string {
	gap := w - cellw.String(left) - cellw.String(right)
	if gap < 2 {
		return fit(left, w)
	}
	return left + strings.Repeat(" ", gap) + right
}

// rushPane is the right pane for a rush-mode agent, or nil when the pane
// shows something else.
func (m *Model) rushPane(w, h int) []string {
	a := m.focused()
	if a == nil {
		return nil
	}
	c := m.host
	if c == nil || c.key != a.Key {
		if !a.Rush {
			return nil // still loading; the summary shows meanwhile
		}
		if a.PID == 0 && m.hostOpening != a.Key {
			return []string{"", dim("  " + oneLine(a.DisplayName) + " isn't running, and has no conversation to show yet")}
		}
		return []string{"", dim("  connecting to " + oneLine(a.DisplayName) + "…")}
	}
	s := c.sess
	var head []string
	if m.zenFull() {
		head = []string{m.zenBar(a, w)} // zen is only the agent and its box
	} else {
		head = m.paneHeader(a, c, w)
	}
	if l := m.subBanner(c, w); l != "" {
		head = append(head, l)
	}
	dock := m.paneDock(a, c, w, h)
	var panels []string
	// Until replay is ready, the placeholder is not a transcript tail. Keep
	// auxiliary panels in the bottom dock instead of attaching them to it.
	scrollingPanels := m.viewName(c) == "conversation" && !m.zen && (c.client == nil || c.ready)
	if scrollingPanels {
		panels, dock = dock[:c.auxRows], dock[c.auxRows:]
		c.boxIdx -= c.auxRows
	}
	activity := m.paneActivity(c, m.conversationWidth(w))
	// In a conversation, activity is its last thing: it follows the latest
	// message, a row of room either side, and scrolls away with it.
	var activityRows []convo.Line
	if scrollingPanels && len(activity) > 0 {
		activityRows = []convo.Line{{}}
		for _, l := range activity {
			activityRows = append(activityRows, convo.Line{Text: l.Text}) // not a row to pick
		}
		activityRows = append(activityRows, convo.Line{})
		activity = nil
	}
	bodyH := max(3, h-len(head)-len(dock)-len(activity))
	c.subPreview.visible = false

	// ponytail: turns already drawn keep their views until redrawn after the hex plugin is switched.
	c.sess.Hex = m.bundledOn["hex"]
	contentW := w
	c.minimap.visible = m.minimapEnabled(c, w)
	if c.minimap.visible {
		contentW -= miniWidth
	} else {
		c.minimap = conversationMap{}
	}
	o := convo.Options{Width: contentW, Now: paneNow(), Tick: m.tick, Open: c.open, View: c.looks, Verbose: c.verbose, Depth: c.depth, History: m.historyOf(c), HideActivity: true,
		// Rows keep to their usual width while cards sit right of them.
		Selected: c.sel, Focused: m.paneFocus, Wide: m.hostedAlone() && len(c.side) == 0, NoRule: m.besideList()}
	if a := m.agentByKey(c.key); a != nil {
		o.Compaction = a.Compaction
	}
	if by := c.sess.Info.Meta["spawnedBy"]; by != "" {
		for _, a := range m.order {
			if a.Rush && a.ID == by {
				o.Parent = a.DisplayName
				break
			}
		}
	}
	if id := c.sess.Info.ID; id != "" && c.briefFor != id {
		cfg, _ := host.ReadConfig(id) // once a session: its config doesn't change
		c.brief, c.briefFor = strings.TrimSpace(cfg.SystemPrompt), id
	}
	o.Hosted, o.Brief = c.sess.Info.ID != "", c.brief

	if a := m.agentByKey(c.key); a != nil && !isRoomKey(c.key) {
		o.Agent = agentHandle(a)
	}
	o.Paused = pausedIDs()
	o.Scripts = c.scriptViews(time.Now())
	var body []convo.Line
	// body is rows [base, base+len(body)) of total; in a conversation only
	// the turns around what's on screen are drawn. transcript is the rows
	// above the panels, ins those put in under shell steps.
	base, total, transcript, ins := 0, 0, 0, 0
	view := m.viewName(c)
	if m.zen {
		view = "zen"
	}
	conv := view == "conversation"
	var panelRows []convo.Line
	if scrollingPanels {
		for i, row := range panels {
			ref := c.panelRefs[i]
			if ref == "" {
				ref = fmt.Sprintf("panel:%d", i)
			}
			panelRows = append(panelRows, convo.Line{Text: row, Ref: ref})
		}
		panelRows = append(activityRows, panelRows...)
	}
	reused := c.scrollOnly && !c.stale && c.shown != nil && c.shownView == view && c.paneW == contentW && view != "stack"
	if reused {
		body, base, transcript, ins = c.shown, c.shownBase, c.transcriptRows, c.shownIns
		if scrollingPanels {
			body = body[:min(max(0, transcript-base), len(body))]
			if base+len(body) == transcript {
				body = append(body, panelRows...)
			}
		}
		total = transcript + len(panelRows)
	} else if !conv {
		// Only a conversation is drawn in slices: any other view is whole,
		// and a stale left from one would ask relayout for frames forever.
		c.stale = false
		switch view {
		case "zen":
			body = m.zenBody(a, c, w)
		case "screen":
			for _, l := range m.liveLines(w) {
				body = append(body, convo.Line{Text: l})
			}
			if len(body) == 0 {
				body = []convo.Line{{Text: ""}, {Text: dim("  connecting to its screen…")}}
			}
		case "overview":
			who := []convo.Line{{}, {Text: "  " + paint(cText+bold, oneLine(a.DisplayName)) + " " + nameHandle(a)}}
			body = append(append(append(who, s.Overview(o)...), m.pluginOverview(c.key)...), m.overviewSections(c, o)...)
		case "changes":
			if m.fullFile(c) {
				view = "file" // a view of its own, so it opens at its top
				body = s.FileView(c.full, o)
				break
			}
			o.Marks = c.marks
			body = s.ChangesView(o)
		case "tasks":
			body = m.taskLines(c, o)
		case "background":
			body = m.jobLines(c, o)
		case "memory":
			body = m.memoryLines(c, o, bodyH)
		case "subagents":
			if c.subOpen != "" {
				o.Selected = c.subSel
			}
			body = m.subagentLines(c, o)
		case "stack":
			body = m.stackLines(c, o, bodyH)
		}
		transcript, total = len(body), len(body)
	}
	// Bottom-anchored: the latest output sits just above the dock unless
	// you've scrolled up. A selection that just moved is scrolled into view.
	if at := view + "/" + c.subOpen; at != c.pinFor {
		c.pinFor, c.pinTop = at, readsFromTop(view, c)
	} else if c.selMoved || c.scroll != c.top.scroll {
		c.pinTop = false // you've scrolled, or picked something to show
	}
	// Scrolled up, the row at the top is kept there, moved by as much as
	// you've scrolled since, whatever was added or counted again around it.
	anchored := func() bool {
		t := c.top
		return c.scroll > 0 && t.scroll > 0 && t.view == view && t.ref != "" && !c.selMoved && !c.pinTop
	}
	moved := c.scroll - c.top.scroll
	var scroll, rows, start, end int
	settle := func() {
		scroll = c.scroll
		if c.pinTop {
			scroll = total // clamped to the top below
		}
		if anchored() {
			i, off := rowOf(body, base, c.top.ref, s, conv), c.top.off
			if i < 0 && conv {
				if k := s.TurnOf(c.top.ref); k >= 0 {
					i, off = s.TurnRow(k), 0 // folded away: its turn stays at the top
				}
			}
			if i >= 0 {
				scroll = total - (i + off + bodyH - 1) + moved // scrolled up, the pill takes a row
			}
		}
		// A row just opened shows what opened under it, scrolling up only as
		// far as that takes and never past the row itself.
		if i := rowOf(body, base, c.reveal, s, conv); c.reveal != "" && i >= 0 {
			last := i + max(0, total-c.revealTotal)
			scroll = max(min(scroll, total-last-1), total-i-bodyH+1)
		}
		if scrollingPanels && c.cardFocus && !c.panelFocus {
			scroll = 0
		}
		if c.selMoved && c.sel != "" {
			// The first row of a selection is the one to show.
			if i := rowOf(body, base, c.sel, s, conv); i >= 0 {
				// A few rows above it stay in view, so the turn heading pinned
				// to the top never covers it.
				above := min(3, (bodyH-2)/2)
				end := total - scroll
				switch {
				case i-above < end-(bodyH-1): // scrolled up, the pill takes a row
					scroll = total - (i - above + bodyH - 1)
				case i >= end:
					scroll = total - (i + 1)
				}
			}
		}
		// Scrolled up, the pill takes a row, so the top is a row further up
		// than the body's height alone says.
		scroll = max(0, min(scroll, total-bodyH+1))
		if total <= bodyH {
			scroll = 0
		}
		// Scrolled up, the "more below" pill takes a row of its own rather
		// than covering the last one (which may be the selected one).
		rows = bodyH
		if scroll > 0 {
			rows--
		}
		end = total - scroll
		start = max(0, end-rows)
	}
	drew := false
	if conv {
		// The window drawn is the screen and one either side, around what
		// it's to show: the row kept at the top, a selection just moved, or
		// the scroll as it stands; a second pass when that moved it.
		var from, to int
		if !reused {
			at := s.Count(o) + c.shownIns + len(panelRows) - c.scroll
			if k := s.TurnOf(c.sel); c.selMoved && k >= 0 {
				at = s.TurnRow(k) + bodyH
			} else if k := s.TurnOf(c.top.ref); anchored() && k >= 0 {
				at = s.TurnRow(k) + c.top.off - moved + bodyH
			} else if c.pinTop {
				at = bodyH
			}
			from, to = at-2*bodyH, at+bodyH
		}
		for pass := 0; pass < 3; pass++ {
			if pass > 0 || !reused {
				body, base, transcript, ins = m.drawWindow(c, o, from, to, panelRows)
				total, drew = transcript+len(panelRows), true
			}
			settle()
			if start >= base && end <= base+len(body) {
				break
			}
			from, to = start-bodyH, end+bodyH
		}
	} else {
		settle()
	}
	// RenderWindow resets Fast; the activity pulse also needs fast frames, even
	// when the body was reused for scrolling or the turn is folded closed.
	if active := m.activitySession(c); active != nil && (active.Live() != nil || active.Compacting()) {
		s.Fast = true
	}
	c.scroll, c.reveal = scroll, ""
	if c.selMoved && c.sel != "" {
		c.selMoved = false
	}
	c.panelFocus = c.cardFocus
	if drew || !reused { // the refs of the rows drawn, for ↑↓
		if c.seenRef == nil {
			c.seenRef = map[string]bool{}
		}
		clear(c.seenRef)
		c.bodyRefs = c.bodyRefs[:0]
		prev := ""
		for _, l := range body[:min(len(body), max(0, transcript-base))] {
			// A ref's rows sit together, so most repeats are the row before's.
			if l.Ref != "" && l.Ref != prev && !c.seenRef[l.Ref] {
				c.seenRef[l.Ref] = true
				c.bodyRefs = append(c.bodyRefs, l.Ref)
			}
			prev = l.Ref
		}
	}
	if c.txt.view == view && base != c.shownBase { // dragged-over rows are body's
		c.txt.a.row += c.shownBase - base
		c.txt.b.row += c.shownBase - base
	}
	c.endShown = conv && c.scroll < rows
	if view == "stack" && c.stackGrow < stackFrames {
		s.Fast, c.endShown = true, true // frames keep coming while it grows
	}
	// Keep scrolling at the requested row. Snapping back to a multiline
	// prompt's beginning makes wheel motion oscillate instead of moving.
	c.top.ref, c.top.view, c.top.scroll = "", view, c.scroll
	if c.scroll > 0 {
		// The first row shown that belongs to something, and how far the
		// window's top (before a heading moved it) is from that thing's
		// first row.
		for i := max(start, base); i < min(end, base+len(body)) && c.top.ref == ""; i++ {
			c.top.ref = body[i-base].Ref
		}
		if i := rowOf(body, base, c.top.ref, s, conv); c.top.ref != "" && i >= 0 {
			c.top.off = total - c.scroll - rows - i
		}
	}
	out := append([]string{}, head...)
	c.rowRefs = make([]string, len(head), h)
	if m.watchingSub(c) {
		c.rowRefs[len(head)-1] = "subback" // clicking the banner goes back
	}
	c.rowBody = c.rowBody[:0]
	for range head {
		c.rowBody = append(c.rowBody, -1)
	}
	panelY := m.paneTop + len(head) + transcript + len(activityRows) - start
	for r := start; r < end; r++ {
		i := r - base
		var l convo.Line
		if i >= 0 && i < len(body) {
			l = body[i]
		} else {
			i = -1 // not drawn in time: a blank row until the next frame
		}
		if c.stale {
			l.Text = fit(l.Text, contentW) // may be drawn for another width
		}
		out = append(out, l.Text)
		c.rowRefs = append(c.rowRefs, l.Ref)
		if scrollingPanels && r >= transcript {
			i = -1
		}
		c.rowBody = append(c.rowBody, i)
	}
	if conv {
		m.pinPrompts(c, s, out, len(head), start, end, contentW)
	}
	bodyBottom := m.paneTop + len(out)
	c.shown, c.bodyTop, c.bodyRows, c.paneW, c.shownView = body, len(head), end-start, contentW, view
	c.shownBase, c.shownTotal, c.shownIns, c.transcriptRows = base, total, ins, transcript
	if c.txt.view != view && !c.txt.drag {
		c.txt = textSel{} // made in another view, whose rows these aren't
	}
	if c.scroll > 0 {
		pill := selBG + " " + paint(cText, fmt.Sprintf("↓ %d more · end follows", c.scroll)) + " " + reset
		pillWidth := contentW
		if view == "subagents" && wideSubagents(c, contentW) {
			pillWidth = subListWidth(contentW)
		}
		out = append(out, spread("", pill, pillWidth))
		c.rowRefs = append(c.rowRefs, "")
	}
	c.paintHover(out)
	c.paintSel(out)
	for _, l := range activity { // a view that isn't the conversation keeps it docked
		out = append(out, l.Text)
	}
	for len(out) < h-len(dock) {
		out = append(out, "")
	}
	if len(c.side) > 0 {
		// Above the dock's first row on screen, never over a card or the
		// queue: what's short of room shows its top.
		foot := h - len(dock)
		for j := len(head); scrollingPanels && j < len(c.rowBody); j++ {
			if c.rowBody[j] == -1 {
				foot = j
				break
			}
		}
		c.side = c.side[:min(len(c.side), max(0, foot-len(head)-1))] // and clicks only what shows
		y, x := max(len(head), foot-len(c.side)-1), contentW-c.sideW-1
		for i := y; i < len(out); i++ {
			out[i] = fit(out[i], contentW)
		}
		pasteAt(out, c.side, y, x)
		c.sideX, c.sideY = x, m.paneTop+y
	}
	if view == "subagents" && wideSubagents(c, contentW) {
		m.drawSubPreview(c, o, out, len(head), bodyH)
	}
	if c.minimap.visible {
		mm := &c.minimap
		mapH := h - len(head)
		if scrollingPanels || drew || !reused || mm.width != contentW || mm.height != mapH || mm.palette != miniPalette() {
			mm.prepareWindow(miniWindow{lines: body, base: base, total: total, transcript: transcript, ins: ins, s: s}, contentW, mapH)
		}
		mm.x, mm.y = m.paneX()+contentW, m.paneTop+len(head)
		mm.viewport, mm.start, mm.end = bodyH, start, end
		for y := 0; y < bodyH && len(head)+y < len(out); y++ {
			i := len(head) + y
			out[i] = fit(out[i], contentW) + reset + mm.line(y)
		}
		for y := range dock { // the map runs on down beside the box
			if bodyH+y < mapH {
				dock[y] = fit(dock[y], contentW) + reset + mm.line(bodyH+y)
			}
		}
	}
	if m.cardModal(c) {
		c.minimap.visible, c.minimap.dragging = false, false
		c.subPreview.visible = false
		m.cardOverlay(a, c, out, len(head), h-len(dock)-len(head), w)
	}
	m.btwOverlay(c, out, len(head), h-len(dock)-len(head), w)
	c.boxY = m.paneTop + len(out) + c.boxIdx
	c.dockY = m.paneTop + len(out)
	if scrollingPanels {
		c.panelClip = [2]int{m.paneTop + len(head), bodyBottom}
		shift := panelY - c.dockY
		if !m.cardModal(c) {
			c.cardTop += shift
		} else {
			c.panelClip = [2]int{}
			c.qAt = nil
		}
		c.qTop += shift
		// Clipped panel controls cannot intercept the header or composer.
		for row := range c.qAt {
			y := c.dockY + c.qTop + row
			if y < m.paneTop+len(head) || y >= bodyBottom {
				delete(c.qAt, row)
			}
		}
	}
	return append(out, dock...)
}

// drawWindow draws the conversation's rows [from, to) as whole turns, with
// what lately run shell steps write under them, and the panels under it
// when it reaches the end.
func (m *Model) drawWindow(c *hostConn, o convo.Options, from, to int, panels []convo.Line) (body []convo.Line, base, transcript, ins int) {
	s := c.sess
	// Resized, a long session is laid out again a slice a frame, from its
	// end: what isn't yet shows as it was, cut to fit.
	o.Budget = relayBudget
	body = s.RenderWindow(o, from, to, c.bodyBuf)
	c.bodyBuf = body
	base, transcript = s.Rows()
	c.drawn, c.drewConvo, c.stale = o, true, s.Stale()
	n := len(body)
	body = m.withWritesFrom(c, body, o.Width, s.TurnRow(s.ActiveFrom(time.Now().Add(-writesFor)))-base)
	ins = len(body) - n
	transcript += ins
	if transcript == 0 {
		message := "  nothing yet · type below to start"
		if c.client != nil && !c.ready {
			message = "  loading recent messages…"
		}
		body, transcript = []convo.Line{{Text: dim(message)}}, 1
	}
	if base+len(body) == transcript {
		body = append(body, panels...)
	}
	return body, base, transcript, ins
}

// fullFile is whether the changes view shows one file in full (alt+v).
func (m *Model) fullFile(c *hostConn) bool {
	return c.full != "" && m.viewName(c) == "changes"
}

// changedFile is the file a changes-view row is of, this session's or the
// working tree's.
func changedFile(ref string) string {
	for _, pre := range []string{"chg:", "tree:"} {
		if p, ok := strings.CutPrefix(ref, pre); ok {
			return p
		}
	}
	return ""
}

// readsFromTop is whether a view opens at its top: a report or a list does,
// a conversation (the main one or a subagent's) or a screen at its end.
func readsFromTop(view string, c *hostConn) bool {
	switch view {
	case "overview", "changes", "file", "tasks", "memory", "background":
		return true
	case "subagents":
		return c.subOpen == ""
	}
	return false
}

// headingStart is the first row of the turn heading that row i is part of.
func headingStart(body []convo.Line, i int) int {
	for i > 0 && body[i-1].Ref == body[i].Ref {
		i--
	}
	return i
}

func (m *Model) paneHeader(a *fleet.Agent, c *hostConn, w int) []string {
	s := c.sess
	info := s.Info
	state := dim("idle")
	switch {
	case c.sleeping || info.Sleeping:
		state = dim("◦ idle")
	case c.client == nil:
		switch {
		case a.NeedsYou() || a.Waiting():
			state = paint(cYellow+bold, "● needs you")
		case a.Live():
			state = paint(cOrange, "✻ working")
		case a.PID != 0:
			state = dim("◦ idle")
		default:
			state = dim("⏹ stopped")
		}
	case info.Limit != nil:
		state = paint(cYellow, "⏸ "+limitText(info.Limit))
	case info.Retry != nil && info.Retry.GaveUp:
		state = paint(cRed, "✗ API error · "+info.Retry.Why)
	case info.Retry != nil && info.Retry.Offline:
		state = paint(cYellow, "⟳ offline · continues when the network is back")
	case info.Retry != nil && info.Retry.Proof:
		state = paint(cYellow, "⟳ cache expired · continues once the connection holds")
	case info.Retry != nil:
		state = paint(cYellow, fmt.Sprintf("⟳ retry %d of %d in %s", info.Retry.Attempt, info.Retry.Max, dur(time.Until(info.Retry.Next).Round(time.Second))))
	case len(s.Pending()) > 0:
		state = paint(cYellow+bold, "● needs you")
	case s.Live() != nil:
		state = paint(cOrange, "✻ working "+dur(time.Since(s.Live().Start)))
	case s.Compacting():
		state = paint(cOrange, "✻ compacting")
	case info.Error != "":
		state = paint(cRed, "✗ stopped mid-turn")
	case info.ClaudePID == 0:
		state = dim("◦ idle") // its process rests until a message wakes it: nothing you need to know
	}
	kind := sessionAgent(c)
	if kind == "" {
		kind = agent.Kind(firstNonEmpty(a.Kind, string(loginsKind)))
	}
	if low := m.lowNote(kind); low != "" {
		state += "   " + paint(cYellow, low)
	}
	// Filling the screen, it's the only thing there to name, and what's on
	// the right lines up with the conversation's own right edge.
	alone := m.paneAlone()
	hw := w
	if alone && !m.hostedAlone() {
		hw = min(w, maxPane-3)
	}
	// The rest is the agent header you build in /statusline: its first
	// line right of the name, its second under it.
	x := &barCtx{m: m, a: a, c: c}
	lead := "  "
	// The lines under the name start where it does.
	indent := lead
	// A name is often the whole first message; it's cut short so the
	// state and what's on the right keep their room.
	name := oneLine(a.DisplayName)
	if room := max(12, min(56, (hw-cellw.String(lead+state))/2)); cellw.String(name) > room {
		name = cellw.Truncate(name, room, "…")
	}
	left1 := lead + paint(cBright+bold, name) + " " + nameHandle(a) + "   " + state
	right := m.barLine(barAgent, 0, x, hw-cellw.String(left1)-4)
	row1 := spread(left1, right+" ", hw)
	c.contextLabel = [2]int{}
	if value := headerContext(c, a); value != "" {
		plain, metric := ansi.Strip(row1), ansi.Strip(value)
		if i := strings.LastIndex(plain, metric); i >= 0 && strings.Contains(ansi.Strip(right), metric) {
			c.contextLabel = [2]int{cellw.String(plain[:i]), cellw.String(metric)}
		}
	}

	conn := "" // connected is the usual: only what isn't is said
	if c.client == nil {
		switch {
		case c.sleeping || info.Sleeping:
		case isRoomRow(a):
			conn = m.roomConn(a)
		case a.Rush:
			conn = dim("send to resume")
		case a.Past:
			conn = dim("past conversation · a message resumes it in rush mode")
		case a.Headless:
			conn = dim("Claude Code · " + a.Where())
		case a.Interactive:
			conn = dim("Claude Code · "+a.Where()+" · ") + paint(cOrange, "/rush") + dim(" copies it here")
		case m.moveWhenIdle[a.Key]:
			conn = paint(cOrange, "moves to rush mode when this turn ends")
		default:
			conn = dim("Claude Code · ") + paint(cOrange, "/rush") + dim(" moves it here")
		}
	}
	if (info.Limit != nil || info.Retry != nil) && !info.CacheWarm.IsZero() {
		if time.Now().Before(info.CacheWarm) {
			conn = strings.TrimRight(dim("cache warm until ")+paint(cGreen, info.CacheWarm.Local().Format("15:04"))+"   "+conn, " ")
		} else {
			conn = strings.TrimRight(paint(cYellow, "cache cold")+"   "+conn, " ")
		}
	}
	tag := m.agentLabel(a, c, firstNonEmpty(s.Model, info.Model))
	if c.pointerHover.label {
		tag = hoverLine(tag, cellw.String(tag))
	}
	shown := hw-cellw.String(conn+tag) > 40
	if shown {
		conn = strings.TrimRight(tag+"   "+conn, " ") // what it runs as, with room
	}
	meta := m.barLine(barAgent, 1, x, hw-cellw.String(indent+conn)-4)
	row2 := spread(indent+meta, conn+" ", hw)
	// Where the label is, for a click on it to open the switch sheet.
	c.label = [2]int{}
	if at := hw - cellw.String(conn+" "); shown && at-cellw.String(indent+meta) >= 2 {
		c.label = [2]int{at, cellw.String(tag)}
	}

	var tabs []string
	c.paneTabs = c.paneTabs[:0]
	views := m.views(c)
	// on is where the tab showing sits on the row, for the rule under it.
	onX, onW := 0, 0
	for i, v := range views {
		if v == "stack" {
			continue // no tab: see openStack
		}
		stat := tabStat(c, v)
		if stat != "" {
			stat += " "
		}
		tabX := cellw.String(indent[1:]+strings.Join(tabs, " ")) + min(i, 1)
		tabW := cellw.String(" " + v + " " + stat)
		c.paneTabs = append(c.paneTabs, paneTab{tabX, min(hw, tabX+tabW), i})
		if i == c.view%len(views) {
			onX, onW = cellw.String(indent[1:]+strings.Join(tabs, " "))+min(i, 1), cellw.String(" "+v+" "+stat)
			tabs = append(tabs, bgTabOn+paint(cText+bold, " "+v+" ")+bgTabOn+strings.ReplaceAll(stat, reset, reset+bgTabOn)+reset+bgChrome)
		} else {
			text := paint(cSub, " "+v+" ") + strings.ReplaceAll(stat, reset, reset+bgChrome)
			if c.pointerHover.tab == i+1 {
				text = hoverLine(text, tabW)
			}
			tabs = append(tabs, text)
		}
	}
	chips := "" // Tool failures stay with their transcript rows, not a lifetime counter.
	switch {
	case c.verbose:
		chips += paint(cOrange, "m3 all shown") + "  "
	case c.depth == convo.DepthProse:
		chips += paint(cOrange, "m0 prose only") + "  "
	case c.depth == convo.DepthRuns:
		chips += paint(cOrange, "m1 steps folded") + "  "
	}
	if alone && !m.hostedAlone() {
		// Nothing says the list is behind it but this.
		chips += paint(cText, "esc") + dim(" back to the list") + " "
	}
	if m.viewName(c) == "screen" {
		chips = dim("typing goes into it · ctrl+] comes back · ctrl+f full screen") + "  "
	}
	// A tab's name lines up with the name above, its padding just before.
	strip := withTabHint(indent[1:]+strings.Join(tabs, " "), "[ ]", "views", "", hw-cellw.String(chips)-2)
	for i := range c.paneTabs {
		c.paneTabs[i].end = min(c.paneTabs[i].end, cellw.String(strip))
	}
	row3 := spread(strip, chips, hw)
	// A rule closes the chrome off, lit under the tab showing, orange
	// while the Session has the keys. A row of the pane's ground keeps
	// the first message off it.
	lit := cSub
	if m.paneFocus {
		lit = cOrange
	}
	border := harnessBorder(kind)
	rule := paint(border, strings.Repeat("─", min(onX, hw))) + paint(lit, strings.Repeat("━", max(0, min(onW, hw-onX)))) +
		paint(border, strings.Repeat("─", max(0, w-onX-onW)))
	// Extend patches through all four rows, retaining tab surfaces on the left.
	tabsPlain := onBg(bgChrome, row3, w)
	tabsTinted := headerWash(row3, w, paneTabsRow, kind)
	start := headerEdgeStart(w)
	tabsRow := ansi.Cut(tabsPlain, 0, start) + ansi.Cut(tabsTinted, start, w)
	return []string{headerWash("", w, 0, kind), headerWash(row1, w, paneTitleRow, kind), headerWash(row2, w, paneMetaRow, kind), tabsRow, rule}
}

// paneAlone is when the Session fills the screen with no list beside it.
func (m *Model) paneAlone() bool {
	return m.listW == 0 && !m.zenFull()
}

func firstNonEmpty(xs ...string) string {
	for _, x := range xs {
		if x != "" {
			return x
		}
	}
	return ""
}

// paneDock is the raised area at the bottom of the pane: the current task,
// the subagents working, the queue, a card waiting, and the input box.
// cardRows are the cards waiting on you (a usage limit to decide on, a
// tool call to allow, Claude's questions), w wide and a question folded to
// fit maxH rows; nil when nothing waits.
func (m *Model) cardRows(a *fleet.Agent, c *hostConn, w, maxH int) []string {
	s := c.sess
	var out []string
	// In a modal the frame is the card: no ground or edge of its own, and
	// the frame's title says what it is.
	card, bar, modal := qCard, paint(cYellow, "▍"), c.inModal
	if modal {
		card, bar = "", " "
	}
	cl := func(txt string) {
		if modal {
			out = append(out, fit(txt, w))
			return
		}
		out = append(out, onBg(card, txt, w))
	}
	if l := s.Info.Limit; l != nil && l.Ask {
		out = append(out, m.limitCardRows(a, c, w, maxH)...)
	}
	if signedOut(c) {
		out = append(out, m.signedOutRows(a, c, w)...)
	}
	if p := s.Pending(); len(p) > 0 && p[0].Approval.Question != nil { //nolint:nestif // the question card, else the approval card
		if len(out) > 0 {
			out = append(out, "")
		}
		c.qHitTop = len(out)
		out = append(out, m.questionCard(c, p[0].Approval.Question, w, maxH)...)
	} else if len(p) > 0 {
		if len(out) > 0 {
			out = append(out, "")
		}
		st := p[0]
		if memoryWrite(st) && !modal {
			rows, btns := memoryCard(c, st, w, maxH)
			for _, b := range btns {
				b.y += len(out)
				c.btns = append(c.btns, b)
			}
			return append(out, rows...)
		}
		count := ""
		if st.Call().Kind == tool.Shell && a.Cwd != "" {
			count = "in " + shortDir(tildify(a.Cwd), 36) // where it runs
		}
		if len(p) > 1 {
			count = strings.TrimPrefix(count+"   "+fmt.Sprintf("1 of %d", len(p)), "   ")
		}
		what := paint(cYellow+bold, "● needs you") + "   "
		if modal {
			what = ""
		}
		if p := st.Under(); p != nil {
			// A subagent's: say whose, since it's not the agent you're talking to.
			who := "a subagent"
			for _, sa := range c.subs {
				if sa.ToolUseID == p.ID {
					who = strings.TrimSpace(sa.Type + " · " + oneLine(sa.Description))
				}
			}
			count = strings.TrimPrefix(paint(cBlue, "⇉ from "+who)+"   "+count, "   ")
		}
		cl(spread(bar+" "+what+paint(cText+bold, approvalTitle(st)), paint(cSub, count)+"  ", w))
		for _, l := range approvalBody(st, a.Cwd, w-6) {
			cl(bar + "     " + l)
		}
		k := func(key, label string) string { return paint(cText+bold, key) + " " + paint(cSub, label) }
		edge := bar
		if c.cardFocus && !modal {
			edge = paint(cOrange, "▍")
		}
		keys := k("y", "allow once")
		if st.Approval != nil && st.Approval.Always {
			keys += "   " + k("a", alwaysWords(st.Approval.AlwaysAs))
		}
		cl(edge + "   " + cardHint(c, keys+"   "+k("n", "deny")))
	}
	return out
}

// taskRow is the agent's task list where it is, sat on the box: how far
// through it is, then the tasks around the one under way (the last done,
// it, the next two). Shut, it's the one row with the task under way on it.
// A click on its head opens or shuts it, anywhere else the tasks view. A
// list all done says nothing.
func taskRow(s *convo.Session, w int, shut bool) []string {
	now, done, total := s.Current()
	if total == 0 || now == nil && done == total {
		return nil
	}
	bar := paint(cOrange, strings.Repeat("▰", done)) + faint(strings.Repeat("▱", total-done))
	if total > 20 {
		bar = ""
	}
	caret := "▾ "
	if shut {
		caret = "▸ "
	}
	head := "  " + paint(cOrange, caret) + paint(cSub+bold, "Tasks") + "  " + dim(fmt.Sprintf("%d of %d done", done, total)) + "  " + bar
	if shut && now != nil {
		head += "  " + paint(cText, oneLine(firstNonEmpty(now.Active, now.Subject)))
	}
	out := []string{spread(head, paint(cSub, taskAll)+"  ", w)}
	if shut {
		return out
	}
	// Around the task under way, or the next to do when none is.
	at := slices.IndexFunc(s.Tasks, func(t convo.Task) bool { return t.Status == "in_progress" })
	if at < 0 {
		at = max(0, slices.IndexFunc(s.Tasks, func(t convo.Task) bool { return t.Status != "completed" }))
	}
	from := max(0, at-1)
	to := min(len(s.Tasks), from+4)
	for i, t := range s.Tasks[from:to] {
		mark, label := dim("☐"), paint(cSub, oneLine(t.Subject))
		switch t.Status {
		case "in_progress":
			mark, label = paint(cOrange, "■"), paint(cText+bold, oneLine(firstNonEmpty(t.Active, t.Subject)))
		case "completed":
			mark, label = paint(cGreen, "✓"), dim(oneLine(t.Subject))
		}
		if more := len(s.Tasks) - to; i == to-from-1 && more > 0 {
			label += "  " + faint(fmt.Sprintf("+%d more", more))
		}
		out = append(out, fit("    "+mark+" "+label, w))
	}
	return out
}

const taskAll = "all tasks ›"

// clickTasks takes a click on the task rows on the box: on the head it
// opens or shuts them, on "all tasks ›" or a task it opens the tasks view.
func (m *Model) clickTasks(c *hostConn, x, y int) bool {
	top := c.boxY - c.taskRows
	if c.taskRows == 0 || y < top || y >= c.boxY {
		return false
	}
	if y == top && x < m.paneX()+c.taskW-2-cellw.String(taskAll) {
		c.tasksShut = !c.tasksShut
		return true
	}
	return m.showView(c, "tasks")
}

func (m *Model) paneDock(a *fleet.Agent, c *hostConn, w, h int) []string {
	s := c.sess
	// A card answered from the card hands the keys on to the next one
	// (the review after a question, the plan to approve after it) as long
	// as nothing's been typed or picked since.
	if c.cardAgain != "" && m.paneFocus && len(c.input) == 0 && c.sel == "" {
		if id := cardID(c); id != "" && id != c.cardAgain {
			c.cardFocus, c.cardAgain = true, ""
		}
	}
	c.btns, c.qAt = nil, nil
	c.panelRefs = map[int]string{}
	c.panelClip = [2]int{}
	fullW := w
	if m.minimapEnabled(c, w) {
		w = m.conversationWidth(w)
	}
	var out []string
	line := func(txt string) { out = append(out, onBg(bgChrome, txt, w)) }
	// Room between parts is ground: in a conversation the dock scrolls
	// with the transcript (paneBody), and a row of chrome there reads as
	// an empty band.
	scrolls := m.viewName(c) == "conversation" && !m.zen && (c.client == nil || c.ready)
	room := func() {
		if scrolls {
			out = append(out, "")
		} else {
			line("")
		}
	}
	// Each part of the dock (the task, a card, the subagents, the queue)
	// is its own block, a row of ground between one and the next.
	blocks := 0
	block := func() {
		if blocks > 0 {
			room()
		}
		blocks++
	}
	// A card waiting on you sits last, just above the box, when you've
	// set its modal aside for later: the first stop for ↑.
	cards := func() {
		if m.cardModal(c) {
			return
		}
		rows := m.cardRows(a, c, w, h*3/5)
		if v, ok := m.verdicts[c.key]; ok && len(rows) == 0 {
			// A room's verdict waits like a card: last, behind any other.
			if c.verdictShown != v.ID {
				c.verdictShown = v.ID
				if len(c.input) == 0 && time.Since(c.lastKeyAt) > time.Second {
					c.cardFocus, c.sel = true, ""
				}
			}
			edge := cBlue
			if c.cardFocus {
				edge = cOrange
			}
			rows = dockCard(bgRuns, edge, m.verdictRows(c, w-1), w)
		}
		if len(rows) > 0 {
			block()
			c.cardTop = len(out)
			c.panelRefs[len(out)] = "panel:card"
			out = append(out, rows...)
		}
	}

	if r := s.Info.Retry; r != nil && r.GaveUp {
		block()
		line("  " + paint(cRed, "✗ "+r.Reason) + dim(" · "+r.Why+" · send anything to try again"))
	}
	// With room right of the conversation's text, the subagents and what
	// runs in the background sit there, at the foot of the pane, rather
	// than taking rows from it.
	c.side, c.sideW = c.side[:0], 0
	if !m.zen {
		c.sideW = sideWidth(w)
	}
	panel := func(edge string, rows func(w int) []string) {
		if c.sideW > 0 {
			if len(c.side) > 0 {
				c.side = append(c.side, "")
			}
			c.previewBase = sideRefs + len(c.side)
			c.side = append(c.side, dockCard(bgRuns, edge, rows(c.sideW-1), c.sideW)...)
			return
		}
		block()
		c.previewBase = len(out)
		out = append(out, dockCard(bgRuns, edge, rows(w-1), w)...)
	}
	if run := c.dockSubs(); len(run) > 0 && m.viewName(c) == "conversation" {
		panel(cBlue, func(w int) []string { return m.runningPreview(c, run, w) })
	}
	if jobs := c.looseJobs(); len(jobs) > 0 && m.viewName(c) == "conversation" {
		panel(cOrange, func(w int) []string { return m.jobsPreview(c, jobs, w) })
	}
	if l := m.sendingLines(c, w); len(l) > 0 {
		block()
		for _, l := range l {
			line(l)
		}
	}
	if l := m.subNoteLines(c, w); len(l) > 0 {
		block()
		for _, l := range l {
			line(l)
		}
	}
	// The queue and a waiting card sit last, just above the box.
	if l := m.chatRoomRows(c, w); len(l) > 0 {
		block()
		for _, l := range l {
			c.panelRefs[len(out)] = dockRoomPrefix + m.chatRoomOn(c).ID
			line(l)
		}
	}
	if qs := m.queueOf(c); len(qs.items) > 0 {
		// Full width, as what you sent: only what runs on its own sits
		// beside the conversation.
		block()
		var rows []string
		line := func(txt string) { rows = append(rows, txt) }
		w := w - 1
		q := qs.items
		when := dim(" · sends when this turn ends")
		if sq, sa := m.subQueue(c); sq != nil {
			when = dim(" · for " + sa.Type + ", one after each step")
		} else if c.client == nil {
			when = dim(" · sends within 15s, or when it's idle")
		}
		if lq := m.localQ[c.key]; c.client == nil && lq != nil && time.Now().Before(lq.retry) {
			when = paint(cYellow, " · send failed, trying again in "+dur(time.Until(lq.retry).Round(time.Second)))
		}
		if qs.held {
			// Never silently stuck: say it's held and how it goes.
			when = paint(cYellow, " · held") + dim(" · "+m.sendNowKey()+" sends it now")
		}
		pick, picked := queueSel(c, len(q))
		var how string
		title := " " + paint(cQueue+bold, "queue") + paint(cFaint, fmt.Sprintf(" ·%d", len(q)))
		// Hovering a row only lights it: the header stays put, and a
		// click picks it, which is when its own keys show.
		if m.paneFocus && !picked && len(c.input) == 0 {
			how = keys("↑", "edit, reorder or steer", m.sendNowKey(), "send now") + "  "
		}
		// One queued: it shares the title's row.
		single := len(q) == 1
		if !single {
			line(spread(title+when, how, w))
		}
		// Three at a time, keeping the picked one in sight.
		start := 0
		if picked && pick >= 3 {
			start = pick - 2
		}
		if start > 0 {
			line(dim(fmt.Sprintf("  … %d before", start)))
		}
		var qi [][]string
		if c.client != nil {
			qi = c.sess.Info.QueueImages
		}
		for i := start; i < min(len(q), start+3); i++ {
			// Its images, as chips where their markers are, as the box has
			// them; any without a marker after its text.
			var imgs []string
			if i < len(qi) {
				imgs = qi[i]
			}
			prefix := ""
			if !qs.local && i < len(c.sess.Info.QueueExchanges) {
				if e := c.sess.Info.QueueExchanges[i]; e != nil {
					name := firstNonEmpty(e.Sender.Name, e.Sender.SessionID, "agent")
					prefix = "← " + name + " · " + harnessName(e.Sender.Kind) + ": "
				}
			}
			text, rest := queueChips(prefix+shortImages(oneLine(q[i])), imgs)
			var pics strings.Builder
			for _, p := range rest {
				pics.WriteString(" " + queueChip(p))
			}
			text = cellw.Truncate(text, max(10, w-8-ansi.StringWidth(pics.String())), "…")
			body := paint(cText, text)
			switch {
			case host.IsCommand(q[i]):
				// A command runs alone, as a command, not words.
				body = paint(cQueue, "⌘ ") + paint(cGreen+bold, text) + dim("  "+commandAbout(c, q[i]))
			case i > 0 && host.IsCommand(q[i-1]):
				body = faint("once it's done ") + body
			}
			row := cellw.Truncate("  "+paint(cSub, strconv.Itoa(i+1))+"  "+body+pics.String(), w, "…")
			if single {
				right := firstNonEmpty(how, strings.Replace(when, " · ", "", 1)+"  ")
				if w-cellw.String(right) < 40 {
					right = ""
				}
				row = spread(cellw.Truncate(title+"  "+body+pics.String(), w-cellw.String(right)-2, "…"), right, w)
			}
			switch {
			case picked && i == pick:
				row = picked1(row, w, m.paneFocus)
			case i == c.qHover-1:
				row = hoverBG + strings.ReplaceAll(fit(row, w), reset, reset+hoverBG) + reset
			}
			if c.qAt == nil {
				c.qAt = map[int]int{}
			}
			c.qAt[len(rows)] = i
			line(row)
		}
		if rest := len(q) - (start + 3); rest > 0 {
			line(dim(fmt.Sprintf("  + %d more", rest)))
		}
		c.qTop = len(out)
		for row, qi := range c.qAt {
			c.panelRefs[c.qTop+row] = fmt.Sprintf("q:%d", qi)
		}
		out = append(out, dockCard(bgQueue, cQueue, rows, w+1)...)
	}
	cards()
	// And room before the box. In a conversation one row scrolls with the
	// transcript, so it never ends flush against the box, and one stays put,
	// so scrolled up it still never runs into what's stacked on the box.
	pad := min(blocks, 1)
	if m.viewName(c) == "conversation" && !m.zen {
		pad = 2
	}
	for range pad {
		room()
	}
	c.auxRows = len(out)
	if scrolls {
		c.auxRows-- // the last row of room stays put with the box
	}
	if !m.minimapEnabled(c, fullW) {
		w = fullW // with the map, the box and its keys stop where it starts: it runs the pane's height
	}
	top := ""
	addTop := func(note string) {
		if top != "" {
			top += dim(" · ")
		}
		top += note
	}
	if sa, ok := m.relaySub(c); ok {
		top = dim("to the subagent ") + paint(cText, cellw.Truncate(oneLine(sa.Type), 28, "…"))
		if c.sess.Info.Inbox {
			top += dim(", straight, after the step it's on")
		} else {
			// The main session passes it on with SendMessage.
			top += dim(", passed on by the main session")
		}
	} else if sa, ok := m.childSub(c); ok {
		top = dim("to the subagent ") + paint(cText, cellw.Truncate(oneLine(sa.Type), 28, "…")) + dim(", passed on by the main session")
	} else if m.watchingSub(c) {
		// A finished subagent takes no messages; the main session does.
		top = dim("to the main session, ") + paint(cText, cellw.Truncate(oneLine(a.DisplayName), 28, "…")) + dim(", not the subagent")
	}
	switch {
	case typingHash(string(c.input)):
		top = dim("rush command · enter runs it")
	case c.client == nil && a.Interactive:
		addTop(dim(a.Where() + ", so it can't take messages here"))
	case c.client == nil && a.Past:
		addTop(paint(cOrange, "enter resumes it") + dim(" in rush mode with your message"))
	case c.client == nil && a.Rush:
		addTop(dim("stopped; ") + paint(cOrange, "enter resumes it") + dim(" with your message"))
	case c.client == nil && busy(a):
		addTop(paint(cDim, "queuing"))
		if len(c.input) > 0 {
			addTop(dim(m.sendNowKey() + " sends it now"))
		}
	case c.client == nil:
		addTop(dim("enter replies through Claude Code"))
	case isQuestion(s.Pending()):
		addTop(paint(cYellow, "pick above, or type your own answer · enter sends it"))
	case len(s.Pending()) > 0:
		addTop(paint(cYellow, "y · a · n answer the card · or type why not, enter denies with it"))
	case s.Live() != nil:
		mode := m.sendModeTop(c)
		if top == "" {
			top = strings.TrimPrefix(mode, dim(" · "))
		} else {
			top += mode
		}
	}
	bt := m.btwFor(c.key)
	btwOn := bt != nil && bt.focused
	typing := m.paneFocus && c.sel == "" && !c.cardFocus && !btwOn
	switch {
	case m.paneFocus && btwOn:
		top = dim("asking on the side, above · esc returns here")
	case m.paneFocus && c.cardFocus && m.cardModal(c):
		top = dim("answering the card above · esc sets it aside for later")
	case m.paneFocus && c.cardFocus:
		top = dim("answering the card above · esc returns here")
	case m.paneFocus && c.memEdit:
		top = dim("editing " + filepath.Base(c.memEd.path) + " above · esc returns here")
	case m.paneFocus && !typing:
		top = dim("typing returns here · ↓ past the last row or esc")
	}
	b := box{w: w, focused: typing, topL: top, text: c.input, cursor: max(0, len(c.input)-c.back), anchor: c.anchor - 1,
		lead: paint(cOrange, "❯ "), holder: "a message for this agent" + m.historyHint(), maxRows: 6}
	if m.chipHot.box == 1 {
		b.hot = m.chipHot.at
	}
	b = b.named(c.imgs)
	if mode := s.Info.PermissionMode; mode != "" {
		b.topR = paint(cOrange, mode)
	}
	b.footR = m.boxNote(c.key)
	out = append(out, m.slashLines(c, w)...)
	if c.editQ > 0 {
		b.topL = paint(cOrange, fmt.Sprintf("editing queued message %d", c.editQ)) + dim(" · enter saves it back · esc cancels")
	}
	if n := c.stopNote; n != nil {
		what := "stops it"
		if n.key == "k" {
			what = "kills that command"
		}
		b.topL = paint(cOrange, "why? told to the agent") + dim(" · enter "+what+" and sends this · esc cancels")
	}
	tasks := taskRow(s, w, c.tasksShut)
	c.taskRows, c.taskW = len(tasks), w
	out = append(out, tasks...)
	b.top = c.box.top
	b = b.scrolled()
	c.box, c.boxIdx = b, len(out)
	out = append(out, b.lines()...)
	// Keys for what can be done now, the box's own first: what enter and
	// sending now do, how it sends while it works, then where what you
	// kept or cleared went. keysFit drops from the end, bar the way back.
	pairs := m.boxKeys(c, a, s)
	if len(s.Turns) > 0 {
		step := "steps"
		if c.client != nil && agent.Supports(sessionAgent(c), agent.FeatureRewind) {
			step = "rewind"
		}
		pairs = append(pairs, "↑", step)
	}
	back := []string{"esc", "back"}
	switch {
	case m.hostedAlone():
		back = []string{"ctrl+6", "Agents", "esc", "leave the box"}
	case m.store.Config.View == "agent" && m.chatAlone() && !m.zen:
		back = []string{"esc", "peek at Agents"}
	}
	hint := fewKeys(w-4, 3, pairs, append([]string{m.boundKey("guide.open"), "more"}, back...)) // the way back goes last: keysFit keeps it
	if m.watchingSub(c) {
		back := "back to the list"
		if c.subBack || m.hostedAlone() {
			back = "back to the conversation"
		}
		hint = keysFit(w-4, "enter", "send to the main session", "esc · ←", back, "shift+↑↓", "other runs", "↑", "pick a step", "ctrl+f", "find in chat", "ctrl+o", "show all")
		if _, ok := m.relaySub(c); ok {
			hint = keysFit(w-4, "enter", "send to the subagent", "ctrl+x", "stop this subagent", "esc · ←", back, "shift+↑↓", "other runs", "↑", "pick a step", "ctrl+f", "find in chat")
		} else if _, live, _ := m.pickedSub(c); live {
			hint = keysFit(w-4, "enter", "send to the main session", "ctrl+x", "stop this subagent", "esc · ←", back, "shift+↑↓", "other runs", "↑", "pick a step", "ctrl+f", "find in chat")
		}
	}
	if c.sel != "" {
		hint = keysFit(w-4, m.rowKey(), "open or close", "↑↓", "pick a step", "esc", "back to message", "ctrl+o", "show all")
		if strings.Contains(c.sel, ":s:") {
			hint = keysFit(w-4, m.rowKey(), "open or close", "w", "wrap lines or not", "↑↓", "pick a step", "esc", "back to message")
			if c.sess.NextView(c.sel, c.looks[c.sel]) != "" {
				hint = keysFit(w-4, "v", "text · pretty · hex", "w", "wrap lines or not", m.rowKey(), "open or close", "↑↓", "pick a step", "esc", "back to message")
			}
		}
		if pickedLink(c) != "" {
			hint = keysFit(w-4, "o", "open the file", m.rowKey(), "open or close", "↑↓", "pick a step", "esc", "back to message")
		}
		if selTurn(c) != nil {
			hint = keysFit(w-4, m.rowKey(), "open or close", m.boundKey("session.history.open"), "open all", m.boundKey("session.history.close"), "close older", "esc", "back to message", m.boundKey("session.rewind"), "rewind", m.boundKey("session.fork"), "fork")
		}
		// A running command picked in the conversation: x stops it.
		if id := m.pickedShell(c); id != "" && strings.Contains(c.sel, ":s:") {
			if rp, ok := c.sess.RunningPart(id); ok {
				hint = keysFit(w-4, "x", "stop this command", "k", "kill "+firstWord(rp.Command)+" only", "p", pauseWord(id), "shift+x · shift+k", "…and say why", "b", "background", m.rowKey(), "open or close", "esc", "back to message")
			} else {
				hint = keysFit(w-4, "x", "stop this command", "shift+x", "…and say why", "b", "background", m.rowKey(), "open or close", "esc", "back to message")
			}
		}
		if strings.HasPrefix(c.sel, "run:") {
			hint = keysFit(w-4, "space", "watch this subagent", "x", "stop it", "shift+x", "…and say why", "↑↓", "pick", "esc", "back to message")
		}
		if _, live, ok := m.pickedSub(c); ok && live && strings.HasPrefix(c.sel, "sub:") {
			hint = keysFit(w-4, "space", "watch it", "x", "stop it", "shift+x", "…and say why", "↑↓", "pick", "esc", "back to message")
		}
		if m.viewName(c) == "background" && strings.HasPrefix(c.sel, "job:") {
			hint = keysFit(w-4, "space", "show output", "d", "launch details", "↑↓", "pick", "esc", "back to message")
		}
		if _, ok := convo.ExchangePeer(c.sel); ok {
			hint = keysFit(w-4, "space", "expand message", "→", "open peer session", "↑↓", "pick", "esc", "back to message")
		}
		if c.sel == "go:memory" {
			hint = keysFit(w-4, "space", "open the Memory tab", "↑↓", "pick", "esc", "back to message")
		}
		if strings.HasPrefix(c.sel, "mem:") {
			hint = keysFit(w-4, "space", "edit it here", "ctrl+g", "open in $EDITOR", "x", "delete", "↑↓", "pick", "esc", "back to message")
		}
		if c.memEdit {
			hint = m.docHint(c.memEd, w-4)
		}
		if changedFile(c.sel) != "" && m.viewName(c) == "changes" {
			hint = keysFit(w-4, m.boundKey("session.full"), "the file in full", m.rowKey(), "open or close", "↑↓", "pick", "esc", "back to message")
		}
		if m.fullFile(c) {
			hint = keysFit(w-4, "↑↓ · pgup pgdn", "scroll", "esc", "back to the changes")
		}
	} else if m.viewName(c) == "memory" && len(c.input) == 0 {
		hint = keysFit(w-4, "↑↓", "pick a file", "[ ]", "views")
	}
	if qs := m.queueOf(c); len(qs.items) > 0 {
		if _, ok := queueSel(c, len(qs.items)); ok {
			hint = queueHint(qs, w-4)
		}
	}
	if !m.paneFocus {
		hint = keysFit(w-4, "enter · →", "type here", "ctrl+n", "next needing you")
		if m.hostedAlone() {
			hint = keysFit(w-4, "enter · →", "type here", "esc", "stop the turn", "ctrl+q", "quit")
			if c.client == nil || c.sess.Live() == nil || !agent.Supports(sessionAgent(c), agent.FeatureInterrupt) {
				hint = keysFit(w-4, "enter · →", "type here", "ctrl+q", "quit")
			}
		}
		b.holder = "enter or → to talk to this agent"
		out = append(out[:len(out)-len(b.lines())], b.lines()...)
	}
	if m.zenFull() {
		return out
	}
	return append(out, "  "+hint)
}

func approvalTitle(st *convo.Step) string {
	switch st.Call().Kind {
	case tool.Shell:
		return "run a command"
	case tool.Edit, tool.Write, tool.Notebook:
		return "change a file"
	case tool.Fetch, tool.WebSearch:
		return "go online"
	case tool.Question:
		return "answer a question"
	}
	return "use " + st.Tool
}

// approvalRows is as much of a command as a permission card shows.
const approvalRows = 6

func approvalBody(st *convo.Step, cwd string, w int) []string {
	c := st.Call()
	in := c.Input
	rel := func(p string) string {
		for _, base := range []string{cwd, "/private" + cwd} {
			if base != "" && strings.HasPrefix(p, base+"/") {
				return strings.TrimPrefix(p, base+"/")
			}
		}
		return tildify(p)
	}
	path := in.Path
	if c.Kind == tool.Search || c.Kind == tool.Glob {
		path = "" // where it looks, not what it's about
	}
	var out []string
	switch {
	case in.Command != "":
		// All of it, wrapped, so you see what you allow; past a screenful,
		// the step's ctrl+o has the rest.
		home, _ := os.UserHomeDir()
		cmd := in.Command
		if home != "" {
			cmd = strings.ReplaceAll(cmd, home+"/", "~/")
		}
		rows := wrap(cmd, w-4)
		for i, l := range rows {
			if i == approvalRows {
				out = append(out, dim(fmt.Sprintf("  … %d more lines · ctrl+o shows all", len(rows)-i)))
				break
			}
			lead := "  "
			if i == 0 {
				lead = paint(cBright, "$ ")
			}
			out = append(out, lead+paint(cText, l))
		}
	case path != "":
		out = append(out, paint(cText, rel(path)))
		// An edit shows what it changes, so you can judge it here.
		add := func(prefix, col, text string, limit int) {
			ls := strings.Split(strings.TrimRight(text, "\n"), "\n")
			for i, l := range ls {
				if i >= limit {
					out = append(out, dim(fmt.Sprintf("  … %d more", len(ls)-i)))
					return
				}
				out = append(out, paint(col, prefix+" ")+paint(cText, cellw.Truncate(strings.ReplaceAll(l, "\t", "  "), w-4, "…")))
			}
		}
		if len(in.Edits) > 0 && in.Edits[0].Old != "" {
			add("−", cRed, in.Edits[0].Old, 4)
			add("+", cGreen, in.Edits[0].New, 4)
		} else if in.Content != "" {
			add("+", cGreen, in.Content, 4)
		}
	case in.URL != "":
		out = append(out, paint(cText, in.URL))
	case in.Query != "":
		out = append(out, paint(cText, in.Query))
	}
	why := ""
	if st.Approval != nil {
		why = oneLine(ansi.Strip(st.Approval.Reason))
	}
	if why != "" && !strings.Contains(strings.Join(out, " "), why) && !strings.HasSuffix(path, why) {
		out = append(out, dim(cellw.Truncate(why, w, "…")))
	}
	return out
}

// --- keys ---

// paneKey handles a key while a rush-mode session's pane has focus. The
// prompt is always live, so letters type; actions are chords, arrows on an
// empty prompt, and the approval card's letters when the prompt is empty.
func (m *Model) paneKey(k tea.KeyPressMsg, s string) tea.Cmd {
	c := m.host
	if c == nil {
		m.paneFocus = false
		return nil
	}
	defer func() { c.lastKeyAt = time.Now() }()
	if cmd, ok := m.roomPaneKey(c, s); ok {
		return cmd
	}
	if c.cardFocus && s != "pgup" && s != "pgdown" && s != "esc" {
		c.scroll = 0
	}
	// A card's modal has the keys: nothing behind it gets them, bar
	// ctrl+x, which stops the turn (and so the card).
	if m.cardModal(c) && c.cardFocus && s != "ctrl+x" {
		if c.cardGuarded() && s != "esc" {
			return nil
		}
		cmd, _ := m.cardKey(c, s, len(c.input) == 0)
		return cmd
	}
	// ctrl+b backgrounds what the turn waits on, while the dock offers it.
	if t := m.btwFor(c.key); s == "ctrl+b" && (t == nil || !t.focused) {
		if cmd, used := m.jobKey(c, s, len(c.input) == 0); used {
			return cmd
		}
	}
	// The side thread (/btw) takes the keys while it has them.
	if cmd, used := m.btwKey(c, k, s); used {
		return cmd
	}
	switch s {
	case "ctrl+home", "ctrl+end":
		m.jumpUserMessage(c, s == "ctrl+end")
		return nil
	case "alt+enter":
		return m.openUserMessage(c, "")
	}
	empty := len(c.input) == 0
	// A selected row owns navigation, even when a draft remains in the box:
	// enter opens or closes the row, and nothing sends that draft until
	// focus returns to it.
	formFocus := c.cardFocus && cardKind(c) != "" || c.memEdit && m.viewName(c) == "memory"
	if c.sel != "" && !formFocus && (s == "ctrl+enter" || s == "ctrl+s" || s == m.notRowKey()) {
		m.flash(m.rowKey()+" opens or closes the row · esc returns to your message", false)
		return nil
	}
	if c.sel != "" && s == "esc" && !formFocus && !m.fullFile(c) {
		c.sel, c.subSel = "", ""
		m.escAt = time.Time{}
		return nil
	}
	if s == "alt+c" && empty {
		if text, ok := c.sess.ExchangeText(c.sel); ok {
			m.copyText(text)
			return nil
		}
		// With nothing typed, alt+c copies the drawing you've picked.
		if i := strings.LastIndex(c.sel, ":s:"); i >= 0 {
			if rows := convo.Drawing(c.sess.Step(c.sel[i+3:])); rows != nil {
				m.copyText(strings.Join(rows, "\n"))
				return nil
			}
		}
		// On a turn, it copies Claude's answer as written, markdown and all.
		if t, _, _ := strings.Cut(c.sel, ":"); isTurnRef(t) {
			for _, turn := range c.sess.Turns {
				if "t"+strconv.Itoa(turn.N) == t {
					if a := turn.Answer(); a != "" {
						m.copyText(strings.TrimSpace(a))
						return nil
					}
				}
			}
		}
	}
	if (s == "alt+r" || s == "alt+f") && empty {
		// On a turn in the history: rewind back to it, or fork from it.
		if t := selTurn(c); t != nil {
			if a := m.agentByKey(c.key); a != nil {
				if s == "alt+f" {
					m.openForkAt(c, a, t)
					return nil
				}
				return m.openRewindTo(c, a, t)
			}
		}
	}
	if cmd, used := m.slashKey(c, s); used {
		return cmd
	}
	if cmd, used := m.verdictKey(c, s, empty); used {
		return cmd // what waits on you goes first
	}
	if cmd, used := m.queueKey(c, s); used {
		return cmd
	}
	if cmd, used := m.stopNoteKey(c, s, empty); used {
		return cmd
	}
	if cmd, used := m.jobKey(c, s, empty || c.sel != "" && (s == "space" || s == "right")); used {
		return cmd
	}
	// ctrl+x on a subagent, picked or watched, stops that one alone; x
	// does too on its row.
	if s == "ctrl+x" || s == "x" && empty && !m.watchingSub(c) {
		if sa, live, ok := m.pickedSub(c); ok && (live || s == "x") {
			return m.stopSub(c, sa, live)
		}
	}
	// v on a picked step shows its output another way: text, pretty, hex.
	if s == "v" && empty && strings.Contains(c.sel, ":s:") {
		if v := c.sess.NextView(c.sel, c.looks[c.sel]); v != "" {
			if c.looks == nil {
				c.looks = map[string]string{}
			}
			c.looks[c.sel], c.open[c.sel] = v, true
			return nil
		}
	}
	// w on a picked step keeps each line of its output to one row, or
	// wraps it again.
	if s == "w" && empty && strings.Contains(c.sel, ":s:") {
		k := convo.NoWrapRef(c.sel)
		c.open[k], c.open[c.sel] = !c.open[k], true
		return nil
	}
	// o on a picked step asks what to do with its file, as a click on it.
	if s == "o" && empty && c.sel != "" {
		if u := pickedLink(c); u != "" && m.openLinkMenu(u) {
			return m.drawShot()
		}
	}
	if cmd, used := m.memoryKey(c, k, s); used {
		return cmd
	}
	if s == "esc" && c.editQ > 0 {
		c.input, c.back = c.input[:0], 0
		return m.endQueueEdit(c)
	}
	if cmd, used := m.cardKey(c, s, empty); used {
		return cmd
	}
	if s == "ctrl+v" {
		return m.pasteClipImage()
	}
	switch s {
	case "esc":
		if empty && m.viewName(c) == "stack" {
			c.view, c.scroll, c.scrollOnly = 0, 0, false
			return nil
		}
		if empty && m.escKey == c.key && time.Since(m.escAt) < time.Second {
			m.escAt = time.Time{}
			return m.askClose(m.focused())
		}
		if empty {
			m.escAt, m.escKey = time.Now(), c.key
		}
		switch {
		case !empty:
			m.wipeBox(c)
		case c.txt.on:
			c.txt = textSel{} // first esc drops the dragged-over text
		case m.fullFile(c):
			c.full, c.selMoved = "", true // back to the file, picked, in the list
		case c.sel != "":
			c.sel, c.subSel = "", "" // first esc drops the step selection
		case m.watchingSub(c):
			m.closeSub(c)
		case canInterrupt(c):
			return m.stopTurn(c)
		case m.zen:
			// Zen keeps the keys on the agent; tab or ctrl+z leaves zen.
		case m.hostedAlone():
			// Alone, there's no list to go to: esc takes the keys off the
			// box, and esc again stops the turn. Only ctrl+q quits.
			m.paneFocus, m.hostedAway = false, true
		default:
			m.leavePane()
		}
		return nil
	case "super+c":
		// cmd+c, when the terminal hands it over: the text dragged over in
		// the conversation, unless the box has a selection of its own,
		// which the editor below copies.
		if c.txt.on && (c.anchor == 0 || c.anchor-1 == len(c.input)-c.back) {
			m.copyText(selectedText(c.shown, c.txt.a, c.txt.b, c.paneW))
			return nil
		}
	case "ctrl+c":
		switch {
		case c.anchor > 0 && c.anchor-1 != len(c.input)-c.back:
			// a selection: copy it (handled by the editor below)
		case empty && c.txt.on:
			// Text dragged over in the conversation: copy it again, as a
			// terminal would, rather than quitting.
			m.copyText(selectedText(c.shown, c.txt.a, c.txt.b, c.paneW))
			return nil
		case !empty:
			m.wipeBox(c)
			return nil
		default:
			return m.quitKey()
		}
	case "enter", "space", "right":
		// enter sends what's typed; with a row picked, it opens or closes
		// it, as space does.
		if s == "enter" && c.sel == "" {
			if empty {
				return nil
			}
			if m.sendModeOf(c) == sendDiscuss {
				return m.discussTyped(c)
			}
			return m.sendPane(c, false)
		}
		if s == "enter" && m.openDockRoom(c.sel) {
			c.sel = ""
			return m.loadRooms()
		}
		if s == "right" && empty && m.viewName(c) == "screen" && m.canEmbed() {
			m.embedded = true
			return nil
		}
		if c.sel == "" {
			break
		}
		if peer, ok := convo.ExchangePeer(c.sel); ok && s == "right" {
			if peer != "" && m.snap != nil {
				for _, target := range m.allAgents() {
					if target.ID == peer || target.SessionID == peer || target.Key == peer {
						return m.goAgent(target)
					}
				}
			}
			m.flash("The peer session is not available in this agent list", false)
			return nil
		}
		if step, ok := strings.CutPrefix(c.sel, "jump:"); ok && s == "right" && step != "" {
			// From a hunk to the step that made it, in the conversation.
			turn, _, _ := strings.Cut(step, ":")
			c.open[turn], c.open[step] = true, true
			if p := c.sess.ParentRef(step); p != "" {
				c.open[p] = true
			}
			c.view, c.sel, c.selMoved = 0, step, true
			return nil
		}
		if url, ok := strings.CutPrefix(c.sel, "art:"); ok && m.viewName(c) == "overview" {
			return browse(url)
		}
		if c.sel == "go:memory" {
			c.sel = ""
			m.showView(c, "memory")
			return nil
		}
		if m.viewName(c) == "subagents" && strings.HasPrefix(c.sel, "sub:") && c.subOpen == "" {
			m.openSub(c, strings.TrimPrefix(c.sel, "sub:"))
			return nil
		}
		// Right/Space on a running subagent in the dock watches it.
		if id, ok := strings.CutPrefix(c.sel, "run:"); ok {
			for i, v := range m.views(c) {
				if v == "subagents" {
					c.view = i
				}
			}
			m.openSub(c, id)
			c.subBack = true
			return nil
		}
		// Right/Space on a subagent's step opens its own conversation.
		if s == "right" && m.viewName(c) == "conversation" {
			if _, id, ok := strings.Cut(c.sel, ":s:"); ok {
				for _, sa := range c.subs {
					if c.subOfStep(sa, id) {
						for i, v := range m.views(c) {
							if v == "subagents" {
								c.view = i
							}
						}
						m.openSub(c, sa.ID)
						return nil
					}
				}
			}
		}
		if strings.Contains(c.sel, ":u:") {
			return m.openUserMessage(c, c.sel)
		}
		c.keepRow(c.sel)
		if s == "right" {
			c.open[c.sel] = true
			return nil
		}
		c.open[c.sel] = !m.isOpen(c, c.sel)
		return nil
	case "ctrl+t":
		m.cycleSendMode(c)
		return nil
	case "ctrl+enter", "ctrl+s":
		// Now, whatever's waiting: the queue, then what's in the box. ctrl+s
		// is for terminals that never pass ctrl+enter on: macOS's Terminal
		// takes it to open its own context menu.
		if !empty {
			return m.sendPane(c, true)
		}
		if len(m.queueOf(c).items) > 0 {
			return m.sendQueueNow(c, "")
		}
	case "up", "down":
		if (empty || c.sel != "") && m.fullFile(c) {
			// Nothing to pick in a file: the arrows scroll it.
			c.scroll = max(0, c.scroll+map[string]int{"up": 1, "down": -1}[s])
			return nil
		}
		if empty || c.sel != "" {
			m.moveSel(c, map[string]int{"up": -1, "down": 1}[s])
			return nil
		}
		m.boxVert(c, map[string]int{"up": -1, "down": 1}[s], false)
		return nil
	case "shift+up", "shift+down":
		if !empty {
			m.boxVert(c, map[string]int{"shift+up": -1, "shift+down": 1}[s], true)
			return nil
		}
		m.cycleSub(c, map[string]int{"shift+up": -1, "shift+down": 1}[s])
		return nil
	case "ctrl+p":
		return m.stashCommand("stash")
	case "ctrl+r":
		return m.stashCommand("history")
	case "left":
		if c.sel != "" && !empty {
			c.sel, c.subSel = "", ""
			return nil
		}
		if !empty && len(c.input) == c.back && !m.zen && !m.hostedAlone() && m.edgePush("pane-left") {
			m.confirm = &confirmation{
				question: "Back to the list?",
				detail:   "what's typed stays in this session's box",
				onYes:    func() tea.Cmd { m.leavePane(); return nil },
				again:    "left",
			}
			return nil
		}
		// ← only ever means back: out of an opened subagent, off a
		// selected row, then to Agents. It never folds anything.
		if empty {
			switch {
			case m.watchingSub(c):
				m.closeSub(c)
			case c.sel != "":
				c.sel = ""
			case m.zen:
				// Zen keeps the keys on the agent; tab or ctrl+z leaves zen.
			default:
				m.leavePane()
			}
			return nil
		}
	case "ctrl+d", "alt+d":
		// Done with this agent: to Done, its idle process stopped.
		return m.markDone(m.agentByKey(c.key))
	case "alt+h":
		// Hold or release the queue from any view, as the dock says.
		c.editHeld = false // yours now, not the edit's
		return m.holdQueue(c, !m.queueOf(c).held)
	case "alt+r":
		// Mark a file reviewed in the changes view, or unmark it.
		if path, ok := strings.CutPrefix(c.sel, "chg:"); ok && m.viewName(c) == "changes" {
			if c.marks == nil {
				c.marks = map[string]bool{}
			}
			c.marks[path] = !c.marks[path]
			return nil
		}
	case "alt+v":
		// The picked changed file in full, its diff laid over it.
		if path := changedFile(c.sel); path != "" && m.viewName(c) == "changes" {
			c.full = path
			return nil
		}
	case "alt+down", "alt+up":
		// Step through the subagent runs in place, like Claude Code's
		// switcher; the main conversation sits at either end.
		d := 1
		if s == "alt+up" {
			d = -1
		}
		m.cycleSub(c, d)
		return nil
	case "{", "}":
		if empty {
			return m.switchFocus()
		}
	case "[", "]":
		if empty {
			d, n := 1, len(slices.DeleteFunc(m.views(c), func(v string) bool { return v == "stack" }))
			if s == "[" {
				d = -1
			}
			c.view = (c.view%n + d + n) % n
			c.scroll = 0
			return nil
		}
	case "ctrl+o":
		c.verbose = !c.verbose
		return nil
	case "pgup":
		c.scroll, c.scrollOnly = c.scroll+10, true
		return nil
	case "pgdown":
		c.scroll, c.scrollOnly = max(0, c.scroll-10), true
		return nil
	case "end":
		if empty {
			c.scroll, c.scrollOnly = 0, true
			return nil
		}
	case "home":
		if empty { // the top: only the turns there are drawn
			c.scroll, c.scrollOnly, c.selMoved = c.shownTotal, true, false
			return nil
		}
	case "ctrl+l":
		// Where the agent works: the same folders as #cd with no path.
		if a := m.focused(); a != nil {
			return m.openMovePicker(a)
		}
		return nil
	case "ctrl+x":
		return m.dismiss(m.focused())
	case "shift+tab":
		return m.openSwitchSheet(c)
	case "ctrl+n":
		if m.zen {
			m.zenSkip()
			return nil
		}
		m.paneFocus = false
		return m.nextNeedingYou()
	case "shift+left", "shift+right", "alt+left", "alt+right":
		if empty && m.watchingSub(c) {
			m.cycleSub(c, map[bool]int{false: -1, true: 1}[strings.HasSuffix(s, "right")])
			return nil
		}
		if empty {
			if cmd, ok := m.stepSplit(strings.HasSuffix(s, "right")); ok {
				return cmd
			}
		}
	}
	if k.Text != "" && c.sel != "" {
		c.sel = "" // typing returns to the box: nothing stays highlighted
	}
	if s == "ctrl+g" {
		return editDraft(&c.pastes, c.input, true)
	}
	if m.undoKey(c, s) {
		return nil
	}
	was, wasBack := c.input, c.back
	buf, pos, anchor, copied, _ := editChips(c.input, max(0, len(c.input)-c.back), c.anchor-1, k, s)
	if !slices.Equal(was, buf) {
		// A run of letters typed is one step to undo; a space, a delete or
		// anything else starts the next.
		c.undo.save(was, wasBack, k.Text != "" && k.Text != " " && anchor < 0 && c.anchor == 0)
	}
	c.input, c.back, c.anchor = buf, len(buf)-pos, anchor+1
	if s == "space" || s == "enter" {
		// A path you typed to an image becomes the image once it's done,
		// where it was typed.
		if t, ok := c.imgs.inline(string(c.input), m.lookPath); ok {
			c.input = []rune(t)
			c.back = min(c.back, len(c.input))
		}
	}
	if copied != "" {
		m.copyText(copied)
	}
	return nil
}

// clickBox places the cursor where a click lands inside either input box,
// and gives that box the keys. It reports whether the click was in one.
func (m *Model) clickBox(x, y int) bool {
	if m.inKind == inPrompt && m.shelfClick(x, y) {
		return true
	}
	if h := m.chipUnder(x, y); h.box != 0 && m.openChip(h) {
		return true
	}
	which, pos := m.boxAt(x, y)
	switch c := m.host; {
	case which == 1 && c != nil:
		m.paneFocus = true
		c.sel, c.subSel, c.cardFocus = "", "", false
		if sp, ok := chipOn(c.input, pos); ok {
			c.back, c.anchor = len(c.input)-sp.to, sp.from+1 // a click on a chip selects it
			return true
		}
		c.back, c.anchor = len(c.input)-pos, pos+1 // a drag from here selects
		if from, to := boxWord(c.input, pos, m.dbl); from < to {
			c.back, c.anchor = len(c.input)-to, from+1
		}
		m.boxDrag = 1
	case which == 2:
		m.paneFocus, m.embedded = false, false
		if sp, ok := chipOn(m.input, pos); ok {
			m.setCursor(sp.to)
			m.anchor = sp.from + 1
			return true
		}
		m.setCursor(pos)
		m.anchor = pos + 1 // a drag from here selects
		if from, to := boxWord(m.input, pos, m.dbl); from < to {
			m.setCursor(to)
			m.anchor = from + 1
		}
		m.boxDrag = 2
	default:
		return false
	}
	return true
}

// boxWord is the word a double-click at pos in a box lands on: the one
// the cursor sits in or just after.
func boxWord(buf []rune, pos int, dbl bool) (int, int) {
	if !dbl {
		return 0, 0
	}
	if from, to := wordAt(buf, pos); from < to {
		return from, to
	}
	return wordAt(buf, pos-1)
}

// dragBox moves the cursor of the box a drag started in to the text under
// the pointer, so the text between it and where the drag began is selected.
// Past the box's edges it stops at the box's first or last row, and never
// takes in anything drawn outside it.
func (m *Model) dragBox(x, y int) {
	switch c := m.host; {
	case m.boxDrag == 1 && c != nil:
		x0 := 2
		if m.listW > 0 {
			x0 = m.listW + 3
		}
		pos := c.box.near(y-c.boxY-1, x-x0)
		c.back = len(c.input) - min(pos, len(c.input))
	case m.boxDrag == 2:
		m.setCursor(min(m.promptBox.near(y-m.promptBoxY-1, x), len(m.input)))
	}
}

// endBoxDrag finishes a drag in an input box: what it selected goes to the
// clipboard, as a terminal's own selection would, unless CopyOnSelect is
// off: then it stays selected.
func (m *Model) endBoxDrag() {
	var buf []rune
	var pos, anchor int
	switch c := m.host; {
	case m.boxDrag == 1 && c != nil:
		buf, pos, anchor = c.input, len(c.input)-c.back, c.anchor-1
	case m.boxDrag == 2:
		buf, pos, anchor = m.input, m.cursorPos(), m.anchor-1
	}
	drag := m.boxDrag
	m.boxDrag = 0
	if anchor >= 0 && anchor != pos && anchor <= len(buf) {
		if m.store.Config.CopiesOnSelect() {
			m.copyText(string(buf[min(anchor, pos):max(anchor, pos)]))
		}
		return
	}
	// A plain click leaves no selection behind.
	if drag == 1 && m.host != nil {
		m.host.anchor = 0
	} else if drag == 2 {
		m.anchor = 0
	}
}

func hostCmd(f func() error) tea.Cmd {
	return func() tea.Msg {
		if err := f(); err != nil {
			return doneMsg{err: err}
		}
		return nil
	}
}

func (m *Model) answerHost(c *hostConn, req *convo.Asking, allow, always bool) tea.Cmd {
	id := req.ID
	if allow {
		return hostCmd(func() error { return c.client.Allow(id, nil, always) })
	}
	return hostCmd(func() error { return c.client.Deny(id, "", false) })
}

// sendPane sends the prompt: now, or queued when the agent is busy (the
// host decides). A trailing backslash continues onto a new line instead.
func (m *Model) sendPane(c *hostConn, now bool) tea.Cmd {
	if isRoomKey(c.key) {
		return m.sendRoom(c)
	}
	if c.intercepting || c.waking {
		return nil
	}
	if c.sess != nil && c.sess.Info.Sleeping && !c.sleeping {
		m.parkHost(c)
	}
	c.rememberSleepDraft()
	working := c.sess != nil && c.sess.Live() != nil
	now = now || working && m.sendModeOf(c) == sendStop
	guide := !now && working && m.sendModeOf(c) == sendGuide
	if m.wantsIntercept(c, now) {
		return m.interceptSend(c, now)
	}
	text := string(c.input)
	if strings.HasSuffix(text, "\\") && !now {
		c.input = append(c.input[:len(c.input)-1], '\n')
		c.back = 0
		return nil
	}
	// Claude Code sessions are typed into, where tags would show as typed.
	text = strings.TrimSpace(c.pastes.expand(text, c.client != nil || m.agentByKey(c.key) == nil || m.agentByKey(c.key).Rush))
	// The pastes go only with the box's text: a send that stops to ask
	// first (a cold cache, an unknown command) runs again with them.
	defer func() {
		if len(c.input) == 0 {
			c.pastes = pastes{}
		}
	}()
	if c.editQ > 0 {
		i, was := c.editQ-1, c.editWas
		c.input, c.back = c.input[:0], 0
		if sq, _ := m.subQueue(c); c.client == nil || sq != nil {
			m.editLocal(c, i, was, text)
			return m.endQueueEdit(c)
		}
		if items := c.sess.Info.Queue; i < len(items) && items[i] == was {
			items[i] = text
		}
		// Saved before the queue is let go, so the old text never goes.
		cl, release := c.client, c.editHeld
		c.editQ, c.editHeld = 0, false
		if release {
			c.sess.Info.QueueHeld = false
		}
		return hostCmd(func() error {
			err := cl.EditQueued(i, was, text)
			if release {
				err = errors.Join(err, cl.HoldQueue(false))
			}
			return err
		})
	}
	if id := m.watchedHost(c); id != "" && text != "" {
		c.input, c.back = c.input[:0], 0
		return viaSub(c, convo.Subagent{ID: c.subOpen, Type: "the spawned agent"}, text, "in its own queue · it reads it when its turn ends", func() error {
			hc, err := host.Dial(id)
			if err != nil {
				return err
			}
			defer hc.Close()
			return hc.Send(text)
		})
	}
	if cmd, ok := m.sendMentioned(text, text); ok {
		c.input, c.back = c.input[:0], 0
		return cmd
	}
	if isHashCmd(text) {
		c.input, c.back = c.input[:0], 0
		return m.command(m.agentByKey(c.key), text)
	}
	if strings.HasPrefix(text, "/") {
		if cmd, ok := m.runRushCommand(c, text); ok {
			c.input, c.back = c.input[:0], 0
			return cmd
		}
		if !c.sendRaw && m.askUnknown(c, text, func() tea.Cmd {
			c.sendRaw = true
			defer func() { c.sendRaw = false }()
			return m.sendPane(c, now)
		}) {
			return nil
		}
	}
	if m.askCold(c, text, func() tea.Cmd { return m.sendPane(c, now) }) {
		return nil
	}
	// Each [Image #N] is its image, numbered again in the order they
	// appear; paths typed or dropped without a paste become images too,
	// at the end.
	text, images := c.imgs.resolve(text)
	if rest, imgs := extractImages(text, m.lookPath); imgs != nil {
		images, text = append(images, imgs...), rest
	}
	if why := agent.Unreadable(sessionAgent(c), argRunning(c, "model"), images); why != "" {
		m.flash(why, true)
		return nil
	}
	if sq, _ := m.subQueue(c); now && len(images) == 0 && c.client != nil && sq == nil && hasQueuedExchange(c) {
		m.flash("agent messages keep their sender · send queued messages separately with s", false)
		return nil
	}
	m.keepSent(c, text)
	text = m.withMentions(text, c.key)
	if sa, ok := m.childSub(c); ok && len(images) == 0 {
		// Codex takes a subagent's input only from its parent, with its tools.
		c.input, c.back, c.imgs = nil, 0, imageRefs{}
		c.undo = undoStack{}
		cl, prompt := c.client, childRelayPrompt(sa, text)
		return viaSub(c, sa, text, "the main session passes it on at its next step", func() error { return cl.SendGuide(prompt, nil) })
	}
	sa, relay := m.relaySub(c)
	if relay && len(images) == 0 && c.sess.Info.Inbox {
		// Into its own queue, or with ctrl+enter straight in at its next
		// tool call.
		c.input, c.back, c.imgs = nil, 0, imageRefs{}
		c.undo = undoStack{}
		q, _ := m.subQueue(c)
		if now {
			m.flash("sent to "+sa.Type+" · it reads it after the step it's on", false)
			return m.sendSubQueueNow(c, q, sa, text)
		}
		m.queueSub(c, q, sa, text)
		return nil
	}
	c.input, c.back, c.imgs = nil, 0, imageRefs{}
	c.undo = undoStack{}
	c.scroll = 0
	c.lastSend = time.Now()
	if a := m.focused(); a != nil {
		m.markSeen(a)
	}
	if relay && len(images) == 0 {
		// Into the main session's turn at its next step, not its queue:
		// one waiting on this subagent ends only once it has.
		cl, prompt := c.client, relayPrompt(sa, text)
		return viaSub(c, sa, text, "the main session passes it on at its next step", func() error { return cl.SendGuide(prompt, nil) })
	}
	if sa, live, ok := m.pickedSub(c); ok && m.watchingSub(c) {
		why := "went to the main session"
		switch {
		case !live:
			why += " · it has finished"
		case len(images) > 0:
			why += " · a subagent takes no images"
		}
		m.noteSub(c.key, sa.ID, subNote{text: text, how: why})
	}
	if now && len(images) == 0 && len(m.queueOf(c).items) > 0 {
		return m.sendQueueNow(c, text)
	}
	if c.client == nil {
		return m.sendOffline(c, text, images, now)
	}
	m.markSending(c, text)
	cl := c.client
	switch {
	case guide:
		return sendingVia(c.key, hostCmd(func() error { return cl.SendGuide(text, images) }))
	case len(images) > 0:
		return sendingVia(c.key, hostCmd(func() error { return cl.SendImages(text, images, now) }))
	case now:
		return sendingVia(c.key, hostCmd(func() error { return cl.SendNow(text) }))
	}
	return sendingVia(c.key, hostCmd(func() error { return cl.Send(text) }))
}

// coldMin is the context, in tokens, below which re-reading it uncached
// isn't worth asking about.
const coldMin = 40_000

// askCold asks before a message you send wakes a session whose prompt cache
// has expired, since it re-reads the whole context uncached; y runs send.
// It's only asked where you pressed send: queued messages, retries, a
// limit's reset and other programs' sends never ask.
func (m *Model) askCold(c *hostConn, text string, send func() tea.Cmd) bool {
	if c == nil || c.sess == nil || text == "/clear" {
		return false
	}
	at, cold := c.sess.CacheCold(time.Now())
	if !cold || at.Equal(c.coldOK) || c.sess.Context > 0 && c.sess.Context < coldMin {
		return false
	}
	if a := m.agentByKey(c.key); a != nil && busy(a) || c.sess.Info.State == "working" {
		return false // the turn under way keeps it warm
	}
	what := "the whole context"
	if c.sess.Context > 0 {
		what = convo.Tokens(c.sess.Context) + " tokens of context"
	}
	m.confirm = &confirmation{
		question: "Send to a cold cache?",
		detail:   fmt.Sprintf("its prompt cache expired %s ago · this re-reads %s uncached", dur(time.Since(at)), what),
		onYes: func() tea.Cmd {
			c.coldOK = at
			return send()
		},
	}
	return true
}

func (m *Model) isOpen(c *hostConn, ref string) bool {
	if v, ok := c.open[ref]; ok {
		return v
	}
	if strings.Contains(ref, ":s:") {
		return c.sess.StepOpen(ref, c.drewConvo && c.drawn.Verbose)
	}
	if _, ok := convo.ExchangePeer(ref); ok {
		return c.drewConvo && c.drawn.Verbose
	}

	// What the renderer opens by default: recent turns and failures. The
	// options the pane last drew with find it in the renderer's cache;
	// any others would redraw its turn, twice.
	o := convo.Options{Width: max(40, m.w/2), Now: time.Now(), Open: c.open, View: c.looks, History: m.historyOf(c)}
	if c.drewConvo {
		o = c.drawn
	}
	o.Budget = 0
	if !strings.Contains(ref, ":") && c.sess.TurnOf(ref) >= 0 {
		return c.sess.TurnOpen(ref, o) // a turn: settled reads as closed
	}
	rows := c.sess.RenderTurn(o, c.sess.TurnOf(ref))
	if rows == nil {
		rows = c.sess.Render(o) // not a turn's: look through them all
	}
	// Folded if any of its rows shows ▸ (a failed step's shows on the error
	// line under it).
	seen := false
	for _, l := range rows {
		if l.Ref == ref {
			seen = true
			if strings.Contains(ansi.Strip(l.Text), "▸") {
				return false
			}
		} else if seen {
			break
		}
	}
	return seen
}

// dockRefs are the rows ↑↓ go through between the conversation and the
// box, top to bottom as they're drawn: under the conversation the running
// subagents and the queue, then in any view the attachments. A card
// waiting sits above them all.
func (m *Model) dockRefs(c *hostConn) []string {
	var refs []string
	if m.viewName(c) == "conversation" {
		for _, sa := range c.dockSubs() {
			refs = append(refs, "run:"+sa.ID)
			for _, j := range c.jobsOf(sa.ID) {
				refs = append(refs, "job:"+j.ID)
			}
		}
		for _, j := range c.looseJobs() {
			refs = append(refs, "job:"+j.ID)
		}
		for i := range m.queueOf(c).items {
			refs = append(refs, fmt.Sprintf("q:%d", i))
		}
	}
	return refs
}

// moveSel moves the selection over rows you can act on: turns and steps,
// then what's between them and the box. ↑ from the box goes up through
// them nearest first; ↓ from it has nowhere to go.
func (m *Model) moveSel(c *hostConn, d int) {
	c.keepRunPick(c.runningSubs())
	refs := append(slices.Clip(c.bodyRefs), m.dockRefs(c)...)
	if len(refs) == 0 {
		return
	}
	cur := c.sel
	if c.subOpen != "" && m.viewName(c) == "subagents" {
		cur = c.subSel
	}
	if cur == "" && d > 0 {
		return
	}
	i := len(refs)
	for j, r := range refs {
		if r == cur {
			i = j
		}
	}
	if next := m.pastWindow(c, i, d); next != "" {
		c.sel, c.selMoved = next, true
		return
	}
	if i+d >= len(refs) && cur != "" {
		// ↓ past the last row: back to typing, nothing highlighted.
		c.sel, c.subSel = "", ""
		return
	}
	i = max(0, min(len(refs)-1, i+d))
	if c.subOpen != "" && m.viewName(c) == "subagents" {
		c.subSel = refs[i]
	}
	c.sel = refs[i]
	c.selMoved = true
	c.keepRunPick(c.runningSubs())
}

// pastWindow is the row ↑↓ go to from ref i of bodyRefs when it's at the
// edge of the turns drawn and more are beyond: the last of the turn before,
// or the first of the turn after. Empty when it's within them.
func (m *Model) pastWindow(c *hostConn, i, d int) string {
	if m.viewName(c) != "conversation" || !c.drewConvo || len(c.bodyRefs) == 0 {
		return ""
	}
	s := c.sess
	lo, hi := s.WindowTurns()
	switch {
	case d < 0 && i == 0 && lo > 0:
		last := ""
		for _, l := range s.RenderTurn(c.drawn, lo-1) {
			if l.Ref != "" {
				last = l.Ref
			}
		}
		return last
	case d > 0 && i == len(c.bodyRefs)-1 && hi < len(s.Turns):
		return s.TurnRef(hi)
	}
	return ""
}

// clickRow selects the row under a click in the pane; clicking the selected
// row again opens or closes it.
// A click on a script's line sets a breakpoint only on its number; past
// it, it's a click on the step.
func (m *Model) clickRow(c *hostConn, x, y int) {
	if q, ok := c.qAt[y-c.dockY-c.qTop]; ok {
		c.sel = fmt.Sprintf("q:%d", q)
		return
	}
	i := y - m.paneTop
	if i < 0 || i >= len(c.rowRefs) || c.rowRefs[i] == "" {
		return
	}
	ref := c.rowRefs[i]
	if step, _, ok := convo.ScriptLine(ref); ok {
		if at, in := m.textCell(c, x, y); in && at.row < len(c.shown) && at.col > numberEnd(c.shown[at.row].Text) {
			ref = step
		}
	}
	m.clickRef(c, ref)
}

// numberEnd is the cell just past the first number in a drawn row.
func numberEnd(row string) int {
	s := ansi.Strip(row)
	i := strings.IndexFunc(s, unicode.IsDigit)
	if i < 0 {
		return 0
	}
	if j := strings.IndexFunc(s[i:], func(r rune) bool { return !unicode.IsDigit(r) }); j >= 0 {
		s = s[:i+j]
	}
	return cellw.String(s)
}

// clickDockSide is a click on the subagents and background tasks drawn
// right of the conversation, reporting whether it was on them.
func (m *Model) clickDockSide(c *hostConn, x, y int) bool {
	i, col := y-c.sideY, x-m.paneX()-c.sideX
	if i < 0 || i >= len(c.side) || col < 0 || col >= c.sideW {
		return false
	}
	if ref := c.panelRefs[sideRefs+i]; ref != "" {
		m.clickRef(c, ref)
	}
	return true
}

// clickRef is a click on the row of ref.
func (m *Model) clickRef(c *hostConn, ref string) {
	if m.openDockRoom(ref) {
		return
	}
	if ref == "subback" {
		m.closeSub(c)
		return
	}
	if t, ok := strings.CutPrefix(ref, pinRefPrefix); ok {
		c.sel, c.selMoved, c.scrollOnly = t, true, false // drawn again, the scroll brings it into view
		return
	}
	if m.toggleScriptBreak(c, ref) {
		return
	}
	if ref == c.sel || strings.HasPrefix(ref, "job:") || (len(ref) > 1 && ref[0] == 't' && ref[1] >= '0' && ref[1] <= '9') {
		// A task opens or closes on the first click: its rows are one
		// thing, with nothing to pick in it first.
		c.sel = ref
		if id, ok := strings.CutPrefix(ref, "sub:"); ok {
			m.openSub(c, id)
			return
		}
		c.keepRow(ref)
		c.open[ref] = !m.isOpen(c, ref)
		return
	}
	c.sel = ref
}

// rowKey is the key that opens or closes a picked row, as its hints name
// it (state.Config.RowKey); notRowKey the one that doesn't, if one doesn't.
func (m *Model) rowKey() string { return firstNonEmpty(m.rowKeySet(), "enter") }
func (m *Model) notRowKey() string {
	return map[string]string{"enter": "space", "space": "enter"}[m.rowKeySet()]
}

func (m *Model) rowKeySet() string {
	if m.store == nil {
		return ""
	}
	return m.store.Config.RowKey
}

// keepRow holds ref's row where it is on screen through the next draw, so
// opening or closing it grows or shrinks below it, never above; what opens
// past the bottom is scrolled up into view (reveal).
func (c *hostConn) keepRow(ref string) {
	for y := c.bodyTop; y < len(c.rowRefs); y++ {
		if c.rowRefs[y] == ref {
			c.scroll = max(c.scroll, 1)
			c.top.ref, c.top.off, c.top.scroll, c.selMoved = ref, c.bodyTop-y, c.scroll, false
			c.reveal, c.revealTotal = ref, c.shownTotal
			return
		}
	}
}

// queueHover finds the queued message under the pointer at y, reporting
// whether that changed.
func (c *hostConn) queueHover(y int) bool {
	hover := 0
	if q, ok := c.qAt[y-c.dockY-c.qTop]; ok {
		hover = q + 1
	}
	changed := hover != c.qHover
	c.qHover = hover
	return changed
}

// leavePane gives the keys back to the list. On a narrow screen, where the
// conversation filled it, the list comes back too.
func (m *Model) leavePane() {
	if m.hostedAlone() {
		return // there's no list to go back to
	}
	m.paneFocus = false
	if m.listW == 0 {
		m.leaveChat()
	}
}

// leaveChat closes the Session for Agents. From the Session alone, kept
// that way, it's a peek: esc goes back, enter opens the one picked.
func (m *Model) leaveChat() {
	m.peekFrom = ""
	if m.store.Config.View == "agent" && m.chatAlone() && !m.zen {
		m.peekFrom = m.sel
	}
	m.preview, m.full = false, false
}

// peeking is whether Agents are on screen for a peek from the Session
// alone.
func (m *Model) peeking() bool {
	return m.peekFrom != "" && m.store.Config.View == "agent" && !m.chatOpen() && !m.zen
}

// openPeeked ends a peek on the picked agent, its Session alone again.
func (m *Model) openPeeked() tea.Cmd {
	m.peekFrom = ""
	return m.switchFocus()
}

// sendOffline sends from the message box of a session rush isn't hosting:
// a stopped rush session resumes with it, a Claude Code session gets it as
// a reply through its daemon.
func (m *Model) sendOffline(c *hostConn, text string, images []string, now bool) tea.Cmd {
	a := m.agentByKey(c.key)
	switch {
	case a == nil:
		return nil
	case a.Interactive:
		m.flash(a.DisplayName+" is "+a.Where()+"; rush can't send to it", true)
		return nil
	case a.Past:
		return m.moveToRushWith(a, withImages(text, images))
	case a.Rush:
		if !m.canResume(a) {
			return nil
		}
		if c.sleeping {
			return m.wakeHost(c, a, text, images, now)
		}
		fallback := host.Config{ID: a.ID, SessionID: a.SessionID, Account: a.Acct, Cwd: a.Cwd, Name: a.DisplayName}
		lean, rest, name := m.store.Config.Dispatch.Lean, host.Duration(m.store.Config.Dispatch.Rest()), a.DisplayName
		m.flash("resuming "+name+"…", false)
		m.markSending(c, text)
		return sendingVia(c.key, func() tea.Msg {
			cfg, err := host.ReadConfig(fallback.ID)
			if err != nil {
				cfg = fallback
			}
			cfg.Resume, cfg.Prompt, cfg.Images = true, text, images
			cfg.Lean, cfg.IdleStop = lean, rest
			if _, err := host.Spawn(cfg); err != nil {
				return doneMsg{err: err}
			}
			return doneMsg{text: "resumed " + name}
		})
	}
	text = withImages(text, images)
	if !now && (busy(a) || len(m.queueOf(c).items) > 0) {
		// It's working: the message waits in the queue and goes when it
		// is idle, together with anything else waiting.
		m.queueLocal(a.Key, text)
		m.flash(fmt.Sprintf("queued · %d waiting · goes to %s within 15s", len(m.localQ[a.Key].items), a.DisplayName), false)
		return nil
	}
	m.loader.Nudge(a.Key)
	m.markSeen(a)
	m.markSending(c, text)
	return sendingVia(c.key, reply(a, text))
}

// focusPane moves keys into the selected rush-mode agent's pane.
func (m *Model) focusPane(a *fleet.Agent) tea.Cmd {
	if a == nil {
		return nil
	}
	if cmd, ok := m.openRoomFor(a); ok {
		return cmd // a room's agents open as the room: alt+1…9 there opens one alone
	}
	m.preview, m.paneFocus = true, true
	return m.loadPreview()
}

func jsonUnmarshal(b []byte, v any) error { return jsonx.Unmarshal(b, v) }

// resume brings a stopped rush-mode session back: a new host, the same
// conversation, the model, effort and mode it last had.
func (m *Model) resume(a *fleet.Agent) tea.Cmd {
	if !m.canResume(a) {
		return nil
	}
	cfg, err := host.ReadConfig(a.ID)
	if err != nil {
		cfg = host.Config{ID: a.ID, SessionID: a.SessionID, Account: a.Acct, Cwd: a.Cwd, Name: a.DisplayName}
	}
	cfg.Resume, cfg.Prompt = true, ""
	cfg.Lean, cfg.IdleStop = m.store.Config.Dispatch.Lean, host.Duration(m.store.Config.Dispatch.Rest())
	m.flash("resuming "+a.DisplayName+"…", false)
	m.preview, m.paneFocus = true, true
	return func() tea.Msg {
		if _, err := host.Spawn(cfg); err != nil {
			return doneMsg{err: err}
		}
		return doneMsg{text: "resumed " + a.DisplayName}
	}
}

// canResume is whether a's agent can pick its conversation up again, and
// says so when it can't.
func (m *Model) canResume(a *fleet.Agent) bool {
	if agent.Supports(agent.Kind(a.Kind), agent.FeatureResume) {
		return true
	}
	m.flash(harnessName(a.Kind)+" can't pick a conversation up again", true)
	return false
}

// startHosted starts a new rush-mode session: rush's own host running
// Claude Code headless, with the model, effort and mode from Settings,
// then whatever with changes.
func (m *Model) startHosted(text, dir string, with ...func(*host.Config)) tea.Cmd {
	text, images := m.imgs.resolve(text)
	if rest, imgs := extractImages(text, m.lookPath); imgs != nil {
		images, text = append(images, imgs...), rest
	}
	name := host.NameFrom(text)
	if name == "" && len(images) > 0 {
		name = "about " + filepath.Base(images[0])
	}
	nameFirst := name == ""
	if nameFirst {
		name = host.FreshName(dir)
	}
	// Each agent starts with what its own Settings page says, or what
	// the start sheet picked for this one session.
	next := m.nextStart(dir)
	kind := next.kind
	if why := agent.Unreadable(agent.Kind(kind), next.model, images); why != "" {
		m.flash(why, true)
		return nil
	}
	cfg := m.configAs(next, dir)
	cfg.Prompt, cfg.Images, cfg.Name, cfg.NameFirst = text, images, name, nameFirst
	cfg.Profile = cmp.Or(next.profile, m.startProfile(dir).Name)
	m.imgs = imageRefs{}
	m.accts.profile = "" // a profile picked with #profile is for one session
	m.startOver = nil    // and a start picked on the sheet
	for _, f := range with {
		f(&cfg)
	}
	if err := cfg.UseAgent(kind); err != nil {
		m.flash(err.Error(), true)
		return nil
	}
	m.flash("starting a new session…", false)
	// On the account picked for it, which every session of its provider
	// moves to: rush signs a provider in as one account at a time.
	return tea.Sequence(m.accountSwitch(agent.Kind(kind), next.account), func() tea.Msg {
		c, err := host.Spawn(cfg)
		if err != nil {
			return doneMsg{err: err}
		}
		return hostStartedMsg{id: c.ID, name: cfg.Name, acct: cfg.Account.Name}
	})
}

// configAs is a new session in dir as o starts it, with what Settings
// gives its agent for the rest.
func (m *Model) configAs(o startOver, dir string) host.Config {
	d := m.store.Config.Dispatch
	cfg := host.Config{Cwd: dir, IdleStop: host.Duration(d.Rest()), Model: o.model, Effort: o.effort,
		PermissionMode: d.StartFor(o.kind).Mode, Billing: o.billing}
	if o.modeSet {
		cfg.PermissionMode = o.mode
	}
	if agent.Kind(o.kind) == loginsKind {
		// Dispatch's own settings are this agent's, and its account the one
		// switched in.
		cfg.Account, cfg.LimitMode, cfg.Lean = m.store.Config.ActiveAccount().Profile(), d.OnLimit, d.Lean
	}
	return cfg
}

// hostStartedMsg is a session started; retire is the one it took over
// from, for Done.
type hostStartedMsg struct{ id, name, acct, retire string }

// sendHosted sends a message to a rush-mode agent from the main prompt,
// through its host.
func sendHosted(a *fleet.Agent, text string) tea.Cmd {
	return sendHostedID(a.ID, a.DisplayName, text, false)
}

// sendHostedID sends to the rush session id, called name: queued while
// it's busy, or mid-turn when now.
func sendHostedID(id, name, text string, now bool) tea.Cmd {
	return func() tea.Msg {
		if err := host.Ensure(id); err != nil {
			return doneMsg{err: err}
		}
		c, err := host.Dial(id)
		if err != nil {
			return doneMsg{err: err}
		}
		defer c.Close()
		send := c.Send
		if now {
			send = c.SendNow
		}
		if err := send(text); err != nil {
			return doneMsg{err: err}
		}
		return doneMsg{text: "sent to " + name}
	}
}

// moveToRush switches a Claude Code session to rush mode: the daemon's
// copy stops (the conversation is kept) and the same conversation resumes
// under rush's own host, which runs it headless from then on.
func (m *Model) moveToRush(a *fleet.Agent) tea.Cmd { return m.moveToRushWith(a, "") }

// moveToRushWith moves it over with prompt as the first message there.
func (m *Model) moveToRushWith(a *fleet.Agent, prompt string) tea.Cmd {
	switch {
	case a.Rush:
		m.flash(a.DisplayName+" already runs in rush mode", false)
		return nil
	case !m.canResume(a):
		return nil
	case a.SessionID == "":
		m.flash("can't find "+a.DisplayName+"'s conversation to resume", true)
		return nil
	case a.Headless:
		m.flash(a.DisplayName+" is driven by another program; rush can't take it over", true)
		return nil
	case !a.Interactive && busy(a) && !m.moveWhenIdle[a.Key]:
		// Stopping it now would lose the turn in progress.
		if m.moveWhenIdle == nil {
			m.moveWhenIdle = map[string]bool{}
		}
		m.moveWhenIdle[a.Key] = true
		m.flash(a.DisplayName+" moves to rush mode when this turn ends · /rush again moves it now", false)
		return nil
	}
	delete(m.moveWhenIdle, a.Key)
	d := m.store.Config.Dispatch
	cfg := host.Config{SessionID: a.SessionID, Resume: true, Prompt: prompt, Account: a.Acct, Cwd: a.Cwd, Name: a.DisplayName,
		IdleStop: host.Duration(d.Rest())}
	if agent.Kind(a.Kind) == loginsKind {
		// Dispatch's own settings are this agent's; another carries on with
		// its own model and mode.
		cfg.Model, cfg.Effort, cfg.PermissionMode, cfg.LimitMode, cfg.Lean = d.Model, d.Effort, d.Permission, d.OnLimit, d.Lean
	}
	if err := cfg.UseAgent(a.Kind); err != nil {
		m.flash(err.Error(), true)
		return nil
	}
	old := a.Key
	if a.Interactive {
		// Its terminal keeps the original; rush carries on with a copy.
		cfg.Fork, cfg.From = true, a.SessionID
		m.flash("copying "+a.DisplayName+" into rush mode · the terminal one is left as it is", false)
		return func() tea.Msg {
			c, err := host.Spawn(cfg)
			if err != nil {
				return doneMsg{err: err}
			}
			// The original goes to Done, the copy takes its name: one agent
			// carrying on, not two.
			return movedToRushMsg{from: old, started: hostStartedMsg{id: c.ID, name: cfg.Name, acct: cfg.Account.Name}}
		}
	}
	m.flash("moving "+a.DisplayName+" to rush mode…", false)
	return func() tea.Msg {
		if a.PID != 0 || a.Live() {
			if err := stopOutside(a); err != nil {
				return doneMsg{err: fmt.Errorf("couldn't stop the Claude Code copy: %w", err)}
			}
		}
		c, err := host.Spawn(cfg)
		if err != nil {
			return doneMsg{err: err}
		}
		return movedToRushMsg{from: old, started: hostStartedMsg{id: c.ID, name: cfg.Name, acct: cfg.Account.Name}}
	}
}

// movePending moves agents waiting to go to rush mode once they're idle.
func (m *Model) movePending() tea.Cmd {
	var cmds []tea.Cmd
	for key := range m.moveWhenIdle {
		a := m.agentByKey(key)
		switch {
		case a == nil || a.Rush:
			delete(m.moveWhenIdle, key)
		case !busy(a):
			cmds = append(cmds, m.moveToRush(a))
		}
	}
	return tea.Batch(cmds...)
}

type movedToRushMsg struct {
	from    string
	started hostStartedMsg
}

func limitText(l *host.Limit) string {
	win := map[string]string{"five_hour": "5h limit", "seven_day": "7d limit", "seven_day_opus": "7d Opus limit"}[l.Window]
	if win == "" {
		win = "usage limit"
	}
	t := win
	if !l.ResetsAt.IsZero() {
		t += " · resets " + l.ResetsAt.Local().Format("15:04")
	}
	switch {
	case l.Continue:
		t += " · continues then"
	case !l.Ask:
		t += " · waiting for you"
	}
	return t
}

// --- Claude's questions (the AskUserQuestion tool) ---

// question is one question of those the agent asks.
type question struct {
	Question    string
	Header      string
	MultiSelect bool
	Options     []event.Choice
}

func isQuestion(p []*convo.Step) bool {
	return len(p) > 0 && p[0].Approval != nil && p[0].Approval.Question != nil
}

func questions(req *event.Question) (title string, qs []question) {
	for _, a := range req.Asks {
		qs = append(qs, question{Question: a.Text, Header: a.Header, MultiSelect: a.Multi, Options: a.Options})
	}
	return req.Title, qs
}

// syncQuestion resets the answering state when a new question arrives.
func (c *hostConn) syncQuestion(req *event.Question) {
	if c.qFor != req.ID {
		c.qFor, c.qIdx, c.qCursor, c.qPicks, c.qAnswer = req.ID, 0, 0, map[int]map[int]bool{}, map[string]string{}
		c.qTyped, c.qFull = map[int][]rune{}, false
	}
}

// picks is what's ticked on a multi-select question.
func (c *hostConn) picks(i int) map[int]bool {
	if c.qPicks[i] == nil {
		c.qPicks[i] = map[int]bool{}
	}
	return c.qPicks[i]
}

// optionLabel is an option's label without "(Recommended)", and whether it
// had it.
func optionLabel(l string) (string, bool) {
	l = strings.TrimSpace(l)
	i := strings.Index(strings.ToLower(l), "(recommended)")
	if i < 0 {
		return l, false
	}
	return strings.TrimSpace(l[:i] + l[i+len("(recommended)"):]), true
}

// questionKey answers Claude's questions. While the card has the keys, ↑↓
// choose, a digit or enter picks (or ticks, for multi-select, where a
// Continue button under the options confirms), and ←→ move between questions; typed text and enter answer in
// your own words. With several questions the last step is a review, where
// enter sends. It reports whether it used the key.
func (m *Model) questionKey(c *hostConn, req *event.Question, s string, empty bool) (tea.Cmd, bool) { //nolint:gocognit,gocyclo // one case per key of the card
	c.syncQuestion(req)
	_, qs := questions(req)
	if len(qs) == 0 {
		return nil, false
	}
	if c.qIdx >= len(qs) {
		// The review: enter sends, a digit or ← goes back to one.
		switch {
		case s == "enter":
			return m.sendAnswers(c, req, qs), true
		case s == "left":
			m.goQuestion(c, qs, len(qs)-1)
			return nil, true
		case empty && len(s) == 1 && s[0] >= '1' && s[0] <= '9' && int(s[0]-'1') < len(qs):
			m.goQuestion(c, qs, int(s[0]-'1'))
			return nil, true
		}
		return nil, false
	}
	q := qs[c.qIdx]
	picked := c.picks(c.qIdx)
	if c.cardFocus && empty {
		switch s {
		case "up":
			if c.qCursor == 0 {
				return nil, false // above the first option: off the card
			}
			c.qCursor--
			return nil, true
		case "down":
			last := len(q.Options)
			if q.MultiSelect {
				last++ // the Continue button
			}
			if c.qCursor >= last {
				return nil, false // past the last row: back to the box
			}
			c.qCursor++
			return nil, true
		case "v":
			c.qFull = !c.qFull
			return nil, true
		case "esc":
			if c.qFull {
				c.qFull = false
				return nil, true
			}
		case "left", "right":
			if len(qs) > 1 {
				to := c.qIdx - 1
				if s == "right" {
					to = c.qIdx + 1
				}
				if to >= 0 {
					m.goQuestion(c, qs, to)
				}
				return nil, true
			}
		case "space":
			if q.MultiSelect && c.qCursor < len(q.Options) {
				picked[c.qCursor] = !picked[c.qCursor]
				return nil, true
			}
		case "enter":
			switch {
			case c.qCursor == len(q.Options):
				c.cardFocus = false // "your own words": type in the box
				return nil, true
			case !q.MultiSelect:
				return m.answerQuestion(c, req, qs, q.Options[c.qCursor].Label), true
			case c.qCursor < len(q.Options):
				picked[c.qCursor] = !picked[c.qCursor]
				return nil, true
			}
			// On Continue: falls through to confirm what's ticked.
		}
	}
	if empty && len(s) == 1 && s[0] >= '1' && s[0] <= '9' {
		i := int(s[0] - '1')
		if i >= len(q.Options) {
			return nil, true
		}
		if q.MultiSelect {
			picked[i] = !picked[i]
			return nil, true
		}
		return m.answerQuestion(c, req, qs, q.Options[i].Label), true
	}
	if s == "enter" {
		if !empty {
			text := strings.TrimSpace(string(c.input))
			c.input, c.back = c.input[:0], 0
			return m.answerQuestion(c, req, qs, text), true
		}
		if q.MultiSelect && anyPicked(picked) {
			var labels []string
			for i, o := range q.Options {
				if picked[i] {
					labels = append(labels, o.Label)
				}
			}
			return m.answerQuestion(c, req, qs, strings.Join(labels, ", ")), true
		}
		return nil, true
	}
	return nil, false
}

func anyPicked(p map[int]bool) bool {
	for _, v := range p {
		if v {
			return true
		}
	}
	return false
}

// goQuestion moves to question i (len(qs) is the review), with the cursor
// on the option already chosen there.
func (m *Model) goQuestion(c *hostConn, qs []question, i int) {
	if len(qs) < 2 {
		i = min(i, len(qs)-1)
	}
	// What's typed goes with its question, and comes back with it: an
	// answer in your own words is back in the box to change.
	if c.qTyped == nil {
		c.qTyped = map[int][]rune{}
	}
	if c.qIdx < len(qs) {
		c.qTyped[c.qIdx] = slices.Clone(c.input)
	}
	c.qIdx, c.qCursor = max(0, min(i, len(qs))), 0
	c.input, c.back = nil, 0
	if c.qIdx == len(qs) {
		// The review: Send answers is picked, even after an answer typed
		// in the box, so enter sends them.
		c.cardFocus = true
	}
	if c.qIdx < len(qs) {
		q := qs[c.qIdx]
		c.input = c.qTyped[c.qIdx]
		if a := c.qAnswer[q.Question]; len(c.input) == 0 && ownAnswer(q, a) {
			c.input = []rune(a)
		}
		for j, o := range q.Options {
			if c.qAnswer[q.Question] == o.Label {
				c.qCursor = j
			}
		}
	}
}

// answerQuestion records one answer, then moves to the next question still
// unanswered. A lone question sends at once; several end on the review.
func (m *Model) answerQuestion(c *hostConn, req *event.Question, qs []question, answer string) tea.Cmd {
	c.qAnswer[qs[c.qIdx].Question] = answer
	if len(qs) == 1 {
		return m.sendAnswers(c, req, qs)
	}
	next := len(qs)
	for k := 1; k < len(qs); k++ {
		j := (c.qIdx + k) % len(qs)
		if _, done := c.qAnswer[qs[j].Question]; !done {
			next = j
			break
		}
	}
	m.goQuestion(c, qs, next)
	return nil
}

// sendAnswers replies with the answers keyed by question text.
func (m *Model) sendAnswers(c *hostConn, req *event.Question, qs []question) tea.Cmd {
	b := answerInput(req, qs, c.qAnswer)
	id := req.ID
	c.qFor = ""
	return hostCmd(func() error { return c.client.Allow(id, b, false) })
}

// answerInput is the tool input that answers req.
func answerInput(req *event.Question, _ []question, answers map[string]string) jsontext.Value {
	return host.AnswerInput(req, answers)
}

// shownAnswer is an answer as the card shows it.
func shownAnswer(a string) string {
	l, _ := optionLabel(a)
	return oneLine(l)
}

// cardKind is the card waiting in the dock, if any.
func cardKind(c *hostConn) string {
	switch {
	case c.sess.Info.Limit != nil && c.sess.Info.Limit.Ask:
		return "limit"
	case isQuestion(c.sess.Pending()):
		return "question"
	case len(c.sess.Pending()) > 0:
		return "approval"
	case signedOut(c):
		return "signin"
	}
	return ""
}

// cardID names the card waiting, to tell a new one from the last.
func cardID(c *hostConn) string {
	switch cardKind(c) {
	case "limit":
		return "limit"
	case "question", "approval":
		return c.sess.Pending()[0].Approval.ID
	case "signin":
		return "signin:" + c.sess.Info.Error
	}
	return ""
}

// cardKey answers a waiting card. Plain letters and digits answer only
// once ↑ has put the keys on the card, so typing a message that starts
// with "yes" or "1." can never answer by accident; alt+y, alt+a and alt+n
// answer from anywhere. It reports whether it used the key.
func (m *Model) cardKey(c *hostConn, s string, empty bool) (tea.Cmd, bool) {
	kind := cardKind(c)
	if _, ok := m.verdicts[c.key]; ok && kind == "" {
		kind = "verdict" // its own keys are verdictKey's: here, ↑↓ onto and off it
	}
	if kind == "" {
		c.cardFocus, c.cardAgain = false, "" // a key between cards: you've moved on
		return nil, false
	}
	done := func(cmd tea.Cmd) (tea.Cmd, bool) {
		if c.cardFocus {
			c.cardAgain = cardID(c)
		}
		c.cardFocus = false
		return cmd, true
	}
	pending := c.sess.Pending()
	switch kind {
	case "limit":
		if s == "r" && c.cardFocus {
			return m.refreshLimitUsage(c), true
		}
		yes, no := s == "alt+y", s == "alt+n"
		if c.cardFocus {
			yes, no = yes || s == "y" || s == "enter", no || s == "n"
		}
		if yes || no {
			return done(hostCmd(func() error { return c.client.ContinueAtReset(yes) }))
		}
	case "signin":
		if cmd, used := m.signinKey(c, s, c.cardFocus || empty && c.sel == "" && !c.cardGuarded()); used {
			if cmd == nil {
				return nil, true // moved through its accounts
			}
			return done(cmd)
		}
	case "approval":
		req := pending[0].Approval
		// A memory card's buttons: ←→ (or tab) pick one, enter presses it.
		if c.cardFocus && memoryWrite(pending[0]) {
			if d := map[string]int{"left": -1, "right": 1, "tab": 1, "shift+tab": -1, "h": -1, "l": 1}[s]; d != 0 {
				c.memPick = (c.memPick + d + len(memBtns)) % len(memBtns)
				return nil, true
			}
			if s == "enter" {
				s = memBtns[c.memPick][0]
			}
		}
		// With the box empty its letters answer it, no ↑ needed, unless
		// it opened on you mid-word.
		direct := c.cardFocus || empty && c.sel == "" && !c.cardGuarded()
		switch {
		case s == "alt+y" || direct && s == "y" || c.cardFocus && s == "enter":
			return done(m.answerHost(c, req, true, false))
		case s == "alt+a" || direct && s == "a":
			return done(m.answerHost(c, req, true, true))
		case s == "alt+n" || direct && s == "n":
			return done(m.answerHost(c, req, false, false))
		case !empty && !c.cardFocus && s == "enter" && !memoryWrite(pending[0]):
			// What's typed is why not: it's denied, and the agent's told.
			why, id := strings.TrimSpace(string(c.input)), req.ID
			c.input, c.back = c.input[:0], 0
			return done(hostCmd(func() error { return c.client.Deny(id, why, false) }))
		case c.cardFocus && s == "e" && memoryWrite(pending[0]):
			// Allowed, and opened in the memory view's editor once written.
			c.editAfter = pending[0].ID
			return done(m.answerHost(c, req, true, false))
		}
	case "question":
		req := pending[0].Approval.Question
		if c.cardFocus && s == "s" {
			id := req.ID
			return done(hostCmd(func() error {
				return c.client.Deny(id, "The user skipped the question; carry on with your best judgement.", false)
			}))
		}
		// Typed text and enter answer in your own words; digits pick only
		// while the card has the keys.
		if !empty && s == "enter" || c.cardFocus {
			if cmd, used := m.questionKey(c, req, s, empty); used {
				if cmd != nil {
					return done(cmd)
				}
				return cmd, true
			}
		}
	}
	// The card sits just above the box: ↑ from the box goes straight onto
	// it, and ↑ again on up through the dock's rows and the conversation;
	// ↓ off the last of those comes back onto it, and ↓ off it to the box.
	if m.cardModal(c) && c.cardFocus {
		// The modal keeps the keys until it's answered or set aside.
		if s == "esc" {
			c.cardLaterNow()
		}
		return nil, true
	}
	above := ""
	if s == "up" || s == "down" {
		refs := append(slices.Clip(c.bodyRefs), m.dockRefs(c)...)
		if len(refs) > 0 {
			above = refs[len(refs)-1]
		}
	}
	switch {
	case !c.cardFocus && empty && s == "up" && c.sel == "":
		c.cardFocus = true
		return nil, true
	case !c.cardFocus && empty && s == "down" && c.sel != "" && c.sel == above:
		c.cardFocus, c.sel, c.subSel = true, "", ""
		return nil, true
	case c.cardFocus && s == "down":
		c.cardFocus = false
		return nil, true
	case c.cardFocus && s == "up" && above != "":
		c.cardFocus, c.sel, c.selMoved = false, above, true
		return nil, true
	case c.cardFocus && s == "up":
		return nil, true // nothing above it: stay
	case c.cardFocus && s == "esc":
		c.cardFocus = false
		return nil, true
	case c.cardFocus:
		c.cardFocus = false // anything else goes back to typing
	}
	return nil, false
}

// cardHint is the last line of a card: its keys when it has focus, how to
// give it focus when not.
func cardHint(c *hostConn, keys string) string {
	if c.inModal && c.cardFocus {
		return paint(cOrange, "▸ ") + keys // the modal's edge says esc
	}
	if c.cardFocus {
		return paint(cOrange, "▸ ") + keys + dim("   ·   esc back to typing")
	}
	return keys
}

// alwaysWords is the always key's label, from what the agent says
// allowing for good does: "always allow Bash(pnpm lint:*)", "allow for
// this session".
func alwaysWords(as string) string {
	if rest, ok := strings.CutPrefix(as, "Always allow"); ok {
		return "always allow" + rest
	}
	if rest, ok := strings.CutPrefix(as, "Yes, "); ok {
		return "allow " + rest
	}
	return "always allow"
}

// selTurn is the turn picked in the conversation, or the one the picked
// step belongs to.
func selTurn(c *hostConn) *convo.Turn {
	t, _, _ := strings.Cut(c.sel, ":")
	if !isTurnRef(t) {
		return nil
	}
	for _, turn := range c.sess.Turns {
		if "t"+strconv.Itoa(turn.N) == t {
			return turn
		}
	}
	return nil
}

// isTurnRef is a turn's own row ("t12"), not one of its steps.
func isTurnRef(r string) bool {
	if len(r) < 2 || r[0] != 't' {
		return false
	}
	for _, c := range r[1:] {
		if c < '0' || c > '9' {
			return false
		}
	}
	return true
}

// pickedSub is the subagent run you're on: the one watched, or the one
// picked in the dock or the subagents view; live when it's still working.
func (m *Model) pickedSub(c *hostConn) (sa convo.Subagent, live, ok bool) {
	id := ""
	switch {
	case m.watchingSub(c):
		id = c.subOpen
	case strings.HasPrefix(c.sel, "run:"):
		id = strings.TrimPrefix(c.sel, "run:")
	case strings.HasPrefix(c.sel, "sub:"):
		id = strings.TrimPrefix(c.sel, "sub:")
	default:
		return sa, false, false
	}
	for _, x := range c.subs {
		if x.ID == id {
			_, live = c.subState(x)
			return x, live, true
		}
	}
	return sa, false, false
}

// relaySub is the subagent you're watching when what you type goes to it,
// through the main session: a live Claude subagent in a session rush runs.
func (m *Model) relaySub(c *hostConn) (convo.Subagent, bool) {
	sa, live, ok := m.pickedSub(c)
	return sa, ok && live && m.watchingSub(c) && c.client != nil && !strings.HasPrefix(sa.ID, spawnPrefix)
}

// stopSub stops one subagent, and nothing else: the turn and the other
// runs carry on.
func (m *Model) stopSub(c *hostConn, sa convo.Subagent, live bool) tea.Cmd {
	switch {
	case !live:
		m.flash("that subagent has already finished", false)
		return nil
	case c.client == nil:
		m.flash("rush can stop a subagent only in a session it runs; this one is Claude Code's", true)
		return nil
	}
	m.flash("stopping "+sa.Type+" · the turn carries on", false)
	// One the shell ran that rush hosts is a session of its own to stop.
	if r := c.spawns[strings.TrimPrefix(sa.ID, spawnPrefix)]; r != nil && r.hosted != "" {
		id := r.hosted
		return hostCmd(func() error {
			hc, err := host.Dial(id)
			if err != nil {
				return nil // already gone
			}
			defer hc.Close()
			return hc.Stop()
		})
	}
	cl, id := c.client, strings.TrimPrefix(sa.ID, spawnPrefix) // a harness's own child: by the call that spawned it
	return hostCmd(func() error { return cl.StopTask(id) })
}

// childSub is the subagent you're watching when it's a thread of the
// session's own harness (Codex's spawn_agent): what you type is passed on
// to it, finished or not.
func (m *Model) childSub(c *hostConn) (convo.Subagent, bool) {
	sa, _, ok := m.pickedSub(c)
	if !ok || !m.watchingSub(c) || c.client == nil {
		return sa, false
	}
	r := c.spawns[strings.TrimPrefix(sa.ID, spawnPrefix)]
	return sa, r != nil && r.hosted == "" && r.sp.Child != ""
}

// subWhere is the checkout a run worked in, when it isn't the session's:
// a worktree of its own, most often.
func (c *hostConn) subWhere(id string) string {
	if wt := c.subWT[id]; wt != "" {
		return "  " + faint("⎇ ") + dim(wt)
	}
	// Working from the session's folder, it may cd into one itself.
	if t := c.subTails[id]; t != nil {
		if wt := t.Sess.Worktree(); wt != "" && wt != c.ownWorktree() {
			return "  " + faint("⎇ ") + dim(wt)
		}
	}
	return ""
}

// ownWorktree is the worktree the session itself works in, by name; ""
// when it's in a main checkout.
func (c *hostConn) ownWorktree() string {
	if c.sess == nil {
		return ""
	}
	return convo.WorktreeIn(firstNonEmpty(c.sess.Info.Cwd, c.sess.Cwd) + "/")
}

// commandAbout is what queued command t does, as the agent describes it.
func commandAbout(c *hostConn, t string) string {
	name := strings.TrimPrefix(strings.Fields(t)[0], "/")
	for _, cmd := range c.sess.Commands {
		if cmd.Name == name || slices.Contains(cmd.Aliases, name) {
			return oneLine(cmd.Description)
		}
	}
	return "runs as a command"
}

// queueChip is a queued message's image as a chip.
func queueChip(path string) string {
	return bgChip + cBlue + "▣ " + cText + convo.ImageLabel(path) + " " + reset
}

// queueChips is a queued message's text with each [Image #N] as image N's
// chip, and the images no marker stands for.
func queueChips(text string, images []string) (string, []string) {
	used := make([]bool, len(images))
	text = imageMarkerRe.ReplaceAllStringFunc(text, func(mk string) string {
		n, _ := strconv.Atoi(imageMarkerRe.FindStringSubmatch(mk)[1])
		if n < 1 || n > len(images) {
			return mk
		}
		used[n-1] = true
		return queueChip(images[n-1]) + cText
	})
	var rest []string
	for i, p := range images {
		if !used[i] {
			rest = append(rest, p)
		}
	}
	return text, rest
}
