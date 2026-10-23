package fleet

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/uithread"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/host"
)

// One rule for every clean-up: a worktree can go when its work is on its
// target branch (an ancestor of origin/<target> or <target>, or its pull
// request merged, squashed or not), or every commit is pushed, with
// nothing uncommitted, and no session works in it. Projects, Clean up,
// Delete, the automatic tidy-up and git-clean all ask Verdict (or
// LandedBranches, for branches alone); only worktrees rush made for a
// session (Own) ever go without being ticked by hand.

// VerdictKind is what clean-up may do with a worktree.
type VerdictKind int

const (
	KeepInUse  VerdictKind = iota // a session works in it now
	KeepPROpen                    // its pull request is still open
	CanGo                         // removing it loses nothing
	WouldLose                     // removing it would lose work: Reason says what
)

// Verdict is what clean-up may do with a worktree, and why, in a few
// words fit to show.
type Verdict struct {
	Kind   VerdictKind
	Reason string
}

// CanGo is a worktree that can go.
func (v Verdict) CanGo() bool { return v.Kind == CanGo }

func (k VerdictKind) String() string {
	return [...]string{"keep-in-use", "keep-pr-open", "can-go", "would-lose"}[k]
}

// Who made a worktree, by where it is and its branch (Worktree.Made).
const (
	MadeByRush     = "rush"     // a session's own: <main>/.claude/worktrees/<id> on rush/<id>
	MadeBySubagent = "subagent" // a subagent's: .claude/worktrees/agent-*
	MadeByClaude   = "claude"   // Claude Code's other ones in .claude/worktrees
	MadeByOther    = "other"    // anywhere else: .worktrees/, ~/worktrees, by hand
)

// madeBy is who made the worktree at path on branch.
func madeBy(path, branch string) string {
	switch {
	case host.IsOwnWorktree(path, branch):
		return MadeByRush
	case !strings.Contains(path, "/.claude/worktrees/"):
		return MadeByOther
	case strings.HasPrefix(filepath.Base(path), "agent-"):
		return MadeBySubagent
	}
	return MadeByClaude
}

// Verdict is what clean-up may do with w, by the one rule. It reads only
// what Check found and FindWorktrees said: no git.
func (w *Worktree) Verdict() Verdict {
	switch {
	case w.InUse != "":
		return Verdict{KeepInUse, w.InUse + " is working in it"}
	case w.Checked.IsZero():
		return Verdict{WouldLose, "not looked at with git yet"}
	case w.Err != "":
		return Verdict{WouldLose, w.Err}
	case !w.Safe():
		return Verdict{WouldLose, w.Losses()}
	case w.mergedPR():
		return Verdict{CanGo, fmt.Sprintf("its pull request #%d was merged", w.PR.Number)}
	case w.Landed:
		return Verdict{CanGo, "its work is on " + w.Target}
	case w.PR != nil && w.PR.Open():
		return Verdict{KeepPROpen, fmt.Sprintf("its pull request #%d is %s", w.PR.Number, w.PR.State)}
	case w.NoRemote || w.OffRemote > 0:
		return Verdict{CanGo, "every commit is in " + firstNonEmpty(w.Target, "main") + ", not pushed"}
	}
	return Verdict{CanGo, "every commit is pushed"}
}

// mergedPR is a worktree whose pull request was merged with everything
// it has: its HEAD is in the pull request's head, so a squash merge
// counts, and commits made after it don't go with it.
func (w *Worktree) mergedPR() bool {
	return w.PR != nil && w.PR.Merged() && w.inPR
}

// Finished is a worktree whose work has landed: its pull request merged,
// or its branch on its target with work of its own since it began
// (Base). A worktree with nothing done yet hasn't landed, though its HEAD
// is on its target already.
func (w *Worktree) Finished() (bool, string) {
	switch {
	case w.Checked.IsZero() || w.Err != "":
		return false, ""
	case w.mergedPR():
		return true, fmt.Sprintf("pull request #%d merged", w.PR.Number)
	case w.Landed && w.Base != "" && w.Head != "" && w.Head != w.Base:
		return true, "its branch is on " + w.Target
	}
	return false, ""
}

// Landing looks with git at whether w's work has landed (Finished), the
// cheap way first: only a worktree whose pull request merged, or whose
// HEAD is on its target with work of its own, is looked at whole (Check,
// without the size), for Verdict.
func (w *Worktree) Landing() (bool, string) {
	uithread.Forbid("fleet.Worktree.Landing")
	if w.PR == nil || !w.PR.Merged() || w.PR.HeadOid == "" {
		w.landed()
		if !w.Landed || w.Base == "" || w.Head == w.Base {
			return false, ""
		}
	}
	w.checkGit()
	return w.Finished()
}

