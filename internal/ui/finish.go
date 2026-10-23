package ui

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/ghpr"
)

// The finishing card: once a session's turn has ended with a pull request
// open, the card above its box says first what stands between it and
// merged ("PR #118 can't merge yet: the unit check fails"), then each step
// on the way (pushed, opened, its checks one by one, review, merge), and
// keys for what to do: f has the agent fix it, m merges, w sets it aside
// till the pull request changes. It never takes the keys by itself; ↑
// gives it them, so a message that starts with m can't merge anything.

// finishCard is the pull request a finishing card is about, and what it
// says of it.
type finishCard struct {
	id   string // its state, to tell a new one from one set aside
	root string // the repository GitHub was asked in; "" when only the transcript knows it
	pr   ghpr.PR
}

// finishFor is the finishing card for the open session, if any: its turn
// over and nothing running, with a pull request open, or merged since.
func (m *Model) finishFor(c *hostConn) *finishCard {
	a := m.agentByKey(c.key)
	if a == nil || a.Live() || a.Busy() || c.sess == nil || len(c.sess.Pending()) > 0 {
		return nil
	}
	var pick *prRow
	prs := m.agentPRs(a)
	for i := range prs { // the latest open one, else the latest merged
		switch p := &prs[i]; {
		case p.pr.State == "OPEN":
			pick = p
		case p.pr.State == "MERGED" && (pick == nil || pick.pr.State != "OPEN"):
			pick = p
		}
	}
	if pick == nil {
		return nil
	}
	ok, failed, pending := pick.pr.Counts()
	f := &finishCard{root: pick.root, pr: pick.pr}
	f.id = fmt.Sprintf("finish:%d:%s:%d/%d/%d:%s", pick.pr.Number, pick.pr.State, ok, failed, pending, pick.pr.Decision)
	if f.id == c.finishGone {
		return nil
	}
	return f
}

// headline is what stands between the pull request and merged, in words.
func (f *finishCard) headline() (string, string) {
	p := &f.pr
	n := fmt.Sprintf("PR #%d", p.Number)
	ok, failed, pending := p.Counts()
	switch {
	case p.State == "MERGED":
		return n + " is merged", cGreen
	case failed == 1:
		return fmt.Sprintf("%s can't merge yet: the %s check fails", n, firstNonEmpty(failedChecks(p)[0], "one")), cRed
	case failed > 1:
		return fmt.Sprintf("%s can't merge yet: %d checks fail", n, failed), cRed
	case p.Decision == "CHANGES_REQUESTED":
		return n + " can't merge yet: changes were asked for", cYellow
	case pending > 0:
		return fmt.Sprintf("%s is waiting on its checks: %d of %d done", n, ok, ok+pending), cBlue
	case p.Draft:
		return n + " is a draft: its checks pass", cBlue
	case p.Decision == "REVIEW_REQUIRED":
		return n + " passes its checks and waits for a review", cBlue
	}
	return n + " is ready to merge", cGreen
}

// failedChecks are the names of a pull request's failing checks, "" for
// one only the transcript counted.
func failedChecks(p *ghpr.PR) []string {
	var out []string
	for _, c := range p.Checks {
		if c.State == "failed" {
			out = append(out, c.Name)
		}
	}
	return out
}

