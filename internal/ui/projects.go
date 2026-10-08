package ui

import (
	"fmt"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// The Projects place: where agents work, and what they leave behind, on
// one page. On the left, every repository (◆) and other folder (◇) an
// agent worked in over the last day, then Temporary and System; on the
// right, the one picked, whole:
//
//   - a project: where it is and pushes to, what git says, its agents and
//     where each works, each of its worktrees and its size, the agents'
//     scratch in it (.claude/tmp), its last commits and pull requests.
//   - Temporary: ◌ /tmp and finished agents' temp work, none of it
//     anyone's work.
//   - System: processes no project owns, orphans first.
//
// ↑↓ move in the list; enter goes into what's picked on the right, ← or
// esc comes back. Nothing is deleted without the Delete sheet saying first
// what goes and what's lost.

// The list's rows that aren't projects, by id.
const (
	paneTemp   = "~temp"
	paneSystem = "~system"
)

// project is one repository on the page, and its agents by worktree ("" for
// the main checkout).
type project struct {
	key, title string
	agents     []*fleet.Agent
	open       bool // an agent in it is running
	repo       bool // a repository, not just a folder
}

func (m *Model) projects() []*project {
	byKey := map[string]*project{}
	keys := map[string]bool{}
	for _, a := range m.workAgents() {
		k := folderKey(a)
		p := byKey[k]
		if p == nil {
			p = &project{key: k}
			byKey[k] = p
		}
		p.agents = append(p.agents, a)
		p.open = p.open || a.Open() || a.Busy()
		keys[k] = true
	}
	titles := folderTitles(keys)
	out := make([]*project, 0, len(byKey))
	for _, p := range byKey {
		p.title = titles[p.key]
		p.repo = filepath.IsAbs(p.key) && m.rootIsRepo(p.key)
		sort.SliceStable(p.agents, func(i, j int) bool {
			a, b := p.agents[i], p.agents[j]
			if t, u := treeOf(a), treeOf(b); t != u {
				return t < u
			}
			return m.sortLess(a, b)
		})
		out = append(out, p)
	}
	// Repositories before other folders; those with an agent running
	// first; then by name, so a project keeps its place.
	sort.Slice(out, func(i, j int) bool {
		a, b := out[i], out[j]
		if a.repo != b.repo {
			return a.repo
		}
		if a.open != b.open {
			return a.open
		}
		return cmpLower(a.title, b.title) < 0
	})
	return out
}

// worktreeAt is what the clean-up scan knows of the worktree at path, or a
// bare unchecked one before it has looked.
func (m *Model) worktreeAt(path string) fleet.Worktree {
	for _, w := range m.clean.wts {
		if w.Path == path {
			return w
		}
	}
	return fleet.Worktree{Path: path}
}

// projectTrees are a project's linked worktrees: those its agents are in,
// then the rest git lists.
func projectTrees(p *project, f fleet.Folder) []string {
	var trees []string
	seen := map[string]bool{}
	for _, a := range p.agents {
		if t := treeOf(a); t != "" && !seen[t] {
			seen[t] = true
			trees = append(trees, t)
		}
	}
	for _, t := range f.Linked {
		if !seen[t] {
			seen[t] = true
			trees = append(trees, t)
		}
	}
	return trees
}

// ownCommits are a worktree's commits past what it came from, or past its
// upstream when its base isn't known.
func ownCommits(st fleet.GitState) int {
	if st.Base != "" {
		return st.BaseAhead
	}
	return st.Ahead
}

// procRow is a process in the System section.
type procRow struct {
	pid, depth, n int
	label, cmd    string
	mem           uint64
	cpu           float64
	start         time.Time
	role          fleet.Role
	other         bool // a Claude process of no agent, not an orphan's child
}

// endProc asks before ending a System process: an orphan's whole tree, or
// one process with SIGTERM.
func (m *Model) endProc(r procRow) {
	if r.role == fleet.RoleOrphan && r.other {
		m.confirm = &confirmation{
			question: fmt.Sprintf("End %s and everything under it, freeing about %s?", trimCmd(r.cmd, 40), mem(r.mem)),
			detail:   "SIGTERM, then SIGKILL after 3s",
			onYes:    func() tea.Cmd { return endOrphans([]procRow{r}) },
			bangText: "SIGKILL it now",
			onBang:   killTree(r.pid, r.start),
		}
		return
	}
	m.confirm = &confirmation{
		question: fmt.Sprintf("Send SIGTERM to %d?", r.pid),
		detail:   trimCmd(r.cmd, 80),
		onYes: func() tea.Cmd {
			return cmdErr(fmt.Sprintf("sent SIGTERM to %d", r.pid), func() error { return actions.Terminate(r.pid) })
		},
		bangText: "SIGKILL it and everything under it",
		onBang:   killTree(r.pid, r.start),
	}
}

// projectPRs are the pull requests a project's agents opened or pushed
// to, each once, open ones first.
func projectPRs(agents []*fleet.Agent) []agent.PR {
	var out []agent.PR
	seen := map[string]bool{}
	for _, a := range agents {
		for _, pr := range a.PRs {
			k := pr.URL
			if k == "" {
				k = strconv.Itoa(pr.Number)
			}
			if !seen[k] {
				seen[k] = true
				out = append(out, pr)
			}
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].State == "OPEN" && out[j].State != "OPEN" })
	return out
}

