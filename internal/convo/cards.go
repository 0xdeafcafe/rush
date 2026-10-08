package convo

import (
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/charmbracelet/x/ansi"
)

// A card is something a command did that outlives the command: a commit, a
// push, a pull request, a test run that failed, work thrown away. It's drawn
// under the step's row, and under a folded run of steps too, so it shows
// without opening anything.
type card struct {
	kind string // "commit", "push", "pr", "tests", "build", "merge", "discard"

	// A commit: git's own report of it, and the message it was given.
	sha, branch, subject string
	body                 []string
	files, add, del      int
	amend, root          bool
	with                 []string // co-authors

	// A push: where to, and each ref it moved.
	remote string
	refs   []pushed

	// A pull request, opened or merged.
	num, url, state, title, head, base string

	// A test run that failed: the tally, and each failure with what it said.
	// A build that failed: its errors, each where it is.
	passed, failed, skipped int
	fails                   []failure

	// A merge, rebase or pull: how it went, what conflicts, where it stopped.
	how       string
	conflicts []string

	// A merge git could show: the commit it made (none for a fast-forward),
	// the branch it went into, where that was, the newest few commits it
	// brought in and how many in all, and whether they grew from where it
	// was.
	made, into string
	was        gitLine
	came       []gitLine
	cameN      int
	forked     bool

	// Work set aside or thrown away (what says which, subject how much),
	// and how to get it back, when git can.
	what string
	lost []string
	undo string
	warn bool // set aside, or deleted safely: yellow rather than red
}

// tone is what a card's action did, for its colour: made something,
// rewrote or set something aside, threw something away or failed, or
// none of those.
type tone int

const (
	toneNone tone = iota
	toneMade
	toneWarn
	toneLost
)

func (c card) tone() tone {
	switch c.kind {
	case "commit":
		if c.amend {
			return toneWarn
		}
		return toneMade
	case "push":
		t := toneNone
		for _, p := range c.refs {
			switch {
			case p.span == "deleted":
				return toneLost
			case p.forced:
				t = toneWarn
			case t == toneNone && strings.HasPrefix(p.span, "new "):
				t = toneMade
			}
		}
		return t
	case "pr":
		if c.state == "draft" {
			return toneNone
		}
		return toneMade
	case "tests", "build":
		return toneLost
	case "merge":
		// Conflicts, or a rebase stopped part way: yours to finish.
		if len(c.conflicts) > 0 || c.sha != "" {
			return toneWarn
		}
	case "discard":
		if c.warn {
			return toneWarn
		}
		return toneLost
	}
	return toneNone
}

// verb is the card's command, for a folded run's list of steps.
func (c card) verb() string {
	switch {
	case c.kind == "push":
		return "git push"
	case c.kind == "pr" && c.state == "merged":
		return "gh pr merge"
	case c.kind == "pr":
		return "gh pr create"
	case c.kind == "tests":
		return "tests"
	case c.kind == "build":
		return c.what
	case c.kind == "merge", c.kind == "discard":
		return "git " + strings.Fields(c.what)[0]
	}
	return "git commit"
}

// gitLine is a commit as git log --oneline has it.
type gitLine struct{ sha, subject string }

// failure is a test that failed and what it said, or a package that
// didn't build and why.
type failure struct {
	name, msg string
	build     bool // the package didn't compile
}

// pushed is one ref a push moved: main -> main, 88c42b1..3225847.
type pushed struct {
	from, to, span string
	forced         bool
}

var (
	gitVerb    = regexp.MustCompile(`\bgit\s+(?:-[Cc]\s+\S+\s+)*(commit|cherry-pick|revert|push|merge|rebase|pull|reset|branch|stash|clean|checkout|restore)\b`)
	ghVerb     = regexp.MustCompile(`\bgh\s+pr\s+(create|merge)\b`)
	buildVerb  = regexp.MustCompile(`\b(?:go (?:build|install|vet)|(?:vue-)?tsc|nx|(?:npm|pnpm|yarn|bun) (?:run )?(?:build|dev|start|typecheck)\S*)\b`)
	testVerb   = regexp.MustCompile(`\b(?:(?:go|cargo|npm|pnpm|yarn|bun|deno|make|mix|dotnet) (?:run )?test\S*|pytest|jest|vitest|rspec)\b`)
	commitHead = regexp.MustCompile(`^\[(.+?) ([0-9a-f]{7,40})\] (.*)$`)
	commitStat = regexp.MustCompile(`^ (\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?`)
	pushTo     = regexp.MustCompile(`^To (\S+)$`)
	pushRef    = regexp.MustCompile(`^ ([ +*-]) (\[[^\]]+\]|\S+)\s+(\S+) -> (\S+)`)
	prURL      = regexp.MustCompile(`https://([^/\s]+/[^/\s]+/[^/\s]+)/pull/(\d+)`)
	prCreating = regexp.MustCompile(`(?m)^Creating (?:draft )?pull request for (\S+) into (\S+) in `)
	prMerged   = regexp.MustCompile(`(?m)^✓ (Squashed and merged|Rebased and merged|Merged) pull request \S*?#(\d+) \((.*)\)$`)
	prDeleted  = regexp.MustCompile(`(?m)^✓ Deleted (?:local |remote )?branch (\S+)`)
	msgFlag    = regexp.MustCompile(`(?:^|\s)(?:-a?m|--message)(?:=|\s*)`)
	trailer    = regexp.MustCompile(`(?i)^(co-authored-by|signed-off-by):\s*(.*?)\s*(?:<[^>]*>)?\s*$`)
	heredocArg = regexp.MustCompile(`^"?\$\(cat\s*<<-?\s*['"]?(\w+)['"]?`)
	stdinMsg   = regexp.MustCompile(`(?:^|\s)(?:-F\s*|--file[=\s])-(?:\s|$)`)
	stdinDoc   = regexp.MustCompile(`<<-?\s*['"]?(\w+)['"]?`)
	quietFlag  = regexp.MustCompile(`(?:^|\s)(?:-q|--quiet)(?:\s|$)|>\s*/dev/null`)
	shellSub   = regexp.MustCompile("\\$[A-Za-z_{(0-9@*#?]|`")
	logOneline = regexp.MustCompile(`^([0-9a-f]{7,40}) (?:\([^)]*\) )?(.*)$`)
)

// textMemo keeps what was worked out of commands' text: a frame reads the
// same few again and again. It starts again past a few thousand.
type textMemo[T any] struct {
	sync.Mutex
	m map[string]T
}

var (
	gitCallsMemo textMemo[[]gitCall]
	heredocsMemo textMemo[string]
)

func memo[T any](c *textMemo[T], k string, f func(string) T) T {
	c.Lock()
	v, ok := c.m[k]
	c.Unlock()
	if ok {
		return v
	}
	v = f(k)
	c.Lock()
	if c.m == nil || len(c.m) >= 4096 {
		c.m = map[string]T{}
	}
	c.m[k] = v
	c.Unlock()
	return v
}

