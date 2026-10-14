package watch

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A Check is one check on a pull request's head commit: a check run, or a
// commit status.
type Check struct {
	ID      int64 // a check run's; 0 for a commit status
	Name    string
	State   string // passed, failed, pending or skipped
	URL     string
	Actions bool   // run by GitHub Actions, so its job's log can be read
	Note    string // what it says of itself: its output's title, a status's description
}

func (c Check) finished() bool { return c.State != "pending" }

// A PR is a pull request and the checks on its head commit, as GitHub
// last said.
type PR struct {
	Repo   string // owner/name
	Number int
	Title  string
	URL    string
	State  string // open, merged or closed
	Head   string // the head commit's sha
	Checks []Check
}

// Counts are its checks by how they went.
func (p PR) Counts() (passed, failed, pending, skipped int) {
	for _, c := range p.Checks {
		switch c.State {
		case "passed":
			passed++
		case "failed":
			failed++
		case "skipped":
			skipped++
		default:
			pending++
		}
	}
	return
}

// Fetch is pull request n of repo as GitHub says now, or as another
// watcher heard within fresh. remaining is the rate limit left, -1 when
// unknown.
func Fetch(ctx context.Context, repo string, n int, fresh time.Duration) (PR, int, error) {
	remaining := -1
	get := func(path string, v any) error {
		b, r, err := Get(ctx, path, fresh)
		if err != nil {
			return err
		}
		if r >= 0 {
			remaining = r
		}
		return jsonx.Unmarshal(b, v)
	}
	var pull struct {
		Number  int    `json:"number"`
		Title   string `json:"title"`
		HTMLURL string `json:"html_url"`
		State   string `json:"state"`
		Merged  bool   `json:"merged"`
		Head    struct {
			SHA string `json:"sha"`
		} `json:"head"`
	}
	base := "repos/" + repo
	if err := get(base+"/pulls/"+strconv.Itoa(n), &pull); err != nil {
		return PR{}, remaining, err
	}
	p := PR{Repo: repo, Number: pull.Number, Title: pull.Title, URL: pull.HTMLURL, State: pull.State, Head: pull.Head.SHA}
	if pull.Merged {
		p.State = "merged"
	}
	var runs struct {
		CheckRuns []struct {
			ID         int64  `json:"id"`
			Name       string `json:"name"`
			Status     string `json:"status"`
			Conclusion string `json:"conclusion"`
			HTMLURL    string `json:"html_url"`
			App        struct {
				Slug string `json:"slug"`
			} `json:"app"`
			Output struct {
				Title string `json:"title"`
			} `json:"output"`
		} `json:"check_runs"`
	}
	if err := get(base+"/commits/"+p.Head+"/check-runs?per_page=100", &runs); err != nil {
		return p, remaining, err
	}
	for _, r := range runs.CheckRuns {
		p.Checks = append(p.Checks, Check{
			ID: r.ID, Name: r.Name, State: runState(r.Status, r.Conclusion), URL: r.HTMLURL,
			Actions: r.App.Slug == "github-actions", Note: r.Output.Title,
		})
	}
	var status struct {
		Statuses []struct {
			Context     string `json:"context"`
			State       string `json:"state"`
			TargetURL   string `json:"target_url"`
			Description string `json:"description"`
		} `json:"statuses"`
	}
	if err := get(base+"/commits/"+p.Head+"/status", &status); err != nil {
		return p, remaining, err
	}
	for _, s := range status.Statuses {
		st := "pending"
		switch s.State {
		case "success":
			st = "passed"
		case "failure", "error":
			st = "failed"
		}
		p.Checks = append(p.Checks, Check{Name: s.Context, State: st, URL: s.TargetURL, Note: s.Description})
	}
	return p, remaining, nil
}

// runState is a check run's status and conclusion as a Check's State.
func runState(status, conclusion string) string {
	if status != "completed" {
		return "pending"
	}
	switch conclusion {
	case "success":
		return "passed"
	case "neutral", "skipped":
		return "skipped"
	}
	return "failed" // failure, cancelled, timed_out, action_required, stale
}

