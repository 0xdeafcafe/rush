package fleet

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/0xdeafcafe/rush/internal/host"
)

func out(t *testing.T, dir string, args ...string) string {
	t.Helper()
	b, err := exec.Command("git", append([]string{"-C", dir}, args...)...).Output()
	if err != nil {
		t.Fatalf("git %v: %v", args, err)
	}
	return strings.TrimSpace(string(b))
}

// landedRepo is a main checkout pushed to a bare origin, with a worktree
// rush made for session abc (rush/abc, from main) holding one commit of its
// own, pushed to no remote.
func landedRepo(t *testing.T) (repo, wt, base string) {
	t.Helper()
	root, _ := filepath.EvalSymlinks(t.TempDir())
	remote := filepath.Join(root, "remote.git")
	repo = filepath.Join(root, "repo")
	_ = os.MkdirAll(repo, 0o755)
	run(t, root, "init", "-q", "--bare", "-b", "main", remote)
	run(t, repo, "init", "-q", "-b", "main")
	_ = os.WriteFile(filepath.Join(repo, "a.txt"), []byte("a\n"), 0o644)
	run(t, repo, "add", ".")
	run(t, repo, "commit", "-qm", "first")
	run(t, repo, "remote", "add", "origin", remote)
	run(t, repo, "push", "-q", "-u", "origin", "main")
	base = out(t, repo, "rev-parse", "HEAD")
	wt = filepath.Join(repo, ".claude", "worktrees", "abc")
	run(t, repo, "worktree", "add", "-q", "-b", "rush/abc", wt)
	_ = os.WriteFile(filepath.Join(wt, "b.txt"), []byte("b\n"), 0o644)
	run(t, wt, "add", ".")
	run(t, wt, "commit", "-qm", "the work")
	return repo, wt, base
}

// squash lands the worktree's work on origin's main as GitHub's squash
// merge would: one new commit with the same change, from another clone.
func squash(t *testing.T, repo string) {
	t.Helper()
	other := filepath.Join(filepath.Dir(repo), "other")
	run(t, filepath.Dir(repo), "clone", "-q", filepath.Join(filepath.Dir(repo), "remote.git"), other)
	_ = os.WriteFile(filepath.Join(other, "b.txt"), []byte("b\n"), 0o644)
	run(t, other, "add", ".")
	run(t, other, "commit", "-qm", "the work (#9)")
	run(t, other, "push", "-q", "origin", "main")
}

// mergedPR has gh say the branch's pull request #9 was merged at head.
func mergedPR(t *testing.T, repo, state, head string) {
	t.Helper()
	ghpr.Forget()
	t.Cleanup(ghpr.Forget)
	fakeGH(t, repo, fmt.Sprintf(`[{"number":9,"title":"t","state":%q,"headRefName":"rush/abc","headRefOid":%q,"isDraft":false,"statusCheckRollup":[],"reviewDecision":"","url":"u"}]`, state, head))
}

func session(wt, repo, base string) *Agent {
	a := &Agent{Key: "s", Done: true, Workspace: &host.Workspace{Path: wt, Repo: repo, Branch: "rush/abc", Base: base, Target: "main", Own: true}}
	a.Cwd = wt
	return a
}

func found(t *testing.T, agents ...*Agent) Worktree {
	t.Helper()
	ws := FindWorktrees(agents)
	if len(ws) != 1 {
		t.Fatalf("found %+v", ws)
	}
	ws[0].Check()
	return ws[0]
}

// The one rule, case by case.
func TestVerdict(t *testing.T) {
	repo, wt, base := landedRepo(t)
	a := session(wt, repo, base)
	ghpr.Forget()

	w := found(t, a)
	if !w.Own || w.Made != MadeByRush || w.Target != "main" || w.Base != base {
		t.Fatalf("own worktree: %+v", w)
	}
	if v := w.Verdict(); v.Kind != WouldLose || !strings.Contains(v.Reason, "1 commit not pushed") {
		t.Fatalf("unpushed commits: %+v", v)
	}

	// Its pull request squash-merged: its commit is on no remote and not
	// in main as itself, but the work is.
	squash(t, repo)
	head := out(t, wt, "rev-parse", "HEAD")
	mergedPR(t, repo, "MERGED", head)
	w = found(t, a)
	if v := w.Verdict(); v.Kind != CanGo || !strings.Contains(v.Reason, "#9 was merged") {
		t.Fatalf("squash-merged pull request: %+v", v)
	}
	if ok, why := w.Finished(); !ok || why != "pull request #9 merged" {
		t.Fatalf("finished: %v %q", ok, why)
	}

	// A commit after the merge would be lost with it.
	run(t, wt, "commit", "-q", "--allow-empty", "-m", "after")
	if w := found(t, a); w.Verdict().Kind != WouldLose {
		t.Fatalf("commit after the merge: %+v", w.Verdict())
	}
	run(t, wt, "reset", "-q", "--hard", head)

	// Uncommitted files stay, merged or not.
	_ = os.WriteFile(filepath.Join(wt, "c.txt"), []byte("c\n"), 0o644)
	if w := found(t, a); w.Verdict().Kind != WouldLose || !strings.Contains(w.Verdict().Reason, "uncommitted") {
		t.Fatalf("uncommitted: %+v", w.Verdict())
	}
	_ = os.Remove(filepath.Join(wt, "c.txt"))

	// A session working in it keeps it.
	busy := session(wt, repo, base)
	busy.Key, busy.Done, busy.PID, busy.DisplayName = "b", false, 42, "Busy one"
	if w := found(t, a, busy); w.Verdict() != (Verdict{KeepInUse, "Busy one is working in it"}) {
		t.Fatalf("in use: %+v", w.Verdict())
	}

	// Pushed with its pull request open: kept for the pull request.
	run(t, wt, "push", "-q", "origin", "rush/abc")
	mergedPR(t, repo, "OPEN", head)
	if w := found(t, a); w.Verdict().Kind != KeepPROpen {
		t.Fatalf("open pull request: %+v", w.Verdict())
	}
}

