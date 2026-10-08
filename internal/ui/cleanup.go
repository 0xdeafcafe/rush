package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Clean-up: agents' worktrees and temp work, what can go without losing
// anything, and what goes by itself. Projects shows it on each worktree and
// agent. Stopping an agent never removes anything; done work that is
// committed and pushed goes once it has been left alone for a while
// (Settings › Claude › Clean up done work after).

// cleanup is what Projects and the automatic tidy-up know of worktrees.
type cleanup struct {
	wts      []fleet.Worktree
	checking bool      // worktrees are being looked at in the background
	checked  time.Time // when they last were, fully
	// kept are done worktrees the tidy-up found unsafe, and when: it says
	// so once and looks again only after an hour.
	kept map[string]time.Time
	// tmp is what's yours in /tmp, looked at every few minutes at most:
	// walking it takes seconds.
	tmp fleet.Scratch
	// agentTmp is the size of each project's agent tmp folders
	// (.claude/tmp), by path, for those that are there.
	agentTmp map[string]int64
	// nudged is when rush last said there's a lot to clean up.
	nudged time.Time
	// left is what agents left running, found by reap, waiting for the
	// next tick to be ended off the UI.
	left []fleet.Leftover
	// orphans are when each orphan (by pid and start) was first seen one:
	// those orphaned for orphanGrace are ended, unless KeepOrphans.
	orphans map[orphanID]time.Time
}

// orphanID is a process, by pid and when it started: a pid can be reused.
type orphanID struct {
	pid   int
	start time.Time
}

// orphanGrace is how long a process whose session ended is left before
// it's ended: long enough for a session being restarted to take it back.
const orphanGrace = 2 * time.Minute

// dueOrphans are the orphans that have been orphans for orphanGrace,
// noting when each new one was first seen.
func (m *Model) dueOrphans(now time.Time) []procRow {
	c := &m.clean
	if m.store.Config.KeepOrphans || m.hosted != "" {
		return nil
	}
	seen := map[orphanID]time.Time{}
	var due []procRow
	for _, r := range m.snap.Machine.Rows {
		if r.Role != fleet.RoleOrphan {
			continue
		}
		id := orphanID{r.PID, r.Start}
		first, ok := c.orphans[id]
		if !ok {
			first = now
		}
		seen[id] = first
		if now.Sub(first) >= orphanGrace {
			due = append(due, procRow{pid: r.PID, start: r.Start, mem: r.Mem, role: r.Role})
			delete(seen, id) // ended now; if it's still there next time, it's tried again after the grace
		}
	}
	c.orphans = seen
	return due
}

type worktreesMsg struct {
	wts  []fleet.Worktree
	full bool           // every worktree checked and measured, for the view
	tmp  *fleet.Scratch // /tmp, when it was due a look
	// agentTmp are the projects' agent tmp folders' sizes, by path.
	agentTmp map[string]int64
}

type scratchClearedMsg struct {
	freed int64
	n     int
	err   error
}

type tidiedMsg struct {
	wts    []fleet.Worktree
	gone   []string // worktree paths removed
	freed  int64
	temp   tempMsg // temp work measured again after cleaning
	kept   map[string]string
	failed error
}

type removedMsg struct {
	path  string
	freed int64
	err   error
}

// agentCopies are the agents as they are now, safe to read from a command.
func (m *Model) agentCopies() []*fleet.Agent {
	out := make([]*fleet.Agent, len(m.snap.Agents))
	for i, a := range m.snap.Agents {
		b := *a
		out[i] = &b
	}
	return out
}

// scanWorktrees finds every agent's worktree and checks each with git (in
// the background: a big checkout takes seconds), for Projects.
func (m *Model) scanWorktrees() tea.Cmd {
	c := &m.clean
	if c.checking {
		return nil
	}
	c.checking = true
	agents := m.agentCopies()
	tmpDue := time.Since(c.tmp.Checked) > 10*time.Minute
	var tmps []string
	for _, p := range m.projects() {
		tmps = append(tmps, agentTmpDirs(p.key)...)
	}
	return func() tea.Msg {
		msg := worktreesMsg{full: true, agentTmp: map[string]int64{}}
		for _, d := range tmps {
			if st, err := os.Stat(d); err == nil && st.IsDir() {
				msg.agentTmp[d] = fleet.DiskUsage([]fleet.TempDir{{Path: d}})
			}
		}
		if tmpDue {
			s := fleet.FindScratch()
			msg.tmp = &s
		}
		msg.wts = fleet.FindWorktrees(agents)
		for i := range msg.wts {
			msg.wts[i].Check()
		}
		return msg
	}
}

