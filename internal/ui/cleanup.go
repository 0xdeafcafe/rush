package ui

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/prdrive"
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
	// outside is what rush sessions wrote outside their projects and
	// scratch, as it was on disk when last looked at.
	outside []fleet.LeftItem
	// left is what agents left running, found by reap, waiting for the
	// next tick to be ended off the UI.
	left []fleet.Leftover
	// orphans are when each orphan (by pid and start) was first seen one:
	// those orphaned for orphanGrace are ended, unless KeepOrphans.
	orphans map[orphanID]time.Time
	// settling is set while sessions' worktrees are looked at for work
	// that landed. settled are the sessions whose work landed, by key:
	// marked done once, so one you move back stays back. kept are those
	// whose worktree stayed (work it would lose), looked at again after an
	// hour.
	settling bool
	settled  map[string]bool
	stayed   map[string]time.Time
	// markedLanded are the sessions marked done because their work landed.
	markedLanded map[string]bool
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
	if m.store.Config.KeepOrphans || !m.leads() {
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
	outside  []fleet.LeftItem
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
	scan := m.worktreeScan()
	return func() tea.Msg { return scan() }
}

// worktreeScan is the look scanWorktrees takes, to run off the UI.
func (m *Model) worktreeScan() func() worktreesMsg {
	c := &m.clean
	agents := m.agentCopies()
	tmpDue := time.Since(c.tmp.Checked) > 10*time.Minute
	var tmps []string
	for _, p := range m.projects() {
		tmps = append(tmps, agentTmpDirs(p.key)...)
	}
	return func() worktreesMsg {
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
		msg.outside = fleet.LookLeft(agents)
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

// untouched is how long ago an agent was last active or put away.
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
		if a == nil || !earlier(a, now) || a.PID != 0 {
			return -1
		}
		due = max(due, after-m.untouched(a, now))
	}
	return due
}

// tidy is the tick's clean-up: what agents left running, ended as soon as
// it's found, work that landed, looked at once a minute, and done work.
func (m *Model) tidy() tea.Cmd {
	cmds := []tea.Cmd{m.endLeftovers(), m.tidyDone(), m.settleLanded()}
	if due := m.dueOrphans(time.Now()); len(due) > 0 {
		cmds = append(cmds, endOrphans(due, fmt.Sprintf("orphaned for over %s", orphanGrace)))
	}
	return tea.Batch(cmds...)
}