// gitCall is one git command of a chain: what it was, and its arguments up
// to the end of that command.
type gitCall struct{ verb, args string }

// gitCalls are cmd's git commands. A command never changes and every frame
// asks again of each one drawn, so what it found is kept.
func gitCalls(cmd string) []gitCall {
	return memo(&gitCallsMemo, cmd, parseGitCalls)
}

func parseGitCalls(cmd string) []gitCall {
	var out []gitCall
	// A heredoc is what a command is fed, not what it runs: a script that
	// mentions git commit doesn't commit.
	for _, loc := range gitVerb.FindAllStringSubmatchIndex(blankHeredocs(cmd), -1) {
		args := cmd[loc[1]:]
		if i := strings.IndexAny(args, ";&|\n"); i >= 0 {
			args = args[:i]
		}
		out = append(out, gitCall{cmd[loc[2]:loc[3]], " " + strings.TrimSpace(args) + " "})
	}
	return out
}

// cardsOf reads the cards out of a finished command: git and gh report
// what they did, and the command holds what git doesn't echo, like a
// commit's body. That's a reading of text: gitCommits asks git what was
// really committed, when the session's folder is on this machine.
func cardsOf(st *Step) []card {
	if st.kind() != tool.Shell || st.Status == Running || st.Status == Waiting {
		return nil
	}
	cmd := st.in().Command
	calls := gitCalls(cmd)
	var verbs []string
	for _, c := range calls {
		verbs = append(verbs, c.verb)
	}
	for _, m := range ghVerb.FindAllStringSubmatch(blankHeredocs(cmd), -1) {
		verbs = append(verbs, "pr "+m[1])
	}
	tests := testVerb.MatchString(blankHeredocs(cmd))
	build := buildVerb.FindString(blankHeredocs(cmd))
	if len(verbs) == 0 && !tests && build == "" {
		return nil
	}
	out := stripANSI(bashOut(st))
	// Progress redraws itself with \r; each redraw reads as a line.
	lines := strings.FieldsFunc(out, func(r rune) bool { return r == '\n' || r == '\r' })
	var cs []card
	has := func(vs ...string) bool {
		for _, x := range verbs {
			for _, v := range vs {
				if x == v {
					return true
				}
			}
		}
		return false
	}
	if has("merge", "rebase", "pull") {
		if c, ok := mergeCard(lines, calls); ok {
			cs = append(cs, c)
		}
	}
	if has("commit", "cherry-pick", "revert") {
		cs = append(cs, commitCards(lines, cmd, st.Status == OK)...)
	}
	if has("push") {
		if c, ok := pushCard(lines); ok {
			cs = append(cs, c)
		}
	}
	if has("pr create") {
		if c, ok := prCreateCard(out, cmd); ok {
			cs = append(cs, c)
		}
	}
	if has("pr merge") {
		if c, ok := prMergeCard(out, cmd, st.Status == OK); ok {
			cs = append(cs, c)
		}
	}
	cs = append(cs, discardCards(lines, calls, st.Status == OK)...)
	if tests {
		if c, ok := testCard(out, lines); ok {
			cs = append(cs, c)
		}
	}
	// A test run that didn't build already says why.
	if build != "" && !slices.ContainsFunc(cs, func(c card) bool {
		return c.kind == "tests" && slices.ContainsFunc(c.fails, func(f failure) bool { return f.build })
	}) {
		if c, ok := buildCard(build, lines); ok {
			cs = append(cs, c)
		}
	}
	return cs
}

// commitCards reads each commit git reported: "[main 3225847] subject" and
// the " 2 files changed, …" under it. A commit told to be quiet reports
// nothing, so one that went through is read from its message instead, and
// its hash from a git log --oneline after it, when there's one.
func commitCards(lines []string, cmd string, ok bool) []card {
	var cs []card
	msgs := commitMessages(cmd)
	for i, l := range lines {
		m := commitHead.FindStringSubmatch(l)
		if m == nil {
			continue
		}
		c := card{kind: "commit", branch: m[1], sha: m[2], subject: m[3], amend: strings.Contains(cmd, "--amend")}
		if b, ok := strings.CutSuffix(c.branch, " (root-commit)"); ok {
			c.branch, c.root = b, true
		}
		// An amend's Date:, an unset identity's Committer: come first.
		for _, n := range lines[i+1 : min(len(lines), i+6)] {
			if s := commitStat.FindStringSubmatch(n); s != nil {
				c.files, _ = strconv.Atoi(s[1])
				c.add, _ = strconv.Atoi(s[2])
				c.del, _ = strconv.Atoi(s[3])
				break
			}
			if commitHead.MatchString(n) {
				break
			}
		}
		for _, msg := range msgs {
			if subj, _, _ := strings.Cut(msg.text, "\n"); strings.TrimSpace(subj) == strings.TrimSpace(c.subject) {
				c.body, c.with = messageBody(msg.text)
				break
			}
		}
		cs = append(cs, c)
	}
	if !ok {
		return cs
	}
	for _, msg := range msgs {
		if !msg.quiet {
			continue
		}
		subj, _, _ := strings.Cut(msg.text, "\n")
		c := card{kind: "commit", subject: strings.TrimSpace(subj), amend: msg.amend}
		c.body, c.with = messageBody(msg.text)
		for _, l := range lines {
			if m := logOneline.FindStringSubmatch(strings.TrimSpace(l)); m != nil && strings.TrimSpace(m[2]) == c.subject {
				c.sha = m[1]
				break
			}
		}
		cs = append(cs, c)
	}
	return cs
}

// commitMsg is the message a git commit was given, and whether it was
// told to keep quiet about it.
type commitMsg struct {
	text         string
	quiet, amend bool
}

// commitMessages is the message each git commit in cmd was given with -m,
// its -m's joined as git joins them, into paragraphs, or with -F - from a
// heredoc.
func commitMessages(cmd string) []commitMsg {
	var out []commitMsg
	locs := gitVerb.FindAllStringSubmatchIndex(blankHeredocs(cmd), -1)
	for i, loc := range locs {
		end := len(cmd)
		if i+1 < len(locs) {
			end = locs[i+1][0]
		}
		if v := cmd[loc[2]:loc[3]]; v != "commit" {
			continue
		}
		args := cmd[loc[1]:end]
		first, _, _ := strings.Cut(args, "\n")
		// What's past the command's own line is a heredoc's, not its flags.
		flags := first
		if k := strings.IndexAny(flags, ";&|"); k >= 0 {
			flags = flags[:k]
		}
		var parts []string
		opaque := false
		for _, loc := range msgFlag.FindAllStringIndex(blankHeredocs(args), -1) {
			if v, ok := argValue(args[loc[1]:]); ok {
				parts = append(parts, strings.TrimSpace(v))
				opaque = opaque || expands(args[loc[1]:], v)
			}
		}
		if len(parts) == 0 && stdinMsg.MatchString(flags) {
			if m := stdinDoc.FindStringSubmatch(first); m != nil {
				if v, ok := argValue("$(cat <<'" + m[1] + "'" + args[len(first):]); ok {
					parts = append(parts, strings.TrimSpace(v))
				}
			}
		}
		// "$msg" is a message only the shell knew.
		if opaque {
			continue
		}
		if len(parts) > 0 {
			out = append(out, commitMsg{text: strings.Join(parts, "\n\n"), quiet: quietFlag.MatchString(first), amend: strings.Contains(flags, "--amend")})
		}
	}
	return out
}

