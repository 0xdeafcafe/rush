package convo

import (
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A commit card read from the command and its output is a guess: a message
// given as "$msg" reads as "$msg", a loop's ten commits as one, a quiet one
// as nothing. When the session's folder is on this machine, git says what
// was committed instead. The hashes git printed are the commits, when it
// printed any; otherwise they're what HEAD's reflog took in while the
// command ran.

var (
	cdTo    = lazyRe(`(?:^|[;&|(\n]\s*)cd\s+("[^"]*"|'[^']*'|\S+)`)
	gitDirC = lazyRe(`\bgit\s+-C\s+("[^"]*"|'[^']*'|\S+)`)
	// What a reflog calls a commit being made; a rebase's picks and a
	// merge's fast-forward are the merge card's.
	reflogCommit = lazyRe(`^(commit(?: \((amend|initial|merge)\))?|cherry-pick|revert): `)
	// A merge or pull that moved HEAD: by a merge commit or a fast-forward.
	reflogMerge = lazyRe(`^(?:merge \S+|pull\b[^:]*): (?:Fast-forward|Merge made by)`)
	shortstat   = lazyRe(`(\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?`)
)

// commitQuery is what's asked of git for a step: the folders it may have
// committed in, when it ran, and the hashes git printed.
type commitQuery struct {
	merge    bool   // a merge's graph, rather than commits
	dirs     string // \n-separated
	from, to int64  // unix seconds the step ran in, as the reflog counts them
	shas     string // \n-separated, as printed
	amended  bool   // the command amends, for printed commits the reflog misses
}

type commitLookup struct {
	done  bool
	cards []card // nil when git couldn't say
}

// A reflog read after a window closed holds all of that window.
type reflogRead struct {
	at      time.Time
	entries []reflogEntry // newest first
}

type reflogEntry struct {
	sha, what string
	at        int64
}

var (
	lookups = struct {
		sync.Mutex
		m      map[commitQuery]*commitLookup
		reflog map[string]reflogRead
	}{m: map[commitQuery]*commitLookup{}, reflog: map[string]reflogRead{}}
	// lookupsGen counts finished lookups, commits and thumbnails, so what
	// was drawn before one finished is drawn again.
	lookupsGen atomic.Int64
	// lookupWaits counts answers of "not yet" from those lookups: a drawing
	// during which it moved waits on one, so only it redraws as gen moves.
	lookupWaits atomic.Int64
	// Only a few lookups at once: a long transcript asks about every
	// commit it made the first time it's drawn.
	lookupSlots = make(chan struct{}, 4)
)

// gitCommits is what git says a finished step committed, and whether it
// could say. It never runs git while a frame is drawn: the first call
// starts the lookup and says no, and a frame after it finishes has it.
func (d *drawer) gitCommits(st *Step) ([]card, bool) {
	q, ok := d.commitQuery(st)
	if !ok {
		return nil, false
	}
	return lookup(q)
}

// gitMerge is what git says a finished merge or pull did, as a merge card
// with its graph, and whether it could say. Like gitCommits, it never runs
// git while a frame is drawn.
func (d *drawer) gitMerge(st *Step) (card, bool) {
	q, ok := d.gitQuery(st, "merge", "pull")
	if !ok {
		return card{}, false
	}
	q.merge, q.shas = true, ""
	cs, ok := lookup(q)
	if !ok || len(cs) == 0 {
		return card{}, false
	}
	return cs[0], true
}

func lookup(q commitQuery) ([]card, bool) {
	lookups.Lock()
	defer lookups.Unlock()
	l := lookups.m[q]
	if l == nil {
		l = &commitLookup{}
		lookups.m[q] = l
		go func() {
			lookupSlots <- struct{}{}
			var cs []card
			if q.merge {
				cs = lookMerge(q)
			} else {
				cs = lookCommits(q)
			}
			<-lookupSlots
			lookups.Lock()
			l.cards, l.done = cs, true
			lookups.Unlock()
			lookupsGen.Add(1)
		}()
	}
	if !l.done {
		lookupWaits.Add(1)
	}
	if !l.done || l.cards == nil {
		return nil, false
	}
	return l.cards, true
}

func (d *drawer) commitQuery(st *Step) (commitQuery, bool) {
	return d.gitQuery(st, "commit", "cherry-pick", "revert")
}

// gitQuery is what to ask git about a step that ran one of verbs.
func (d *drawer) gitQuery(st *Step, verbs ...string) (commitQuery, bool) {
	if st.kind() != tool.Shell || st.Status == Running || st.Status == Waiting || st.Start.IsZero() || st.End.IsZero() {
		return commitQuery{}, false
	}
	cmd := st.in().Command
	commits := false
	for _, c := range gitCalls(cmd) {
		commits = commits || slices.Contains(verbs, c.verb)
	}
	base := firstNonEmpty(d.s.Info.Cwd, d.s.Cwd)
	if !commits || base == "" {
		return commitQuery{}, false
	}
	dirs := []string{base}
	plain := blankHeredocs(cmd)
	for _, re := range []*regexp.Regexp{cdTo(), gitDirC()} {
		for _, m := range re.FindAllStringSubmatch(plain, -1) {
			dir := unquote(m[1])
			if strings.ContainsAny(dir, "$`") || dir == "-" {
				continue
			}
			if rest, ok := strings.CutPrefix(dir, "~"); ok {
				home, _ := os.UserHomeDir()
				dir = home + rest
			}
			if !filepath.IsAbs(dir) {
				dir = filepath.Join(base, dir)
			}
			if !slices.Contains(dirs, dir) {
				dirs = append(dirs, dir)
			}
		}
	}
	var shas []string
	out := stripANSI(bashOut(st))
	for _, l := range strings.FieldsFunc(out, func(r rune) bool { return r == '\n' || r == '\r' }) {
		if m := commitHead().FindStringSubmatch(l); m != nil {
			shas = append(shas, m[2])
		}
	}
	return commitQuery{
		dirs:    strings.Join(dirs, "\n"),
		from:    st.Start.Unix(),
		to:      st.End.Unix(),
		shas:    strings.Join(shas, "\n"),
		amended: strings.Contains(cmd, "--amend"),
	}, true
}

// lookCommits asks git for q's commits: nil when it can't say, as when the
// folder isn't here or isn't a repository, or nothing turned up.
func lookCommits(q commitQuery) []card {
	var printed []string
	if q.shas != "" {
		printed = strings.Split(q.shas, "\n")
	}
	for _, dir := range strings.Split(q.dirs, "\n") {
		if _, err := os.Stat(dir); err != nil {
			continue
		}
		made := windowCommits(dir, q.from, q.to)
		shas := printed
		if len(shas) == 0 {
			// Another session in this folder may commit meanwhile; with
			// nothing printed, what HEAD took in is the best there is.
			for _, e := range made {
				shas = append(shas, e.sha)
			}
		}
		if len(shas) == 0 {
			continue
		}
		cs := showCommits(dir, shas)
		if len(cs) != len(shas) {
			continue // not this folder's, or not all of them
		}
		for i := range cs {
			for _, e := range made {
				if strings.HasPrefix(e.sha, cs[i].sha) || strings.HasPrefix(cs[i].sha, e.sha) {
					cs[i].amend = e.amend
				}
			}
			if len(printed) > 0 && len(made) == 0 {
				cs[i].amend = q.amended // no reflog to say
			}
		}
		return cs
	}
	return nil
}

// windowCommits is each commit HEAD's reflog in dir took in between from
// and to, oldest first.
func windowCommits(dir string, from, to int64) []commitEntry {
	var out []commitEntry
	for _, e := range reflog(dir, to) {
		if m := reflogCommit().FindStringSubmatch(e.what); m != nil && e.at >= from && e.at <= to {
			out = append(out, commitEntry{sha: e.sha, amend: m[2] == "amend"})
		}
	}
	slices.Reverse(out) // the reflog is newest first
	return out
}

type commitEntry struct {
	sha   string
	amend bool
}

// reflog is HEAD's reflog in dir, newest first, read since to.
func reflog(dir string, to int64) []reflogEntry {
	lookups.Lock()
	r, ok := lookups.reflog[dir]
	lookups.Unlock()
	if !ok || r.at.Unix() <= to {
		at := time.Now()
		raw, err := git(dir, "reflog", "show", "--date=unix", "--format=%H%x1f%gd%x1f%gs", "HEAD")
		if err != nil {
			return nil
		}
		r = reflogRead{at: at}
		for _, l := range strings.Split(string(raw), "\n") {
			f := strings.Split(l, "\x1f")
			if len(f) != 3 {
				continue
			}
			// HEAD@{1727260000}
			_, ts, _ := strings.Cut(strings.TrimSuffix(f[1], "}"), "@{")
			n, err := strconv.ParseInt(ts, 10, 64)
			if err != nil {
				continue
			}
			r.entries = append(r.entries, reflogEntry{sha: f[0], what: f[2], at: n})
		}
		lookups.Lock()
		lookups.reflog[dir] = r
		lookups.Unlock()
	}
	return r.entries
}

// showCommits is a card for each of shas, as git has it, in their order.
func showCommits(dir string, shas []string) []card {
	raw, err := git(dir, append([]string{"show", "--shortstat", "--format=%x1e%H%x1f%P%x1f%s%x1f%b%x1f"}, shas...)...)
	if err != nil {
		return nil
	}
	var cs []card
	var full []string
	for _, rec := range strings.Split(string(raw), "\x1e")[1:] {
		f := strings.Split(rec, "\x1f")
		if len(f) != 5 {
			return nil
		}
		c := card{kind: "commit", sha: f[0], subject: f[2], root: f[1] == ""}
		c.body, c.with = messageBody("\n" + strings.TrimRight(f[3], "\n"))
		if s := shortstat().FindStringSubmatch(f[4]); s != nil {
			c.files, _ = strconv.Atoi(s[1])
			c.add, _ = strconv.Atoi(s[2])
			c.del, _ = strconv.Atoi(s[3])
		}
		cs = append(cs, c)
		full = append(full, f[0])
	}
	// The branch each is on: main~2 is main's.
	if names, err := git(dir, append([]string{"name-rev", "--name-only", "--refs=refs/heads/*"}, full...)...); err == nil {
		for i, n := range strings.Split(strings.TrimSpace(string(names)), "\n") {
			if n = strings.TrimSpace(n); i < len(cs) && n != "undefined" {
				cs[i].branch = strings.TrimPrefix(strings.FieldsFunc(n, func(r rune) bool { return r == '~' || r == '^' })[0], "heads/")
			}
		}
	}
	return cs
}

// withCommits is cs with its commit cards swapped for git's, where the
// first of them was, or after any merge's card. The branch git printed
// with a commit is the one it was made on, "detached HEAD" too.
func withCommits(cs, commits []card) []card {
	commits = slices.Clone(commits)
	var out []card
	at := -1
	for _, c := range cs {
		if c.kind == "commit" {
			if at < 0 {
				at = len(out)
			}
			for i := range commits {
				if c.sha != "" && c.branch != "" && strings.HasPrefix(commits[i].sha, c.sha) {
					commits[i].branch = c.branch
				}
			}
			continue
		}
		out = append(out, c)
	}
	if at < 0 {
		for at = 0; at < len(out) && out[at].kind == "merge"; at++ {
		}
	}
	return slices.Concat(out[:at], commits, out[at:])
}

// lookMerge asks git for the last merge q's step made: the commit it made,
// if any, where the branch was, and what it brought in. nil when it can't
// say.
func lookMerge(q commitQuery) []card {
	for dir := range strings.SplitSeq(q.dirs, "\n") {
		if _, err := os.Stat(dir); err != nil {
			continue
		}
		es := reflog(dir, q.to)
		for i, e := range es[:max(0, len(es)-1)] {
			if e.at < q.from || e.at > q.to || !reflogMerge().MatchString(e.what) {
				continue
			}
			// The entry before it is where HEAD was.
			if c, ok := mergeGraph(dir, es[i+1].sha, e.sha); ok {
				return []card{c}
			}
			break
		}
	}
	return nil
}

// mergeGraph is the merge that took HEAD from was to now: a merge commit,
// when now has two parents, else a fast-forward.
func mergeGraph(dir, was, now string) (card, bool) {
	raw, err := git(dir, "rev-list", "--parents", "-n1", now)
	if err != nil {
		return card{}, false
	}
	c := card{kind: "merge", forked: true}
	from, to := was, now
	if p := strings.Fields(string(raw)); len(p) >= 3 {
		c.made, from, to = p[0], p[1], p[2]
		if base, err := git(dir, "merge-base", from, to); err == nil {
			c.forked = strings.TrimSpace(string(base)) == from
		}
	}
	raw, err = git(dir, "log", "--format=%H%x1f%s", "-n5", from+".."+to)
	if err != nil {
		return card{}, false
	}
	for l := range strings.SplitSeq(strings.TrimSpace(string(raw)), "\n") {
		if sha, subj, ok := strings.Cut(l, "\x1f"); ok {
			c.came = append(c.came, gitLine{sha, subj})
		}
	}
	c.cameN = len(c.came)
	if n, err := git(dir, "rev-list", "--count", from+".."+to); err == nil {
		c.cameN, _ = strconv.Atoi(strings.TrimSpace(string(n)))
	}
	if raw, err := git(dir, "show", "-s", "--format=%s", from); err == nil {
		c.was = gitLine{from, strings.TrimSpace(string(raw))}
	}
	if n, err := git(dir, "name-rev", "--name-only", "--refs=refs/heads/*", now); err == nil {
		if n := strings.TrimSpace(string(n)); n != "" && n != "undefined" {
			c.into = strings.TrimPrefix(strings.FieldsFunc(n, func(r rune) bool { return r == '~' || r == '^' })[0], "heads/")
		}
	}
	return c, len(c.came) > 0
}

// withMerge is cs with git's graph on the merge card it's of: a merge
// commit's on one that made a merge commit, a fast-forward's on one that
// fast-forwarded.
func withMerge(cs []card, g *card) []card {
	cs = slices.Clone(cs)
	for i := range cs {
		c := &cs[i]
		if c.kind != "merge" || (c.how == "merge commit") != (g.made != "") || (c.how == "fast-forward") != (g.made == "") {
			continue
		}
		c.made, c.into, c.was, c.came, c.cameN, c.forked = g.made, g.into, g.was, g.came, g.cameN, g.forked
	}
	return cs
}