// prLine is a pull request in a line: number, state, checks and title.
func prLine(pr agent.PR, w int) string {
	col := cGreen
	switch {
	case pr.State == "MERGED":
		col = cBlue
	case pr.State == "CLOSED" || pr.State == "unknown":
		col = cDim
	case pr.Checks.Failed > 0:
		col = cRed
	}
	s := paint(col, "⇡ #"+strconv.Itoa(pr.Number)) + " " + dim(strings.ToLower(pr.State))
	if c := pr.Checks; c.Passed+c.Failed+c.Pending > 0 {
		s += "  " + paint(cGreen, fmt.Sprintf("%d✓", c.Passed))
		if c.Failed > 0 {
			s += " " + paint(cRed, fmt.Sprintf("%d✗", c.Failed))
		}
		if c.Pending > 0 {
			s += " " + dim(fmt.Sprintf("%d…", c.Pending))
		}
	}
	if pr.Review != "" {
		s += dim(" · " + strings.ToLower(strings.ReplaceAll(pr.Review, "_", " ")))
	}
	return s + "  " + paint(cText, fit(pr.Title, max(10, w-cellw.String(s)-2)))
}

// ---- drawing ----

// pbox is lines in a rounded box w wide, its title and meta in the top
// edge; lit when it has the cursor. Each line is fitted inside.
func pbox(title, meta string, body []string, w int, lit bool) []string {
	col := cEdge
	if lit {
		col = cOrange
	}
	e := func(s string) string { return paint(col, s) }
	head := e("╭─ ") + title + " "
	tail := ""
	if meta != "" {
		tail = " " + meta + " "
	}
	fill := max(1, w-cellw.String(head)-cellw.String(tail)-2)
	out := []string{fit(head+e(strings.Repeat("─", fill))+tail+e("─╮"), w)}
	for _, l := range body {
		out = append(out, e("│")+" "+fit(l, w-4)+" "+e("│"))
	}
	return append(out, e("╰"+strings.Repeat("─", w-2)+"╯"))
}

// psection heads a part of a box: a label in capitals, a count, and a
// hairline to the edge.
func psection(label, meta string, w int) string {
	s := paint(cSub+bold, strings.ToUpper(label))
	if meta != "" {
		s += "  " + dim(meta)
	}
	return s + " " + faint(strings.Repeat("─", max(0, w-cellw.String(s)-1)))
}

// thead is a table's column names.
func thead(cols ...any) string {
	var b strings.Builder
	for i := 0; i+1 < len(cols); i += 2 {
		b.WriteString(fit(cols[i].(string), cols[i+1].(int)))
	}
	return faint(b.String())
}

// shade paints a whole line on bg, keeping it through inner resets.
func shade(line, bg string, w int) string {
	line = fit(line, w)
	return bg + strings.ReplaceAll(line, reset, reset+bg) + reset
}

// lines draws rows w wide, the one at pick on the cursor: selected when
// the cursor is here, faintly otherwise.
func drawRows(rows []workRow, pick int, w int, here bool) []string {
	out := make([]string, len(rows))
	for i, r := range rows {
		out[i] = fit(r.line, w)
		if i == pick {
			bg := hoverBG
			if here {
				bg = selBG
			}
			out[i] = shade(r.line, bg, w)
		}
	}
	return out
}

// sideBySide sets two boxes of lines next to each other, the shorter
// padded with blank lines.
func sideBySide(l, r []string, lw int) []string {
	n := max(len(l), len(r))
	out := make([]string, n)
	for i := range n {
		a, b := "", ""
		if i < len(l) {
			a = l[i]
		}
		if i < len(r) {
			b = r[i]
		}
		out[i] = fit(a, lw) + " " + b
	}
	return out
}

// pad fills body out to n lines, so boxes set side by side end together.
func pad(body []string, n int) []string {
	for len(body) < n {
		body = append(body, "")
	}
	return body
}

// pickInProjects picks the list's row id ("" for the first project),
// from outside what's picked.
func (m *Model) pickInProjects(id string) {
	m.work.projIn, m.work.projPos, m.work.projSel = false, 0, id
}

// projectsOpen is what opening the Projects place starts: finding its
// folders' git state.
func (m *Model) projectsOpen() tea.Cmd {
	if m.mode != modeProjects {
		return nil
	}
	return m.refreshFolders()
}