// expands is whether the shell put something into v, an argument given
// as raw: a variable, a command's output. Single quotes and a quoted
// heredoc tag keep it as written.
func expands(raw, v string) bool {
	if strings.HasPrefix(raw, "'") {
		return false
	}
	if m := heredocArg.FindStringSubmatch(raw); m != nil {
		tag, _, _ := strings.Cut(raw, "\n")
		return !strings.ContainsAny(tag[strings.Index(tag, "<<"):], `'"`) && shellSub.MatchString(v)
	}
	return shellSub.MatchString(v)
}

// messageBody is a message's body, without its subject or trailers, and
// who it was written with.
func messageBody(msg string) (body, with []string) {
	lines := strings.Split(msg, "\n")[1:]
	var keep []string
	for _, l := range lines {
		l = strings.TrimRight(l, " \t")
		if m := trailer.FindStringSubmatch(strings.TrimSpace(l)); m != nil {
			if strings.EqualFold(m[1], "co-authored-by") && m[2] != "" {
				with = append(with, m[2])
			}
			continue
		}
		if strings.Contains(l, "Generated with [Claude Code]") {
			continue
		}
		keep = append(keep, l)
	}
	for len(keep) > 0 && strings.TrimSpace(keep[0]) == "" {
		keep = keep[1:]
	}
	for len(keep) > 0 && strings.TrimSpace(keep[len(keep)-1]) == "" {
		keep = keep[:len(keep)-1]
	}
	return keep, with
}

// flagValues is the value given after each match of flag in s: a heredoc
// fed through $(cat <<'EOF' … EOF), a quoted string or a bare word.
func flagValues(s string, flag *regexp.Regexp) []string {
	var out []string
	// A heredoc's lines are what's fed in, not flags: a message that
	// mentions -m isn't given one.
	for _, loc := range flag.FindAllStringIndex(blankHeredocs(s), -1) {
		if v, ok := argValue(s[loc[1]:]); ok {
			out = append(out, v)
		}
	}
	return out
}

// blankHeredocs is s with each heredoc's lines blanked out, byte for byte,
// so what's found in it is where it is in s.
func blankHeredocs(s string) string {
	if !strings.Contains(s, "<<") {
		return s
	}
	return memo(&heredocsMemo, s, blankHeredocsOf)
}

func blankHeredocsOf(s string) string {
	b := []byte(s)
	for _, m := range stdinDoc.FindAllStringSubmatchIndex(s, -1) {
		nl := strings.IndexByte(s[m[1]:], '\n')
		if nl < 0 {
			continue
		}
		tag := s[m[2]:m[3]]
		for i := m[1] + nl + 1; i < len(s); {
			j := strings.IndexByte(s[i:], '\n')
			if j < 0 {
				j = len(s) - i
			}
			if strings.TrimSpace(s[i:i+j]) == tag {
				break
			}
			for k := i; k < i+j; k++ {
				b[k] = ' '
			}
			i += j + 1
		}
	}
	return string(b)
}

func argValue(s string) (string, bool) {
	if m := heredocArg.FindStringSubmatch(s); m != nil {
		_, rest, ok := strings.Cut(s, "\n")
		if !ok {
			return "", false
		}
		var b []string
		for _, l := range strings.Split(rest, "\n") {
			if strings.TrimSpace(l) == m[1] {
				return strings.Join(b, "\n"), true
			}
			b = append(b, l)
		}
		return "", false
	}
	if s == "" {
		return "", false
	}
	switch q := s[0]; q {
	case '"', '\'':
		var b strings.Builder
		for i := 1; i < len(s); i++ {
			c := s[i]
			switch {
			case c == q:
				return b.String(), true
			case q == '"' && c == '\\' && i+1 < len(s) && strings.IndexByte("\"\\$`", s[i+1]) >= 0:
				i++
				b.WriteByte(s[i])
			default:
				b.WriteByte(c)
			}
		}
		return "", false
	}
	if f := strings.Fields(s); len(f) > 0 && !strings.HasPrefix(f[0], "-") {
		return f[0], true
	}
	return "", false
}

// pushCard reads git's report of a push: "To github.com:o/r.git" and the
// refs it moved under it. Nothing moved (everything up-to-date) or all of it
// rejected is no card.
func pushCard(lines []string) (card, bool) {
	c := card{kind: "push"}
	for _, l := range lines {
		if m := pushTo.FindStringSubmatch(l); m != nil {
			c.remote = repoName(m[1])
			continue
		}
		m := pushRef.FindStringSubmatch(l)
		if m == nil || c.remote == "" {
			continue
		}
		p := pushed{from: m[3], to: m[4], forced: m[1] == "+", span: strings.Trim(m[2], "[]")}
		if m[1] == "-" {
			p.span = "deleted"
		}
		c.refs = append(c.refs, p)
	}
	return c, len(c.refs) > 0
}

// repoName is a remote as you'd say it: github.com/o/r.
func repoName(u string) string {
	u = strings.TrimSuffix(strings.TrimSuffix(u, "/"), ".git")
	scheme := false
	if i := strings.Index(u, "://"); i >= 0 {
		u, scheme = u[i+3:], true
	}
	host, path, _ := strings.Cut(u, "/")
	if at := strings.LastIndex(host, "@"); at >= 0 {
		host = host[at+1:]
	}
	u = host
	if path != "" {
		u += "/" + path
	}
	if !scheme {
		u = strings.Replace(u, ":", "/", 1) // host:o/r
	}
	return u
}

var (
	titleFlag = regexp.MustCompile(`\s(?:--title|-t)(?:=|\s+)`)
	bodyFlag  = regexp.MustCompile(`\s(?:--body|-b)(?:=|\s+)`)
	baseFlag  = regexp.MustCompile(`\s(?:--base|-B)(?:=|\s+)`)
	headFlag  = regexp.MustCompile(`\s(?:--head|-H)(?:=|\s+)`)
	draftFlag = regexp.MustCompile(`\s(?:--draft|-d)\b`)
	mergeHow  = regexp.MustCompile(`\s--(squash|rebase|merge)\b|\s-([srm])\b`)
	prArg     = regexp.MustCompile(`\bgh\s+pr\s+merge\s+(?:\S*/pull/)?#?(\d+)\b`)
)

