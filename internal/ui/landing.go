package ui

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// What's left before an agent's work lands, and Earlier has it by itself
// (settleLanded): an agent in a worktree of its own that's stopped work
// says so on its row, worst first. An agent in the main checkout has
// nothing to land, so it says nothing.

type landKind int

const (
	landNone        landKind = iota // not in a worktree, at work, or git hasn't said yet
	landUncommitted                 // files changed and not committed
	landPRFailing                   // its pull request's checks fail
	landPRChanges                   // its pull request has changes asked for
	landPRWaiting                   // its pull request waits on checks, a review or a draft
	landPRReady                     // its pull request could be merged now
	landUnmerged                    // commits not on its target, and no pull request open
	landLanded                      // merged, or on its target: Earlier has it shortly
	landNothing                     // no work of its own in the worktree
)

// landState is what's left before an agent's work lands: what kind, and
// the few words its row says it in.
type landState struct {
	kind   landKind
	text   string
	color  string
	target string // the branch its work is meant to land on
	pr     int
}

// toLand is whether all that's left is yours to do: merge it.
func (l landState) toLand() bool { return l.kind == landPRReady || l.kind == landUnmerged }

// tag is the landState as its row shows it.
func (l landState) tag() string {
	if l.kind == landNone {
		return ""
	}
	return paint(l.color, l.text)
}

// landOf works out what's left before a's work lands, from what git and
// GitHub last said: no git is run here.
func (m *Model) landOf(a *fleet.Agent) landState {
	tree := treeOf(a)
	if tree == "" || a.Done || a.Peer != "" || a.Live() || a.Busy() || a.Checking {
		return landState{}
	}
	st, known := m.folders.byRoot[a.Root].Trees[tree]
	known = known && st.Err == ""
	l := landState{target: landTarget(a, st)}
	pr := a.PR
	if pr != nil {
		l.pr = pr.Number
	}
	n := ""
	if pr != nil {
		n = fmt.Sprintf("PR #%d ", pr.Number)
	}
	switch {
	case known && st.Changed > 0:
		l.kind, l.color, l.text = landUncommitted, cRed, fmt.Sprintf("±%d not committed", st.Changed)
	case pr != nil && pr.Merged():
		l.kind, l.color, l.text = landLanded, cGreen, "landed · Earlier soon"
	case pr != nil && pr.Open() && pr.Checks == "fail":
		l.kind, l.color, l.text = landPRFailing, cRed, n+"✗ checks"
	case pr != nil && pr.Open() && pr.Review == "changes":
		l.kind, l.color, l.text = landPRChanges, cYellow, n+"changes asked"
	case pr != nil && pr.State == "draft":
		l.kind, l.color, l.text = landPRWaiting, cDim, n+"draft"
	case pr != nil && pr.Open() && pr.Checks == "pending":
		l.kind, l.color, l.text = landPRWaiting, cDim, n+"checks running"
	case pr != nil && pr.Open():
		l.kind, l.color, l.text = landPRReady, cOrange, n+"ready to merge"
	case !known:
	case st.BaseAhead > 0:
		l.kind, l.color = landUnmerged, cOrange
		l.text = fmt.Sprintf("%d commit%s not on %s", st.BaseAhead, plural(st.BaseAhead), l.target)
	default:
		l.kind, l.color, l.text = landNothing, cDim, "nothing to land"
	}
	return l
}

// landTarget is the branch a's work is meant to land on: its session's,
// else the one its worktree was made from, else main.
func landTarget(a *fleet.Agent, st fleet.GitState) string {
	if w := a.Workspace; w != nil && w.Target != "" {
		return w.Target
	}
	return firstNonEmpty(st.Base, "main")
}

// landText is what the agent is told to do to land its work.
func landText(l landState) string {
	pr := fmt.Sprintf("pull request #%d", l.pr)
	switch l.kind {
	case landUncommitted:
		return "Land your work on " + l.target + ": commit what's left in this worktree, then get it onto " + l.target + " the way this project does (open a pull request and merge it once it's green, or merge the branch straight in)."
	case landPRFailing:
		return "The checks on " + pr + " are failing. Fix them, push, and merge it once it's green."
	case landPRChanges:
		return "Changes were asked for on " + pr + ". Make them, push, and merge it once it's approved and green."
	case landPRWaiting:
		return "Get " + pr + " merged: mark it ready if it's a draft, see its checks through, and merge it once it's green."
	case landPRReady:
		return "Merge " + pr + " into " + l.target + "."
	case landUnmerged:
		return "Land your work on " + l.target + ": get this branch onto " + l.target + " the way this project does (open a pull request and merge it once it's green, or merge the branch straight in)."
	}
	return ""
}

// land tells a to land its work, as a message you see it get (alt+p,
// #land).
func (m *Model) land(a *fleet.Agent) tea.Cmd {
	if a == nil {
		m.flash("select an agent first", true)
		return nil
	}
	l := m.landOf(a)
	switch {
	case a.Interactive:
		m.flash(a.DisplayName+" is open in another terminal", true)
		return nil
	case a.Live() || a.Busy():
		m.flash(a.DisplayName+" is still working", false)
		return nil
	case treeOf(a) == "":
		m.flash(a.DisplayName+" works in the main checkout: nothing to land", false)
		return nil
	case l.kind == landNone:
		m.flash("git hasn't said yet what "+a.DisplayName+" has to land · try again in a moment", false)
		return nil
	case l.kind == landLanded:
		m.flash(a.DisplayName+"'s work has landed · Earlier has it soon", false)
		return nil
	case l.kind == landNothing:
		m.flash(a.DisplayName+" has nothing to land · alt+d puts it in Earlier", false)
		return nil
	}
	text := landText(l)
	return m.replyTo(a, text, text)
}