// agentTmpDirs are where the agents keep scratch in the project at root:
// each one's own folder's tmp (.claude/tmp), and the one rush gives its
// sessions there (a folder per session).
func agentTmpDirs(root string) []string {
	if !filepath.IsAbs(root) {
		return nil
	}
	out := []string{host.ProjectTemp(root)}
	for _, f := range agent.ProjectFolders() {
		out = append(out, filepath.Join(root, f, "tmp"))
	}
	return out
}

// untouched is how long ago an agent was last active or marked done.
func (m *Model) untouched(a *fleet.Agent, now time.Time) time.Duration {
	d := a.Age(now)
	if t, ok := m.store.Overlay.Done[a.Key]; ok && now.Sub(t) < d {
		d = now.Sub(t)
	}
	return d
}

// dueIn is how long until done work of these agents goes by itself: zero
// when it's due now, negative when it never will (not all done, one still
// running, or the tidy-up is off).
func (m *Model) dueIn(keys []string, now time.Time) time.Duration {
	after := m.store.Config.CleanupAfter()
	if after == 0 || len(keys) == 0 {
		return -1
	}
	due := time.Duration(0)
	for _, k := range keys {
		a := m.agentByKey(k)
		if a == nil || !a.Done || a.PID != 0 {
			return -1
		}
		due = max(due, after-m.untouched(a, now))
	}
	return due
}

// tidy is the tick's clean-up: what agents left running, ended as soon as
// it's found, and done work, looked at once a minute.
func (m *Model) tidy() tea.Cmd {
	cmds := []tea.Cmd{m.endLeftovers(), m.tidyDone()}
	if due := m.dueOrphans(time.Now()); len(due) > 0 {
		cmds = append(cmds, endOrphans(due))
	}
	return tea.Batch(cmds...)
}

// tidyDone is the automatic clean-up of done work: the worktrees and temp
// work of agents done and untouched for long enough. A worktree goes only
// if git says every change is committed and pushed; one that isn't is
// kept, and said once.
func (m *Model) tidyDone() tea.Cmd {
	c := &m.clean
	if m.tick%60 != 30 || c.checking || m.store.Config.CleanupAfter() == 0 {
		return nil
	}
	now := time.Now()
	agents := m.agentCopies()
	after := m.store.Config.CleanupAfter()
	var tempDue, stale []*fleet.Agent
	for _, a := range agents {
		switch {
		case a.Temp >= tempShown && m.dueIn([]string{a.Key}, now) == 0:
			tempDue = append(tempDue, a)
		case a.Rush && a.PID == 0 && m.untouched(a, now) >= after:
			// Stopped, done or not: the tmp folder rush gave it goes.
			stale = append(stale, a)
		}
	}
	due := map[string]bool{} // agent keys whose work is due
	for _, a := range agents {
		due[a.Key] = m.dueIn([]string{a.Key}, now) == 0
	}
	kept := map[string]bool{}
	for p, t := range c.kept {
		kept[p] = now.Sub(t) < time.Hour
	}
	c.checking = true
	return func() tea.Msg {
		msg := tidiedMsg{kept: map[string]string{}, temp: tempMsg{}}
		msg.wts = fleet.FindWorktrees(agents)
		for _, w := range msg.wts {
			ready := len(w.Agents) > 0 && !kept[w.Path]
			for _, k := range w.Agents {
				ready = ready && due[k]
			}
			if !ready {
				continue
			}
			size, err := fleet.TidyWorktree(w)
			if err != nil {
				msg.kept[w.Path] = err.Error()
				continue
			}
			msg.gone = append(msg.gone, w.Path)
			msg.freed += size
		}
		for _, a := range tempDue {
			before := a.Temp
			if err := fleet.CleanTemp(a); err != nil && msg.failed == nil {
				msg.failed = err
			}
			at := time.Now()
			left := fleet.DiskUsage(a.TempDirs())
			msg.temp[a.Key] = fleet.TempSize{Bytes: left, At: at, Took: time.Since(at)}
			msg.freed += before - left
		}
		for _, a := range stale {
			emptied, err := fleet.CleanStaleTemp(a, after, now)
			if err != nil && msg.failed == nil {
				msg.failed = err
			}
			if !emptied {
				continue
			}
			at := time.Now()
			left := fleet.DiskUsage(a.TempDirs())
			msg.temp[a.Key] = fleet.TempSize{Bytes: left, At: at, Took: time.Since(at)}
			msg.freed += max(0, a.Temp-left)
		}
		return msg
	}
}