// Until is when a watch ends.
type Until string

const (
	UntilDone  Until = "done"  // every check has finished, or the pull request merged or closed
	UntilFail  Until = "fail"  // a check failed, else as done
	UntilFirst Until = "first" // the first check to finish, or anything else that changes
	UntilEach  Until = "each"  // as done, saying each check as it finishes
)

// Exit codes a watch ends with.
const (
	ExitPassed   = 0 // everything passed, or it merged
	ExitFailed   = 1 // a check failed
	ExitTimedOut = 2 // still waiting: run it again
	ExitError    = 3 // closed, or rush couldn't ask
)

// Options are what a watch waits for.
type Options struct {
	Repo    string
	Number  int
	Until   Until
	Timeout time.Duration
	// Say is told each line to print; Fetch, Sleep and Logs are GitHub
	// and the clock, which tests set.
	Say   func(string)
	Fetch func(ctx context.Context, repo string, n int, fresh time.Duration) (PR, int, error)
	Sleep func(ctx context.Context, d time.Duration) bool
	Logs  func(ctx context.Context, repo string, job int64) ([]byte, error)
	Now   func() time.Time
}

// noChecksAfter is how long a watch waits for a check to show up before
// deciding none will.
const noChecksAfter = 3 * time.Minute

// Interval is how long to wait before asking again, a watch since long:
// often at first, less as the wait grows, and much less when the user's
// rate limit runs low.
func Interval(since time.Duration, remaining int) time.Duration {
	d := 15 * time.Second
	switch {
	case since >= 15*time.Minute:
		d = time.Minute
	case since >= 5*time.Minute:
		d = 30 * time.Second
	}
	switch {
	case remaining >= 0 && remaining < 100:
		d = max(d, 5*time.Minute)
	case remaining >= 0 && remaining < 500:
		d = max(d, 2*time.Minute)
	}
	return d
}

// WatchPR waits on pull request o.Number as o.Until has it, and says once,
// briefly, what happened; it gives the exit code to end with.
func WatchPR(ctx context.Context, o Options) int {
	o = withDefaults(o)
	start := o.Now()
	var prev *PR
	fails := 0
	remaining := -1
	for {
		p, r, err := o.Fetch(ctx, o.Repo, o.Number, 10*time.Second)
		if r >= 0 {
			remaining = r
		}
		switch {
		case ctx.Err() != nil:
			return ExitError
		case errors.Is(err, errLimited):
			remaining = 0
		case err != nil:
			// A blip is asked again; three in a row end it.
			if fails++; fails >= 3 || prev == nil {
				o.Say("rush watch: " + err.Error())
				return ExitError
			}
		default:
			fails = 0
			if code, done := step(ctx, o, prev, p, o.Now().Sub(start)); done {
				return code
			}
			prev = &p
		}
		since := o.Now().Sub(start)
		wait := Interval(since, remaining)
		if o.Timeout > 0 && since+wait > o.Timeout {
			if left := o.Timeout - since; left > 0 && o.Sleep(ctx, left) {
				// one last look before giving up
				if p, _, err := o.Fetch(ctx, o.Repo, o.Number, 0); err == nil {
					if code, done := step(ctx, o, prev, p, o.Now().Sub(start)); done {
						return code
					}
					prev = &p
				}
			}
			o.Say(waiting(prev, o.Now().Sub(start)))
			return ExitTimedOut
		}
		if !o.Sleep(ctx, wait) {
			return ExitError
		}
	}
}