// prCreateCard reads gh pr create: the pull request's address, which gh
// prints, and its title, branches and body, which the command gave it.
func prCreateCard(out, cmd string) (card, bool) {
	all := prURL.FindAllStringSubmatch(out, -1)
	if len(all) == 0 || strings.Contains(out, "already exists") {
		return card{}, false
	}
	m := all[len(all)-1]
	_, args, _ := strings.Cut(cmd, "gh pr create")
	c := card{kind: "pr", state: "open", url: m[1] + "/pull/" + m[2], num: m[2]}
	if draftFlag.MatchString(" " + args) {
		c.state = "draft"
	}
	first := func(f *regexp.Regexp) string {
		if v := flagValues(" "+args, f); len(v) > 0 {
			return strings.TrimSpace(v[0])
		}
		return ""
	}
	c.title, c.base, c.head = first(titleFlag), first(baseFlag), first(headFlag)
	if b := first(bodyFlag); b != "" {
		c.body, _ = messageBody("\n" + b)
	}
	if w := prCreating.FindStringSubmatch(out); w != nil {
		c.head, c.base = firstNonEmpty(c.head, w[1]), firstNonEmpty(c.base, w[2])
	}
	return c, true
}

// prMergeCard reads gh pr merge. gh says what it merged only on a
// terminal; otherwise the command's own number and way of merging have to
// do.
func prMergeCard(out, cmd string, ok bool) (card, bool) {
	c := card{kind: "pr", state: "merged"}
	if m := mergeHow.FindStringSubmatch(cmd); m != nil {
		c.base = map[string]string{"squash": "squashed", "s": "squashed", "rebase": "rebased", "r": "rebased"}[m[1]+m[2]]
	}
	if m := prMerged.FindStringSubmatch(out); m != nil {
		c.num, c.title = m[2], m[3]
		switch {
		case strings.HasPrefix(m[1], "Squashed"):
			c.base = "squashed"
		case strings.HasPrefix(m[1], "Rebased"):
			c.base = "rebased"
		}
	} else if m := prArg.FindStringSubmatch(cmd); m != nil && ok && !strings.Contains(cmd, "--auto") {
		c.num = m[1]
	} else {
		return card{}, false
	}
	if m := prDeleted.FindStringSubmatch(out); m != nil {
		c.head = m[1]
	}
	return c, true
}

var (
	conflictRe = regexp.MustCompile(`^CONFLICT \([^)]*\): (?:Merge conflict in )?(.+)$`)
	stoppedRe  = regexp.MustCompile(`^(?:error: )?[Cc]ould not apply ([0-9a-f]{7,40})\.\.\. ?(.*)$`)
	rebasedRe  = regexp.MustCompile(`^Successfully rebased and updated (?:refs/heads/)?(\S+?)\.?$`)
	mergedBy   = regexp.MustCompile(`^Merge made by the '(\S+)' strategy\.$`)
)

// mergeCard reads a merge, rebase or pull that did something: fast-forward,
// a merge commit, a rebase, or conflicts left for you.
func mergeCard(lines []string, calls []gitCall) (card, bool) {
	c := card{kind: "merge"}
	for _, g := range calls {
		switch g.verb {
		case "merge", "rebase", "pull":
			c.what, c.branch = g.verb, firstArg(g.args, "-m", "-X", "-s", "-F", "--strategy", "--strategy-option")
		}
	}
	stat := -1
	for i, l := range lines {
		if m := conflictRe.FindStringSubmatch(l); m != nil {
			if !slices.Contains(c.conflicts, m[1]) {
				c.conflicts = append(c.conflicts, m[1])
			}
		} else if m := stoppedRe.FindStringSubmatch(l); m != nil {
			c.sha, c.subject = m[1], m[2]
		} else if l == "Fast-forward" {
			c.how, stat = "fast-forward", i
		} else if m := mergedBy.FindStringSubmatch(l); m != nil {
			c.how, stat = "merge commit", i
		} else if m := rebasedRe.FindStringSubmatch(l); m != nil {
			c.how, c.head = "rebased", m[1]
		}
	}
	for i := stat + 1; stat >= 0 && i < len(lines); i++ {
		if m := commitStat.FindStringSubmatch(lines[i]); m != nil {
			c.files, _ = strconv.Atoi(m[1])
			c.add, _ = strconv.Atoi(m[2])
			c.del, _ = strconv.Atoi(m[3])
			break
		}
	}
	return c, c.what != "" && (c.how != "" || len(c.conflicts) > 0 || c.sha != "")
}

// firstArg is the first word of args that isn't a flag or a flag's value.
func firstArg(args string, valued ...string) string {
	var f []string
	for _, t := range shellTokens(args) {
		if strings.TrimSpace(t) != "" {
			f = append(f, t)
		}
	}
	for i := 0; i < len(f); i++ {
		switch {
		case slices.Contains(valued, f[i]):
			i++
		case strings.HasPrefix(f[i], "-"):
		default:
			return unquote(f[i])
		}
	}
	return ""
}

var (
	headNow   = regexp.MustCompile(`^HEAD is now at ([0-9a-f]{7,40}) ?(.*)$`)
	branchDel = regexp.MustCompile(`^Deleted (?:remote-tracking )?branch (\S+) \(was ([0-9a-f]{7,40})\)\.$`)
	stashSave = regexp.MustCompile(`^Saved working directory and index state (.*)$`)
	stashDrop = regexp.MustCompile(`^Dropped (\S+) \(([0-9a-f]{7,40})\)$`)
	removing  = regexp.MustCompile(`^Removing (.+)$`)
)