// summaryBar is the page's top line: the machine's load and what can go
// without losing anything, at the right.
func (m *Model) summaryBar(w int) string {
	running := 0
	for _, p := range m.projects() {
		for _, a := range p.agents {
			if a.Live() || a.Busy() {
				running++
			}
		}
	}
	return spread("", m.projectsSummary(running), w)
}

// projectsSummary is the machine's load and how much can go without
// losing anything.
func (m *Model) projectsSummary(running int) string {
	mc := m.snap.Machine
	var safe int64
	for _, r := range m.cleanRows() {
		switch {
		case r.wt != nil && r.wt.Safe() && m.running(r.wt.Agents) == nil:
			safe += r.wt.Size
		case r.agent != nil && r.agent.PID == 0:
			safe += r.agent.Temp
		}
	}
	safe += m.clean.tmp.Stale
	s := paint(cOrange, fmt.Sprintf("● %d running", running)) + dim(fmt.Sprintf(" · %s ram · %.0f%% cpu", mem(mc.TotalMem), mc.TotalCPU))
	switch {
	case m.clean.checking && m.clean.checked.IsZero():
		s += dim(" · ") + paint(cOrange, spinner[m.tick%len(spinner)]) + dim(" asking git…")
	case safe > 0:
		s += dim(" · ") + paint(cGreen, disk(safe)) + dim(" can go ") + paint(cSub, "c")
	}
	return s
}

// projectRows are every row the page can pick, list and what's picked,
// for tests and the command bar.
func (m *Model) projectRows() []workRow {
	w := m.w - 4
	return append(m.projectList(w), m.pickedRows(w)...)
}

// pickedRows are what the list's pick shows on the right: a project whole,
// Temporary's or System's table.
func (m *Model) pickedRows(w int) []workRow {
	switch m.work.projSel {
	case paneTemp:
		return m.tempRows(w)
	case paneSystem:
		return m.systemRows(w)
	}
	if p := m.pickedProject(); p != nil {
		return m.projectDetail(p, w)
	}
	return nil
}

// ---- Projects ----

const projNameW, projGitW = 22, 24

// projectList is every project in one table: repositories, then the
// folders that aren't.
func (m *Model) projectList(w int) []workRow {
	list := m.projects()
	now := m.snap.At
	rows := []workRow{{line: thead("  NAME", projNameW, "GIT", projGitW, "AGENTS", 10)}}
	if len(list) == 0 {
		rows = []workRow{{line: dim("no agent has worked anywhere in the last day")}}
	}
	for i, p := range list {
		if i > 0 && p.repo != list[i-1].repo {
			rows = append(rows, workRow{}, workRow{line: psection("Other folders", "◇ not a repository", w)})
		}
		mark := kindMark(kindFolder)
		if p.repo {
			mark = kindMark(kindProject)
		}
		git := ""
		if f, ok := m.folders.byRoot[p.key]; ok && p.repo {
			git = paint(cSub, f.Git.Branch)
			if n := f.Git.Changed; n > 0 {
				git += " " + paint(cYellow, fmt.Sprintf("±%d", n))
			}
			if n := f.Worktrees; n > 0 {
				git += " " + paint(cBlue, fmt.Sprintf("⎇%d", n))
			}
		} else if p.repo {
			git = faint("asking git…")
		}
		line := mark + " " + fit(paint(cText+bold, p.title), projNameW-2) + fit(git, projGitW) + agentCounts(p.agents, now)
		rows = append(rows, workRow{id: "p" + p.key, proj: p, line: line})
	}
	// What's no project's: temp work, and processes.
	var temp int64
	for _, a := range m.snap.Agents {
		if a.Temp >= tempShown {
			temp += a.Temp
		}
	}
	temp += m.clean.tmp.Size
	procs := dim(fmt.Sprintf("%d processes", len(m.procs())))
	if n := m.snap.Machine.Orphans; n > 0 {
		procs = paint(cYellow, fmt.Sprintf("%d orphaned", n)) + dim(" · ") + procs
	}
	return append(rows, workRow{}, workRow{line: psection("No project's", "", w)},
		workRow{id: paneTemp, pane: true, line: kindMark(kindTemp) + " " + fit(paint(cText+bold, "Temporary"), projNameW-2) + dim(disk(temp))},
		workRow{id: paneSystem, pane: true, line: faint("⚙") + " " + fit(paint(cText+bold, "System"), projNameW-2) + procs})
}

// agentCounts is a project's agents in a few cells: working, needing you,
// your turn, then how many in all.
func agentCounts(agents []*fleet.Agent, now time.Time) string {
	var working, waiting, turn int
	for _, a := range agents {
		switch {
		case a.NeedsYou() || a.Waiting() || a.Halted():
			waiting++
		case a.YourTurn(now):
			turn++
		case a.Live() || a.Busy() || a.Checking:
			working++
		}
	}
	var parts []string
	if working > 0 {
		parts = append(parts, paint(cOrange, fmt.Sprintf("✱%d", working)))
	}
	if waiting > 0 {
		parts = append(parts, paint(cYellow+bold, fmt.Sprintf("?%d", waiting)))
	}
	if turn > 0 {
		parts = append(parts, paint(cGreen, fmt.Sprintf("◆%d", turn)))
	}
	return strings.Join(append(parts, dim(fmt.Sprintf("%d in all", len(agents)))), " ")
}