// tidyDone is the automatic clean-up of done work: the worktrees and temp
// work of agents done and untouched for long enough. A worktree goes only
// if git says every change is committed and pushed; one that isn't is
// kept, and said once.
func (m *Model) tidyDone() tea.Cmd {
	c := &m.clean
	// Work is due hours after it's done: a look every ten minutes is soon
	// enough, and each walks every worktree with git.
	if m.tick%600 != 30 || c.checking || m.store.Config.CleanupAfter() == 0 {
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
	due := map[string]bool{}           // agent keys whose work is due
	finished := map[string]time.Time{} // done agents' keys, by when they were last touched
	for _, a := range agents {
		due[a.Key] = m.dueIn([]string{a.Key}, now) == 0
		if earlier(a, now) && !a.InUse() {
			finished[a.Key] = now.Add(-m.untouched(a, now))
		}
	}
	kept := map[string]bool{}
	for p, t := range c.kept {
		kept[p] = now.Sub(t) < time.Hour
	}
	c.checking = true
	return func() tea.Msg {
		msg := tidiedMsg{kept: map[string]string{}, temp: tempMsg{}}
		msg.wts = fleet.FindWorktrees(agents)
		for _, w := range tidyable(msg.wts, due, finished, sessionGone) {
			if kept[w.Path] {
				continue
			}
			size, err := fleet.TidyWorktree(*w)
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

// keepFinished is how many finished worktrees of rush's each repository
// keeps, newest first, however long ago they were done.
const keepFinished = 3

// tidyable are the worktrees the automatic clean-up may try: rush's own
// (one not made by rush only ever goes by hand), each of whose sessions is
// due, or whose session is gone (as finished long ago), past the newest
// keepFinished finished ones in its repository. TidyWorktree then asks
// the rule of each.
func tidyable(wts []fleet.Worktree, due map[string]bool, finished map[string]time.Time, gone func(*fleet.Worktree) bool) []*fleet.Worktree {
	type fin struct {
		w    *fleet.Worktree
		last time.Time
	}
	byRepo := map[string][]fin{}
	for i := range wts {
		w := &wts[i]
		if !w.Own {
			continue
		}
		if len(w.Agents) == 0 {
			if gone != nil && gone(w) {
				byRepo[w.Repo] = append(byRepo[w.Repo], fin{w: w}) // finished long ago: the first to go
			}
			continue
		}
		f := fin{w: w}
		done := true
		for _, k := range w.Agents {
			t, ok := finished[k]
			done = done && ok
			if t.After(f.last) {
				f.last = t
			}
		}
		if done {
			byRepo[w.Repo] = append(byRepo[w.Repo], f)
		}
	}
	var out []*fleet.Worktree
	for _, fs := range byRepo {
		sort.SliceStable(fs, func(i, j int) bool { return fs[i].last.After(fs[j].last) })
		for _, f := range fs[min(keepFinished, len(fs)):] {
			ready := true
			for _, k := range f.w.Agents {
				ready = ready && due[k]
			}
			if ready {
				out = append(out, f.w)
			}
		}
	}
	return out
}

func (m *Model) onWorktrees(msg worktreesMsg) {
	c := &m.clean
	c.checking = false
	if msg.tmp != nil {
		c.tmp = *msg.tmp
	}
	if msg.full {
		c.wts, c.checked, c.agentTmp, c.outside = msg.wts, time.Now(), msg.agentTmp, msg.outside
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
	for p, why := range msg.kept {
		if _, said := c.kept[p]; !said {
			m.news("kept "+m.worktreeName(p)+"'s worktree: "+strings.TrimPrefix(why, filepath.Base(p)+" isn't safe to remove: ")+" · open it in Projects", newsRefProjects+m.worktreeRepo(p))
		}
		c.kept[p] = time.Now()
	}
	if len(msg.temp) > 0 {
		m.onCleanedQuiet(msg.temp)
	}
	if len(msg.gone) > 0 || len(msg.temp) > 0 || len(msg.kept) > 0 || msg.failed != nil {
		events.Log("tidy", map[string]any{"worktreesRemoved": msg.gone, "worktreesKept": msg.kept, "tempCleaned": len(msg.temp), "freed": msg.freed, "why": "done work untouched for " + m.store.Config.CleanupAfter().String(), "err": errText(msg.failed)})
	}
	switch {
	case len(msg.gone) > 0:
		names := make([]string, len(msg.gone))
		for i, p := range msg.gone {
			names[i] = m.worktreeName(p)
		}
		m.news("removed the worktrees of done work: "+strings.Join(names, ", ")+" · freed "+disk(msg.freed)+", nothing lost, branches kept", "")
	case msg.freed >= tempShown:
		m.news("cleaned up done agents' temp work: freed "+disk(msg.freed), "")
	}
	if msg.failed != nil {
		m.news("couldn't clean up done work: "+msg.failed.Error(), "")
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
		m.fail(text + " · " + msg.err.Error())
	} else {
		m.flash(text, false)
	}
	return m.scanWorktrees()
}

// removeLeft deletes what an agent left outside its project, looked at
// once more first.
func (m *Model) removeLeft(it fleet.LeftItem) tea.Cmd {
	return later(func() error { _, err := fleet.RemoveLeft(it); return err }, func(m *Model, err error) tea.Cmd {
		if err != nil {
			m.fail(err.Error())
		} else {
			m.flash("deleted "+tildify(it.Path), false)
		}
		return m.scanWorktrees()
	})
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
		m.fail(msg.err.Error())
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
			events.Log("leftover-ended", map[string]any{"agent": left[i].Agent, "pid": left[i].PID, "procs": left[i].Procs, "cmd": clipLog(left[i].Cmd), "why": "still running after its agent stopped"})
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

// ownWorktreeCommand is #worktree on|off: whether a session started from
// the box in this project gets its own worktree. Alone, it says which.
func (m *Model) ownWorktreeCommand(arg string) tea.Cmd {
	project := fleet.MainCheckout(m.startDir())
	name := filepath.Base(project)
	switch arg {
	case "on", "off":
		if m.store.Config.OwnWorktrees == nil {
			m.store.Config.OwnWorktrees = map[string]bool{}
		}
		m.store.Config.OwnWorktrees[project] = arg == "on"
		_ = m.store.SaveConfig()
	case "":
	default:
		m.flash("#worktree on or #worktree off", true)
		return nil
	}
	if m.store.Config.OwnWorktree(project) {
		m.flash("new sessions in "+name+" get a worktree and branch of their own · #worktree off", false)
	} else {
		m.flash("new sessions in "+name+" work in the checkout itself · #worktree on", false)
	}
	return nil
}

// landedMsg is sessions whose work landed: marked done, with what was done
// with their worktrees.
type landedMsg struct {
	done  map[string]string // agent key → why it's finished
	did   []string
	stays map[string]string // agent key → why its worktree stays
}

// settleLanded looks, once a minute and in the background, for sessions
// whose work has landed: their pull request merged (squashed too), or
// their branch on its target. Each is marked done, as ctrl+d would; a
// worktree rush made for it goes, by the rule (fleet.Verdict), with its
// branch, and the main checkout is fast-forwarded when it's clean and on
// the target. Another's worktree only ever goes by hand. One still asking
// you something waits till it's answered, and one with many pull requests
// till every one of them has merged or closed: one landing isn't its work
// done.
func (m *Model) settleLanded() tea.Cmd {
	c := &m.clean
	if m.tick%60 != 15 || c.settling || !m.leads() {
		return nil
	}
	type look struct {
		key, name, id string // id: the rush session's, for the pull requests it follows
		w             fleet.Worktree
	}
	now := time.Now()
	var looks []look
	for _, a := range m.snap.Agents {
		if a.Live() || a.Busy() || a.Peer != "" || c.settled[a.Key] || now.Sub(c.stayed[a.Key]) < time.Hour {
			continue
		}
		if asks(a) {
			continue // a question still open isn't finished, merged or not: looked at again once it's answered
		}
		if openPR(a) {
			continue // another of its pull requests is still open
		}
		var w fleet.Worktree
		switch ws := a.Workspace; {
		case ws != nil:
			w = fleet.Worktree{Path: ws.Path, Repo: ws.Repo, Branch: ws.Branch, Target: ws.Target, Base: ws.Base, Own: ws.Own}
		case a.PR != nil && a.PR.Merged() && a.PR.HeadOid != "" && a.Root != "" && a.Repo != a.Root:
			// One in a worktree of its own making, known by its merged
			// pull request alone: git isn't asked about the others.
			w = fleet.Worktree{Path: a.Repo, Repo: a.Root, Branch: a.Branch}
		default:
			continue
		}
		w.PR, w.Agents = a.PR, []string{a.Key}
		l := look{key: a.Key, name: oneLine(a.DisplayName), w: w}
		if a.Rush {
			l.id = a.ID
		}
		looks = append(looks, l)
	}
	if len(looks) == 0 {
		return nil
	}
	c.settling = true
	return func() tea.Msg {
		msg := landedMsg{done: map[string]string{}, stays: map[string]string{}}
		for i := range looks {
			l := &looks[i]
			if st, err := os.Stat(l.w.Path); err != nil || !st.IsDir() {
				continue // gone already
			}
			if l.id != "" && len(prdrive.Following(l.id)) > 0 {
				continue // still driving a pull request: a follower stops once it's merged or closed
			}
			ok, why := l.w.Landing()
			if !ok || l.id != "" && stirring(l.id) {
				continue
			}
			msg.done[l.key] = why
			did, err := fleet.Settle(&l.w)
			if err != nil {
				msg.stays[l.key] = err.Error()
			}
			if len(did) > 0 {
				msg.did = append(msg.did, l.name+": "+strings.Join(did, ", "))
			}
		}
		return msg
	}
}

// stirring is a rush session that's come to life since the snapshot
// settleLanded chose from: in a turn, with work of its own running, or
// with something queued for it. Its worktree is looked at again later:
// the look at git takes seconds, and a session woken in them works in a
// checkout being removed under it.
func stirring(id string) bool {
	info, err := host.ReadInfo(id)
	if err != nil {
		return true // can't say: not now
	}
	return info.State != "idle" && info.State != "stopped" || len(info.Background) > 0 || len(info.Queue) > 0
}

func (m *Model) onLanded(msg landedMsg) {
	c := &m.clean
	c.settling = false
	if c.settled == nil {
		c.settled, c.stayed, c.markedLanded = map[string]bool{}, map[string]time.Time{}, map[string]bool{}
	}
	now := time.Now()
	marked := 0
	type stay struct{ text, repo string }
	var stays []stay
	for k, why := range msg.done {
		if s, ok := msg.stays[k]; ok {
			if _, said := c.stayed[k]; !said {
				name, repo := "a done session", ""
				if a := m.agentByKey(k); a != nil {
					name, repo = "“"+oneLine(a.DisplayName)+"”", folderKey(a)
					if _, r, ok := strings.Cut(s, " isn't safe to remove: "); ok {
						s = r
					}
				}
				stays = append(stays, stay{"kept " + name + "'s worktree: " + s + " · open it in Projects", repo})
			}
			c.stayed[k] = now
		} else {
			c.settled[k] = true
		}
		if _, done := m.store.Overlay.Done[k]; !done && !c.markedLanded[k] {
			c.markedLanded[k] = true
			m.store.Overlay.Done[k] = now
			marked++
			events.Log("landed-done", map[string]any{"agent": k, "why": why})
		}
	}
	if marked > 0 {
		_ = m.store.SaveOverlay()
		m.refresh()
	}
	switch {
	case len(msg.did) > 0:
		m.news("work landed: "+strings.Join(msg.did, " · "), "")
	case marked > 0:
		m.news(fmt.Sprintf("work landed: %d put away in Earlier", marked), "")
	}
	for _, st := range stays {
		m.news(st.text, newsRefProjects+st.repo)
	}
}

// sessionGone is a worktree of rush's whose session is no more: rush names
// each after its session, and the session's folder is gone.
func sessionGone(w *fleet.Worktree) bool {
	_, err := os.Stat(filepath.Join(host.Root(), filepath.Base(w.Path)))
	return errors.Is(err, fs.ErrNotExist)
}

// openPR is whether any pull request a links is still open or a draft.
func openPR(a *fleet.Agent) bool {
	for _, pr := range a.PRs {
		if pr.State == "OPEN" || pr.State == "DRAFT" {
			return true
		}
	}
	return false
}

// worktreeName is a worktree by the session working in it, quoted, or by
// its branch, or its folder's name when rush knows neither.
func (m *Model) worktreeName(path string) string {
	for _, a := range m.snap.Agents {
		if a.Workspace != nil && a.Workspace.Path == path || a.Repo == path && a.Root != "" && a.Root != path {
			return "“" + oneLine(a.DisplayName) + "”"
		}
	}
	for _, w := range m.clean.wts {
		if w.Path == path && w.Branch != "" {
			return w.Branch
		}
	}
	return filepath.Base(path)
}

// worktreeRepo is the main checkout a worktree belongs to, "" if unknown.
func (m *Model) worktreeRepo(path string) string {
	for _, w := range m.clean.wts {
		if w.Path == path {
			return w.Repo
		}
	}
	for _, a := range m.snap.Agents {
		if a.Workspace != nil && a.Workspace.Path == path {
			return a.Workspace.Repo
		}
	}
	return ""
}
