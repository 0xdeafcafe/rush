package ui

import (
	"fmt"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
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
// says of it. An agent on several has them all on the one card, a tab
// each: ← → (or a click) moves between them.
type finishCard struct {
	id   string // its state, to tell a new one from one set aside
	root string // the repository GitHub was asked in; "" when only the transcript knows it
	pr   ghpr.PR
	// merging is how it's being merged, while it is; asking is whether
	// GitHub is being asked about it again.
	merging string
	asking  bool
	// failed is why its last merge from here didn't go through.
	failed error
	// tabs are every pull request on the card, this one among them, in
	// number order; at is this one's place in them.
	tabs []finishTab
	at   int
}

// finishTab is one pull request on the card.
type finishTab struct {
	id   string
	root string
	pr   ghpr.PR
}

// finishFor is the finishing card for the open session, if any: its turn
// over, with pull requests open, or one merged since.
func (m *Model) finishFor(c *hostConn) *finishCard {
	a := m.agentByKey(c.key)
	// Its turn over: what it left running (a follower waiting on the
	// checks) doesn't hide what the pull request needs.
	if a == nil || a.Live() || c.sess == nil || len(c.sess.Pending()) > 0 {
		return nil
	}
	// Every open one, and the latest merged: what was just finished.
	var pick []prRow
	merged := -1
	for _, p := range m.agentPRs(a) {
		switch {
		case p.pr.State == "OPEN":
			pick = append(pick, p)
		case p.pr.State == "MERGED" && (merged < 0 || p.pr.Number > pick[merged].pr.Number):
			if merged >= 0 {
				pick = slices.Delete(pick, merged, merged+1)
			}
			merged = len(pick)
			pick = append(pick, p)
		}
	}
	if len(pick) == 0 && a.PR != nil && (a.PR.Open() || a.PR.Merged()) {
		pick = []prRow{{pr: fromStatus(*a.PR)}} // found from its branch: any harness's
	}
	f := &finishCard{asking: m.prs.asking[folderKey(a)]}
	for _, p := range pick {
		key := fmt.Sprintf("%s#%d", folderKey(a), p.pr.Number)
		if m.prs.merged[key] && p.pr.State == "OPEN" {
			p.pr.State = "MERGED" // merged from here: GitHub's lists catch up
		}
		ok, failed, pending := p.pr.Counts()
		id := fmt.Sprintf("finish:%d:%s:%d/%d/%d:%s", p.pr.Number, p.pr.State, ok, failed, pending, p.pr.Decision)
		if c.finishGone[id] {
			continue
		}
		f.tabs = append(f.tabs, finishTab{id: id, root: p.root, pr: p.pr})
	}
	if len(f.tabs) == 0 {
		return nil
	}
	slices.SortFunc(f.tabs, func(x, y finishTab) int { return x.pr.Number - y.pr.Number })
	// The one you picked while it's still here; else, one that just
	// changed; else the one that most needs you.
	f.at = -1
	for i, t := range f.tabs {
		if t.pr.Number == c.finishAt {
			f.at = i
		}
	}
	if old := c.finish; old != nil && !c.cardFocus {
		was := map[int]string{}
		for _, t := range old.tabs {
			was[t.pr.Number] = t.id
		}
		for i, t := range f.tabs {
			if id, ok := was[t.pr.Number]; ok && id != t.id {
				f.at = i // it moved on: show it
			}
		}
	}
	if f.at < 0 {
		f.at = 0
		for i, t := range f.tabs {
			if needs(&t.pr) > needs(&f.tabs[f.at].pr) {
				f.at = i
			}
		}
	}
	m.finishShow(c, f, f.at)
	return f
}

// finishShow puts the card's tab i up.
func (m *Model) finishShow(c *hostConn, f *finishCard, i int) {
	t := f.tabs[i]
	f.at, f.id, f.root, f.pr = i, t.id, t.root, t.pr
	key := fmt.Sprintf("%s#%d", folderKeyOf(m, c), t.pr.Number)
	f.merging, f.failed = m.prs.merging[key], m.prs.mergeFailed[key]
	c.finishAt = t.pr.Number
}

// folderKeyOf is the folder key of the session's agent.
func folderKeyOf(m *Model, c *hostConn) string {
	if a := m.agentByKey(c.key); a != nil {
		return folderKey(a)
	}
	return ""
}

// needs is how much a pull request needs you, to show first the one that
// most does: failing or changes asked, then ready, a review, its checks,
// a draft, and merged last.
func needs(p *ghpr.PR) int {
	_, failed, pending := p.Counts()
	switch {
	case p.State == "MERGED":
		return 0
	case failed > 0 || p.Decision == "CHANGES_REQUESTED":
		return 6
	case p.Draft:
		return 1
	case pending > 0:
		return 3
	case p.Decision == "REVIEW_REQUIRED":
		return 4
	}
	return 5
}

// headline is what stands between the pull request and merged, in words.
func (f *finishCard) headline() (string, string) {
	p := &f.pr
	n := fmt.Sprintf("PR #%d", p.Number)
	ok, failed, pending := p.Counts()
	switch {
	case p.State == "MERGED":
		return n + " is merged", cGreen
	case f.merging != "":
		return n + " is merging…", cOrange
	case f.failed != nil:
		return n + " didn't merge: " + f.failed.Error(), cRed
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
	row := func(s string) { out = append(out, onBg(qCard, bar+"  "+fit(s, w-4), w)) }
	c.finishBtns = c.finishBtns[:0]
	if len(f.tabs) > 1 {
		row(m.finishStrip(c, f, w-4))
	} else {
		row("") // room above, as below
	}
	num := link(p.URL, paint(cBlue, fmt.Sprintf("#%d ↗", p.Number)))
	if f.asking {
		num = paint(cOrange, "◌ ") + dim("asking GitHub  ") + num
	}
	row(spread(paint(cBright+bold, head), num, w-4))
	row(prFacts(p, w-4))
	for _, name := range failedChecks(p) {
		if name != "" {
			row(paint(cRed, "✗ ") + paint(cText, name))
		}
	}
	row("")
	ok, failed, pending := p.Counts()
	// ✓ done, ✗ stopped, – not needed here, ▸ where it is now, ○ still to come.
	here := false
	step := func(mark, col, s string) string { return paint(col, mark) + " " + paint(cText, s) }
	next := func(s string) string {
		if here {
			return step("○", cDim, dim(s))
		}
		here = true
		return paint(cOrange+bold, "▸ "+s)
	}
	steps := []string{step("✓", cGreen, "pushed"), step("✓", cGreen, map[bool]string{true: "opened as a draft", false: "opened"}[p.Draft])}
	switch total := ok + failed + pending; {
	case total == 0:
		steps = append(steps, step("–", cDim, dim("no checks")))
	case failed > 0:
		here = true
		steps = append(steps, step("✗", cRed, fmt.Sprintf("checks %d of %d", ok, total)))
	case pending > 0:
		here = true
		steps = append(steps, step("◌", cOrange, fmt.Sprintf("checks %d of %d", ok, total)))
	default:
		steps = append(steps, step("✓", cGreen, fmt.Sprintf("checks %d of %d", ok, total)))
	}
	switch p.Decision {
	case "APPROVED":
		steps = append(steps, step("✓", cGreen, "approved"))
	case "CHANGES_REQUESTED":
		here = true
		steps = append(steps, step("✗", cYellow, "changes asked for"))
	case "REVIEW_REQUIRED":
		steps = append(steps, next("review"))
	default:
		steps = append(steps, step("–", cDim, dim("no review needed")))
	}
	switch {
	case p.State == "MERGED":
		steps = append(steps, step("✓", cGreen, "merged"))
	case f.merging != "":
		steps = append(steps, step("◌", cOrange, "merging ("+f.merging+")…"))
	case f.failed != nil:
		steps = append(steps, step("✗", cRed, "merge failed"))
	default:
		steps = append(steps, next("merge"))
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
	if p.State == "OPEN" && f.merging == "" {
		label := "merge"
		if failed+pending > 0 || p.Decision != "" && p.Decision != "APPROVED" {
			label = "merge anyway"
		}
		if f.failed != nil {
			label = "merge again"
		}
		keys = append(keys, k("m", label))
	}
	keys = append(keys, k("w", "set aside"), k("g", "GitHub"))
	if len(f.tabs) > 1 {
		keys = append(keys, k("← →", "the others"))
	}
	if !c.cardFocus {
		keys = append(keys, faint("↑ gives it the keys"))
	}
	row(strings.Join(keys, "    "))
	row("")
	return out
}

// finishStrip is the card's top row with several pull requests: a tab
// each, its number and where it stands, the one shown lit; a click on one
// shows it.
func (m *Model) finishStrip(c *hostConn, f *finishCard, w int) string {
	var b strings.Builder
	x := 3 // past the card's edge and its room
	for i, t := range f.tabs {
		tab := " " + finishTabText(&t.pr) + " "
		if i == f.at {
			tab = paint(cBright+bold, "▸") + selBG + strings.ReplaceAll(paint(cBright+bold, tab), reset, reset+selBG) + reset
		} else {
			tab = " " + dim(tab)
		}
		tw := cellw.String(tab)
		if x+tw > w {
			b.WriteString(dim(fmt.Sprintf(" +%d", len(f.tabs)-i)))
			break
		}
		c.finishBtns = append(c.finishBtns, cardBtn{x: x, y: 0, w: tw, h: 1, key: fmt.Sprintf("pr:%d", t.pr.Number)})
		b.WriteString(tab)
		x += tw
	}
	return b.String()
}

// finishTabText is a pull request on its tab: its number, and in a word
// or a mark where it stands.
func finishTabText(p *ghpr.PR) string {
	ok, failed, pending := p.Counts()
	s := paint(prStateColor(p), fmt.Sprintf("#%d", p.Number))
	switch {
	case p.State == "MERGED":
		return s + " " + paint(cBlue, "merged")
	case failed > 0:
		return s + " " + paint(cRed, fmt.Sprintf("✗ %d failing", failed))
	case p.Decision == "CHANGES_REQUESTED":
		return s + " " + paint(cYellow, "changes asked")
	case pending > 0:
		return s + " " + paint(cOrange, fmt.Sprintf("◌ %d/%d", ok, ok+pending))
	case p.Draft:
		return s + " " + dim("draft")
	case p.Decision == "REVIEW_REQUIRED":
		return s + " " + paint(cBlue, "review")
	}
	return s + " " + paint(cGreen, "✓ ready")
}

// prFacts is a pull request's title, then how big it is, its comments
// and where its review stands, as much as GitHub has said.
func prFacts(p *ghpr.PR, w int) string {
	var facts []string
	if p.Added+p.Removed > 0 {
		facts = append(facts, paint(cGreen, fmt.Sprintf("+%d", p.Added))+" "+paint(cRed, fmt.Sprintf("−%d", p.Removed)))
	}
	if p.Files > 0 {
		facts = append(facts, dim(fmt.Sprintf("%d file%s", p.Files, plural(p.Files))))
	}
	if p.Comments > 0 {
		facts = append(facts, paint(cText, fmt.Sprintf("%d comment%s", p.Comments, plural(p.Comments))))
	}
	if len(p.Waiting) > 0 {
		facts = append(facts, dim("review asked of "+strings.Join(p.Waiting, ", ")))
	}
	right := strings.Join(facts, faint(" · "))
	return spread(paint(cText, cellwTrunc(p.Title, max(10, w-cellw.String(right)-3))), right, w)
}

// link is text that opens url when clicked, where the terminal can.
func link(url, text string) string {
	if url == "" {
		return text
	}
	return "\x1b]8;;" + url + "\x1b\\" + text + "\x1b]8;;\x1b\\"
}

// fromStatus is a pull request as its branch's status says it: its
// checks only told as a whole, one standing in for all.
func fromStatus(s fleet.PR) ghpr.PR {
	p := ghpr.PR{Number: s.Number, Title: s.Title, URL: s.URL, State: strings.ToUpper(s.State), Draft: s.State == "draft"}
	if p.Draft {
		p.State = "OPEN"
	}
	switch s.Review {
	case "approved":
		p.Decision = "APPROVED"
	case "changes":
		p.Decision = "CHANGES_REQUESTED"
	}
	switch s.Checks {
	case "pass":
		p.Checks = []ghpr.Check{{State: "passed"}}
	case "fail":
		p.Checks = []ghpr.Check{{Name: s.Failing, State: "failed"}}
	case "pending":
		p.Checks = []ghpr.Check{{State: "pending"}}
	}
	return p
}

// finishKey answers the finishing card's keys, only while it has them.
func (m *Model) finishKey(c *hostConn, s string) (tea.Cmd, bool) {
	f := c.finish
	if f == nil || !c.cardFocus {
		return nil, false
	}
	if n, ok := strings.CutPrefix(s, "pr:"); ok {
		for i, t := range f.tabs {
			if fmt.Sprint(t.pr.Number) == n {
				m.finishShow(c, f, i)
			}
		}
		return nil, true
	}
	if d := map[string]int{"left": -1, "right": 1, "tab": 1, "shift+tab": -1, "h": -1, "l": 1}[s]; d != 0 && len(f.tabs) > 1 {
		m.finishShow(c, f, (f.at+d+len(f.tabs))%len(f.tabs))
		return nil, true
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
		// Just this one: the others stay on the card.
		if c.finishGone == nil {
			c.finishGone = map[string]bool{}
		}
		c.finishGone[f.id] = true
		if f.at < len(f.tabs) {
			f.tabs = slices.Delete(f.tabs, f.at, f.at+1)
		}
		if len(f.tabs) == 0 {
			c.finish, c.cardFocus = nil, false
			return nil, true
		}
		m.finishShow(c, f, min(f.at, len(f.tabs)-1))
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