// discardCards reads git throwing work away or setting it aside: a hard
// reset, a deleted branch, a stash, untracked files cleaned, changes
// checked out over. Each says how to get it back, when git can.
func discardCards(lines []string, calls []gitCall, ok bool) []card {
	var cs []card
	find := func(re *regexp.Regexp) [][]string {
		var out [][]string
		for _, l := range lines {
			if m := re.FindStringSubmatch(l); m != nil {
				out = append(out, m)
			}
		}
		return out
	}
	done := map[string]bool{}
	for _, g := range calls {
		if done[g.verb] {
			continue
		}
		flag := func(fs ...string) bool {
			for _, f := range fs {
				if strings.Contains(g.args, " "+f+" ") {
					return true
				}
			}
			return false
		}
		switch g.verb {
		case "reset":
			if m := find(headNow); flag("--hard") && len(m) > 0 {
				last := m[len(m)-1]
				cs = append(cs, card{kind: "discard", what: "reset --hard", sha: last[1], subject: last[2], undo: "git reset --hard HEAD@{1}"})
			}
		case "branch":
			if !flag("-d", "-D", "--delete") {
				continue
			}
			safe := !flag("-D", "--force", "-f")
			for _, m := range find(branchDel) {
				cs = append(cs, card{kind: "discard", what: "branch -D", branch: m[1], sha: m[2], warn: safe,
					undo: "git branch " + m[1] + " " + shortSHA(m[2])})
				if safe {
					cs[len(cs)-1].what = "branch -d"
				}
			}
		case "stash":
			switch sub := firstArg(g.args); sub {
			case "", "push", "save":
				if m := find(stashSave); len(m) > 0 {
					cs = append(cs, card{kind: "discard", what: "stash", subject: m[0][1], warn: true, undo: "git stash pop"})
				}
			case "drop":
				for _, m := range find(stashDrop) {
					cs = append(cs, card{kind: "discard", what: "stash drop", branch: m[1], sha: m[2], undo: "git stash apply " + shortSHA(m[2])})
				}
			case "clear":
				if ok {
					cs = append(cs, card{kind: "discard", what: "stash clear"})
				}
			}
		case "clean":
			var gone []string
			for _, m := range find(removing) {
				gone = append(gone, m[1])
			}
			if len(gone) > 0 {
				cs = append(cs, card{kind: "discard", what: "clean", lost: gone})
			}
		case "checkout", "restore":
			// Over your changes: checkout -- paths, checkout ., or restore
			// of the working tree rather than just the index.
			over := flag("--") || firstArg(g.args) == "."
			if g.verb == "restore" {
				over = !flag("--staged", "-S") || flag("--worktree", "-W")
			}
			if !ok || !over {
				continue
			}
			var paths []string
			after := g.args
			if _, a, cut := strings.Cut(g.args, " -- "); cut {
				after = " " + a
			}
			for _, f := range strings.Fields(after) {
				if !strings.HasPrefix(f, "-") {
					paths = append(paths, unquote(f))
				}
			}
			if g.verb == "checkout" && !strings.Contains(g.args, " -- ") && len(paths) > 1 {
				paths = paths[1:] // checkout <ref> . reads from ref
			}
			if len(paths) > 0 {
				cs = append(cs, card{kind: "discard", what: g.verb, lost: paths})
			}
		default:
			continue
		}
		done[g.verb] = true
	}
	return cs
}

var (
	goFailT    = regexp.MustCompile(`^\s*--- FAIL: (\S+)`)
	goPassT    = regexp.MustCompile(`^\s*--- PASS: `)
	goSkipT    = regexp.MustCompile(`^\s*--- SKIP: `)
	goBuildF   = regexp.MustCompile(`^FAIL\s+(\S+) \[(build failed|setup failed)\]`)
	goErrLine  = regexp.MustCompile(`^\S+\.go:\d+:\d+: .+`)
	goEnd      = regexp.MustCompile(`^\s*(?:---|===) |^(?:FAIL|ok|PASS)\b|^panic: |^exit status`)
	pyFailed   = regexp.MustCompile(`^(?:FAILED|ERROR) (\S+?)(?: - (.*))?$`)
	pySummary  = regexp.MustCompile(`^=+ (.*\d+ (?:failed|passed|error).*) in [\d.]+s`)
	jestSum    = regexp.MustCompile(`^Tests:\s+(.*\d.*)$`)
	vitestSum  = regexp.MustCompile(`^\s*Tests\s+(.*\d+ (?:failed|passed).*)$`)
	cargoSum   = regexp.MustCompile(`^test result: \w+\. (.*)$`)
	cargoFail  = regexp.MustCompile(`^test (\S+) \.\.\. FAILED$`)
	jestFail   = regexp.MustCompile(`^\s+● (.+ › .+)$`)
	vitestFail = regexp.MustCompile(`^\s*FAIL\s+(.+ > .+?)(?:\s+\[.*\])?$`)
	vitestX    = regexp.MustCompile(`^\s*[×✗]\s+(.+?)(?:\s+\d+m?s)?$`)
	vitestFile = regexp.MustCompile(`^\s*(?:❯|>|FAIL)\s+(\S+\.\w+)\s+\((\d+) tests?\s*\|\s*(\d+) failed`)
	anError    = regexp.MustCompile(`^\s*(?:\w*(?:Error|Exception)\b:?|[Ee]xpected\b|assert(?:ion)?\b).*`)
	testTally  = regexp.MustCompile(`(\d+) (failed|passed|skipped|errors?|ignored)\b`)
	srcLoc     = regexp.MustCompile(`^(?:\S*/)?([^/\s]+\.\w+:\d+)(?::\d+)?:\s*`)
)

