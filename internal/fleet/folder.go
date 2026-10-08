package fleet

import (
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/uithread"
)

// Folder is a repository agents work in, as git sees it: the main
// checkout's state and that of each linked worktree an agent is in.
type Folder struct {
	Root string
	Git  GitState
	// Worktrees counts every linked worktree the repository has, whether
	// an agent works in it or not.
	Worktrees int
	// Trees are the linked worktrees agents work in, by path; checked
	// whole, every linked worktree.
	Trees map[string]GitState

	// Checked whole, for the Overview's Projects page: where it's pushed,
	// its last commits, and every linked worktree in the order git lists
	// them.
	Whole  bool
	Remote string // origin, as host/owner/repo where it looks like one
	Recent []Commit
	Linked []string
	// Checked is when git was asked.
	Checked time.Time
}

// Commit is one commit on a checkout's branch.
type Commit struct {
	Subject string
	At      time.Time
}

// GitState is one checkout's branch and how far it is from committed and
// pushed.
type GitState struct {
	Branch   string
	Upstream bool   // the branch tracks a remote one
	Ahead    int    // commits not on the upstream yet
	Behind   int    // commits on the upstream not here yet
	Changed  int    // files with uncommitted changes, untracked ones included
	Commit   string // HEAD's short hash
	// Target is the upstream the branch tracks, as git names it
	// ("origin/main").
	Target string
	// Base is the branch a linked worktree's branch was made from, and
	// how far it's gone from it: commits of its own, and the base's since.
	Base                  string
	BaseAhead, BaseBehind int
	Err                   string
}

// CheckFolder asks git about root and the worktrees under it that agents
// work in; whole, about every worktree it has, and its remote and last
// commits too. It runs git several times and can take seconds on a big
// checkout: never on the UI's goroutine.
func CheckFolder(root string, trees []string, whole bool) Folder {
	uithread.Forbid("fleet.CheckFolder")
	f := Folder{Root: root, Whole: whole, Checked: time.Now()}
	var rootDone sync.WaitGroup
	rootDone.Go(func() { f.Git = gitState(root) })
	if list, err := git(root, "worktree", "list", "--porcelain"); err == nil {
		for l := range strings.SplitSeq(list, "\n") {
			if p, ok := strings.CutPrefix(l, "worktree "); ok && p != root {
				f.Worktrees++
				if whole {
					f.Linked = append(f.Linked, p)
				}
			}
		}
	}
	if whole {
		trees = f.Linked
		if u, err := git(root, "remote", "get-url", "origin"); err == nil {
			f.Remote = shortRemote(u)
		}
		if log, err := git(root, "log", "-3", "--format=%ct%x09%s"); err == nil {
			for l := range strings.SplitSeq(log, "\n") {
				at, subject, ok := strings.Cut(l, "\t")
				if !ok {
					continue
				}
				n, _ := strconv.ParseInt(at, 10, 64)
				f.Recent = append(f.Recent, Commit{Subject: subject, At: time.Unix(n, 0)})
			}
		}
	}
	// Each checkout's status takes a second or more on a big repository:
	// they're asked at once rather than one after another.
	states := make([]GitState, len(trees))
	var wg sync.WaitGroup
	gate := make(chan struct{}, 6) // the Projects page asks of every worktree
	for i, t := range trees {
		wg.Go(func() {
			gate <- struct{}{}
			states[i] = gitState(t)
			rootDone.Wait() // the main checkout's branch stands in for an unknown base
			states[i].base(t, f.Git.Branch)
			<-gate
		})
	}
	wg.Wait()
	for i, t := range trees {
		if f.Trees == nil {
			f.Trees = map[string]GitState{}
		}
		f.Trees[t] = states[i]
	}
	rootDone.Wait()
	return f
}