func (m *Model) onWorktrees(msg worktreesMsg) {
	c := &m.clean
	c.checking = false
	if msg.tmp != nil {
		c.tmp = *msg.tmp
	}
	if msg.full {
		c.wts, c.checked, c.agentTmp = msg.wts, time.Now(), msg.agentTmp
		m.nudgeClean()
		return
	}
	c.wts = mergeChecks(msg.wts, c.wts)
}

// mergeChecks keeps what was known about worktrees still there.
func mergeChecks(found, known []fleet.Worktree) []fleet.Worktree {
	by := map[string]fleet.Worktree{}
	for _, w := range known {
		by[w.Path] = w
	}
	for i, w := range found {
		if k, ok := by[w.Path]; ok && !k.Checked.IsZero() {
			k.Agents = w.Agents
			found[i] = k
		}
	}
	return found
}

func (m *Model) onTidied(msg tidiedMsg) {
	c := &m.clean
	c.checking = false
	gone := map[string]bool{}
	for _, p := range msg.gone {
		gone[p] = true
	}
	var left []fleet.Worktree
	for _, w := range mergeChecks(msg.wts, c.wts) {
		if !gone[w.Path] {
			left = append(left, w)
		}
	}
	c.wts = left
	if c.kept == nil {
		c.kept = map[string]time.Time{}
	}
	var notes []string
	for p, why := range msg.kept {
		if _, said := c.kept[p]; !said {
			notes = append(notes, "kept "+filepath.Base(p)+": "+strings.TrimPrefix(why, filepath.Base(p)+" isn't safe to remove: "))
		}
		c.kept[p] = time.Now()
	}
	if len(msg.temp) > 0 {
		m.onCleanedQuiet(msg.temp)
	}
	switch {
	case len(msg.gone) > 0:
		names := make([]string, len(msg.gone))
		for i, p := range msg.gone {
			names[i] = filepath.Base(p)
		}
		m.flash("cleaned up done work: "+strings.Join(names, ", ")+" · freed "+disk(msg.freed)+" (committed and pushed; branches kept)", false)
	case msg.freed >= tempShown:
		m.flash("cleaned up "+disk(msg.freed)+" of done agents' temp work", false)
	case len(notes) > 0:
		m.flash(strings.Join(notes, " · ")+" · Projects has it", true)
	}
	if msg.failed != nil {
		m.flash("cleaning up: "+msg.failed.Error(), true)
	}
}

// onCleanedQuiet takes in temp sizes measured after a clean-up.
func (m *Model) onCleanedQuiet(sizes tempMsg) {
	m.loader.Temp.Set(sizes)
	for _, a := range m.snap.Agents {
		if e, ok := sizes[a.Key]; ok {
			a.Temp = e.Bytes
		}
	}
	m.rebuild()
}

// cleanRow is something clean-up could remove: a worktree, or an agent's
// temp work.
type cleanRow struct {
	wt    *fleet.Worktree
	agent *fleet.Agent // temp work
}

func (m *Model) cleanRows() []cleanRow {
	var rows []cleanRow
	for i := range m.clean.wts {
		rows = append(rows, cleanRow{wt: &m.clean.wts[i]})
	}
	var temp []*fleet.Agent
	for _, a := range m.snap.Agents {
		if a.Temp >= tempShown {
			temp = append(temp, a)
		}
	}
	sort.SliceStable(temp, func(i, j int) bool { return temp[i].Temp > temp[j].Temp })
	for _, a := range temp {
		rows = append(rows, cleanRow{agent: a})
	}
	return rows
}

// running is the first agent in keys with a process.
func (m *Model) running(keys []string) *fleet.Agent {
	for _, k := range keys {
		if a := m.agentByKey(k); a != nil && a.PID != 0 {
			return a
		}
	}
	return nil
}