// finishRows draw the card: the headline, the failing checks, the steps
// with each check under them, and its keys.
func (m *Model) finishRows(c *hostConn, w int) []string {
	f := c.finish
	p := &f.pr
	head, col := f.headline()
	bar := paint(col, "▍")
	if c.cardFocus {
		bar = paint(cOrange, "▍")
	}
	var out []string
	row := func(s string) { out = append(out, onBg(qCard, bar+" "+fit(s, w-3), w)) }
	row(spread(paint(cBright+bold, head), dim(oneLine(p.Title)), w-3))
	for _, name := range failedChecks(p) {
		if name != "" {
			row(paint(cRed, "✗ ") + paint(cText, name))
		}
	}
	row("")
	ok, failed, pending := p.Counts()
	step := func(mark, col, s string) string { return paint(col, mark) + " " + paint(cText, s) }
	steps := []string{step("✓", cGreen, "pushed"), step("✓", cGreen, map[bool]string{true: "opened as a draft", false: "opened"}[p.Draft])}
	switch total := ok + failed + pending; {
	case total == 0:
		steps = append(steps, step("○", cDim, "no checks"))
	case failed > 0:
		steps = append(steps, step("✗", cRed, fmt.Sprintf("checks %d of %d", ok, total)))
	case pending > 0:
		steps = append(steps, step("◌", cOrange, fmt.Sprintf("checks %d of %d", ok, total)))
	default:
		steps = append(steps, step("✓", cGreen, fmt.Sprintf("checks %d of %d", ok, total)))
	}
	switch p.Decision {
	case "APPROVED":
		steps = append(steps, step("✓", cGreen, "approved"))
	case "CHANGES_REQUESTED":
		steps = append(steps, step("✗", cYellow, "changes asked for"))
	default:
		steps = append(steps, step("○", cDim, "review"))
	}
	if p.State == "MERGED" {
		steps = append(steps, step("✓", cGreen, "merged"))
	} else {
		steps = append(steps, step("○", cDim, "merge"))
	}
	row(strings.Join(steps, "   "))
	named := p.Checks[:0:0]
	for _, ch := range p.Checks {
		if ch.Name != "" {
			named = append(named, ch)
		}
	}
	const most = 8
	for i, ch := range named {
		if i == most {
			row(faint("└─ ") + dim(fmt.Sprintf("%d more", len(named)-most)))
			break
		}
		branch := "├─ "
		if i == len(named)-1 {
			branch = "└─ "
		}
		mark, mc := "◌", cOrange
		switch ch.State {
		case "passed":
			mark, mc = "✓", cGreen
		case "skipped":
			mark, mc = "–", cDim
		case "failed":
			mark, mc = "✗", cRed
		}
		row(faint(branch) + paint(mc, mark) + " " + paint(cText, ch.Name) + "  " + dim(ch.Workflow))
	}
	row("")
	k := func(key, label string) string { return paint(cText+bold, key) + " " + paint(cSub, label) }
	var keys []string
	if failed > 0 || p.Decision == "CHANGES_REQUESTED" {
		keys = append(keys, k("f", "fix it with this agent"))
	}
	if p.State == "OPEN" {
		label := "merge"
		if failed+pending > 0 || p.Decision != "" && p.Decision != "APPROVED" {
			label = "merge anyway"
		}
		keys = append(keys, k("m", label))
	}
	keys = append(keys, k("w", "set aside"), k("g", "GitHub"))
	if !c.cardFocus {
		keys = append(keys, faint("↑ gives it the keys"))
	}
	row(strings.Join(keys, "    "))
	return out
}

// finishKey answers the finishing card's keys, only while it has them.
func (m *Model) finishKey(c *hostConn, s string) (tea.Cmd, bool) {
	f := c.finish
	if f == nil || !c.cardFocus {
		return nil, false
	}
	p := &f.pr
	_, failed, _ := p.Counts()
	switch s {
	case "f":
		if failed == 0 && p.Decision != "CHANGES_REQUESTED" {
			return nil, true
		}
		c.cardFocus = false
		return m.sendAs(c, fixText(p)), true
	case "m":
		if p.State != "OPEN" {
			return nil, true
		}
		if f.root == "" {
			m.flash("rush hasn't heard from GitHub about this pull request yet", true)
			return nil, true
		}
		s := &prSheet{root: f.root, num: p.Number, last: *p}
		s.askMerge(m, p)
		return nil, true
	case "w":
		c.finishGone, c.finish, c.cardFocus = f.id, nil, false
		return nil, true
	case "g":
		return browse(p.URL), true
	}
	return nil, false
}

// fixText is what f asks the agent: the failing checks by name, or the
// changes a reviewer asked for.
func fixText(p *ghpr.PR) string {
	if names := failedChecks(p); len(names) > 0 {
		which := "Its failing checks"
		if len(names) == 1 && names[0] != "" {
			which = "The " + names[0] + " check"
		} else if named := strings.Join(names, ", "); strings.Trim(named, ", ") != "" {
			which = "These checks (" + named + ")"
		}
		return fmt.Sprintf("PR #%d can't merge yet. %s fails. Read why in its log (gh run view --log-failed), fix it, push, and tell me once the checks pass.", p.Number, which)
	}
	return fmt.Sprintf("PR #%d had changes asked for in review. Read the review (gh pr view %d --comments), make the changes, push, and tell me what you changed.", p.Number, p.Number)
}
