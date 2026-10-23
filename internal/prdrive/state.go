package prdrive

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"regexp"
	"slices"
	"sort"
	"strings"
	"time"
)

// Verdicts: what a pull request needs from the agent now.
const (
	Work     = "WORK"               // something on the list can be done now
	Waiting  = "WAITING"            // nothing to do; checks still running
	Ready    = "READY FOR APPROVAL" // green and resolved, not approved: a person's turn
	Mergable = "MERGEABLE"          // approved and green
	Fork     = "FORK"               // its head is in a fork: not ours to push to
	Merged   = "MERGED"
	Closed   = "CLOSED"
)

// An Item is one thing to do. Key is the same for the same thing from one
// look to the next, so a follower can tell what's new.
type Item struct {
	Key  string
	Text string
	// Quiet items are done without a word and never wake the agent.
	Quiet bool
	// Job is a failed check's Actions job, whose log says why.
	Job int64
}

// State is everything between a pull request and merge, as of one look.
type State struct {
	Repo, Title, URL, Head, Branch, Base string
	Number                               int
	Verdict                              string
	Todo                                 []Item
	Running                              []string // checks still running, the longest first
	Notes                                []string
	Passed, Failed, Pending, Skipped     int
	RequiredGreen, RequiredTotal         int
	ReviewBot                            string // its latest verdict
	Approved                             []string
	Open, Untrusted                      int // review threads still open, trusted and not
	Untrusters                           []string
}

