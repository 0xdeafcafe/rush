package ui

import (
	"fmt"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Split by project, each section of the list has its agents together by
// the repository they work in, a linked worktree's under the repository it
// came from, each headed by what git says of it. The Agents place's
// Projects page shows the same repositories whole. Git is asked in the
// background, every folderEvery at most.
const folderEvery = 15 * time.Second

// splitProjects is whether the list's sections are split by project.
func (m *Model) splitProjects() bool { return m.store.Config.SplitBy != "none" }

// toggleSplit splits the list's sections by project, or stops.
func (m *Model) toggleSplit() {
	if m.splitProjects() {
		m.store.Config.SplitBy = "none"
		m.flash("sections no longer split by project", false)
	} else {
		m.store.Config.SplitBy = "project"
		m.flash("each section split by project", false)
	}
	_ = m.store.SaveConfig()
	m.rebuild()
}

// folderCache is what git last said of each folder in the list.
type folderCache struct {
	byRoot  map[string]fleet.Folder
	looking int // projects git is still being asked about
	asked   time.Time
}

// foldersMsg is one project as git said it: each project's answer shows
// as soon as it's in, not once the slowest repository's is.
type foldersMsg fleet.Folder

// scratchSection holds agents working in temp folders, which are no one's
// project; noFolder those whose folder isn't known.
const (
	scratchSection = "scratch"
	noFolder       = "No folder"
	rushSection    = "rush's advisor · Haiku looks for savings, Opus checks"
)

// folderKey is the folder an agent's row sits under: its repository's
// main checkout, or where it works when that isn't a repository.
func folderKey(a *fleet.Agent) string {
	switch {
	case a.Advisor:
		return rushSection
	case a.Root != "":
		return a.Root
	case host.IsTemp(a.Cwd):
		return scratchSection
	case a.Cwd == "":
		return noFolder
	}
	return a.Cwd
}

// folderTitles names each folder by its last element; where two share
// it, by as many of their last elements as tells them apart, else by the
// whole path.
func folderTitles(keys map[string]bool) map[string]string {
	tail := func(k string, n int) string {
		parts := strings.Split(strings.Trim(k, "/"), "/")
		return strings.Join(parts[max(0, len(parts)-n):], "/")
	}
	out := make(map[string]string, len(keys))
	for n := 1; n <= 3; n++ {
		count := map[string]int{}
		for k := range keys {
			if _, ok := out[k]; !ok {
				count[tail(k, n)]++
			}
		}
		for k := range keys {
			if _, ok := out[k]; ok {
				continue
			}
			switch {
			case k == scratchSection || k == noFolder || k == rushSection:
				out[k] = k
			case !filepath.IsAbs(k):
				out[k] = tildify(k)
			case count[tail(k, n)] == 1:
				out[k] = tail(k, n)
			}
		}
	}
	for k := range keys {
		if _, ok := out[k]; !ok {
			out[k] = tildify(k)
		}
	}
	return out
}

// treeOf is the linked worktree an agent works in, or "" for the main
// checkout and anywhere that isn't a repository.
func treeOf(a *fleet.Agent) string {
	if a.Root != "" && a.Repo != a.Root {
		return a.Repo
	}
	return ""
}

// folderMeta is a folder section's count of what its agents are doing:
// only what's happening, working first, then what waits on you.
func folderMeta(agents []*fleet.Agent, now time.Time) string {
	var working, waiting, turn int
	var cost float64
	for _, a := range agents {
		cost += a.Spend.Cost
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
		parts = append(parts, paint(cOrange, fmt.Sprintf("%d working", working)))
	}
	if waiting > 0 {
		parts = append(parts, paint(cYellow+bold, fmt.Sprintf("%d need you", waiting)))
	}
	if turn > 0 {
		parts = append(parts, paint(cGreen, fmt.Sprintf("%d your turn", turn)))
	}
	parts = append(parts, dim(sectionMeta(len(agents), cost)))
	return strings.Join(parts, dim(" · "))
}

// gitBits is a checkout's state in a few cells: branch, how far it is
// from its upstream, and how much is uncommitted.
func gitBits(s fleet.GitState) string {
	if s.Err != "" {
		return faint("git failed")
	}
	var parts []string
	if s.Branch != "" {
		b := paint(cSub, s.Branch)
		if s.Ahead > 0 {
			b += " " + paint(cOrange, fmt.Sprintf("↑%d", s.Ahead))
		}
		if s.Behind > 0 {
			b += " " + paint(cYellow, fmt.Sprintf("↓%d", s.Behind))
		}
		parts = append(parts, b)
	}
	if s.Changed > 0 {
		parts = append(parts, dim(fmt.Sprintf("%d changed", s.Changed)))
	} else {
		parts = append(parts, faint("clean"))
	}
	return strings.Join(parts, dim(" · "))
}

func (m *Model) rootIsRepo(root string) bool {
	for _, a := range m.snap.Agents {
		if a.Root == root {
			return true
		}
	}
	return false
}

// projectLine heads a project's rows inside a section, as a quiet rule
// under the section's so it reads as a heading, not a row: its name and
// what git says of it in short, under every section it heads.
// Too narrow for both, git's part goes under the name rather than being cut.
func (m *Model) projectLine(l listLine, w int) string {
	s := "  " + paint(cSub, l.title)
	if g := m.folderShort(l.root, m.headPR(l.root, "")); g != "" {
		if cellw.String(s)+2+cellw.String(g)+2 > w {
			return s + "\n    " + fit(g, w-5)
		}
		s += "  " + g
	}
	return s + " " + faint(strings.Repeat("┄", max(0, w-cellw.String(s)-3)))
}

// headPR is the pull request a heading's agents linked, an open one first,
// as "#7536 open": the agents in root's checkout, or tree's when given.
func (m *Model) headPR(root, tree string) string {
	var agents []*fleet.Agent
	for _, a := range m.order {
		if folderKey(a) == root && treeOf(a) == tree {
			agents = append(agents, a)
		}
	}
	prs := projectPRs(agents)
	if len(prs) == 0 {
		return ""
	}
	pr := prs[0]
	s := paint(prColor(pr), fmt.Sprintf("#%d %s", pr.Number, strings.ToLower(pr.State)))
	if len(prs) > 1 {
		s += faint(fmt.Sprintf(" +%d", len(prs)-1))
	}
	return s
}

// folderShort is what git says of a project in a few cells, its PR among
// it (gitShort), then how many worktrees it has.
func (m *Model) folderShort(root, pr string) string {
	f, ok := m.folders.byRoot[root]
	if !ok {
		if filepath.IsAbs(root) && m.rootIsRepo(root) {
			return faint("…")
		}
		return ""
	}
	s := gitShort(f.Git, pr, targetShort(f.Git))
	switch {
	case f.Worktrees == 1:
		s += faint("  1 worktree")
	case f.Worktrees > 1:
		s += faint(fmt.Sprintf("  %d worktrees", f.Worktrees))
	}
	return s
}

// gitShort is gitBits in a few cells, for the list: branch ↑ahead
// ↓behind, its PR, what it targets, HEAD's commit and ±changed files.
func gitShort(s fleet.GitState, pr, target string) string {
	if s.Err != "" {
		return faint("git failed")
	}
	var head []string
	if s.Branch != "" {
		head = append(head, dim(s.Branch))
	}
	if s.Ahead > 0 {
		head = append(head, paint(cOrange, fmt.Sprintf("↑%d", s.Ahead)))
	}
	if s.Behind > 0 {
		head = append(head, paint(cYellow, fmt.Sprintf("↓%d", s.Behind)))
	}
	changed := ""
	if s.Changed > 0 {
		changed = dim(fmt.Sprintf("±%d", s.Changed))
	}
	var parts []string
	for _, p := range []string{strings.Join(head, " "), pr, target, faint(s.Commit), changed} {
		if ansi.Strip(p) != "" {
			parts = append(parts, p)
		}
	}
	return strings.Join(parts, "  ")
}

// targetShort is the upstream a branch tracks, unless it's only the
// branch's own name on origin, which says nothing.
func targetShort(s fleet.GitState) string {
	if s.Target == "" || s.Target == "origin/"+s.Branch {
		return ""
	}
	return faint("→ ") + dim(s.Target)
}

// baseShort is where a worktree's branch came from, and how far it's
// gone from there: "from main ↑3 ↓1". With an upstream of its own, the
// arrows are the upstream's, so only the name shows.
func baseShort(s fleet.GitState) string {
	if s.Base == "" {
		return ""
	}
	out := faint("from ") + dim(s.Base)
	if s.Upstream {
		return out
	}
	if s.BaseAhead > 0 {
		out += " " + paint(cOrange, fmt.Sprintf("↑%d", s.BaseAhead))
	}
	if s.BaseBehind > 0 {
		out += " " + paint(cYellow, fmt.Sprintf("↓%d", s.BaseBehind))
	}
	return out
}

// treeLine heads a linked worktree's rows under its project.
func (m *Model) treeLine(l listLine, w int) string {
	s := "    " + m.treeTag(l)
	if cellw.String(s)+2 > w {
		return fit(s, w)
	}
	return s + " " + faint(strings.Repeat("┄", max(0, w-cellw.String(s)-3)))
}

// treeTag is a worktree's name, what git says of it, and where it came
// from, for a tree line: its heading, or beside its one agent's name.
func (m *Model) treeTag(l listLine) string {
	name := filepath.Base(l.root)
	s := faint("↳ ") + paint(cBlue, name)
	if st, ok := m.folders.byRoot[l.title].Trees[l.root]; ok {
		if strings.ReplaceAll(st.Branch, "/", "-") == name {
			st.Branch = "" // the folder is named for it: once is enough
		}
		if g := gitShort(st, m.headPR(l.title, l.root), baseShort(st)); g != "" {
			s += "  " + g
		}
	} else if m.folders.byRoot[l.title].Root != "" || m.folders.looking > 0 {
		s += "  " + faint("…")
	}
	return s
}

// refreshFolders asks git about the projects on screen, in the
// background: the list's when split by project, every repository with an
// agent in the last day on the Projects page, which wants each whole. It
// asks every folderEvery, and at once when a project has no answer yet.
func (m *Model) refreshFolders() tea.Cmd {
	if m.folders.looking > 0 {
		return nil
	}
	var listed []*fleet.Agent
	whole := m.mode == modeProjects
	switch {
	case whole:
		listed = m.workAgents()
	case m.mode == modeList && m.splitProjects():
		for _, a := range m.order {
			if m.groupOf[a.Key] != "Earlier" {
				listed = append(listed, a)
			}
		}
	default:
		return nil
	}
	wants := fleet.FolderWants(listed)
	if len(wants) == 0 {
		return nil
	}
	stale := time.Since(m.folders.asked) >= folderEvery
	for root, trees := range wants {
		f, ok := m.folders.byRoot[root]
		if !ok || whole && !f.Whole {
			stale = true
			break
		}
		for _, t := range trees {
			if _, ok := f.Trees[t]; !ok {
				stale = true
			}
		}
	}
	if !stale {
		return nil
	}
	m.folders.looking, m.folders.asked = len(wants), time.Now()
	cmds := make([]tea.Cmd, 0, len(wants))
	for root, trees := range wants {
		cmds = append(cmds, func() tea.Msg {
			folderGate <- struct{}{}
			defer func() { <-folderGate }()
			return foldersMsg(fleet.CheckFolder(root, trees, whole))
		})
	}
	return tea.Batch(cmds...)
}

// folderGate lets a few projects ask git at once, not one per repository.
var folderGate = make(chan struct{}, 4)

// onFolders keeps what git said of one project; the others keep what it
// said before until theirs is in.
func (m *Model) onFolders(msg foldersMsg) {
	m.folders.looking = max(0, m.folders.looking-1)
	if m.folders.byRoot == nil {
		m.folders.byRoot = map[string]fleet.Folder{}
	}
	m.folders.byRoot[msg.Root] = fleet.Folder(msg)
}
