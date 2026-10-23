package ghpr

import (
	"context"
	"os/exec"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Pull requests by branch: a session works on one branch, and its pull
// request is the one GitHub has for it, whichever agent made it. One gh
// pr list per repository covers every session in it: asked in the
// background, kept here for the process, and asked again at most every
// BranchEvery while something reads it, or at once on Refresh.

// Status is a pull request as a session needs it, found by its branch.
type Status struct {
	Number int
	Title  string
	URL    string
	Branch string // its head branch
	State  string // open, draft, merged or closed
	// Checks is how its checks went, all told: pass, fail or pending; ""
	// when it has none. Failing names the first that failed.
	Checks  string
	Failing string
	// Review is approved or changes (requested); "" while neither.
	Review   string
	HeadOid  string // the commit GitHub has at its head
	MergedAt time.Time
}

// Merged is a pull request merged, squashed or not.
func (s Status) Merged() bool { return s.State == "merged" }

// Open is a pull request still open, draft or not.
func (s Status) Open() bool { return s.State == "open" || s.State == "draft" }

// BranchEvery is how often, at most, a repository's pull requests are
// asked again while they're read.
const BranchEvery = time.Minute

// branchFields are what gh is asked for of each pull request by branch.
const branchFields = "number,title,state,headRefName,headRefOid,isDraft,statusCheckRollup,reviewDecision,mergedAt,url"

// RunGH runs gh in dir and gives its output. Tests set it.
var RunGH = func(ctx context.Context, dir string, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, "gh", args...)
	cmd.Dir = dir
	return cmd.Output()
}

// branchRepo is what GitHub last said of one repository's pull requests.
type branchRepo struct {
	at     time.Time // when it was last asked; zero before
	by     map[string]Status
	err    string // why it couldn't say, last time: no gh, not GitHub, signed out
	asking bool
}

var branches = struct {
	sync.Mutex
	repos map[string]*branchRepo
	gate  chan struct{}
}{repos: map[string]*branchRepo{}, gate: make(chan struct{}, 3)}

// ByBranch is root's pull requests by head branch, as GitHub last said
// them; it never waits on GitHub. ok is false until GitHub has answered
// once, and while gh can't (not installed, not on GitHub, signed out).
// An answer older than BranchEvery is asked for again in the background.
func ByBranch(root string) (by map[string]Status, ok bool) {
	if root == "" {
		return nil, false
	}
	branches.Lock()
	defer branches.Unlock()
	r := branches.repos[root]
	if r == nil {
		r = &branchRepo{}
		branches.repos[root] = r
	}
	every := BranchEvery
	if r.err != "" {
		every *= 10 // no gh, or not on GitHub: that changes seldom
	}
	if !r.asking && time.Since(r.at) >= every {
		ask(root, r)
	}
	return r.by, r.by != nil && r.err == ""
}

// For is the pull request GitHub has for branch in root, as last said.
func For(root, branch string) (Status, bool) {
	if branch == "" {
		return Status{}, false
	}
	by, _ := ByBranch(root)
	s, ok := by[branch]
	return s, ok
}

// Refresh asks GitHub again about root's pull requests now, in the
// background, however recently it was: on demand, as when a session's
// pull request is opened or merged from rush.
func Refresh(root string) {
	if root == "" {
		return
	}
	branches.Lock()
	defer branches.Unlock()
	r := branches.repos[root]
	if r == nil {
		r = &branchRepo{}
		branches.repos[root] = r
	}
	if !r.asking {
		ask(root, r)
	}
}

// LookBranches asks GitHub about root's pull requests and waits for the
// answer, keeping it for ByBranch too; one fresher than BranchEvery is
// used as it is. For what runs off the UI and must know now: git-clean.
func LookBranches(ctx context.Context, root string) (map[string]Status, error) {
	branches.Lock()
	r := branches.repos[root]
	if r != nil && r.by != nil && r.err == "" && time.Since(r.at) < BranchEvery {
		by := r.by
		branches.Unlock()
		return by, nil
	}
	branches.Unlock()
	by, err := listBranches(ctx, root)
	keep(root, by, err)
	return by, err
}