// pickedProject is the project the list's cursor is on.
func (m *Model) pickedProject() *project {
	if strings.HasPrefix(m.work.projSel, "~") {
		return nil
	}
	key := strings.TrimPrefix(m.work.projSel, "p")
	list := m.projects()
	for _, p := range list {
		if p.key == key {
			return p
		}
	}
	if len(list) > 0 {
		return list[0]
	}
	return nil
}

// projectDetail is one project whole: where it is, git, its agents and
// where each works, its worktrees, last commits and pull requests. Its
// agents and worktrees can be picked once enter has gone into it.
func (m *Model) projectDetail(p *project, w int) []workRow {
	now := m.snap.At
	f, known := m.folders.byRoot[p.key]
	var rows []workRow
	text := func(s string) { rows = append(rows, workRow{line: s, owner: p.key}) }
	if filepath.IsAbs(p.key) {
		where := paint(cSub, tildify(p.key))
		if f.Remote != "" {
			where += dim("  →  ") + paint(cBlue, f.Remote)
		}
		text(where)
	}
	switch {
	case !p.repo:
		text(dim("◇ not a repository") + dim(" · ") + paint(cSub, m.peekLine(p.key, now)))
	case !known:
		text(faint("asking git…"))
	default:
		g := gitBits(f.Git)
		if f.Git.Branch != "" && !f.Git.Upstream && f.Git.Err == "" {
			g += dim(" · ") + faint("not pushed anywhere")
		}
		text(g)
	}
	// Agents, each with where it works.
	text("")
	goes := 0
	for _, a := range p.agents {
		if m.canGo(a, now) {
			goes++
		}
	}
	meta := strconv.Itoa(len(p.agents))
	if goes > 0 {
		meta += fmt.Sprintf(" · %d can go: stopped, untouched %s, nothing left only in them · x closes one", goes, age(m.goesAfter()))
	}
	text(psection("Agents", meta, w))
	text(thead(" WHERE", 20, "   AGENT", 30, "DOING", 10))
	for _, a := range p.agents {
		where := dim("main checkout")
		if t := treeOf(a); t != "" {
			where = paint(cBlue, "⎇ ") + paint(cSub, filepath.Base(t))
		}
		line := " " + fit(where, 19) + m.workSession(a, w-20, now)
		switch {
		case m.canGo(a, now):
			line = fit(line, w-24) + "  " + paint(cGreen, "✓ can go")
		case a.Temp >= tempShown:
			line = fit(line, w-24) + "  " + m.tempStatus(a, now)
		}
		rows = append(rows, workRow{id: "a" + a.Key, a: a, owner: p.key, line: line})
	}
	// Worktrees, each with its size.
	if trees := projectTrees(p, f); p.repo && len(trees) > 0 {
		var size int64
		wrows := make([]workRow, 0, len(trees))
		for _, t := range trees {
			running := 0
			for _, a := range p.agents {
				if treeOf(a) == t && (a.Open() || a.Busy()) {
					running++
				}
			}
			wt := m.worktreeAt(t)
			size += wt.Size
			line := m.worktreeLine(t, f.Trees[t], wt, running, w)
			if _, ok := f.Trees[t]; !ok {
				nw, _, _, _ := wtCols(w)
				line = fit(kindMark(kindWorktree)+" "+dim(filepath.Base(t)), nw) + faint("asking git…")
			}
			wrows = append(wrows, workRow{id: "w" + t, wt: &wt, owner: p.key, line: line})
		}
		meta = strconv.Itoa(len(trees))
		if size > 0 {
			meta += " · " + disk(size)
		}
		text("")
		text(psection("Worktrees", meta, w))
		text(worktreeHead(w))
		rows = append(rows, wrows...)
	}
	// What agents keep as scratch in it: .claude/tmp.
	var scratch []workRow
	for _, d := range agentTmpDirs(p.key) {
		if n, ok := m.clean.agentTmp[d]; ok {
			nw, bw, sw, zw := wtCols(w)
			name := strings.TrimPrefix(d, p.key+"/")
			if d == host.ProjectTemp(p.key) {
				name = "rush sessions' tmp"
			}
			line := fit(kindMark(kindTemp)+" "+paint(cSub, name), nw+bw) + fit(dim(m.peekLine(d, now)), sw) + dim(right(disk(n), zw))
			scratch = append(scratch, workRow{id: "e" + d, path: &pathRow{path: d, scratch: p}, owner: p.key, line: line})
		}
	}
	if len(scratch) > 0 {
		text("")
		text(psection("Agents' scratch", "files agents kept while they worked here · x empties it", w))
		rows = append(rows, scratch...)
	}
	// What its agents wrote outside it and their scratch.
	var left []fleet.LeftItem
	var leftSize int64
	for _, l := range m.clean.outside {
		if !l.Gone && slices.ContainsFunc(p.agents, func(a *fleet.Agent) bool { return a.Key == l.Agent.Key }) {
			left, leftSize = append(left, l), leftSize+l.Size
		}
	}
	if len(left) > 0 {
		text("")
		text(psection("Left outside", fmt.Sprintf("%d · %s · c cleans up what's scratch", len(left), disk(leftSize)), w))
		for _, l := range left {
			nw, bw, sw, zw := wtCols(w)
			text(fit(kindMark(kindTemp)+" "+paint(cSub, tildify(l.Path)), nw+bw) + fit(dim(oneLine(l.Agent.DisplayName)+leftNote(l)), sw) + dim(right(disk(l.Size), zw)))
		}
	}
	if known && len(f.Recent) > 0 {
		text("")
		text(psection("Recent commits", "", w))
		for _, c := range f.Recent {
			text(faint("● ") + dim(right(age(now.Sub(c.At)), 4)+"  ") + paint(cText, c.Subject))
		}
	}
	if prs := projectPRs(p.agents); len(prs) > 0 {
		text("")
		text(psection("Pull requests", strconv.Itoa(len(prs)), w))
		for _, pr := range prs {
			text(prLine(pr, w))
		}
	}
	return rows
}