// testCard reads a test run that failed: how many failed, passed and were
// skipped, and which failed with what each said. A run that passed is no
// card; its row says so.
func testCard(out string, lines []string) (card, bool) {
	c := card{kind: "tests", what: "test"}
	tallied := false
	add := func(s string) {
		tallied = true
		for _, m := range testTally.FindAllStringSubmatch(s, -1) {
			n, _ := strconv.Atoi(m[1])
			switch m[2] {
			case "failed", "error", "errors":
				c.failed += n
			case "passed":
				c.passed += n
			default:
				c.skipped += n
			}
		}
	}
	seen := map[string]bool{}
	fail := func(f failure) {
		if !seen[f.name] {
			seen[f.name] = true
			f.msg = strings.TrimSpace(f.msg)
			c.fails = append(c.fails, f)
		}
	}
	// next is the first line after i that says something, when it isn't
	// where the runner moves on.
	next := func(i int) string {
		for _, n := range lines[i+1 : min(len(lines), i+4)] {
			if n = strings.TrimSpace(n); n != "" {
				return n
			}
		}
		return ""
	}
	var files []failure
	goPass, goSkip := 0, 0
	for i, l := range lines {
		switch {
		case goFailT.MatchString(l):
			// What the test logged, up to the next test or the package's end.
			var msg []string
			for _, n := range lines[i+1:] {
				if goEnd.MatchString(n) || len(msg) == 6 {
					break
				}
				msg = append(msg, n)
			}
			// Run with -v, it logged before it said it failed.
			if strings.TrimSpace(dedent(msg)) == "" {
				msg = nil
				for j := i - 1; j >= 0 && !goEnd.MatchString(lines[j]) && len(msg) < 6; j-- {
					msg = append([]string{lines[j]}, msg...)
				}
			}
			fail(failure{name: goFailT.FindStringSubmatch(l)[1], msg: dedent(msg)})
		case goPassT.MatchString(l):
			goPass++
		case goSkipT.MatchString(l):
			goSkip++
		case goBuildF.MatchString(l):
			m := goBuildF.FindStringSubmatch(l)
			// The compiler's errors under "# pkg", each where it is.
			var msg []string
			for j := i - 1; j >= 0 && !strings.HasPrefix(lines[j], "# "); j-- {
				if goErrLine.MatchString(lines[j]) {
					msg = append([]string{srcLoc.ReplaceAllString(lines[j], "$1: ")}, msg...)
				}
			}
			if len(msg) == 0 {
				msg = []string{m[2]}
			}
			fail(failure{name: m[1][strings.LastIndexByte(m[1], '/')+1:], msg: strings.Join(msg[:min(len(msg), 4)], "\n"), build: true})
		case pyFailed.MatchString(l):
			m := pyFailed.FindStringSubmatch(l)
			fail(failure{name: m[1], msg: m[2]})
		case cargoFail.MatchString(l):
			fail(failure{name: cargoFail.FindStringSubmatch(l)[1]})
		case jestFail.MatchString(l):
			m := jestFail.FindStringSubmatch(l)[1]
			fail(failure{name: strings.ReplaceAll(m, " › ", " > "), msg: next(i)})
		case vitestFile.MatchString(l):
			m := vitestFile.FindStringSubmatch(l)
			files = append(files, failure{name: m[1], msg: m[3] + " of " + m[2] + " failed"})
		case vitestFail.MatchString(l):
			fail(failure{name: vitestFail.FindStringSubmatch(l)[1], msg: next(i)})
		case vitestX.MatchString(l):
			msg := next(i)
			if !strings.HasPrefix(msg, "→") {
				msg = ""
			}
			fail(failure{name: vitestX.FindStringSubmatch(l)[1], msg: strings.TrimSpace(strings.TrimPrefix(msg, "→"))})
		case pySummary.MatchString(l):
			add(pySummary.FindStringSubmatch(l)[1])
		case jestSum.MatchString(l):
			add(jestSum.FindStringSubmatch(l)[1])
		case vitestSum.MatchString(l):
			add(vitestSum.FindStringSubmatch(l)[1])
		case cargoSum.MatchString(l):
			add(cargoSum.FindStringSubmatch(l)[1])
		}
	}
	// A failed Go subtest fails its parent too, and vitest names a failure
	// once as it runs and again in full: the one that says more stays.
	keep := c.fails[:0]
	for _, f := range c.fails {
		dup := false
		for k, g := range c.fails {
			if strings.HasPrefix(g.name, f.name+"/") {
				dup = true
			}
			if strings.HasSuffix(g.name, " > "+f.name) {
				dup = true
				if g.msg == "" {
					c.fails[k].msg = f.msg
				}
			}
		}
		if !dup {
			keep = append(keep, f)
		}
	}
	c.fails = keep
	if !tallied {
		c.failed, c.passed, c.skipped = len(c.fails), goPass, goSkip
	}
	// Named no test (the output was cut, say): which files failed, else
	// the first thing that reads like an error.
	if c.failed > 0 && len(c.fails) == 0 {
		c.fails = files
		if len(c.fails) == 0 {
			for _, l := range lines {
				if anError.MatchString(l) {
					c.fails = []failure{{msg: strings.TrimSpace(l)}}
					break
				}
			}
		}
	}
	return c, c.failed > 0 || len(c.fails) > 0
}

var (
	tscParen  = regexp.MustCompile(`^(\S+?)\((\d+),\d+\): error TS\d+: (.*)$`)
	tscPretty = regexp.MustCompile(`^(\S+?):(\d+):\d+ - error TS\d+: (.*)$`)
	tscFound  = regexp.MustCompile(`^Found (\d+) errors?\b`)
	esbuildE  = regexp.MustCompile(`^\s*[✘X] \[ERROR\] (.*?)(?:\s+\[plugin .*\])?$`)
	esbuildAt = regexp.MustCompile(`^\s+(\S+:\d+):\d+:\s*$`)
	webpackE  = regexp.MustCompile(`^ERROR in (\S+?)(?: (\d+):\d+(?:-\d+)?)?$`)
	nxFailed  = regexp.MustCompile(`^\s*-\s+(\S+:\S+)\s*$`)
)

// buildCard reads a build or type check that failed: Go's compiler and
// vet, tsc, esbuild and vite, webpack, and the tasks nx says failed. Each
// error is a row, where it is first. A build that went is no card.
func buildCard(verb string, lines []string) (card, bool) {
	c := card{kind: "build", what: verb}
	seen := map[string]bool{}
	add := func(file, line, msg string) {
		at := ""
		if file != "" {
			at = file[strings.LastIndexByte(file, '/')+1:] + ":" + line + ": "
		}
		if msg = strings.TrimSpace(msg); msg != "" && !seen[at+msg] {
			seen[at+msg] = true
			c.fails = append(c.fails, failure{msg: at + msg})
		}
	}
	nx := false
	for i, l := range lines {
		switch {
		case goErrLine.MatchString(l) && strings.Contains(l, ".go:"):
			add("", "", srcLoc.ReplaceAllString(l, "$1: "))
		case tscParen.MatchString(l):
			m := tscParen.FindStringSubmatch(l)
			add(m[1], m[2], m[3])
		case tscPretty.MatchString(l):
			m := tscPretty.FindStringSubmatch(l)
			add(m[1], m[2], m[3])
		case tscFound.MatchString(l):
			c.failed, _ = strconv.Atoi(tscFound.FindStringSubmatch(l)[1])
		case esbuildE.MatchString(l):
			msg := esbuildE.FindStringSubmatch(l)[1]
			file, line := "", ""
			for _, n := range lines[i+1 : min(len(lines), i+4)] {
				if m := esbuildAt.FindStringSubmatch(n); m != nil {
					file, line, _ = strings.Cut(m[1], ":")
					break
				}
			}
			add(file, line, msg)
		case webpackE.MatchString(l) && i+1 < len(lines):
			m := webpackE.FindStringSubmatch(l)
			if m[2] == "" {
				add("", "", strings.TrimPrefix(m[1], "./")+": "+lines[i+1])
			} else {
				add(m[1], m[2], lines[i+1])
			}
		case strings.Contains(l, "Failed tasks:"):
			nx = true
		case nx && nxFailed.MatchString(l):
			c.fails = append(c.fails, failure{name: nxFailed.FindStringSubmatch(l)[1], build: true})
		}
	}
	c.failed = max(c.failed, len(c.fails))
	return c, len(c.fails) > 0
}

// dedent is lines less the indentation they share, blank ends dropped.
func dedent(lines []string) string {
	shared := -1
	for _, l := range lines {
		if strings.TrimSpace(l) == "" {
			continue
		}
		if n := len(l) - len(strings.TrimLeft(l, " \t")); shared < 0 || n < shared {
			shared = n
		}
	}
	var out []string
	for _, l := range lines {
		if len(l) >= shared && shared > 0 {
			l = l[shared:]
		}
		out = append(out, strings.TrimRight(l, " \t"))
	}
	return strings.Trim(strings.Join(out, "\n"), "\n")
}

// --- drawing ---