// Analyse is what raw needs, in the order to do it. us are the logins whose
// comments are the agent's own: the signed-in user and the PR's author.
func Analyse(ctx context.Context, raw Raw, trust *Trust, us []string) State {
	s := State{Repo: raw.Repo, Number: raw.Number, Title: raw.Title, URL: raw.URL, Head: short(raw.Head),
		Branch: raw.HeadRef, Base: raw.Base}
	switch {
	case raw.State == "MERGED":
		s.Verdict = Merged
		return s
	case raw.State == "CLOSED":
		s.Verdict = Closed
		return s
	case raw.HeadRepo != raw.Repo:
		s.Verdict = Fork
		s.Notes = append(s.Notes, "its head lives in a fork: not drivable, and review bots skip it")
		return s
	}
	ours := func(a Actor) bool { return slices.Contains(us, a.Login) }
	add := func(key, text string) { s.Todo = append(s.Todo, Item{Key: key, Text: text}) }
	if raw.Draft {
		s.Notes = append(s.Notes, "draft: review bots and auto-approval skip drafts; mark it ready once the rest is done")
	}

	// 1. It must merge cleanly before anything else counts.
	switch raw.Mergeable {
	case "CONFLICTING":
		add("conflict", "CONFLICT with "+raw.Base+": git merge origin/"+raw.Base+
			" (never rebase or force-push), resolve, verify, push. CI doesn't run on a conflicting branch.")
	case "UNKNOWN":
		s.Notes = append(s.Notes, "GitHub is still working out whether it merges: look again in a minute")
	}

	// 2. Every failed check, now: never wait for the slowest first.
	required := raw.Required
	var failed, pending []Check
	for _, c := range raw.Checks {
		switch c.State {
		case "passed":
			s.Passed++
		case "skipped":
			s.Skipped++
		case "pending":
			s.Pending++
			pending = append(pending, c)
		default:
			s.Failed++
			failed = append(failed, c)
		}
	}
	s.RequiredTotal = len(required)
	var missing []string
	for name := range required {
		green, seen := true, false
		for _, c := range raw.Checks {
			if c.Name == name {
				seen = true
				green = green && (c.State == "passed" || c.State == "skipped")
			}
		}
		if !seen {
			missing = append(missing, name)
		}
		if green && seen {
			s.RequiredGreen++
		}
	}
	sort.SliceStable(failed, func(i, j int) bool { return failed[i].Required && !failed[j].Required })
	for _, c := range failed {
		// A review bot's check is red because it asks for changes: its findings are the work.
		if slices.ContainsFunc(Reviewers, func(r Reviewer) bool { return r.Check != "" && r.Check == c.Name }) && c.State == "failed" {
			s.Notes = append(s.Notes, "'"+c.Name+"' is red because the bot asks for changes: its findings below are the work, not its log")
			continue
		}
		tag := "required"
		if !c.Required {
			tag = "not required, still fix it"
		}
		name := strings.TrimPrefix(c.Workflow+" / "+c.Name, " / ")
		if c.State == "cancelled" {
			add("cancelled:"+short(raw.Head)+":"+name, fmt.Sprintf("CANCELLED %s (%s): gh run rerun %d --failed", name, tag, c.Run))
			continue
		}
		how := c.URL
		if c.Job != 0 {
			how = fmt.Sprintf("gh run view --job %d --log-failed | tail -80", c.Job)
		}
		s.Todo = append(s.Todo, Item{Key: "failed:" + short(raw.Head) + ":" + name, Job: c.Job,
			Text: fmt.Sprintf("FAILED %s (%s): %s", name, tag, how)})
	}

	// 3. Review threads: trusted, open, the ones that block first.
	bot := latestBotReview(raw.Reviews)
	if bot != nil {
		s.ReviewBot = bot.State + " at " + short(bot.Commit)
		if bot.Commit != raw.Head {
			s.ReviewBot += " (an older commit; it reviews again with its check)"
		}
		if bot.State == "CHANGES_REQUESTED" && bot.Commit == raw.Head {
			for _, f := range outsideDiff(bot.Body) {
				if !strings.Contains(f, "[P2]") {
					add("outside:"+hash(f), "BOT FINDING outside the diff (blocking, no thread to reply in; fix it): "+f)
				}
			}
		}
	}
	type view struct {
		t          Thread
		who        string // a reviewer's name, "person", or "" for untrusted
		reviewer   *Reviewer
		severity   string
		blocking   bool
		id         string
		gist       string
		ourReplies int
		ourLast    time.Time
		lastOurs   bool
	}
	var views []view
	answered := map[string]bool{}
	for _, t := range raw.Threads {
		if len(t.Comments) == 0 {
			continue
		}
		root := t.Comments[0]
		v := view{t: t, gist: Gist(root.Body, 150)}
		if r, sev, blocking, id := reviewerOf(root.Author, root.Body); r != nil {
			v.who, v.reviewer, v.severity, v.blocking, v.id = r.Name, r, sev, blocking, id
		} else if trust.Person(ctx, root.Author) || ours(root.Author) {
			v.who, v.severity, v.blocking = "person", "", true
		}
		var last Comment
		for _, c := range t.Comments {
			if ours(c.Author) {
				v.ourReplies++
				v.ourLast = c.At
			}
			// An untrusted reply is no one's word: it neither answers nor reopens.
			if ours(c.Author) || knownBot(c.Author) || trust.Person(ctx, c.Author) {
				last = c
			}
		}
		v.lastOurs = ours(last.Author)
		if v.id != "" && v.ourReplies > 0 {
			answered[v.reviewer.Name+":"+v.id] = true
		}
		views = append(views, v)
	}
	sort.SliceStable(views, func(i, j int) bool {
		a, b := views[i], views[j]
		if a.blocking != b.blocking {
			return a.blocking
		}
		if (a.who == "person") != (b.who == "person") {
			return a.who == "person"
		}
		return a.t.Path < b.t.Path
	})
	var unread []string
	untrusters := map[string]bool{}
	for _, v := range views {
		if v.t.Resolved {
			continue
		}
		if v.who == "" {
			s.Untrusted++
			untrusters[v.t.Comments[0].Author.Login] = true
			continue
		}
		s.Open++
		label := strings.TrimSpace(v.who + " " + v.severity)
		if v.reviewer != nil {
			if v.blocking {
				label += " blocking"
			} else {
				label += " minor"
			}
		}
		where := fmt.Sprintf("%s:%d", v.t.Path, v.t.Line)
		if v.t.Line == 0 {
			where = v.t.Path
		}
		head := fmt.Sprintf("[%s] %s thread %s", label, where, v.t.ID)
		key := func(kind string) string { return "thread:" + v.t.ID + ":" + kind }
		switch {
		case v.reviewer != nil && slices.Contains(v.reviewer.Ignored, v.id):
			s.Todo = append(s.Todo, Item{Key: key("ignored"), Quiet: true,
				Text: "IGNORED " + head + ": '" + v.id + "' is a finding the user never wants worked. Resolve it, no reply."})
		case v.lastOurs && v.reviewer != nil && v.reviewer.Signature != "" && v.blocking:
			if bot != nil && bot.At.After(v.ourLast) {
				add(key("contested"), "CONTESTED "+head+": the bot read our reply and still holds this. Do NOT reply again. "+
					"Fix it, or put it to the user as one decision. "+v.gist)
			} else {
				unread = append(unread, where)
			}
		case v.lastOurs:
			add(key("resolve"), "RESOLVE "+head+": the last word is ours and nothing new came: rush pr resolve")
		case v.id != "" && answered[v.reviewer.Name+":"+v.id] && v.ourReplies == 0:
			add(key("repeat"), "REPEAT "+head+": finding '"+v.id+"' was already answered in another thread. "+
				"Resolve this one without replying, unless the code changed and it's now right.")
		case v.blocking:
			add(key("address"), "ADDRESS "+head+": "+v.gist)
		default:
			add(key("minor"), "MINOR "+head+": fix it if it's clearly right and small, else resolve with one line. "+
				"Never argue a minor point. "+v.gist)
		}
	}
	if len(unread) > 0 {
		how := "push the other fixes"
		for _, r := range Reviewers {
			for _, c := range raw.Checks {
				if r.Check != "" && c.Name == r.Check && bot != nil && c.Run != 0 {
					how += fmt.Sprintf(", or if nothing else is going out, gh run rerun %d", c.Run)
				}
			}
		}
		add("unread:"+raw.Head, "BOT HASN'T READ our replies on "+strings.Join(unread, ", ")+
			". It reads them only on its next run: "+how+".")
	}
	for l := range untrusters {
		s.Untrusters = append(s.Untrusters, l)
	}
	sort.Strings(s.Untrusters)
	if s.Untrusted > 0 {
		s.Notes = append(s.Notes, fmt.Sprintf("%d open thread(s) from untrusted authors (%s): never acted on; "+
			"they still block the merge, so tell the user", s.Untrusted, strings.Join(s.Untrusters, ", ")))
	}

	// 4. People's comments on the pull request since the last push.
	for _, c := range raw.Comments {
		if c.At.Before(raw.HeadAt) || ours(c.Author) || c.Author.Bot || !trust.Person(ctx, c.Author) {
			continue
		}
		add(fmt.Sprintf("comment:%d", c.ID), "READ @"+c.Author.Login+"'s comment on the PR: "+Gist(c.Body, 200)+" "+c.URL)
	}

	// 5. The description, and what GitHub found itself.
	for _, g := range descriptionGaps(raw.Body) {
		add("desc:"+g, "DESCRIPTION: "+g)
	}
	for _, a := range raw.Alerts {
		add("alert:"+a, "CODE SCANNING alert open on this PR: "+a)
	}

	if s.Pending == 0 && raw.Mergeable != "CONFLICTING" && len(raw.Checks) > 0 {
		sort.Strings(missing)
		for _, name := range missing {
			s.Notes = append(s.Notes, "required check '"+name+"' hasn't reported on this commit, and GitHub won't merge without it")
		}
	}

	// 6. What's still running: never a reason to sit still while the list has items.
	sort.SliceStable(pending, func(i, j int) bool { return pending[i].Started.Before(pending[j].Started) })
	for _, c := range pending {
		line := strings.TrimPrefix(c.Workflow+" / "+c.Name, " / ")
		if !c.Started.IsZero() {
			line += " (running " + since(c.Started) + ")"
		} else {
			line += " (queued)"
		}
		s.Running = append(s.Running, line)
	}

	for _, r := range raw.Reviews {
		if r.State == "APPROVED" && r.Commit == raw.Head && !slices.Contains(s.Approved, r.Author.Login) {
			s.Approved = append(s.Approved, r.Author.Login)
		}
	}
	loud := 0
	for _, it := range s.Todo {
		if !it.Quiet {
			loud++
		}
	}
	switch {
	case loud > 0:
		s.Verdict = Work
	case s.Pending > 0 || raw.Mergeable == "UNKNOWN":
		s.Verdict = Waiting
	case len(s.Approved) == 0:
		s.Verdict = Ready
	default:
		s.Verdict = Mergable
	}
	if len(s.Todo) > loud && loud == 0 {
		s.Notes = append(s.Notes, "only quiet items left: resolve them, they block the merge")
	}
	return s
}