// ---- Worktrees ----

// wtCols are a worktree table's column widths at w: name, branch, state,
// size, and what it came from in the rest.
func wtCols(w int) (name, branch, state, size int) {
	name, branch, size = min(28, w/5), min(26, w/5), 8
	return name, branch, max(16, w-name-branch-size-3-min(30, w/6)), size
}

func worktreeHead(w int) string {
	nw, bw, sw, zw := wtCols(w)
	return thead("  WORKTREE", nw, "BRANCH", bw, "STATE", sw, right("SIZE", zw), zw, "   CAME FROM", 12)
}

// worktreeLine is a worktree in a table row: its name, branch, what it
// has of its own or that it's clean, its size, and what it came from.
func (m *Model) worktreeLine(t string, st fleet.GitState, wt fleet.Worktree, running, w int) string {
	var work []string
	if running > 0 {
		work = append(work, paint(cOrange, fmt.Sprintf("● %d running", running)))
	}
	if st.Err != "" {
		work = append(work, paint(cRed, st.Err))
	}
	if n := ownCommits(st); n > 0 {
		work = append(work, paint(cOrange, fmt.Sprintf("%d commit%s", n, plural(n))))
	}
	if st.Changed > 0 {
		work = append(work, paint(cYellow, fmt.Sprintf("%d changed", st.Changed)))
	}
	if wt.Err != "" {
		work = append(work, paint(cRed, wt.Err))
	}
	name := paint(cText, filepath.Base(t))
	if len(work) == 0 && st.Err == "" && wt.Err == "" {
		work = append(work, paint(cGreen, "✓ clean"))
		name = dim(filepath.Base(t))
	}
	from := ""
	if st.Base != "" {
		from = st.Base
		if st.BaseBehind > 0 {
			from += fmt.Sprintf(" · %d behind", st.BaseBehind)
		}
	}
	size := ""
	if wt.Size > 0 {
		size = disk(wt.Size)
	}
	branch := st.Branch
	if base := filepath.Base(t); branch == base || branch == "worktree-"+base {
		branch = "" // it says nothing the name doesn't
	}
	nw, bw, sw, zw := wtCols(w)
	return fit(kindMark(kindWorktree)+" "+name, nw) + fit(dim(branch), bw) +
		fit(strings.Join(work, dim(" · ")), sw) + dim(right(size, zw)) + "   " + faint(from)
}

// ---- Temporary ----