// A worktree rush didn't make (a subagent's) is judged by the same rule,
// but never goes by itself.
func TestForeignWorktreeOnlyByHand(t *testing.T) {
	repo, _, _ := landedRepo(t)
	ghpr.Forget()
	sub := filepath.Join(repo, ".claude", "worktrees", "agent-x")
	run(t, repo, "worktree", "add", "-q", "-b", "feature", sub)
	a := &Agent{Key: "k", Done: true}
	a.Cwd = sub
	var w Worktree
	for _, x := range FindWorktrees([]*Agent{a}) {
		if x.Path == sub {
			w = x
		}
	}
	w.Check()
	if w.Own || w.Made != MadeBySubagent || !w.Verdict().CanGo() {
		t.Fatalf("subagent's: %+v %+v", w, w.Verdict())
	}
	if did, err := Settle(&w); err != nil || did != nil {
		t.Fatalf("settled one not rush's: %v %v", did, err)
	}
	if _, err := os.Stat(sub); err != nil {
		t.Fatal("removed one not rush's")
	}
}

// Work that landed: the worktree goes, its branch with it (here; not the
// remote's), and the main checkout, clean and on its target, is
// fast-forwarded.
func TestSettleLanded(t *testing.T) {
	repo, wt, base := landedRepo(t)
	squash(t, repo)
	mergedPR(t, repo, "MERGED", out(t, wt, "rev-parse", "HEAD"))
	w := Worktree{Path: wt, Repo: repo, Branch: "rush/abc", Target: "main", Base: base, Own: true, Agents: []string{"s"}}
	w.PR = &PR{Number: 9, State: "merged", HeadOid: out(t, wt, "rev-parse", "HEAD")}
	if ok, why := w.Landing(); !ok || why == "" {
		t.Fatalf("landing: %v %q", ok, why)
	}
	did, err := Settle(&w)
	if err != nil || len(did) != 3 {
		t.Fatalf("settle: %q %v", did, err)
	}
	if _, err := os.Stat(wt); !os.IsNotExist(err) {
		t.Fatal("worktree still there")
	}
	if out(t, repo, "branch", "--list", "rush/abc") != "" {
		t.Fatal("branch still there")
	}
	if out(t, repo, "rev-parse", "HEAD") != out(t, repo, "rev-parse", "origin/main") || out(t, repo, "log", "-1", "--format=%s") != "the work (#9)" {
		t.Fatal("main checkout not fast-forwarded")
	}

	// Its branch merged locally, no pull request: landed on its target too;
	// with nothing of its own yet, it hasn't.
	fresh := filepath.Join(repo, ".claude", "worktrees", "def")
	run(t, repo, "worktree", "add", "-q", "-b", "rush/def", fresh)
	head := out(t, repo, "rev-parse", "HEAD")
	f := Worktree{Path: fresh, Repo: repo, Branch: "rush/def", Target: "main", Base: head, Own: true}
	if ok, _ := f.Landing(); ok {
		t.Fatal("a worktree with nothing done has landed")
	}
	run(t, fresh, "commit", "-q", "--allow-empty", "-m", "local work")
	run(t, repo, "merge", "-q", "--ff-only", "rush/def")
	if ok, why := f.Landing(); !ok || why != "its branch is on main" {
		t.Fatalf("merged locally: %v %q", ok, why)
	}
}

// A worktree moved on to a newer target with nothing of its own (reset to
// origin's main, as an agent starting afresh does) is past its base and on
// its target, but its work hasn't landed: it has none.
func TestResetToNewerTargetHasNotLanded(t *testing.T) {
	repo, wt, base := landedRepo(t)
	squash(t, repo) // someone else's commit on origin's main
	run(t, wt, "fetch", "-q", "origin")
	run(t, wt, "reset", "-q", "--hard", "origin/main")
	w := Worktree{Path: wt, Repo: repo, Branch: "rush/abc", Target: "main", Base: base, Own: true}
	if ok, why := w.Landing(); ok {
		t.Fatalf("reset to a newer main landed: %q", why)
	}

	// A commit of its own, then on main: that has.
	run(t, wt, "commit", "-q", "--allow-empty", "-m", "mine")
	run(t, wt, "push", "-q", "origin", "HEAD:main")
	if ok, why := w.Landing(); !ok || why != "its branch is on main" {
		t.Fatalf("own commit on main: %v %q", ok, why)
	}
}

// git-clean's branches by the same rule: squash-merged ones too.
func TestLandedBranches(t *testing.T) {
	repo, wt, _ := landedRepo(t)
	head := out(t, wt, "rev-parse", "HEAD")
	run(t, repo, "worktree", "remove", "--force", wt)
	run(t, repo, "branch", "plain") // on main as it is
	squash(t, repo)
	run(t, repo, "fetch", "-q", "origin")
	prs, _ := ghpr.ParseBranches([]byte(fmt.Sprintf(`[{"number":9,"state":"MERGED","headRefName":"rush/abc","headRefOid":%q}]`, head)))
	if got := LandedBranches(repo, "main", prs); strings.Join(got, ",") != "plain,rush/abc" {
		t.Fatalf("landed %q", got)
	}
	if got := LandedBranches(repo, "main", nil); strings.Join(got, ",") != "plain" {
		t.Fatalf("without gh %q", got)
	}
}