// landed looks with git at whether w's HEAD is on its target, here or on
// origin, and whether it's in its merged pull request's head.
func (w *Worktree) landed() {
	w.Landed, w.inPR = false, false
	w.Head, _ = git(w.Path, "rev-parse", "HEAD")
	if w.Target != "" && w.Branch != w.Target {
		for _, ref := range []string{"refs/remotes/origin/" + w.Target, "refs/heads/" + w.Target} {
			if _, err := git(w.Path, "merge-base", "--is-ancestor", "HEAD", ref); err == nil {
				w.Landed = true
				break
			}
		}
	}
	if w.PR != nil && w.PR.Merged() && w.PR.HeadOid != "" {
		_, err := git(w.Path, "merge-base", "--is-ancestor", "HEAD", w.PR.HeadOid)
		w.inPR = err == nil
	}
}

// LandedBranches are repo's local branches, checked out nowhere, whose
// work is on target by the same rule as Verdict: in origin/<target> or
// <target>, or in the head of their merged pull request (prs, by branch).
// Deleting them loses nothing. It runs git: never on the UI's goroutine.
func LandedBranches(repo, target string, prs map[string]PR) []string {
	uithread.Forbid("fleet.LandedBranches")
	out, err := git(repo, "for-each-ref", "--format=%(refname:short)\t%(worktreepath)", "refs/heads")
	if err != nil {
		return nil
	}
	merged := map[string]bool{}
	for _, ref := range []string{"refs/remotes/origin/" + target, "refs/heads/" + target} {
		if m, err := git(repo, "for-each-ref", "--merged", ref, "--format=%(refname:short)", "refs/heads"); err == nil {
			for n := range strings.SplitSeq(m, "\n") {
				merged[n] = true
			}
		}
	}
	var landed []string
	for l := range strings.SplitSeq(out, "\n") {
		name, wt, _ := strings.Cut(l, "\t")
		if name == "" || name == target || wt != "" {
			continue
		}
		if !merged[name] {
			pr, ok := prs[name]
			if !ok || !pr.Merged() || pr.HeadOid == "" {
				continue
			}
			if _, err := git(repo, "merge-base", "--is-ancestor", "refs/heads/"+name, pr.HeadOid); err != nil {
				continue
			}
		}
		landed = append(landed, name)
	}
	slices.Sort(landed)
	return landed
}

// Settle is what's done when a worktree's work has landed: the worktree
// removed, if rush made it and the rule says it can go, its branch deleted
// (here only: the remote's is never touched), and the main checkout
// fast-forwarded to its target, if it's clean and on it. It says what it
// did, for a note. It runs git: never on the UI's goroutine.
func Settle(w *Worktree) (did []string, err error) {
	uithread.Forbid("fleet.Settle")
	if !w.Own {
		return nil, nil // not rush's: only ever removed by hand
	}
	if v := w.Verdict(); !v.CanGo() {
		return nil, fmt.Errorf("%s stays: %s", filepath.Base(w.Path), v.Reason)
	}
	if _, err := os.Stat(w.Path); err == nil {
		if _, err := removeWorktree(*w, false, false); err != nil {
			worktreeEvent("worktree-settled", *w, map[string]any{"why": "its work landed"}, err)
			return nil, err
		}
		did = append(did, "removed "+filepath.Base(w.Path))
	}
	if w.Branch != "" && w.Branch != w.Target {
		// -D: a squash-merged branch isn't merged as git sees it, but
		// Verdict has said its work is on the target.
		if _, err := git(w.Repo, "branch", "-D", w.Branch); err == nil {
			did = append(did, "deleted branch "+w.Branch)
		}
	}
	if ff, err := FastForward(w.Repo, w.Target); err == nil && ff {
		did = append(did, "brought "+filepath.Base(w.Repo)+" up to date with "+w.Target)
	}
	worktreeEvent("worktree-settled", *w, map[string]any{"why": "its work landed", "did": did}, nil)
	return did, nil
}

// FastForward brings the main checkout at repo up to origin's target with
// git merge --ff-only, and nothing else: only when it's on target with
// nothing uncommitted (files git doesn't track don't count). It fetches
// target first. ok is whether it moved.
func FastForward(repo, target string) (ok bool, err error) {
	uithread.Forbid("fleet.FastForward")
	if target == "" {
		return false, nil
	}
	if b, err := git(repo, "symbolic-ref", "--short", "-q", "HEAD"); err != nil || b != target {
		return false, nil
	}
	if st, err := git(repo, "status", "--porcelain", "--untracked-files=no"); err != nil || st != "" {
		return false, nil
	}
	if r, _ := git(repo, "remote"); !slices.Contains(strings.Fields(r), "origin") {
		return false, nil
	}
	if _, err := git(repo, "fetch", "--quiet", "origin", target); err != nil {
		return false, err
	}
	before, _ := git(repo, "rev-parse", "HEAD")
	if _, err := git(repo, "merge", "--ff-only", "--quiet", "refs/remotes/origin/"+target); err != nil {
		return false, err
	}
	after, _ := git(repo, "rev-parse", "HEAD")
	if after != before {
		events.Log("main-fast-forwarded", map[string]any{"repo": repo, "target": target, "from": before, "to": after})
	}
	return after != before, nil
}