// tempRows are what agents leave behind that's no project's: what's yours
// in /tmp, and finished agents' temp work. None of it is anyone's work.
func (m *Model) tempRows(w int) []workRow {
	now := m.snap.At
	const nameW, sizeW = 40, 9
	rows := []workRow{{line: dim("Session scratch and caches, including running and older sessions. Stop a running session before cleaning its files.")}, {}, {line: thead("  WHAT", nameW, "     SIZE", sizeW, "   STATUS / LOCATION", 20)}}
	row := func(name, size, status string) string {
		return kindMark(kindTemp) + " " + fit(name, nameW-2) + dim(right(size, sizeW)) + "   " + status
	}
	if m.measuring {
		rows = append(rows, workRow{line: dim("measuring session storage… sizes appear as each folder finishes")})
	}
	n := len(rows)
	if s := m.clean.tmp; !s.Checked.IsZero() && s.Items > 0 {
		status := dim("all touched in the last day")
		if s.StaleItems > 0 {
			status = paint(cGreen, fmt.Sprintf("✓ %s in %d untouched for a day", disk(s.Stale), s.StaleItems)) + dim(" · x clears them")
		}
		rows = append(rows, workRow{id: "t", tmp: true, line: row(paint(cText, "/tmp")+dim(fmt.Sprintf("  %d of yours", s.Items)), disk(s.Size), status)})
	}
	var temp []*fleet.Agent
	for _, a := range m.snap.Agents {
		if a.Temp >= tempShown {
			temp = append(temp, a)
		}
	}
	sort.SliceStable(temp, func(i, j int) bool { return temp[i].Temp > temp[j].Temp })
	for _, a := range temp {
		open := m.work.opened[a.Key]
		mark := " ▸"
		if open {
			mark = " ▾"
		}
		rows = append(rows, workRow{id: "T" + a.Key, temp: a, line: row(paint(cSub, oneLine(a.DisplayName))+dim(mark), disk(a.Temp), m.tempStatus(a, now))})
		who := sessionEnded(a, now)
		if k, ok := agent.Get(agent.Kind(a.Kind)); ok {
			who = dim(k.Name()+" · ") + who
		}
		for _, d := range a.TempDirs() {
			rows = append(rows, workRow{line: fit(dim("    "+tildify(d.Path)), nameW+sizeW+1) + "   " + who})
		}
		if open {
			rows = append(rows, m.tempEntryRows(a, nameW, sizeW, now)...)
		}
	}
	if len(rows) == n && !m.measuring {
		return []workRow{{line: dim("no measured scratch in /tmp or session temp folders")}}
	}
	return rows
}

// tempStatus is when an agent's temp work goes.
func (m *Model) tempStatus(a *fleet.Agent, now time.Time) string {
	switch due := m.dueIn([]string{a.Key}, now); {
	case a.PID != 0:
		return paint(cYellow, "running · stop before cleaning")
	case due == 0:
		return paint(cGreen, "at the next tidy-up")
	case due > 0:
		return dim("in " + dur(due.Round(time.Minute)))
	default:
		return dim("x removes it")
	}
}

// ---- System ----

// procs are the processes no project owns: orphans, with the few holding
// most of each one's memory, then Claude processes that belong to no agent.
func (m *Model) procs() []procRow {
	tab := m.snap.Table
	if tab == nil {
		return nil
	}
	var procs []procRow
	for _, r := range m.snap.Machine.Rows {
		if r.Role != fleet.RoleOrphan {
			continue
		}
		procs = append(procs, procRow{pid: r.PID, label: r.Label, cmd: r.Cmd, mem: r.Mem, cpu: r.CPU, n: r.Procs, start: r.Start, role: r.Role, other: true})
		kids := tab.Tree(r.PID)[1:]
		sort.SliceStable(kids, func(i, j int) bool { return kids[i].Footprint > kids[j].Footprint })
		for _, n := range kids[:min(3, len(kids))] {
			if n.Footprint >= 32<<20 {
				procs = append(procs, procRow{pid: n.PID, depth: 1, cmd: m.shortCmd(n.PID, n.Comm), mem: n.Footprint, cpu: n.CPU, n: 1, start: n.Start, role: fleet.RoleOrphan})
			}
		}
	}
	for _, r := range m.snap.Machine.Rows {
		if r.Role == fleet.RoleWorker || r.Role == fleet.RoleOrphan {
			continue
		}
		procs = append(procs, procRow{pid: r.PID, label: r.Label, cmd: r.Cmd, mem: r.Mem, cpu: r.CPU, n: r.Procs, start: r.Start, role: r.Role, other: true})
	}
	return procs
}