func withDefaults(o Options) Options {
	if o.Until == "" {
		o.Until = UntilDone
	}
	if o.Say == nil {
		o.Say = func(string) {}
	}
	if o.Fetch == nil {
		o.Fetch = Fetch
	}
	if o.Now == nil {
		o.Now = time.Now
	}
	if o.Sleep == nil {
		o.Sleep = func(ctx context.Context, d time.Duration) bool {
			t := time.NewTimer(d)
			defer t.Stop()
			select {
			case <-t.C:
				return true
			case <-ctx.Done():
				return false
			}
		}
	}
	if o.Logs == nil {
		o.Logs = func(ctx context.Context, repo string, job int64) ([]byte, error) {
			return Raw(ctx, "repos/"+repo+"/actions/jobs/"+strconv.FormatInt(job, 10)+"/logs")
		}
	}
	return o
}

// step looks at p, after prev (nil the first time), and says whether the
// watch is over, and with what code.
func step(ctx context.Context, o Options, prev *PR, p PR, since time.Duration) (int, bool) {
	if p.State == "merged" || p.State == "closed" {
		o.Say(fmt.Sprintf("PR #%d was %s · %s", p.Number, p.State, p.Title))
		if p.State == "merged" {
			return ExitPassed, true
		}
		return ExitError, true
	}
	newHead := prev != nil && prev.Head != p.Head
	if newHead && o.Until == UntilFirst {
		o.Say(fmt.Sprintf("PR #%d has a new commit, %s: its checks start again", p.Number, short(p.Head)))
		return ExitPassed, true
	}
	if newHead && o.Until == UntilEach {
		o.Say(fmt.Sprintf("· new commit %s: checks start again", short(p.Head)))
	}
	var finished []Check // since the last look
	if prev != nil {
		before := map[string]bool{}
		if !newHead {
			for _, c := range prev.Checks {
				if c.finished() {
					before[c.Name] = true
				}
			}
		}
		for _, c := range p.Checks {
			if c.finished() && !before[c.Name] {
				finished = append(finished, c)
			}
		}
	}
	if o.Until == UntilEach {
		for _, c := range finished {
			o.Say(checkLine(c))
		}
	}
	_, failed, pending, _ := p.Counts()
	all := len(p.Checks) > 0 && pending == 0
	switch {
	case o.Until == UntilFail && failed > 0, all:
		o.Say(Summary(ctx, o, p, since))
		if failed > 0 {
			return ExitFailed, true
		}
		return ExitPassed, true
	case len(p.Checks) == 0 && since >= noChecksAfter:
		o.Say(fmt.Sprintf("PR #%d: no checks on %s after %s · %s", p.Number, short(p.Head), round(since), p.Title))
		return ExitPassed, true
	case o.Until == UntilFirst && len(finished) > 0:
		lines := []string{fmt.Sprintf("PR #%d: %s", p.Number, progress(p))}
		code := ExitPassed
		for _, c := range finished {
			lines = append(lines, checkLine(c))
			if c.State == "failed" {
				code = ExitFailed
				lines = append(lines, errorLines(ctx, o, c)...)
			}
		}
		o.Say(strings.Join(lines, "\n"))
		return code, true
	}
	return 0, false
}

// Summary is what a finished watch says: the counts, and each failed
// check with the lines of its log that say why. Passes aren't named.
func Summary(ctx context.Context, o Options, p PR, since time.Duration) string {
	o = withDefaults(o)
	passed, failed, pending, skipped := p.Counts()
	head := fmt.Sprintf("PR #%d checks", p.Number)
	if pending == 0 {
		head += " finished"
	}
	parts := []string{fmt.Sprintf("%d passed", passed)}
	if failed > 0 {
		parts = append(parts, fmt.Sprintf("%d failed", failed))
	}
	if skipped > 0 {
		parts = append(parts, fmt.Sprintf("%d skipped", skipped))
	}
	if pending > 0 {
		parts = append(parts, fmt.Sprintf("%d still running", pending))
	}
	lines := []string{fmt.Sprintf("%s after %s: %s · %s", head, round(since), strings.Join(parts, ", "), p.Title)}
	shown := 0
	for _, c := range p.Checks {
		if c.State != "failed" {
			continue
		}
		if shown++; shown > 5 {
			lines = append(lines, fmt.Sprintf("… and %d more failed", failed-5))
			break
		}
		lines = append(lines, checkLine(c))
		lines = append(lines, errorLines(ctx, o, c)...)
	}
	return strings.Join(lines, "\n")
}