// stepCards is cardsOf, kept while the step doesn't change: a running turn
// redraws every second.
func (d *drawer) stepCards(st *Step) []card {
	if st.Tool != "Bash" {
		return nil
	}
	s := d.s
	k := stepKey{st: st, what: 'c', status: st.Status, out: len(st.Output), res: len(st.Result)}
	if v, ok := s.cards[k]; ok {
		return v
	}
	v, ok := s.cardsOld[k]
	waits := lookupWaits.Load()
	if !ok {
		v = cardsOf(st)
		if cs, ok := d.gitCommits(st); ok {
			v = withCommits(v, cs)
		}
		if g, ok := d.gitMerge(st); ok {
			v = withMerge(v, &g)
		}
	}
	if s.cards != nil && lookupWaits.Load() == waits { // kept once git has answered
		s.cards[k] = v
	}
	return v
}

// cards draws each card a step left, indent cells in.
func (d *drawer) cards(st *Step, indent int) {
	for _, c := range d.stepCards(st) {
		d.card(c, indent)
	}
}

func (d *drawer) card(c card, indent int) {
	// "│ " … " │" around the inside.
	room := min(d.cw, capRow) - indent - 4
	if room < 24 {
		return
	}
	// Every card the same width, so a stack of them reads as one.
	room = min(room, 72)
	frame, mark := cFaint, cOrange
	switch c.tone() {
	case toneMade:
		frame, mark = cOKq, cGreen
	case toneWarn:
		frame, mark = cWarnQ, cYellow
	case toneLost:
		frame, mark = cLostQ, cLost
	}
	glyph := func(g string) string { return paint(mark, g) + " " }
	var headL, headR, foot string
	var rows []string
	switch c.kind {
	case "commit":
		headL = glyph("●") + paint(cYellow, shortSHA(c.sha))
		if c.sha == "" {
			headL = glyph("●") + dim("committed")
		}
		if c.branch != "" {
			headL += faint(" · ") + paint(cBlue, c.branch)
		}
		if c.amend {
			headL += faint(" · ") + dim("amended")
		}
		if c.root {
			headL += faint(" · ") + dim("first commit")
		}
		if c.files > 0 {
			headR = paint(cGreen, "+"+strconv.Itoa(c.add)) + " " + paint(cRed, "−"+strconv.Itoa(c.del)) + faint(" · ") + dim(plural(c.files, "file"))
		}
		rows = cardText(c.subject, c.body, room)
		if len(c.with) > 0 {
			foot = dim("with " + strings.Join(c.with, ", "))
		}
	case "push":
		headL = glyph("↑") + dim("pushed to ") + text(c.remote)
		for _, p := range c.refs {
			r := paint(cBlue, p.from)
			if p.to != p.from {
				r += faint(" → ") + paint(cBlue, p.to)
			}
			span := dim(p.span)
			if p.span == "deleted" {
				span = paint(cLost, "deleted")
			}
			if p.forced {
				span = paint(cYellow, "forced") + faint(" · ") + dim(p.span)
			}
			rows = append(rows, r+"  "+span)
		}
	case "pr":
		state := paint(cGreen, c.state)
		switch c.state {
		case "draft":
			state = dim("draft")
		case "merged":
			state = paint(cBlue, "merged")
			if c.base != "" {
				state += dim(" · " + c.base)
			}
		}
		headL = glyph("⎇")
		if c.num != "" {
			headL += text("#"+c.num) + faint(" · ")
		}
		headL += state
		if c.state != "merged" && c.head != "" && c.base != "" {
			headR = paint(cBlue, c.head) + faint(" → ") + paint(cBlue, c.base)
		}
		if c.title != "" {
			rows = cardText(c.title, c.body, room)
		}
		switch {
		case c.url != "":
			foot = dim(c.url)
		case c.state == "merged" && c.head != "":
			foot = dim(c.head + " deleted")
		}
	case "tests":
		headL = glyph("✗") + paint(cLost, strconv.Itoa(max(c.failed, len(c.fails)))+" failed")
		var tally []string
		if c.passed > 0 {
			tally = append(tally, strconv.Itoa(c.passed)+" passed")
		}
		if c.skipped > 0 {
			tally = append(tally, strconv.Itoa(c.skipped)+" skipped")
		}
		headR = dim(strings.Join(tally, " · "))
		rows = failRows(c.fails, room)
	case "build":
		headL = glyph("✗") + paint(cLost, plural(c.failed, "error"))
		headR = dim(c.what)
		rows = failRows(c.fails, room)
	case "merge":
		g, verb := "⇣", c.what+"d"
		switch c.what {
		case "rebase":
			g, verb = "↻", "rebased"
		case "pull":
			verb = "pulled"
		}
		// Stopped part way, it's still going: rebasing feat onto main.
		if len(c.conflicts) > 0 || c.sha != "" {
			g, verb = "⚠", strings.TrimSuffix(c.what, "e")+"ing"
		}
		headL = glyph(g) + dim(verb)
		switch {
		case c.what == "rebase" && c.head != "":
			headL += " " + paint(cBlue, c.head)
			if c.branch != "" {
				headL += dim(" onto ") + paint(cBlue, c.branch)
			}
		case c.what == "rebase" && c.branch != "":
			headL += dim(" onto ") + paint(cBlue, c.branch)
		case c.branch != "":
			headL += " " + paint(cBlue, c.branch)
		}
		// The graph shows a merge commit for itself.
		if c.how != "" && c.how != "rebased" && c.made == "" {
			headL += faint(" · ") + dim(c.how)
		}
		if c.files > 0 {
			headR = paint(cGreen, "+"+strconv.Itoa(c.add)) + " " + paint(cRed, "−"+strconv.Itoa(c.del)) + faint(" · ") + dim(plural(c.files, "file"))
		}
		if c.sha != "" {
			rows = append(rows, dim("stopped at ")+paint(cYellow, shortSHA(c.sha))+" "+text(c.subject))
		}
		rows = append(rows, listRows(c.conflicts, paint(cYellow, "conflict")+"  ")...)
		if len(c.conflicts) > 0 {
			foot = dim(plural(len(c.conflicts), "file") + " to resolve")
		}
		if len(c.came) > 0 {
			rows = append(rows, mergeRows(&c, mark)...)
			if c.into != "" {
				foot = dim("into ") + paint(cBlue, c.into)
			}
		}
	case "discard":
		what := c.what
		switch c.what {
		case "stash":
			what = "stashed"
		case "clean":
			what = "removed " + plural(len(c.lost), "untracked file")
		case "checkout", "restore":
			what = "discarded changes"
		case "stash clear":
			what = "cleared every stash"
		}
		headL = glyph("⚠") + text(what)
		switch {
		case c.branch != "" && c.sha != "":
			headL += " " + paint(cBlue, c.branch) + faint(" · was ") + paint(cYellow, shortSHA(c.sha))
		case c.sha != "":
			headL += faint(" · now at ") + paint(cYellow, shortSHA(c.sha))
		}
		if c.subject != "" {
			rows = append(rows, text(c.subject))
		}
		rows = append(rows, listRows(c.lost, "")...)
		switch {
		case c.undo != "":
			foot = dim("back: " + c.undo)
		case c.warn:
		default:
			foot = paint(cLostQ, "git can't bring this back")
		}
	}
	if c.kind == "tests" || c.kind == "build" {
		pad := d.spine() + blanks(indent-1)
		d.add("", bgFailure, pad+headL, headR)
		for _, line := range rows {
			d.add("", bgFailure, pad+line, "")
		}
		if foot != "" {
			d.add("", bgFailure, pad+dim(foot), "")
		}
		return
	}
	d.box("", indent, headL, headR, rows, foot, room, frame)
}