func (m *Model) systemRows(w int) []workRow {
	now := m.snap.At
	procs := m.procs()
	if len(procs) == 0 {
		return []workRow{{line: dim("no process here belongs to no one")}}
	}
	const labelW, cpuW, memW, procW = 28, 9, 8, 7
	cmdW := max(20, w-labelW-cpuW-memW-procW-2)
	cols := func(r procRow) string {
		p := ""
		if r.n > 1 {
			p = strconv.Itoa(r.n)
		}
		return cpuColor(r.cpu, right(fmt.Sprintf("%.1f%%", r.cpu), cpuW)) + memColor(r.mem, right(mem(r.mem), memW)) + dim(right(p, procW))
	}
	var rows []workRow
	if mc := m.snap.Machine; mc.Orphans > 0 {
		fate := " · their sessions ended; they run until you end them · "
		if !m.store.Config.KeepOrphans {
			fate = " · their sessions ended; each is ended two minutes after · "
		}
		rows = append(rows, workRow{line: paint(cYellow+bold, fmt.Sprintf("%d orphaned", mc.Orphans)) + paint(cYellow, " · holding "+mem(mc.OrphanMem)) +
			dim(fate) + paint(cOrange, "x") + dim(" ends one, ") + paint(cOrange, "X") + dim(" all")}, workRow{})
	}
	rows = append(rows, workRow{line: thead("  PROCESS", labelW, "COMMAND", cmdW, right("CPU", cpuW), cpuW, right("MEM", memW), memW, right("PROCS", procW), procW)})
	lastRole := fleet.Role(-1)
	for _, r := range procs {
		switch {
		case r.other && r.role == fleet.RoleOrphan:
			lbl := fit(r.label+" · "+age(now.Sub(r.start))+" old", labelW-2)
			rows = append(rows, workRow{id: "s" + strconv.Itoa(r.pid), proc: &r, line: paint(cYellow, "! "+lbl) + paint(cText, fit(trimCmd(r.cmd, cmdW-2), cmdW)) + cols(r)})
		case r.other:
			if r.role != lastRole {
				lastRole = r.role
				rows = append(rows, workRow{line: psection(roleName(r.role), "", w)})
			}
			rows = append(rows, workRow{id: "s" + strconv.Itoa(r.pid), proc: &r, line: "  " + paint(cSub, fit(r.label, labelW-2)) + faint(fit(trimCmd(r.cmd, cmdW-2), cmdW)) + cols(r)})
		default:
			rows = append(rows, workRow{line: strings.Repeat(" ", labelW) + faint(fit("└ "+trimCmd(r.cmd, cmdW-4), cmdW)) + cols(r)})
		}
	}
	return rows
}

// ---- the page ----

func (m *Model) projectsBody() []string {
	w := m.w - 4
	out := []string{m.summaryBar(w), ""}
	lw := min(max(w*2/5, 64), 84)
	rw := w - lw - 1
	stack := w < 130
	if stack {
		lw, rw = w, w
	}
	list := m.projectList(lw - 4)
	pick := pickRow(list, &m.work.projPos, &m.work.projSel)
	// The boxes run to the bottom of the screen, whatever is in them.
	fill := m.wallH() - len(out) - 2
	detail := m.pickedRows(rw - 4)
	dpick := -1
	if m.work.projIn {
		dpick = pickRow(detail, &m.work.inPos, &m.work.inSel)
	}
	left := drawRows(list, pick, lw-4, !m.work.projIn)
	right := drawRows(detail, dpick, rw-4, m.work.projIn)
	var title, meta string
	switch p := m.pickedProject(); {
	case m.work.projSel == paneTemp:
		title, meta = kindMark(kindTemp)+" "+paint(cText+bold, "Temporary"), dim("/tmp and all sessions’ temp work")
	case m.work.projSel == paneSystem:
		title, meta = paint(cText+bold, "System"), dim("processes no project owns")
	case p != nil:
		mark := kindMark(kindFolder)
		if p.repo {
			mark = kindMark(kindProject)
		}
		title, meta = mark+" "+paint(cText+bold, p.title), folderMeta(p.agents, m.snap.At)
	}
	lbox := func(n int) []string {
		return pbox(paint(cText+bold, "Projects"), dim(fmt.Sprintf("%d", len(m.projects()))), pad(left, n), lw, !m.work.projIn)
	}
	rbox := func(n int) []string {
		return pbox(title, meta, pad(right, n), rw, m.work.projIn)
	}
	if stack {
		return append(append(append(out, lbox(0)...), ""), rbox(0)...)
	}
	n := max(len(left), len(right), fill)
	return append(out, sideBySide(lbox(n), rbox(n), lw)...)
}

// projCur is the row the cursor is on: in what's picked, once gone into
// it, or in the list.
func (m *Model) projCur() (rows []workRow, i int, pos *int, sel *string) {
	w := m.w - 4
	if m.work.projIn {
		rows = m.pickedRows(w)
		return rows, pickRow(rows, &m.work.inPos, &m.work.inSel), &m.work.inPos, &m.work.inSel
	}
	rows = m.projectList(w)
	return rows, pickRow(rows, &m.work.projPos, &m.work.projSel), &m.work.projPos, &m.work.projSel
}

