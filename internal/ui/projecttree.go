package ui

import (
	"fmt"
	"path/filepath"
	"sort"

	"github.com/0xdeafcafe/photon/cellw"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// In the rail look, Projects is one tree: each repository on an edge, its
// main checkout and every worktree hung off it, each line ending in what
// clean-up may do with it (fleet.Worktree.Verdict). Temporary, System and
// the pull requests stay as rows at the bottom. Enter on a repository
// opens it whole, as ever; on a worktree, asks to remove it.

// Column widths of a worktree's line in the tree.
const (
	treeNameW   = 22
	treeBranchW = 24
	treeSizeW   = 7
	treeVerdW   = 34
)

// projectTree is the Projects list as a tree, for the rail look.
func (m *Model) projectTree(w int) []workRow {
	list := m.projects()
	var rows []workRow
	if len(list) == 0 {
		rows = append(rows, workRow{line: dim("no agent has worked anywhere in the last day")})
	}
	for i, p := range list {
		if i > 0 && p.repo != list[i-1].repo {
			rows = append(rows, workRow{}, workRow{line: psection("Other folders", "◇ not a repository", w)})
		}
		rows = append(rows, m.projectTreeRows(p, w)...)
	}
	tail := m.classicProjectList(w) // its last rows: pull requests, Temporary, System
	for i, r := range tail {
		if r.id == panePRs {
			rows = append(append(rows, workRow{}), tail[i:]...)
			break
		}
	}
	return rows
}

// projectTreeRows are one repository's: its heading, its main checkout,
// then its worktrees, a subagent's after the rest and those rush didn't
// make last. A folder that isn't a repository is only its heading.
func (m *Model) projectTreeRows(p *project, w int) []workRow {
	edge := paint(cFaint, "┃")
	if p.open {
		edge = paint(cOrange, "┃")
	}
	f, asked := m.folders.byRoot[p.key]
	trees := m.repoWorktrees(p, f)
	var total int64
	for _, t := range trees {
		total += t.Size
	}
	name := paint(cText+bold, p.title)
	if !p.repo {
		name = kindMark(kindFolder) + " " + name // not a repository: no worktrees to hang off it
	}
	head := edge + " " + name + "  " + dim(tildify(p.key))
	if asked && p.repo {
		head += dim(" · " + f.Git.Branch)
		if n := f.Git.Behind; n > 0 {
			head += dim(" · ") + paint(cYellow, fmt.Sprintf("%d behind", n))
		}
	} else if p.repo {
		head += faint(" · asking git…")
	}
	right := ""
	if total > 0 {
		right = dim(disk(total))
	}
	rows := []workRow{{id: "p" + p.key, proj: p, line: spread(head, right+" ", w)}}
	if !p.repo {
		return rows
	}
	n := len(trees) + 1
	branch := func(i int) string {
		if i == n-1 {
			return "└─"
		}
		return "├─"
	}
	// The main checkout: what's uncommitted in it, and whether it's behind.
	main := faint("clean")
	switch {
	case !asked:
		main = faint("…")
	case f.Git.Changed > 0:
		main = paint(cYellow, fmt.Sprintf("%d changed", f.Git.Changed))
	}
	verdict := faint("keep")
	if asked && f.Git.Behind > 0 {
		verdict = paint(cYellow, "needs a pull")
	}
	rows = append(rows, workRow{line: m.wtTreeLine(edge, branch(0), " ", dim("main checkout"), dim(f.Git.Branch), main, 0, verdict, w)})
	for i, t := range trees {
		rows = append(rows, workRow{id: "w" + t.Path, wt: t, owner: p.key,
			line: m.wtTreeLine(edge, branch(i+1), m.treeMark(t), paint(cText, treeName(t)), dim(t.Branch), m.treeWhat(t), t.Size, treeVerdict(t), w)})
	}
	return rows
}

// repoWorktrees are a repository's worktrees as clean-up's scan found
// them: rush's own and Claude Code's first, a subagent's next, those made
// by hand last; by name within each.
func (m *Model) repoWorktrees(p *project, f fleet.Folder) []*fleet.Worktree {
	var out []*fleet.Worktree
	seen := map[string]bool{}
	for i := range m.clean.wts {
		if t := &m.clean.wts[i]; t.Repo == p.key {
			out, seen[t.Path] = append(out, t), true
		}
	}
	for _, path := range projectTrees(p, f) { // not scanned yet: shown, unchecked
		if !seen[path] {
			t := m.worktreeAt(path)
			t.Repo = p.key
			out = append(out, &t)
		}
	}
	rank := map[string]int{fleet.MadeByRush: 0, fleet.MadeByClaude: 0, fleet.MadeBySubagent: 1, fleet.MadeByOther: 2}
	sort.SliceStable(out, func(i, j int) bool {
		if a, b := rank[out[i].Made], rank[out[j].Made]; a != b {
			return a < b
		}
		return treeName(out[i]) < treeName(out[j])
	})
	return out
}

// treeName is a worktree's name in the tree: its folder's, with where it
// sits when rush didn't make it.
func treeName(t *fleet.Worktree) string {
	name := filepath.Base(t.Path)
	if t.Made == fleet.MadeByOther {
		if dir := filepath.Base(filepath.Dir(t.Path)); dir != "" && dir != "." {
			return dir + "/" + name
		}
	}
	return name
}

// treeMark is a worktree's session's state: working, waiting on you, or
// finished; none without one.
func (m *Model) treeMark(t *fleet.Worktree) string {
	now := m.snap.At
	for _, a := range m.order {
		if treeOf(a) != t.Path {
			continue
		}
		switch {
		case a.Live() || a.Busy():
			return paint(cOrange, "✱")
		case a.NeedsYou() || a.Waiting() || a.Halted():
			return paint(cGreen, "●")
		case a.YourTurn(now) || a.JustFinished(now):
			return paint(cBlue, "●")
		}
		return faint("·")
	}
	return " "
}

// treeWhat is the middle of a worktree's line: its pull request, else its
// session's title, else who made it.
func (m *Model) treeWhat(t *fleet.Worktree) string {
	var s string
	if pr := t.PR; pr != nil {
		n := fmt.Sprintf("PR #%d", pr.Number)
		switch {
		case pr.Merged():
			s = dim(n + " merged")
		case pr.Checks == "fail":
			s = paint(cRed, n+" ✗") + " " + dim(firstNonEmpty(pr.Failing, ""))
		case pr.Checks == "pass" || pr.Review == "approved":
			s = paint(cGreen, n+" ✓")
		default:
			s = paint(cBlue, n)
		}
		return s + " " + paint(cText, oneLine(pr.Title))
	}
	for _, a := range m.order {
		if treeOf(a) == t.Path {
			s = paint(cText, oneLine(a.DisplayName))
			if t.Changed > 0 {
				s += dim(fmt.Sprintf(" · %d uncommitted", t.Changed))
			}
			return s
		}
	}
	switch t.Made {
	case fleet.MadeBySubagent:
		return dim("a subagent's")
	case fleet.MadeByOther:
		return dim("not made by rush")
	}
	return dim("no session in it")
}

// treeVerdict is what clean-up may do with a worktree, in a few words:
// keep it while in use or its PR is open, blue when it can go, yellow
// with what it would lose.
func treeVerdict(t *fleet.Worktree) string {
	v := t.Verdict()
	switch v.Kind {
	case fleet.KeepInUse:
		return dim("in use")
	case fleet.KeepPROpen:
		return dim("PR open")
	case fleet.CanGo:
		if !t.Own {
			return paint(cBlue, "can go, by hand")
		}
		return paint(cBlue, "can go")
	}
	return paint(cYellow, v.Reason)
}

// wtTreeLine lays out one line under a repository: branch, mark, name,
// branch name, what it is, size, verdict.
func (m *Model) wtTreeLine(edge, branch, mark, name, gitBranch, what string, size int64, verdict string, w int) string {
	left := edge + " " + faint(branch) + " " + mark + " " + fit(name, treeNameW) + fit(gitBranch, treeBranchW)
	sz := ""
	if size > 0 {
		sz = disk(size)
	}
	tail := faint(right1(sz, treeSizeW)) + "  " + fit(verdict, treeVerdW)
	room := w - cellw.String(left) - cellw.String(tail) - 1
	return left + fit(what, max(0, room)) + " " + tail
}