// shortRemote names a remote URL the way you'd say it: host/owner/repo,
// from either an https or an ssh URL.
func shortRemote(u string) string {
	u = strings.TrimSuffix(strings.TrimSpace(u), ".git")
	scheme := strings.Contains(u, "://")
	if _, rest, ok := strings.Cut(u, "://"); ok {
		u = rest
	}
	if i := strings.Index(u, "@"); i >= 0 {
		u = u[i+1:]
	}
	host, path, ok := strings.Cut(u, ":")
	switch {
	case !ok:
		return u
	case scheme: // host:port/path
		if _, p, ok := strings.Cut(path, "/"); ok {
			return host + "/" + p
		}
		return host
	}
	return host + "/" + path // scp-like host:path
}

// gitState reads a checkout's branch, upstream distance and changes from
// one git status.
func gitState(dir string) GitState {
	out, err := git(dir, "status", "--porcelain=v2", "--branch", "--untracked-files=normal")
	if err != nil {
		return GitState{Err: "git status failed"}
	}
	var s GitState
	for l := range strings.SplitSeq(out, "\n") {
		switch {
		case l == "":
		case strings.HasPrefix(l, "# branch.head "):
			s.Branch = strings.TrimPrefix(l, "# branch.head ")
			if s.Branch == "(detached)" {
				s.Branch = "detached"
			}
		case strings.HasPrefix(l, "# branch.oid "):
			if oid := strings.TrimPrefix(l, "# branch.oid "); len(oid) >= 7 && oid != "(initial)" {
				s.Commit = oid[:7]
			}
		case strings.HasPrefix(l, "# branch.upstream "):
			s.Upstream, s.Target = true, strings.TrimPrefix(l, "# branch.upstream ")
		case strings.HasPrefix(l, "# branch.ab "):
			for f := range strings.FieldsSeq(strings.TrimPrefix(l, "# branch.ab ")) {
				n, _ := strconv.Atoi(f[1:])
				if f[0] == '+' {
					s.Ahead = n
				} else {
					s.Behind = n
				}
			}
		case strings.HasPrefix(l, "#"):
		default:
			s.Changed++
		}
	}
	return s
}

// base finds the branch tree's was made from, as its reflog says
// ("Created from main"), else the main checkout's branch, and counts
// the commits between them.
func (s *GitState) base(tree, fallback string) {
	if s.Branch == "" || s.Branch == "detached" {
		return
	}
	b := fallback
	if log, err := git(tree, "reflog", "show", "--format=%gs", "refs/heads/"+s.Branch); err == nil {
		lines := strings.Split(log, "\n")
		if from, ok := strings.CutPrefix(lines[len(lines)-1], "branch: Created from "); ok && from != "HEAD" && !isHash(from) {
			b = from
		}
	}
	if b == "" || b == s.Branch {
		return
	}
	s.Base = b
	if n, err := git(tree, "rev-list", "--left-right", "--count", b+"...HEAD"); err == nil {
		if behind, ahead, ok := strings.Cut(n, "\t"); ok {
			s.BaseBehind, _ = strconv.Atoi(behind)
			s.BaseAhead, _ = strconv.Atoi(ahead)
		}
	}
}

// isHash is whether s looks like a commit's hash rather than a branch.
func isHash(s string) bool {
	if len(s) < 7 {
		return false
	}
	for _, r := range s {
		if !strings.ContainsRune("0123456789abcdef", r) {
			return false
		}
	}
	return true
}

// FolderWants is what CheckFolder should be asked for the agents given:
// each repository they work in, with the linked worktrees they're in.
func FolderWants(agents []*Agent) map[string][]string {
	out := map[string][]string{}
	for _, a := range agents {
		if a.Root == "" {
			continue
		}
		trees := out[a.Root]
		if a.Repo != a.Root && a.Repo != "" && !slices.Contains(trees, a.Repo) {
			trees = append(trees, a.Repo)
		}
		out[a.Root] = trees
	}
	for _, t := range out {
		sort.Strings(t)
	}
	return out
}