func (m *Model) projectsHint() string {
	var r workRow
	if rows, i, _, _ := m.projCur(); i >= 0 {
		r = rows[i]
	}
	w := m.w - 4
	var tabs []string
	switch {
	case r.proc != nil:
		k := []string{"↑↓", "move", "x", "end", "!", "SIGKILL tree"}
		if m.snap.Machine.Orphans > 0 {
			k = append(k, "X", "end all orphans")
		}
		return keysFit(w, append(append(k, tabs...), "esc", "back")...)
	case r.wt != nil:
		return keysFit(w, append([]string{"↑↓", "move", "x", "remove…", "c", "clean up", "r", "check again"}, append(tabs, "esc", "back")...)...)
	case r.tmp:
		return keysFit(w, append([]string{"↑↓", "move", "x", "clear untouched…", "c", "clean up", "r", "look again"}, append(tabs, "esc", "back")...)...)
	case r.temp != nil:
		open := "open it"
		if m.work.opened[r.temp.Key] {
			open = "close it"
		}
		return keysFit(w, append([]string{"↑↓", "move", "enter", open, "x", "delete all of it…", "X", "every finished agent's…"}, append(tabs, "esc", "back")...)...)
	case r.path != nil:
		x := "delete it…"
		if r.path.scratch != nil {
			x = "empty it…"
		}
		return keysFit(w, append([]string{"↑↓", "move", "x", x}, append(tabs, "←", "back")...)...)
	case r.proj != nil || r.pane:
		return keysFit(w, append([]string{"↑↓", "move", "enter", "into it", "c", "clean up"}, append(tabs, "esc", "back")...)...)
	case r.a != nil:
		k := []string{"↑↓", "move", "enter", "open", "x", "close it…"}
		if r.a.Temp >= tempShown && r.a.PID == 0 {
			k = append(k, "t", "remove temp work")
		}
		k = append(k, "ctrl+y", "its PR", "ctrl+b", "keep going")
		return keysFit(w, append(k, "←", "the list", "esc", "back")...)
	}
	return keysFit(w, append(tabs, "esc", "back")...)
}

func (m *Model) projectsKey(s string) tea.Cmd {
	rows, i, pos, sel := m.projCur()
	var r workRow
	if i >= 0 {
		r = rows[i]
	}
	move := func(d int) {
		for j, n := i+sign(d), 0; i >= 0 && j >= 0 && j < len(rows); j += sign(d) {
			if rows[j].pickable() {
				*pos, *sel = j, rows[j].id
				if n++; n == abs(d) {
					break
				}
			}
		}
	}
	switch s {
	case "esc", "q", "left":
		if m.work.projIn {
			m.work.projIn = false
			return nil
		}
		m.setView(placeAgents)
	case "up", "k":
		move(-1)
	case "down", "j":
		move(1)
	case "pgup":
		move(-10)
	case "pgdown":
		move(10)
	case "enter", "right":
		switch {
		case r.proj != nil || r.pane:
			m.work.projIn, m.work.inPos, m.work.inSel = true, 0, ""
			if rows, j, _, _ := m.projCur(); j < 0 || rows[j].id == "" {
				m.work.projIn = false // nothing in it to pick
			}
		case r.a != nil:
			m.setView(placeAgents)
			m.sel = r.a.Key
			m.rebuild()
			return m.focusPane(r.a)
		case r.wt != nil:
			return m.askRemoveWorktree(*r.wt)
		case r.tmp:
			return m.askClearScratch()
		case r.temp != nil:
			return m.openTemp(r.temp)
		case r.path != nil:
			return m.askRemovePath(*r.path)
		}
	case "ctrl+y":
		return m.openPR(r.a)
	case "ctrl+b", "alt+g", "g":
		return m.keepGoing(r.a)
	case "x", "ctrl+x", "backspace", "delete":
		switch {
		case r.wt != nil:
			return m.askRemoveWorktree(*r.wt)
		case r.proc != nil:
			m.endProc(*r.proc)
		case r.tmp:
			return m.askClearScratch()
		case r.temp != nil:
			return m.askClean(r.temp)
		case r.path != nil:
			return m.askRemovePath(*r.path)
		case r.a != nil:
			return m.askClose(r.a)
		}
	case "t":
		if r.a != nil && r.a.Temp >= tempShown {
			return m.askClean(r.a)
		}
	case "!":
		if p := r.proc; p != nil {
			m.confirm = &confirmation{
				question: fmt.Sprintf("SIGKILL %d and everything under it?", p.pid),
				detail:   trimCmd(p.cmd, 80),
				onYes:    killTree(p.pid, p.start),
			}
		}
	case "X":
		if r.temp != nil {
			return m.askCleanAll()
		}
		if mc := m.snap.Machine; mc.Orphans > 0 {
			var ends []procRow
			for _, o := range m.systemRows(m.w - 4) {
				if o.proc != nil && o.proc.role == fleet.RoleOrphan && o.proc.other {
					ends = append(ends, *o.proc)
				}
			}
			m.confirm = &confirmation{
				question: fmt.Sprintf("End all %d orphaned process trees and free about %s?", len(ends), mem(mc.OrphanMem)),
				detail:   "SIGTERM, then SIGKILL after 3s",
				onYes:    func() tea.Cmd { return endOrphans(ends) },
			}
		}
	case "A", "c":
		return m.openCleanSheet()
	case "r":
		m.clean.tmp.Checked = time.Now().Add(-time.Hour) // /tmp too, not only when it's due
		return m.scanWorktrees()
	}
	return nil
}