// mergeRows is a merge as git log --graph draws it, newest first: the merge
// commit, the commits it brought in on a lane of their own, and where the
// branch was. A fast-forward is one lane.
func mergeRows(c *card, mark string) []string {
	lane := ""
	if c.made != "" {
		lane = faint("│") + " "
	}
	line := func(g, sha, subject string, dimmed bool) string {
		s := text(oneLine(subject))
		if dimmed {
			s = dim(oneLine(subject))
		}
		return g + " " + paint(cYellow, shortSHA(sha)) + "  " + s
	}
	var rows []string
	if c.made != "" {
		rows = append(rows, line(paint(mark, "●")+faint("─╮"), c.made, "merge commit", true))
	}
	for _, g := range c.came[:min(len(c.came), 4)] {
		rows = append(rows, line(lane+paint(cGreen, "●"), g.sha, g.subject, false))
	}
	if more := c.cameN - min(len(c.came), 4); more > 0 {
		rows = append(rows, lane+faint("┊")+" "+dim(fmt.Sprintf("+%d more", more)))
	}
	if c.was.sha == "" {
		return rows
	}
	switch {
	case c.made == "":
		rows = append(rows, line(faint("○"), c.was.sha, c.was.subject, true))
	case c.forked:
		rows = append(rows, line(faint("●─╯"), c.was.sha, c.was.subject, true))
	default:
		// The brought-in lane goes on down, past where the branch was.
		rows = append(rows, line(faint("● │"), c.was.sha, c.was.subject, true))
	}
	return rows
}

// failRows is each failure's name, and under it what it said: the first
// few rows of it for the first few failures, then names alone.
func failRows(fs []failure, w int) []string {
	var rows []string
	for i, f := range fs {
		if len(rows) >= 11 && i < len(fs)-1 {
			rows = append(rows, dim(fmt.Sprintf("+%d more", len(fs)-i)))
			break
		}
		lead := ""
		if f.name != "" {
			name, where := f.name, ""
			// vitest's and jest's file > describe > test: the test first.
			if file, rest, ok := strings.Cut(name, " > "); ok {
				name, where = rest, file[strings.LastIndexByte(file, '/')+1:]
			}
			r := paint(cWhite, name)
			if f.build {
				r += faint(" · ") + paint(cLost, "doesn't build")
			}
			if where != "" {
				r += "  " + faint(where)
			}
			rows = append(rows, r)
			lead = "  "
		}
		if f.msg == "" {
			continue
		}
		msg := strings.Split(f.msg, "\n")
		keep := 3
		if i >= 3 {
			keep = 1
		}
		for j, l := range msg[:min(len(msg), keep)] {
			r := lead
			if loc := srcLoc.FindStringSubmatch(l); loc != nil {
				r += paint(cBlue, loc[1]) + " "
				l = l[len(loc[0]):]
			}
			l = expandTabs(l)
			r += blanks(len(l)-len(strings.TrimLeft(l, " "))) + sub(oneLine(l))
			if j == keep-1 && len(msg) > keep {
				r = ansi.Truncate(r, w-1, "") + faint("…")
			}
			rows = append(rows, r)
		}
	}
	return rows
}

// listRows is a list for a card, five at most, each after lead.
func listRows(xs []string, lead string) []string {
	var rows []string
	for i, x := range xs {
		if i == 4 && len(xs) > 5 {
			rows = append(rows, dim(fmt.Sprintf("+%d more", len(xs)-4)))
			break
		}
		rows = append(rows, lead+text(x))
	}
	return rows
}

// cardText is a title, up to two rows, and the start of a body under it, to
// five rows in all.
func cardText(title string, body []string, w int) []string {
	rows := wrap(paint(cWhite, oneLine(title)), w)
	if len(rows) > 2 {
		rows = append(rows[:1], ansi.Truncate(rows[1], w-1, "")+dim("…"))
	}
	// The body's first paragraphs, git's hard wraps undone.
	var paras []string
	cur := ""
	for _, l := range body {
		if strings.TrimSpace(l) == "" {
			if cur != "" {
				paras, cur = append(paras, cur), ""
			}
			continue
		}
		cur = strings.TrimSpace(cur + " " + strings.TrimSpace(l))
	}
	if cur != "" {
		paras = append(paras, cur)
	}
	var bl []string
	for _, p := range paras {
		bl = append(bl, wrap(sub(p), w)...)
	}
	left := 5 - len(rows)
	if len(bl) > left {
		bl = bl[:left]
		last := bl[left-1]
		bl[left-1] = ansi.Truncate(last, w-1, "") + dim("…")
	}
	return append(rows, bl...)
}

// box draws a card as lines under the turn's rule, not a frame: its head
// with the right side flush right, its rows, its foot. A rail in colour
// frame runs down them when there's more than the head: ╭ on the head,
// ╰ on the last. The head is row ref.
func (d *drawer) box(ref string, indent int, headL, headR string, rows []string, foot string, inner int, frame string) {
	pad := d.spine() + blanks(indent-1)
	body := make([]string, 0, len(rows)+1)
	for _, r := range rows {
		if cellw.String(r) > inner {
			r = ansi.Truncate(r, inner-1, "") + faint("…")
		}
		body = append(body, r)
	}
	if foot != "" {
		body = append(body, dim(foot))
	}
	rail := func(i int) string {
		switch {
		case len(body) == 0:
			return " "
		case i < 0:
			return paint(frame, "╭")
		case i == len(body)-1:
			return paint(frame, "╰")
		}
		return paint(frame, "│")
	}
	d.add(ref, "", pad+rail(-1)+" "+headL, headR)
	for i, r := range body {
		d.add("", "", pad+rail(i)+" "+r, "")
	}
}

func shortSHA(s string) string {
	if len(s) > 7 {
		return s[:7]
	}
	return s
}

// testsFailed is whether a step ran tests that failed, whatever it exited
// with: piped through tail, a failing run exits 0. Every step is asked
// each frame, so the answer is kept by the step alone: a tests card is
// read from the output, and a git lookup finishing doesn't change it.
func (d *drawer) testsFailed(st *Step) bool {
	return d.stepMemo(st, 't', func(st *Step) string {
		cs := d.stepCards(st)
		for i := range cs {
			if cs[i].kind == "tests" || cs[i].kind == "build" {
				return "failed"
			}
		}
		return ""
	}) != ""
}