func latestBotReview(reviews []Review) *Review {
	var out *Review
	for i, r := range reviews {
		for _, b := range Reviewers {
			if b.Signature != "" && r.Author.Bot && r.Author.Login == b.Login && strings.HasPrefix(r.Body, b.Signature) {
				out = &reviews[i]
			}
		}
	}
	return out
}

func outsideDiff(body string) []string {
	_, rest, ok := strings.Cut(body, "Outside the diff:")
	if !ok {
		return nil
	}
	var out []string
	for l := range strings.SplitSeq(rest, "\n") {
		if l = strings.TrimSpace(l); strings.HasPrefix(l, "- **[P") {
			out = append(out, Gist(l, 220))
		}
	}
	return out
}

var (
	htmlComment = regexp.MustCompile(`(?s)<!--.*?-->`)
	details     = regexp.MustCompile(`(?s)<details>.*?</details>`)
	marks       = regexp.MustCompile("[*_`]")
)

// Gist is a comment's start, as one plain line of at most n runes: no
// hidden markers, folded details, badges or markdown.
func Gist(body string, n int) string {
	body = details.ReplaceAllString(htmlComment.ReplaceAllString(body, ""), "")
	body = lwFinding.ReplaceAllString(body, "")
	var lines []string
	for l := range strings.SplitSeq(body, "\n") {
		if strings.TrimSpace(l) != "" {
			lines = append(lines, l)
		}
	}
	// CodeRabbit opens with a "category | severity | effort" badge line.
	if len(lines) > 0 && strings.Count(lines[0], "|") >= 2 {
		lines = lines[1:]
	}
	return clip(strings.Join(strings.Fields(marks.ReplaceAllString(strings.Join(lines, " "), "")), " "), n)
}