func checkLine(c Check) string {
	mark := map[string]string{"passed": "✓", "failed": "✗", "skipped": "-"}[c.State]
	line := mark + " " + c.Name + " " + c.State
	if c.State == "failed" && c.URL != "" {
		line += " · " + c.URL
	}
	return line
}

// errorLines are the lines that say why c failed, indented: its job's log's
// errors when GitHub Actions ran it, else what it says of itself.
func errorLines(ctx context.Context, o Options, c Check) []string {
	var lines []string
	if c.Actions && c.ID != 0 {
		if b, err := o.Logs(ctx, o.Repo, c.ID); err == nil {
			lines = LogErrors(string(b), 12)
		}
	}
	if len(lines) == 0 && c.Note != "" {
		lines = []string{c.Note}
	}
	for i, l := range lines {
		lines[i] = "    " + l
	}
	return lines
}

var (
	stamp   = regexp.MustCompile(`^\d{4}-\d\d-\d\dT[\d:.]+Z ?`)
	ansi    = regexp.MustCompile(`\x1b\[[0-9;?]*[A-Za-z]`)
	spaces  = regexp.MustCompile(` {3,}`)
	errLike = regexp.MustCompile(`(?i)(^|[^a-z])(error|fail(ed|ure)?|panic|fatal|exception|traceback)([^a-z]|$)|✗|✘|×`)
)

// noise are lines a failed step ends with that say only that it failed.
var noise = regexp.MustCompile(`^\[ELIFECYCLE\]|^npm (ERR!|error) (code|errno|command|path|A complete log)|^error Command failed with exit code|^Error: Process completed`)

// LogErrors are at most n lines of an Actions job's log that say what went
// wrong, without timestamps or colour. For each step that failed, its name
// and then its errors and the last lines it wrote; the step's command and
// the steps that passed are left out.
func LogErrors(log string, n int) []string {
	type step struct {
		name string
		out  []string
	}
	var cur step
	var failed []step
	var all []string // a log with no steps: what it says before it gives up
	inGroup := false
	for raw := range strings.SplitSeq(log, "\n") {
		l := strings.TrimSpace(ansi.ReplaceAllString(stamp.ReplaceAllString(strings.TrimRight(raw, "\r"), ""), ""))
		switch {
		case strings.HasPrefix(l, "##[group]Run "):
			cur, inGroup = step{name: strings.TrimPrefix(l, "##[group]Run ")}, true
			continue
		case strings.HasPrefix(l, "##[group]"):
			inGroup = true // the command, its shell and env: not what it said
			continue
		case strings.HasPrefix(l, "##[endgroup]"):
			inGroup = false
			continue
		case strings.HasPrefix(l, "##[error]Process completed with exit code"):
			if len(failed) == 0 && cur.name == "" {
				cur.out = all
			}
			failed = append(failed, cur)
			cur = step{}
			continue
		case inGroup || l == "" || strings.HasPrefix(l, "[command]") || noise.MatchString(l):
			continue
		}
		l = clip(spaces.ReplaceAllString(strings.TrimPrefix(l, "##[error]"), "  "), 200)
		cur.out = append(cur.out, l)
		all = append(all, l)
	}
	if len(failed) == 0 {
		failed = []step{{out: all}}
	}
	var out []string
	per := max(4, n/len(failed))
	for i, s := range failed {
		if i == 3 {
			out = append(out, fmt.Sprintf("… and %d more failed steps", len(failed)-3))
			break
		}
		if s.name != "" {
			out = append(out, "step: "+clip(s.name, 80))
		}
		out = append(out, pick(s.out, per)...)
	}
	return out
}

