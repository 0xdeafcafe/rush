package ui

import (
	"context"
	"fmt"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Pull requests in Projects: each repository's open pull requests that
// concern you, the ones you opened and the ones asking for your review,
// as GitHub says them through gh. The list's Pull requests row has every
// project's, a project's own page its own; enter on one opens it whole:
// its checks, who approved and who it still waits on, and its body.
// GitHub is asked in the background while Projects is open, every
// prEvery at most.
const (
	prEvery = time.Minute
	panePRs = "~prs"
)

// prCache is what GitHub last said of each repository's pull requests.
type prCache struct {
	byRoot map[string]ghpr.Repo
	asking map[string]bool
	// logged is each session's pull requests already written to its
	// events, by state: "<id>#<n>" → state.
	logged map[string]string
}

// prRow is a pull request on the page, and the repository it's in.
type prRow struct {
	root string
	pr   ghpr.PR
}

// prGate lets a couple of repositories ask GitHub at once.
var prGate = make(chan struct{}, 3)

// refreshPRs asks GitHub again of each repository on the Projects page
// whose answer is older than prEvery.
func (m *Model) refreshPRs() tea.Cmd {
	var roots []string
	if m.mode == modeProjects {
		for _, p := range m.projects() {
			if p.repo {
				roots = append(roots, p.key)
			}
		}
	}
	// The open agent's, for its Overview and the edge of its box.
	if c := m.host; c != nil {
		if a := m.agentByKey(c.key); a != nil && a.Repo != "" {
			if k := folderKey(a); filepath.IsAbs(k) && !slices.Contains(roots, k) {
				roots = append(roots, k)
			}
		}
	}
	var cmds []tea.Cmd
	for _, key := range roots {
		if m.prs.asking[key] || time.Since(m.prs.byRoot[key].At) < prEvery {
			continue
		}
		if m.prs.asking == nil {
			m.prs.asking = map[string]bool{}
		}
		m.prs.asking[key] = true
		root := key
		cmds = append(cmds, func() tea.Msg {
			prGate <- struct{}{}
			defer func() { <-prGate }()
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()
			r := ghpr.List(ctx, root)
			return sheetMsg{apply: func(m *Model) tea.Cmd { m.onPRs(r); return nil }}
		})
	}
	return tea.Batch(cmds...)
}

func (m *Model) onPRs(r ghpr.Repo) {
	delete(m.prs.asking, r.Root)
	if m.prs.byRoot == nil {
		m.prs.byRoot = map[string]ghpr.Repo{}
	}
	old := m.prs.byRoot[r.Root]
	if r.Err != "" && len(old.PRs) > 0 {
		r.PRs = old.PRs // GitHub failing once doesn't lose what it said before
	}
	if r.Err != old.Err {
		events.Log("prs-fetch", map[string]any{"root": r.Root, "err": clipLog(r.Err), "was": clipLog(old.Err)})
	}
	m.prs.byRoot[r.Root] = r
	m.logPRs(r)
}

// logPRs writes to each rush session's events the pull requests GitHub
// says it has in r, when one is new to it or changed state: when and how
// rush linked it, to debug by.
func (m *Model) logPRs(r ghpr.Repo) {
	if r.Err != "" {
		return
	}
	for _, a := range m.snap.Agents {
		if !a.Rush || folderKey(a) != r.Root {
			continue
		}
		for _, row := range m.agentPRs(a) {
			p := row.pr
			key := fmt.Sprintf("%s#%d", a.ID, p.Number)
			if m.prs.logged[key] == p.State {
				continue
			}
			if m.prs.logged == nil {
				m.prs.logged = map[string]string{}
			}
			m.prs.logged[key] = p.State
			how := "its transcript"
			if a.Branch != "" && p.Head == a.Branch {
				how = "its branch " + a.Branch
			}
			host.LogEvent(a.ID, "pr", map[string]any{"number": p.Number, "state": p.State, "title": p.Title, "head": p.Head, "found": how, "fromGitHub": row.root != ""})
		}
	}
}

// askPRsAgain forgets when GitHub was last asked, so it's asked now.
func (m *Model) askPRsAgain() tea.Cmd {
	for k, r := range m.prs.byRoot {
		r.At = time.Time{}
		m.prs.byRoot[k] = r
	}
	return m.refreshPRs()
}

// prsOf is a project's pull requests as rows, or a line saying why there
// are none: GitHub not asked yet, or not able to say.
func (m *Model) prsOf(p *project, w int) []workRow {
	r, ok := m.prs.byRoot[p.key]
	switch {
	case !ok && m.prs.asking[p.key]:
		return []workRow{{line: paint(cOrange, spinner[m.tick%len(spinner)]) + faint(" asking GitHub…"), owner: p.key}}
	case !ok:
		return nil
	case r.Err != "" && len(r.PRs) == 0:
		return []workRow{{line: dim("⇡ " + r.Err), owner: p.key}}
	}
	rows := make([]workRow, 0, len(r.PRs)+1)
	for _, pr := range r.PRs { //nolint:gocritic // each row keeps its own copy
		rows = append(rows, workRow{id: "r" + p.key + "#" + strconv.Itoa(pr.Number), pr: &prRow{root: p.key, pr: pr}, owner: p.key, line: ghPRLine(pr, w)})
	}
	if r.Err != "" {
		rows = append(rows, workRow{line: faint("as GitHub last said · asking again failed: " + r.Err), owner: p.key})
	}
	return rows
}

// prCounts are how many pull requests every project has open that concern
// you, and how many of those want something of you: your review, or your
// fixing of failed checks or asked-for changes.
func (m *Model) prCounts() (open, want int) {
	for _, r := range m.prs.byRoot {
		for i := range r.PRs {
			open++
			if prWantsYou(&r.PRs[i]) {
				want++
			}
		}
	}
	return open, want
}

func prWantsYou(pr *ghpr.PR) bool {
	if !pr.Mine {
		return pr.AskedYou
	}
	_, failed, _ := pr.Counts()
	return failed > 0 || pr.Decision == "CHANGES_REQUESTED"
}

// prsListLine is the list's Pull requests row.
func (m *Model) prsListLine() string {
	open, want := m.prCounts()
	s := dim(fmt.Sprintf("%d open", open))
	if want > 0 {
		s += dim(" · ") + paint(cYellow, fmt.Sprintf("%d want you", want))
	}
	if len(m.prs.byRoot) == 0 {
		s = faint("asking GitHub…")
	}
	return paint(cBlue, "⇡") + " " + fit(paint(cText+bold, "Pull requests"), projNameW-2) + s
}

// prsRows are every project's pull requests, project by project.
func (m *Model) prsRows(w int) []workRow {
	var rows []workRow
	for _, p := range m.projects() {
		if !p.repo {
			continue
		}
		prs := m.prsOf(p, w)
		if len(prs) == 0 {
			continue
		}
		meta := ""
		if r := m.prs.byRoot[p.key]; len(r.PRs) > 0 {
			meta = strconv.Itoa(len(r.PRs))
		}
		if len(rows) > 0 {
			rows = append(rows, workRow{})
		}
		rows = append(rows, workRow{line: psection(p.title, meta, w)})
		rows = append(rows, prs...)
	}
	if len(rows) == 0 {
		return []workRow{{line: dim("no open pull request of yours or asking for your review")}}
	}
	return rows
}

// ghPRLine is a pull request in a line: number, checks, where review is,
// and its title.
func ghPRLine(pr ghpr.PR, w int) string {
	ok, failed, pending := pr.Counts()
	s := paint(prStateColor(&pr), fit("⇡ #"+strconv.Itoa(pr.Number), 8))
	checks := ""
	if n := ok + failed + pending; n > 0 {
		c := cGreen
		switch {
		case failed > 0:
			c = cRed
		case pending > 0:
			c = cSub
		}
		checks = paint(c, fmt.Sprintf("%d/%d", ok, n))
		if failed > 0 {
			checks += paint(cRed, fmt.Sprintf(" ✗%d", failed))
		}
		if pending > 0 {
			checks += dim(fmt.Sprintf(" …%d", pending))
		}
	}
	s += fit(checks, 12) + fit(prReviewWord(pr), 26)
	title := paint(cText, pr.Title)
	if pr.Draft {
		title = dim("draft ") + title
	}
	if !pr.Mine {
		title += dim(" · @" + pr.Author)
	}
	return s + fit(title, max(10, w-ansi.StringWidth(s)))
}

// prReviewWord is where a pull request's review is, in a few words.
func prReviewWord(pr ghpr.PR) string {
	switch {
	case pr.State == "MERGED":
		return paint(cBlue, "merged")
	case pr.State == "CLOSED":
		return dim("closed")
	case pr.AskedYou && !pr.Mine:
		return paint(cOrange+bold, "your review")
	case pr.Decision == "APPROVED":
		return paint(cGreen, "✓ approved")
	case pr.Decision == "CHANGES_REQUESTED":
		return paint(cRed, "changes asked")
	case len(pr.Waiting) > 0:
		return paint(cYellow, "waiting on ") + paint(cSub, strings.Join(pr.Waiting, ", "))
	case pr.Decision == "REVIEW_REQUIRED":
		return paint(cYellow, "needs a review")
	}
	return ""
}

// ---- one pull request, whole ----

// prSheet is a pull request whole: its branch, checks, reviews and body.
type prSheet struct {
	root string
	num  int
	top  int
	last ghpr.PR // as last drawn, should GitHub's next answer drop it

	comments []ghpr.Comment
	said     string // why the conversation couldn't be read, or "" once it is
	read     bool

	// writing is what's being written: "comment" or "changes" (a review
	// asking for them); input and pos the text and where the cursor is.
	writing string
	input   []rune
	pos     int
	sending string // what's on its way to GitHub
}

func (m *Model) openPRSheet(r *prRow) tea.Cmd {
	s := &prSheet{root: r.root, num: r.pr.Number, last: r.pr}
	m.sheet = s
	return s.readComments()
}

// readComments asks GitHub for the pull request's conversation.
func (s *prSheet) readComments() tea.Cmd {
	root, n := s.root, s.num
	return sheetDo(func() ([]ghpr.Comment, error) {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		return ghpr.Comments(ctx, root, n)
	}, func(m *Model, cs []ghpr.Comment, err error) tea.Cmd {
		s.read, s.comments, s.said = true, cs, ""
		if err != nil {
			s.said = err.Error()
		}
		return nil
	})
}

// act does something on GitHub as you, then says how it went and asks
// GitHub again of the pull request.
func (s *prSheet) act(what, done string, f func(ctx context.Context) error) tea.Cmd {
	s.sending = what
	return sheetDo(func() (struct{}, error) {
		ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
		defer cancel()
		return struct{}{}, f(ctx)
	}, func(m *Model, _ struct{}, err error) tea.Cmd {
		s.sending = ""
		if err != nil {
			m.flash("GitHub said no: "+err.Error(), true)
			return nil
		}
		m.flash(done, false)
		if r, ok := m.prs.byRoot[s.root]; ok {
			r.At = time.Time{}
			m.prs.byRoot[s.root] = r
		}
		return tea.Batch(m.refreshPRs(), s.readComments())
	})
}

func (s *prSheet) width(m *Model) int { return min(140, m.w-6) }

// pr is the pull request as GitHub last said it.
func (s *prSheet) pr(m *Model) ghpr.PR {
	prs := m.prs.byRoot[s.root].PRs
	for i := range prs {
		if prs[i].Number == s.num {
			s.last = prs[i]
			break
		}
	}
	return s.last
}

func (s *prSheet) lines(m *Model, w int) []string {
	pr := s.pr(m)
	var out []string
	add := func(l ...string) { out = append(out, l...) }
	who := "@" + pr.Author + " wants "
	if pr.Mine {
		who = "you want "
	}
	l := dim(who) + paint(cBlue, pr.Head) + dim(" into ") + paint(cSub, pr.Base)
	if !pr.Updated.IsZero() {
		l += dim(" · updated " + age(time.Since(pr.Updated)) + " ago")
	}
	add(l, "")

	// Reviews: who's said what, and who it still waits on.
	add(psection("Review", prReviewWord(pr), w))
	if len(pr.Reviews) == 0 && len(pr.Waiting) == 0 {
		add(dim("  no one has reviewed it or been asked to"))
	}
	for _, r := range pr.Reviews {
		mark, col, word := "●", cSub, "commented"
		switch r.State {
		case "APPROVED":
			mark, col, word = "✓", cGreen, "approved"
		case "CHANGES_REQUESTED":
			mark, col, word = "✗", cRed, "asked for changes"
		case "DISMISSED":
			mark, col, word = "○", cDim, "dismissed"
		}
		said := ""
		if b := oneLine(htmlComment.ReplaceAllString(r.Body, "")); b != "" {
			said = dim(" · ") + faint(b)
		}
		add(fit("  "+paint(col, mark+" "+r.Who)+dim(" "+word)+said, w))
	}
	for _, q := range pr.Waiting {
		if q == "you" {
			add("  " + paint(cOrange+bold, "◌ you") + dim(" are asked to review it"))
			continue
		}
		add("  " + paint(cYellow, "◌ "+q) + dim(" asked, not reviewed yet"))
	}

	// Checks, failed first.
	ok, failed, pending := pr.Counts()
	meta := fmt.Sprintf("%d of %d passed", ok, len(pr.Checks))
	if failed > 0 {
		meta += fmt.Sprintf(" · %d failed", failed)
	}
	if pending > 0 {
		meta += fmt.Sprintf(" · %d running", pending)
	}
	add("", psection("Checks", meta, w))
	if len(pr.Checks) == 0 {
		add(dim("  none"))
	}
	for _, want := range []string{"failed", "pending", "passed", "skipped"} {
		for _, c := range pr.Checks {
			if c.State != want {
				continue
			}
			mark := map[string]string{"failed": paint(cRed, "✗"), "pending": paint(cOrange, spinner[m.tick%len(spinner)]), "passed": paint(cGreen, "✓"), "skipped": faint("–")}[want]
			name := c.Name
			if c.Workflow != "" {
				name = c.Workflow + " / " + name
			}
			add(fit("  "+mark+" "+paint(cText, name), w))
		}
	}

	add("", psection("Description", "", w))
	if strings.TrimSpace(pr.Body) == "" {
		add(dim("  no description"))
	} else {
		add(prMarkdown(pr.Body, w)...)
	}

	return append(out, s.conversation(w)...)
}

// htmlComment is what bots hide in what they write: markers no one reads.
var htmlComment = regexp.MustCompile(`(?s)<!--.*?-->`)

// prMarkdown is something said on GitHub drawn w wide.
func prMarkdown(s string, w int) []string {
	return convo.Markdown(strings.TrimSpace(htmlComment.ReplaceAllString(s, "")), w)
}

// conversation is what's been said on the pull request, oldest first.
func (s *prSheet) conversation(w int) []string {
	switch {
	case s.root == "":
		return nil
	case !s.read:
		return []string{"", psection("Conversation", "", w), faint("  asking GitHub…")}
	case s.said != "":
		return []string{"", psection("Conversation", "", w), dim("  " + s.said)}
	}
	out := []string{"", psection("Conversation", strconv.Itoa(len(s.comments)), w)}
	if len(s.comments) == 0 {
		out = append(out, dim("  no one has said anything yet"))
	}
	for _, c := range s.comments {
		out = append(out, "", "  "+paint(cSub+bold, c.Who)+dim(" · "+age(time.Since(c.At))+" ago"))
		out = append(out, prMarkdown(c.Body, w-2)...)
	}
	return out
}

func (s *prSheet) body(m *Model, w, h int) []string {
	pr := s.pr(m)
	state := strings.ToLower(pr.State)
	if pr.Draft {
		state = "draft"
	}
	head := make([]string, 0, h)
	head = append(head, sheetTitle(fmt.Sprintf("#%d %s", pr.Number, pr.Title), state, w), "")
	foot := []string{"", s.keys(m, pr, w)}
	switch {
	case s.sending != "":
		foot = append(foot[:1], paint(cOrange, spinner[m.tick%len(spinner)]+" "+s.sending+"…"))
	case s.writing != "":
		label := "comment ❯ "
		if s.writing == "changes" {
			label = "what to change ❯ "
		}
		foot = append(foot[:1], paint(cOrange, label)+textField(s.input, s.pos, true, "posted on GitHub as you", w-ansi.StringWidth(label)-2), keysFit(w, "enter", "post it", "esc", "don't"))
	}
	all := s.lines(m, w)
	room := max(3, h-len(head)-len(foot))
	s.top = max(0, min(s.top, len(all)-room))
	end := min(len(all), s.top+room)
	view := all[s.top:end]
	if end < len(all) {
		foot[0] = faint(fmt.Sprintf("  ↓ %d more", len(all)-end))
	}
	return append(append(head, view...), foot...)
}

// keys are what the sheet's keys do here: no reviewing your own pull
// request, which GitHub refuses, nor acting on one GitHub hasn't listed.
func (s *prSheet) keys(m *Model, pr ghpr.PR, w int) string {
	k := []string{"↑↓", "scroll", "o", "on GitHub", "y", "copy link"}
	if s.root != "" && pr.State == "OPEN" {
		k = append(k, "c", "comment")
		if !pr.Mine {
			k = append(k, "a", "approve", "x", "ask for changes")
		}
		k = append(k, "m", "merge…")
	}
	return keysFit(w, append(k, "esc", "back")...)
}

func (s *prSheet) key(m *Model, kp tea.KeyPressMsg, k string) tea.Cmd {
	if s.sending != "" && k != "esc" {
		return nil
	}
	if s.writing != "" {
		return s.writeKey(m, kp, k)
	}
	pr := s.pr(m)
	open := s.root != "" && pr.State == "OPEN"
	switch k {
	case "c":
		if open {
			s.writing, s.input, s.pos = "comment", nil, 0
		}
		return nil
	case "x":
		if open && !pr.Mine {
			s.writing, s.input, s.pos = "changes", nil, 0
		}
		return nil
	case "a":
		if open && !pr.Mine {
			s.askApprove(m, &pr)
		}
		return nil
	case "m":
		if open {
			s.askMerge(m, &pr)
		}
		return nil
	}
	switch k {
	case "esc", "q", "left":
		m.sheet = nil
	case "up", "k":
		s.top = max(0, s.top-1)
	case "down", "j":
		s.top++
	case "pgup", "shift+space":
		s.top = max(0, s.top-10)
	case "pgdown", "space":
		s.top += 10
	case "home", "g":
		s.top = 0
	case "end", "G":
		s.top = 1 << 30
	case "o", "enter":
		return browse(pr.URL)
	case "y":
		m.copyText(pr.URL)
	}
	return nil
}

// askApprove asks before approving the pull request as you.
func (s *prSheet) askApprove(m *Model, pr *ghpr.PR) {
	root, n := s.root, s.num
	m.confirm = &confirmation{
		question: fmt.Sprintf("Approve #%d on GitHub, as you?", n),
		detail:   oneLine(pr.Title),
		onYes: func() tea.Cmd {
			return s.act("approving", fmt.Sprintf("approved #%d", n), func(ctx context.Context) error { return ghpr.Approve(ctx, root, n, "") })
		},
	}
}

// askMerge asks how to merge the pull request, saying what's not ready.
func (s *prSheet) askMerge(m *Model, pr *ghpr.PR) {
	root, n := s.root, s.num
	merge := func(how string) func() tea.Cmd {
		return func() tea.Cmd {
			return s.act("merging", fmt.Sprintf("merged #%d", n), func(ctx context.Context) error { return ghpr.Merge(ctx, root, n, how) })
		}
	}
	detail := pr.Head + " into " + pr.Base
	if _, failed, pending := pr.Counts(); failed+pending > 0 {
		detail += fmt.Sprintf(" · %d checks failed, %d still running", failed, pending)
	}
	if pr.Decision != "" && pr.Decision != "APPROVED" {
		detail += " · not approved"
	}
	m.confirm = &confirmation{
		question: fmt.Sprintf("Merge #%d on GitHub?", n),
		detail:   detail,
		yesText:  "squash", onYes: merge("squash"),
		more: []confirmChoice{{key: "r", text: "rebase", do: merge("rebase")}, {key: "M", text: "merge commit", do: merge("merge")}},
	}
}

// writeKey types a comment, or what to change, and posts it with enter.
func (s *prSheet) writeKey(m *Model, kp tea.KeyPressMsg, k string) tea.Cmd {
	switch k {
	case "esc":
		s.writing, s.input, s.pos = "", nil, 0
		return nil
	case "enter":
		text := strings.TrimSpace(string(s.input))
		if text == "" {
			return nil
		}
		root, n, what := s.root, s.num, s.writing
		s.writing, s.input, s.pos = "", nil, 0
		if what == "changes" {
			return s.act("asking for changes", fmt.Sprintf("asked for changes on #%d", n), func(ctx context.Context) error { return ghpr.RequestChanges(ctx, root, n, text) })
		}
		return s.act("commenting", fmt.Sprintf("commented on #%d", n), func(ctx context.Context) error { return ghpr.Say(ctx, root, n, text) })
	}
	s.input, s.pos, _ = edit(s.input, s.pos, kp, k)
	return nil
}

func (s *prSheet) mouse(m *Model, ev mouseEv, _, _ int) tea.Cmd {
	switch ev {
	case mouseWheelUp:
		s.top = max(0, s.top-3)
	case mouseWheelDown:
		s.top += 3
	}
	return nil
}

// ---- an agent's pull requests ----

// agentPRs are the pull requests an agent knows of: those it opened or
// pushed to, and GitHub's for the branch it works on, each as GitHub last
// said it where it has, else as the agent's transcript did.
func (m *Model) agentPRs(a *fleet.Agent) []prRow {
	if a == nil {
		return nil
	}
	root := folderKey(a)
	gh := m.prs.byRoot[root].PRs
	var out []prRow
	seen := map[int]bool{}
	for i := range gh {
		p := &gh[i]
		known := a.Branch != "" && p.Head == a.Branch && p.Head != p.Base
		for _, t := range a.PRs {
			known = known || t.Number == p.Number
		}
		if known && !seen[p.Number] {
			seen[p.Number] = true
			out = append(out, prRow{root: root, pr: *p})
		}
	}
	for _, t := range a.PRs {
		if !seen[t.Number] {
			seen[t.Number] = true
			out = append(out, prRow{pr: fromTranscript(t)})
		}
	}
	return out
}

// fromTranscript is a pull request as an agent's transcript says it: its
// checks only counted, so each stands in nameless.
func fromTranscript(t agent.PR) ghpr.PR {
	p := ghpr.PR{Number: t.Number, Title: t.Title, URL: t.URL, State: t.State, Decision: t.Review, Mine: true}
	for state, n := range map[string]int{"passed": t.Checks.Passed, "failed": t.Checks.Failed, "pending": t.Checks.Pending} {
		for range n {
			p.Checks = append(p.Checks, ghpr.Check{State: state})
		}
	}
	return p
}

// openAgentPR opens one of an agent's pull requests whole, or on GitHub
// when GitHub hasn't said more of it than its transcript.
func (m *Model) openAgentPR(a *fleet.Agent, num int) tea.Cmd {
	prs := m.agentPRs(a)
	for i := range prs {
		if prs[i].pr.Number != num {
			continue
		}
		if prs[i].root == "" {
			return browse(prs[i].pr.URL)
		}
		return m.openPRSheet(&prs[i])
	}
	return nil
}

// prSectionLines are an agent's pull requests for its Overview and
// Changes: a heading, and a row for each that space opens.
func (m *Model) prSectionLines(c *hostConn, w int, focused bool) []convo.Line {
	prs := m.agentPRs(m.agentByKey(c.key))
	if len(prs) == 0 {
		return nil
	}
	h := "  " + faint("▾") + " " + paint(cSub+bold, "Pull requests") + "  " + dim(fmt.Sprintf("%d · space opens one", len(prs)))
	out := []convo.Line{{}, {Text: fit(h+" "+faint(strings.Repeat("─", max(0, w-ansi.StringWidth(h)-3))), w)}}
	for i := range prs {
		ref := "pr:" + strconv.Itoa(prs[i].pr.Number)
		row := "    " + ghPRLine(prs[i].pr, w-6)
		if c.sel == ref {
			row = picked1(row, w, focused)
		}
		out = append(out, convo.Line{Text: fit(row, w), Ref: ref})
	}
	return out
}

// prBoxNote is an agent's pull requests on the edge under its box: one or
// two by number, with the one's title; more, how many and where they are.
func (m *Model) prBoxNote(a *fleet.Agent, w int) string {
	prs := m.agentPRs(a)
	short := func(p *ghpr.PR) string {
		ok, failed, pending := p.Counts()
		s := paint(prStateColor(p), "#"+strconv.Itoa(p.Number))
		switch {
		case p.State == "MERGED":
			s += dim(" merged")
		case failed > 0:
			s += paint(cRed, fmt.Sprintf(" ✗%d", failed))
		case pending > 0:
			s += dim(fmt.Sprintf(" …%d", pending))
		case ok > 0:
			s += paint(cGreen, " ✓")
		}
		if p.Decision == "APPROVED" && p.State == "OPEN" {
			s += paint(cGreen, " approved")
		}
		return s
	}
	switch len(prs) {
	case 0:
		return ""
	case 1:
		p := &prs[0].pr
		s := paint(cBlue, "⇡ ") + short(p)
		return s + dim(" · ") + paint(cSub, cellwTrunc(p.Title, max(0, w/2-ansi.StringWidth(s))))
	case 2:
		return paint(cBlue, "⇡ ") + short(&prs[0].pr) + dim(" · ") + short(&prs[1].pr)
	}
	return paint(cBlue, "⇡ ") + dim(fmt.Sprintf("%d pull requests · Overview has them", len(prs)))
}

func cellwTrunc(s string, w int) string { return ansi.Truncate(oneLine(s), w, "…") }

// prStateColor is a pull request's colour: green open, red while its
// checks fail or changes are asked, blue merged, quiet a draft or closed.
func prStateColor(p *ghpr.PR) string {
	_, failed, _ := p.Counts()
	switch {
	case p.State == "MERGED":
		return cBlue
	case p.State == "CLOSED" || p.Draft:
		return cDim
	case failed > 0 || p.Decision == "CHANGES_REQUESTED":
		return cRed
	}
	return cGreen
}