// ask asks GitHub in the background. Called with branches held.
func ask(root string, r *branchRepo) {
	r.asking = true
	go func() {
		branches.gate <- struct{}{}
		defer func() { <-branches.gate }()
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		by, err := listBranches(ctx, root)
		keep(root, by, err)
	}()
}

// keep takes in GitHub's answer for root. One that failed keeps what it
// said before, and is asked again after BranchEvery like any other.
func keep(root string, by map[string]Status, err error) {
	branches.Lock()
	defer branches.Unlock()
	r := branches.repos[root]
	if r == nil {
		r = &branchRepo{}
		branches.repos[root] = r
	}
	r.asking, r.at, r.err = false, time.Now(), ""
	if err != nil {
		r.err = err.Error()
		return
	}
	r.by = by
}

// Forget drops every repository's answer, so the next read asks GitHub:
// for tests.
func Forget() {
	branches.Lock()
	defer branches.Unlock()
	branches.repos = map[string]*branchRepo{}
}

// listBranches asks gh for root's last hundred pull requests, of any
// state, by head branch.
func listBranches(ctx context.Context, root string) (map[string]Status, error) {
	out, err := RunGH(ctx, root, "pr", "list", "--state", "all", "--limit", "100", "--json", branchFields)
	if err != nil {
		return nil, ghErr(err)
	}
	return ParseBranches(out)
}

type rawBranchPR struct {
	Number      int       `json:"number"`
	Title       string    `json:"title"`
	URL         string    `json:"url"`
	State       string    `json:"state"`
	IsDraft     bool      `json:"isDraft"`
	HeadRefName string    `json:"headRefName"`
	HeadRefOid  string    `json:"headRefOid"`
	Decision    string    `json:"reviewDecision"`
	Rollup      []rawChk  `json:"statusCheckRollup"`
	MergedAt    time.Time `json:"mergedAt"`
}

// ParseBranches reads gh pr list's JSON as pull requests by head branch.
// A branch with several keeps the one that matters most: open, then
// merged, then closed; the newest of those.
func ParseBranches(b []byte) (map[string]Status, error) {
	var raw []rawBranchPR
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		return nil, err
	}
	by := make(map[string]Status, len(raw))
	for i := range raw {
		r := &raw[i]
		s := Status{Number: r.Number, Title: r.Title, URL: r.URL, Branch: r.HeadRefName, HeadOid: r.HeadRefOid, MergedAt: r.MergedAt}
		switch r.State {
		case "MERGED":
			s.State = "merged"
		case "CLOSED":
			s.State = "closed"
		default:
			s.State = "open"
			if r.IsDraft {
				s.State = "draft"
			}
		}
		switch r.Decision {
		case "APPROVED":
			s.Review = "approved"
		case "CHANGES_REQUESTED":
			s.Review = "changes"
		}
		for j := range r.Rollup {
			c := check(&r.Rollup[j])
			switch {
			case c.State == "failed":
				if s.Checks != "fail" {
					s.Checks, s.Failing = "fail", c.Name
				}
			case c.State == "pending" && s.Checks != "fail":
				s.Checks = "pending"
			case s.Checks == "":
				s.Checks = "pass"
			}
		}
		if was, ok := by[s.Branch]; ok && !matters(s, was) {
			continue
		}
		by[s.Branch] = s
	}
	return by, nil
}

// matters is whether a matters more than b for their branch: open beats
// merged beats closed, then the newer.
func matters(a, b Status) bool {
	rank := func(s Status) int {
		switch {
		case s.Open():
			return 2
		case s.Merged():
			return 1
		}
		return 0
	}
	if rank(a) != rank(b) {
		return rank(a) > rank(b)
	}
	return a.Number > b.Number
}