// pick is at most n of a failed step's lines, in order: the last that
// look like errors, up to half, and the last it wrote.
func pick(lines []string, n int) []string {
	keep := map[int]bool{}
	for i := len(lines) - 1; i >= 0 && len(keep) < n/2; i-- {
		if errLike.MatchString(lines[i]) {
			keep[i] = true
		}
	}
	for i := len(lines) - 1; i >= 0 && len(keep) < n; i-- {
		keep[i] = true
	}
	var out []string
	for i, l := range lines {
		if keep[i] {
			out = append(out, l)
		}
	}
	return out
}

// waiting is what a watch that timed out says: how far the checks got.
func waiting(p *PR, since time.Duration) string {
	if p == nil {
		return fmt.Sprintf("still waiting after %s: GitHub hasn't answered yet; run it again", round(since))
	}
	var running []string
	for _, c := range p.Checks {
		if !c.finished() {
			running = append(running, c.Name)
		}
	}
	if len(running) > 8 {
		running = append(running[:8], "…")
	}
	line := fmt.Sprintf("PR #%d still waiting after %s: %s; run it again", p.Number, round(since), progress(*p))
	if len(running) > 0 {
		line += "\n    running: " + strings.Join(running, ", ")
	}
	return line
}

// progress is how far its checks have got: "9/12 finished, 1 failed".
func progress(p PR) string {
	_, failed, pending, _ := p.Counts()
	if len(p.Checks) == 0 {
		return "no checks yet"
	}
	s := fmt.Sprintf("%d/%d finished", len(p.Checks)-pending, len(p.Checks))
	if failed > 0 {
		s += fmt.Sprintf(", %d failed", failed)
	}
	return s
}

// Resolve is the repository and pull request number what names: a number
// (in dir's repository), owner/name#number, a pull request's URL, or ""
// for the pull request of dir's branch. Tests set it.
var Resolve = func(ctx context.Context, dir, what string) (string, int, error) {
	what = strings.TrimSpace(what)
	if m := prURL.FindStringSubmatch(what); m != nil {
		n, _ := strconv.Atoi(m[2])
		return m[1], n, nil
	}
	if repo, num, ok := strings.Cut(what, "#"); ok && strings.Count(repo, "/") == 1 {
		n, err := strconv.Atoi(num)
		if err != nil || n <= 0 {
			return "", 0, fmt.Errorf("%q isn't a pull request", what)
		}
		return repo, n, nil
	}
	gh := func(args ...string) (string, error) {
		cmd := exec.CommandContext(ctx, "gh", args...)
		cmd.Dir = dir
		out, err := cmd.Output()
		if ee, ok := errors.AsType[*exec.ExitError](err); ok {
			return "", ghError(0, nil, ee.Stderr, err)
		}
		return strings.TrimSpace(string(out)), err
	}
	n := 0
	if what != "" {
		var err error
		if n, err = strconv.Atoi(strings.TrimPrefix(what, "#")); err != nil || n <= 0 {
			return "", 0, fmt.Errorf("%q isn't a pull request: give its number, owner/name#number or its URL", what)
		}
	}
	repo, err := gh("repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner")
	if err != nil {
		return "", 0, err
	}
	if n == 0 {
		s, err := gh("pr", "view", "--json", "number", "--jq", ".number")
		if err != nil {
			return "", 0, fmt.Errorf("no pull request for this branch: %w", err)
		}
		if n, err = strconv.Atoi(s); err != nil {
			return "", 0, fmt.Errorf("no pull request for this branch")
		}
	}
	return repo, n, nil
}

var prURL = regexp.MustCompile(`github\.com/([^/\s]+/[^/\s]+)/pull/(\d+)`)

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

// round is d for people: 40s, 14m, 1h5m.
func round(d time.Duration) string {
	switch {
	case d < time.Minute:
		return strconv.Itoa(int(d.Seconds())) + "s"
	case d < time.Hour:
		return strconv.Itoa(int(d.Minutes())) + "m"
	}
	return strings.TrimSuffix(d.Truncate(time.Minute).String(), "0s")
}
