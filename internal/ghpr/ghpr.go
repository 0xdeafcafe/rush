// Package ghpr asks GitHub, through the gh command, about a repository's
// pull requests that concern you: those you opened, and those waiting on
// your review. It runs gh, so never on the UI.
package ghpr

import (
	"context"
	"errors"
	"os/exec"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// fields are what gh is asked for of each pull request.
const fields = "number,title,url,state,isDraft,author,headRefName,baseRefName,reviewDecision,reviewRequests,latestReviews,statusCheckRollup,updatedAt,body,additions,deletions,changedFiles,comments"

// Check is one check on a pull request's last commit.
type Check struct {
	Name, Workflow string
	State          string // passed, failed, pending or skipped
	URL            string
}

// Review is someone's last word on a pull request: APPROVED,
// CHANGES_REQUESTED, COMMENTED or DISMISSED.
type Review struct {
	Who, State, Body string
}

// PR is a pull request as GitHub says it.
type PR struct {
	Number         int
	Title, URL     string
	State          string // OPEN, MERGED or CLOSED
	Draft          bool
	Author         string
	Head, Base     string
	Decision       string   // APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED or ""
	Waiting        []string // who is asked to review and hasn't: logins, or teams' names
	Reviews        []Review
	Checks         []Check
	Updated        time.Time
	Body           string
	Mine, AskedYou bool
	// Added and Removed are its lines, Files the files it changes, and
	// Comments the comments on its conversation.
	Added, Removed, Files, Comments int
}

// Counts are a pull request's checks by how they went.
func (p PR) Counts() (passed, failed, pending int) {
	for _, c := range p.Checks {
		switch c.State {
		case "passed", "skipped":
			passed++
		case "failed":
			failed++
		default:
			pending++
		}
	}
	return passed, failed, pending
}

// Repo is what GitHub said of one repository's pull requests.
type Repo struct {
	Root string
	PRs  []PR
	Err  string // why it couldn't say: no gh, not on GitHub, signed out
	At   time.Time
	// Next is when Look asks GitHub again: BranchEvery after an answer,
	// longer after each failure in a row (see retryIn).
	Next  time.Time
	fails int
}

// List asks gh, in the repository at root, for the open pull requests you
// opened and those asking for your review, yours first, then the most
// lately touched.
func List(ctx context.Context, root string) Repo {
	r := Repo{Root: root, At: time.Now()}
	mine, err := list(ctx, root, "author:@me")
	if err != nil {
		r.Err = err.Error()
		return r
	}
	asked, err := list(ctx, root, "review-requested:@me")
	if err != nil {
		r.Err = err.Error()
		return r
	}
	// Asked of you by name, it's waiting on you.
	if me := login(ctx); me != "" {
		for _, ps := range [][]PR{mine, asked} {
			for i := range ps {
				for j, q := range ps[i].Waiting {
					if q == me {
						ps[i].Waiting[j] = "you"
					}
				}
			}
		}
	}
	byNum := map[int]int{}
	for i := range mine {
		mine[i].Mine = true
		byNum[mine[i].Number] = len(r.PRs)
		r.PRs = append(r.PRs, mine[i])
	}
	for i := range asked {
		if j, ok := byNum[asked[i].Number]; ok {
			r.PRs[j].AskedYou = true
			continue
		}
		asked[i].AskedYou = true
		r.PRs = append(r.PRs, asked[i])
	}
	sort.SliceStable(r.PRs, func(i, j int) bool {
		a, b := r.PRs[i], r.PRs[j]
		if a.Mine != b.Mine {
			return a.Mine
		}
		return a.Updated.After(b.Updated)
	})
	return r
}

// lists is what List last said of each repository, by its name on
// GitHub (its folder when GitHub can't say), shared by everything in this
// process that shows it: a repository's every worktree asks once.
var lists = struct {
	sync.Mutex
	by map[string]Repo
}{by: map[string]Repo{}}

// Look is List through a shared answer: one not due again (Next) is used
// as it is, and GitHub is asked through the same gate as ByBranch, so the
// Projects page and the branches never ask more than a few at once. One
// that fails keeps the pull requests the last answer had, and is asked
// again later each time it fails in a row.
func Look(ctx context.Context, root string) Repo {
	key := repoName(ctx, root)
	if key == "" {
		key = root
	}
	lists.Lock()
	old, ok := lists.by[key]
	lists.Unlock()
	if ok && time.Now().Before(old.Next) {
		old.Root = root
		return old
	}
	select {
	case branches.gate <- struct{}{}:
	case <-ctx.Done():
		return Repo{Root: root, PRs: old.PRs, Err: ctx.Err().Error(), At: time.Now(), Next: old.Next, fails: old.fails}
	}
	r := List(ctx, root)
	<-branches.gate
	if r.Err != "" {
		r.fails = old.fails + 1
		if len(old.PRs) > 0 {
			r.PRs = old.PRs // GitHub failing once doesn't lose what it said before
		}
	}
	r.Next = r.At.Add(retryIn(r.fails, r.Err))
	lists.Lock()
	lists.by[key] = r
	lists.Unlock()
	return r
}

// retryIn is how long after an answer GitHub is asked again: BranchEvery,
// doubled for each failure in a row up to a quarter of an hour, and never
// sooner than ten minutes once GitHub has said it's being asked too much.
func retryIn(fails int, err string) time.Duration {
	d := BranchEvery << min(fails, 4)
	d = min(d, 15*time.Minute)
	if low := strings.ToLower(err); strings.Contains(low, "rate limit") {
		d = max(d, 10*time.Minute)
	}
	return d
}

var me struct {
	sync.Mutex
	login string
}

// login is who gh is signed in as, asked once.
func login(ctx context.Context) string {
	me.Lock()
	defer me.Unlock()
	if me.login == "" {
		if out, err := exec.CommandContext(ctx, "gh", "api", "user", "--jq", ".login").Output(); err == nil {
			me.login = strings.TrimSpace(string(out))
		}
	}
	return me.login
}

func list(ctx context.Context, root, search string) ([]PR, error) {
	args := []string{"pr", "list", "--state", "open", "--limit", "30", "--search", search, "--json", fields}
	if name := repoName(ctx, root); name != "" {
		args = append(args, "--repo", name)
	}
	out, err := RunGH(ctx, root, args...)
	if err != nil {
		return nil, ghErr(err)
	}
	return Parse(out)
}

var names struct {
	sync.Mutex
	by map[string]string
}

// repoName is the name GitHub has for root's repository now, asked once:
// a remote still on a renamed repository's old name reaches it, but a
// search by that name finds nothing. "" when GitHub can't say.
func repoName(ctx context.Context, root string) string {
	names.Lock()
	name, ok := names.by[root]
	names.Unlock()
	if ok {
		return name
	}
	out, err := RunGH(ctx, root, "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner")
	if err != nil {
		return "" // asked again next time: GitHub may answer then
	}
	name = strings.TrimSpace(string(out))
	names.Lock()
	if names.by == nil {
		names.by = map[string]string{}
	}
	names.by[root] = name
	names.Unlock()
	return name
}

// ghErr is gh's failure in a few words.
func ghErr(err error) error {
	if errors.Is(err, exec.ErrNotFound) {
		return errors.New("gh isn't installed")
	}
	if ee, ok := errors.AsType[*exec.ExitError](err); ok {
		msg := strings.TrimSpace(string(ee.Stderr))
		switch {
		case strings.Contains(msg, "no git remotes") || strings.Contains(msg, "none of the git remotes"):
			return errors.New("not on GitHub")
		case strings.Contains(msg, "gh auth login") || strings.Contains(msg, "authentication"):
			return errors.New("gh isn't signed in · gh auth login")
		case msg != "":
			if i := strings.IndexByte(msg, '\n'); i > 0 {
				msg = msg[:i]
			}
			return errors.New(msg)
		}
	}
	return err
}

type rawPR struct {
	Number      int        `json:"number"`
	Title       string     `json:"title"`
	URL         string     `json:"url"`
	State       string     `json:"state"`
	IsDraft     bool       `json:"isDraft"`
	Author      who        `json:"author"`
	HeadRefName string     `json:"headRefName"`
	BaseRefName string     `json:"baseRefName"`
	Decision    string     `json:"reviewDecision"`
	Requests    []who      `json:"reviewRequests"`
	Reviews     []rawRev   `json:"latestReviews"`
	Rollup      []rawChk   `json:"statusCheckRollup"`
	UpdatedAt   time.Time  `json:"updatedAt"`
	Body        string     `json:"body"`
	Additions   int        `json:"additions"`
	Deletions   int        `json:"deletions"`
	Files       int        `json:"changedFiles"`
	Comments    []struct{} `json:"comments"`
}

type who struct {
	Login string `json:"login"`
	Name  string `json:"name"`
	Slug  string `json:"slug"`
}

func (w who) String() string {
	switch {
	case w.Login != "":
		return w.Login
	case w.Slug != "":
		return w.Slug
	}
	return w.Name
}

type rawRev struct {
	Author who    `json:"author"`
	State  string `json:"state"`
	Body   string `json:"body"`
}

type rawChk struct {
	Type       string `json:"__typename"`
	Name       string `json:"name"`
	Context    string `json:"context"`
	Workflow   string `json:"workflowName"`
	Status     string `json:"status"`
	Conclusion string `json:"conclusion"`
	State      string `json:"state"`
	DetailsURL string `json:"detailsUrl"`
	TargetURL  string `json:"targetUrl"`
}

// Parse reads gh pr list's JSON.
func Parse(b []byte) ([]PR, error) {
	var raw []rawPR
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		return nil, err
	}
	out := make([]PR, 0, len(raw))
	for i := range raw {
		r := &raw[i]
		p := PR{
			Number: r.Number, Title: r.Title, URL: r.URL, State: r.State, Draft: r.IsDraft,
			Author: r.Author.String(), Head: r.HeadRefName, Base: r.BaseRefName,
			Decision: r.Decision, Updated: r.UpdatedAt, Body: r.Body,
			Added: r.Additions, Removed: r.Deletions, Files: r.Files, Comments: len(r.Comments),
		}
		for _, q := range r.Requests {
			p.Waiting = append(p.Waiting, q.String())
		}
		for _, v := range r.Reviews {
			p.Reviews = append(p.Reviews, Review{Who: v.Author.String(), State: v.State, Body: v.Body})
		}
		for j := range r.Rollup {
			p.Checks = append(p.Checks, check(&r.Rollup[j]))
		}
		out = append(out, p)
	}
	return out, nil
}

// check is a CheckRun or a commit status as one Check.
func check(c *rawChk) Check {
	if c.Type == "StatusContext" {
		st := "pending"
		switch c.State {
		case "SUCCESS":
			st = "passed"
		case "FAILURE", "ERROR":
			st = "failed"
		}
		return Check{Name: c.Context, State: st, URL: c.TargetURL}
	}
	st := "pending"
	if c.Status == "COMPLETED" {
		switch c.Conclusion {
		case "SUCCESS":
			st = "passed"
		case "NEUTRAL", "SKIPPED":
			st = "skipped"
		default:
			st = "failed"
		}
	}
	return Check{Name: c.Name, Workflow: c.Workflow, State: st, URL: c.DetailsURL}
}