// askRemoveWorktree opens Delete on a worktree: what goes, what's lost,
// and who worked there. Its branch always stays.
func (m *Model) askRemoveWorktree(wt fleet.Worktree) tea.Cmd {
	if wt.Checked.IsZero() || wt.Repo == "" {
		m.flash(filepath.Base(wt.Path)+" hasn't been looked at yet · a moment", false)
		return nil
	}
	return m.openDelete(nil, doomed{wt: &wt})
}

// askClearScratch opens Delete on what's yours in /tmp that nothing has
// touched for a day.
func (m *Model) askClearScratch() tea.Cmd {
	if m.clean.tmp.StaleItems == 0 {
		m.flash("everything of yours in /tmp was touched in the last day; it stays", false)
		return nil
	}
	return m.openDelete(nil, doomed{tmp: true})
}

func (m *Model) clearScratch() tea.Cmd {
	m.flash("clearing /tmp…", false)
	return func() tea.Msg {
		freed, n, err := fleet.ClearScratch()
		return scratchClearedMsg{freed: freed, n: n, err: err}
	}
}

func (m *Model) onScratchCleared(msg scratchClearedMsg) tea.Cmd {
	s := &m.clean.tmp
	s.Items, s.StaleItems, s.Size, s.Stale = s.Items-msg.n, s.StaleItems-msg.n, s.Size-msg.freed, s.Stale-msg.freed
	s.Checked = time.Now().Add(-time.Hour) // and look again at what's left
	text := fmt.Sprintf("cleared %d things in /tmp · freed %s", msg.n, disk(msg.freed))
	if msg.err != nil {
		m.flash(text+" · "+msg.err.Error(), true)
	} else {
		m.flash(text, false)
	}
	return m.scanWorktrees()
}

func (m *Model) removeWorktree(wt fleet.Worktree, force bool) tea.Cmd {
	m.flash("removing "+filepath.Base(wt.Path)+"…", false)
	return func() tea.Msg {
		err := fleet.RemoveWorktree(wt, force)
		return removedMsg{path: wt.Path, freed: wt.Size, err: err}
	}
}

func (m *Model) onRemoved(msg removedMsg) {
	name := filepath.Base(msg.path)
	if msg.err != nil {
		m.flash(msg.err.Error(), true)
		return
	}
	c := &m.clean
	for i, w := range c.wts {
		if w.Path == msg.path {
			c.wts = append(c.wts[:i], c.wts[i+1:]...)
			break
		}
	}
	m.flash("removed "+name+" · freed "+disk(msg.freed), false)
}

// reap finds what agents left running when they stopped: a dev server, a
// watcher, a shell still in a loop. It only reads the process table it was
// given; endLeftovers, on the next tick, ends them off the UI.
func (m *Model) reap() {
	if left := m.reaper.Scan(m.snap.Table, m.snap.Agents); len(left) > 0 {
		m.clean.left = append(m.clean.left, left...)
	}
}

// endLeftovers ends what reap found, in the background, and says what it
// ended.
func (m *Model) endLeftovers() tea.Cmd {
	left := m.clean.left
	if len(left) == 0 {
		return nil
	}
	m.clean.left = nil
	return func() tea.Msg {
		n := 0
		for i := range left {
			left[i].Describe() // before it's ended, while it's there to ask
			n += max(1, left[i].Procs)
			go left[i].End(3 * time.Second)
		}
		l := left[0]
		what := trimCmd(orphanWhat(l.Cmd), 60)
		text := fmt.Sprintf("ended what %s left running: %s", oneLine(l.Agent), what)
		if n > 1 {
			text += fmt.Sprintf(" (%d processes)", n)
		}
		return sheetMsg{apply: func(m *Model) tea.Cmd {
			m.flash(text, false)
			return nil
		}}
	}
}

// orphanWhat is the command a Bash-tool shell was running, without Claude
// Code's wrapper around it.
func orphanWhat(cmd string) string {
	if i := strings.Index(cmd, "eval '"); i >= 0 {
		rest := strings.ReplaceAll(cmd[i+6:], `'"'"'`, "'")
		if j := strings.Index(rest, "' "); j >= 0 {
			rest = rest[:j]
		}
		return strings.TrimSuffix(rest, "'")
	}
	return cmd
}