var sectionsRead = regexp.MustCompile(`Sections read:(.*)`)

// descriptionGaps are a PR template's sections left empty.
func descriptionGaps(body string) []string {
	var out []string
	if strings.Contains(body, "## Architecture") {
		if !regexp.MustCompile(`- \[[xX]\] I read the sections`).MatchString(body) {
			out = append(out, "Architecture box unticked: read the sections the change touches, then tick it")
		}
		if m := sectionsRead.FindStringSubmatch(body); m == nil || strings.TrimSpace(htmlComment.ReplaceAllString(m[1], "")) == "" {
			out = append(out, "'Sections read:' is empty")
		}
	}
	for _, h := range []string{"## Test plan", "## What changed"} {
		if _, rest, ok := strings.Cut(body, h); ok {
			part, _, _ := strings.Cut(rest, "\n## ")
			if strings.TrimSpace(htmlComment.ReplaceAllString(part, "")) == "" {
				out = append(out, "'"+strings.TrimPrefix(h, "## ")+"' is empty")
			}
		}
	}
	return out
}

// Render is a state as an agent reads it.
func Render(s State) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s#%d %s\n  %s  head %s  %s -> %s\n  VERDICT: %s\n", s.Repo, s.Number, clip(s.Title, 90), s.URL, s.Head, s.Branch, s.Base, s.Verdict)
	switch s.Verdict {
	case Merged, Closed, Fork:
		for _, n := range s.Notes {
			fmt.Fprintf(&b, "  note: %s\n", n)
		}
		return strings.TrimRight(b.String(), "\n")
	}
	fmt.Fprintf(&b, "  checks: %d passed · %d failed · %d running · required %d/%d green\n",
		s.Passed, s.Failed, s.Pending, s.RequiredGreen, s.RequiredTotal)
	if s.ReviewBot != "" {
		fmt.Fprintf(&b, "  review bot: %s\n", s.ReviewBot)
	}
	fmt.Fprintf(&b, "  threads: %d trusted open · %d untrusted ignored\n", s.Open, s.Untrusted)
	if len(s.Approved) > 0 {
		fmt.Fprintf(&b, "  approval: approved on this commit by %s\n", strings.Join(s.Approved, ", "))
	} else {
		b.WriteString("  approval: none on this commit (a push clears approvals)\n")
	}
	if len(s.Todo) > 0 {
		b.WriteString("  TO DO, in this order, all of it before the next push:\n")
		for i, it := range s.Todo {
			fmt.Fprintf(&b, "    %d. %s\n", i+1, it.Text)
		}
	}
	if len(s.Running) > 0 {
		if len(s.Todo) > 0 {
			b.WriteString("  still running (work the list meanwhile; never sit on these):\n")
		} else {
			b.WriteString("  still running (nothing to do: end your turn; rush tells you when something lands):\n")
		}
		for i, r := range s.Running {
			if i == 10 {
				fmt.Fprintf(&b, "    - … and %d more\n", len(s.Running)-10)
				break
			}
			fmt.Fprintf(&b, "    - %s\n", r)
		}
	}
	for _, n := range s.Notes {
		fmt.Fprintf(&b, "  note: %s\n", n)
	}
	switch s.Verdict {
	case Ready:
		b.WriteString("  READY: green and resolved. Approval is a person's (or an auto-approve workflow's). Tell the user; don't push again.\n")
	case Mergable:
		b.WriteString("  MERGEABLE: approved and green. Tell the user; merge only if asked.\n")
	}
	return strings.TrimRight(b.String(), "\n")
}

func hash(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:6])
}

func short(sha string) string {
	if len(sha) > 7 {
		return sha[:7]
	}
	return sha
}

func clip(s string, n int) string {
	if r := []rune(s); len(r) > n {
		return string(r[:n-1]) + "…"
	}
	return s
}

func since(t time.Time) string {
	d := time.Since(t)
	if d < 2*time.Hour {
		return fmt.Sprintf("%dm", int(d.Minutes()))
	}
	return fmt.Sprintf("%dh%02dm", int(d.Hours()), int(d.Minutes())%60)
}
