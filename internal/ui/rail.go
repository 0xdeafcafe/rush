package ui

import (
	"cmp"
	"fmt"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// The rail look draws the Agents list as one left edge per section,
// coloured by what the section wants of you, with each agent hung off it
// as a branch, a dim line under it for where it works, and every other
// agent on a faint band. Settings › Interface › Look of the list; "classic"
// is the list as it was, with a heading line per project.

// bandBG is the faint ground every other agent sits on in the rail look.
var bandBG string

// rail is whether the Agents list is drawn in the rail look.
func (m *Model) rail() bool { return m.store.Config.ListLook != "classic" }

// railColor is the colour of a section's edge: what it wants of you.
func railColor(section string) string {
	switch section {
	case needsSection:
		return cGreen
	case finishedSection:
		return cBlue
	case stuckSection:
		return cYellow
	case workingSection:
		return cOrange
	}
	return cFaint
}

// railSection is a section's heading on its edge: its name and figures,
// without the rule the classic look runs out to the right.
func (m *Model) railSection(l listLine, w int) string {
	meta := l.meta
	if l.title == needsSection && m.sharedContext() != "" {
		meta += "  ·  " + m.sharedContext()
	}
	head := paint(railColor(l.title), "┃") + " " + paint(cBright+bold, l.title) + "  " + dim(meta)
	return fit(head, w)
}

// railLast is whether the agent on line i is the last at its depth before
// its section ends, so its branch closes (└─) rather than runs on (├─).
func railLast(lines []listLine, i int) bool {
	for _, l := range lines[i+1:] {
		switch l.kind {
		case lineAgent:
			if l.inset <= lines[i].inset {
				return false
			}
		case lineSection, lineBlank:
			return true
		}
	}
	return true
}

// agentWhere is an agent's second line in the rail look: its project,
// branch, how far it is from pushed, its uncommitted files and its PR.
func (m *Model) agentWhere(a *fleet.Agent, w int) string {
	root := folderKey(a)
	var parts []string
	if t := m.projTitles[root]; t != "" {
		parts = append(parts, dim(t))
	}
	f := m.folders.byRoot[root]
	st := f.Git
	if t := treeOf(a); t != "" {
		st = f.Trees[t]
	}
	if b := cmp.Or(st.Branch, a.Branch); b != "" {
		parts = append(parts, dim(b))
	}
	if st.Ahead > 0 {
		parts = append(parts, paint(cOrange, fmt.Sprintf("↑%d", st.Ahead)))
	}
	if st.Behind > 0 {
		parts = append(parts, paint(cYellow, fmt.Sprintf("↓%d", st.Behind)))
	}
	if st.Changed > 0 {
		parts = append(parts, dim(fmt.Sprintf("±%d", st.Changed)))
	}
	switch n := len(a.PRs); {
	case a.PR != nil: // found from the branch, for every harness
		parts = append(parts, statusShort(*a.PR))
	case n > 0:
		parts = append(parts, prShort(a.PRs[n-1]))
	}
	return fit(strings.Join(parts, faint(" · ")), w)
}

// prShort is a PR in a few words, coloured by what it needs: failing
// checks first, then where it stands.
func prShort(pr agent.PR) string {
	n, c := fmt.Sprintf("PR #%d", pr.Number), pr.Checks
	switch {
	case pr.State == "MERGED":
		return paint(cBlue, n+" merged")
	case pr.State == "CLOSED":
		return dim(n + " closed")
	case c.Failed > 0:
		return paint(cRed, fmt.Sprintf("%s ✗ %d failed", n, c.Failed))
	case c.Pending > 0:
		return dim(fmt.Sprintf("%s · checks %d of %d", n, c.Passed, c.Passed+c.Failed+c.Pending))
	case strings.EqualFold(pr.Review, "APPROVED"):
		return paint(cGreen, n+" ✓ approved")
	case pr.State == "DRAFT":
		return dim(n + " draft")
	case c.Passed > 0:
		return paint(cGreen, n+" ✓")
	}
	return paint(cBlue, n)
}

// railRows draws one agent in the rail look: its row on a branch off the
// section's edge, then its where-it-works line, and the stacked look's
// reason under that when it has one. Every other top-level agent sits on
// a band; a row that wants you keeps its own wash.
func (m *Model) railRows(l listLine, i, w, nameCol int, two, sel bool, edge string, band *bool, wash *string, emit func(line, key string, sel bool), cont func()) {
	a, last := l.agent, railLast(m.lines, i)
	// The row's own first cell is a blank (the classic look's marker), so
	// the branch needs no space after it.
	branch, under := "├─", "│"
	if last {
		branch, under = "└─", " "
	}
	pad := edge + " " + faint(branch)
	if l.inset > 0 { // a room's member, under the row that opened it
		pad = edge + " " + faint("│ ") + strings.Repeat(" ", l.inset)
	} else {
		*band = !*band
	}
	padW := 1 + 1 + 2 + max(0, l.inset)
	rw := w - padW
	*wash = m.rowWash(a, sel)
	if *wash == "" && *band && !sel {
		*wash = bandBG
	}
	key := a.Key
	_, inRoom := m.roomOf(a)
	switch {
	case inRoom && strings.HasPrefix(key, roomKeyPrefix):
		emit(pad+m.roomLine(a, rw, sel), key, sel)
	case inRoom:
		emit(pad+m.roomMemberLine(a, rw, sel), key, sel)
	default:
		emit(pad+m.agentLine(a, rw, w, sel, nameCol-padW, two, ""), key, sel)
	}
	hang := edge + " " + faint(under) + "    " // under the title, past the row's marker
	where := m.agentWhere(a, rw-3)
	if sum := m.viewSummary(a); sum != "" {
		where = fit(paint(cText, oneLine(sum)), rw-3) // its own view says where it stands
	}
	if where != "" {
		emit(hang+where, key, sel)
		cont()
	}
	if two && (m.saysWhy(a) || m.hasFilterMatch(a)) {
		emit(hang+m.agentSub(a, rw-3), key, sel)
		cont()
	}
	*wash = ""
}

// statusShort is a branch's pull request in a few words, as GitHub last
// said it: its first failing check by name.
func statusShort(pr fleet.PR) string {
	n := fmt.Sprintf("PR #%d", pr.Number)
	switch {
	case pr.Merged():
		return paint(cBlue, n+" merged")
	case pr.State == "closed":
		return dim(n + " closed")
	case pr.Checks == "fail":
		return paint(cRed, strings.TrimSpace(n+" ✗ "+pr.Failing))
	case pr.Checks == "pending":
		return dim(n + " · checks running")
	case pr.Review == "approved":
		return paint(cGreen, n+" ✓ approved")
	case pr.Review == "changes":
		return paint(cYellow, n+" · changes asked for")
	case pr.State == "draft":
		return dim(n + " draft")
	case pr.Checks == "pass":
		return paint(cGreen, n+" ✓")
	}
	return paint(cBlue, n)
}
