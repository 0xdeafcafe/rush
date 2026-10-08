package clean

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// gitSettings are git-clean's: how long a worktree sits untouched before
// it's offered for removal.
var gitSettings = []plugin.SettingSpec{{
	Key: "worktree-idle-days", Title: "Worktrees idle for (days)", Type: "choice",
	Choices: []string{"1", "3", "7", "14", "30"}, Default: "3",
	Description: "Days with no file changed before a clean, merged worktree nothing runs in is offered for removal",
}}

// reapAfter is the idle time setting, read afresh each time it's asked.
func reapAfter() time.Duration {
	v := plugin.SettingValuesOf(&plugin.Manifest{Name: "git-clean", Settings: gitSettings})["worktree-idle-days"]
	n, err := strconv.Atoi(v)
	if err != nil || n < 1 {
		n = 3
	}
	return time.Duration(n) * 24 * time.Hour
}

// reapFacts is what decides whether a worktree can go: gathered by look,
// judged by refusals.
type reapFacts struct {
	Main    bool // the repo's own checkout
	Locked  bool
	Err     string // a check that failed
	Changed int    // uncommitted files, untracked ones included; ignored ones not
	Held    int    // processes working in it; -1 when that can't be told
	Merged  string // how its work is kept: "" when it isn't
	Idle    time.Duration
}

// refusals are the reasons a worktree stays, none when it can go. The
// repo's own checkout always stays.
func refusals(f reapFacts, idleFor time.Duration) []string {
	if f.Main {
		return []string{"the main checkout"}
	}
	var why []string
	if f.Err != "" {
		why = append(why, f.Err)
	}
	if f.Locked {
		why = append(why, "locked")
	}
	if f.Changed > 0 {
		why = append(why, plural(f.Changed, "uncommitted file", "uncommitted files"))
	}
	switch {
	case f.Held < 0:
		why = append(why, "can't tell what runs there")
	case f.Held > 0:
		why = append(why, plural(f.Held, "process", "processes")+" working there")
	}
	if f.Merged == "" {
		why = append(why, "its commits aren't all in the default branch")
	}
	if f.Idle < idleFor {
		why = append(why, "touched "+days(f.Idle)+" ago")
	}
	return why
}

func days(d time.Duration) string { return strconv.Itoa(int(d.Hours()/24)) + "d" }

// reapRows are the stale worktrees of the repos sessions were seen in:
// those that can go, and those idle but kept, with why. Worktrees touched
// within the idle time aren't listed.
func reapRows(ctx context.Context, seen []session) []thing {
	wts, busy := worktrees(seen)
	if len(wts) == 0 {
		return nil
	}
	idleFor := reapAfter()
	cwds, cwdsOK := processCwds(ctx)
	bases := map[string]string{}
	var out []thing
	for _, w := range wts {
		if busy[w.Path] {
			continue
		}
		base, ok := bases[w.Repo]
		if !ok {
			base, _ = command(ctx, w.Repo, "git", "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
			bases[w.Repo] = base
		}
		f := look(ctx, w, base, cwds, cwdsOK, idleFor)
		if f.Idle < idleFor {
			continue
		}
		label := w.Path
		if rel, err := filepath.Rel(w.Repo, w.Path); err == nil {
			label = rel
		}
		name := filepath.Base(w.Repo) + ": worktree " + label
		if why := refusals(f, idleFor); len(why) > 0 {
			out = append(out, thing{ID: "stale:" + w.Path, Title: name + " is stale but kept", Meta: "kept", In: w.Path,
				Does: "does nothing: " + strings.Join(why, ", "), Clean: keep(why)})
			continue
		}
		out = append(out, thing{ID: "reap:" + w.Path, Title: name + " is no longer needed", Path: w.Path, In: w.Path,
			Does:  "runs git worktree remove, then git worktree prune: idle " + days(f.Idle) + ", " + f.Merged + ", clean, nothing runs there",
			Clean: reap(w, base, idleFor)})
	}
	return out
}

// look gathers a worktree's facts. It stops at its idle time when that's
// short of idleFor, since the rest can't change the answer.
func look(ctx context.Context, w fleet.Worktree, base string, cwds []string, cwdsOK bool, idleFor time.Duration) reapFacts {
	f := reapFacts{Main: filepath.Clean(w.Path) == filepath.Clean(w.Repo), Locked: w.Locked}
	f.Idle = time.Since(lastTouched(w.Path, time.Now().Add(-idleFor)))
	if f.Main || f.Idle < idleFor {
		return f
	}
	status, err := command(ctx, w.Path, "git", "status", "--porcelain", "--untracked-files=normal")
	if err != nil {
		f.Err = "git status failed"
	} else if status != "" {
		f.Changed = len(strings.Split(status, "\n"))
	}
	f.Held = -1
	if cwdsOK {
		f.Held = 0
		dir, err := filepath.EvalSymlinks(w.Path)
		if err != nil {
			dir = w.Path
		}
		for _, c := range cwds {
			if c == dir || strings.HasPrefix(c, dir+"/") {
				f.Held++
			}
		}
	}
	f.Merged = merged(ctx, w, base)
	return f
}

// merged says how a worktree's commits are kept, "" when they aren't: its
// branch is in the default branch, or its upstream is gone and every
// patch is in it, or it's detached at a commit the default branch has. The branch itself stays.
func merged(ctx context.Context, w fleet.Worktree, base string) string {
	if w.Branch == "" {
		if base != "" {
			if _, err := command(ctx, w.Path, "git", "merge-base", "--is-ancestor", "HEAD", base); err == nil {
				return "detached at a commit in " + base
			}
		}
		return ""
	}
	if base != "" {
		if _, err := command(ctx, w.Repo, "git", "merge-base", "--is-ancestor", "refs/heads/"+w.Branch, base); err == nil {
			return w.Branch + " is merged into " + base
		}
	}
	if base == "" {
		return ""
	}
	// A gone upstream alone isn't enough: every commit's patch must be in base (squash merges).
	if track, err := command(ctx, w.Repo, "git", "for-each-ref", "--format=%(upstream:track)", "refs/heads/"+w.Branch); err == nil && track == "[gone]" {
		if out, err := command(ctx, w.Repo, "git", "cherry", base, "refs/heads/"+w.Branch); err == nil && !strings.Contains("\n"+out, "\n+") {
			return w.Branch + "'s upstream is gone and its changes are in " + base
		}
	}
	return ""
}

// errStop ends a walk early.
var errStop = errors.New("stop")

// lastTouched is when anything in a worktree last changed, outside
// node_modules, and its git index and HEAD; it stops at the first change
// after since, as that settles it.
func lastTouched(path string, since time.Time) time.Time {
	var last time.Time
	touch := func(t time.Time) bool {
		if t.After(last) {
			last = t
		}
		return t.After(since)
	}
	if b, err := os.ReadFile(filepath.Join(path, ".git")); err == nil {
		gd := strings.TrimSpace(strings.TrimPrefix(string(b), "gitdir:"))
		if !filepath.IsAbs(gd) {
			gd = filepath.Join(path, gd)
		}
		for _, n := range []string{"index", "HEAD", "logs/HEAD"} {
			if st, err := os.Stat(filepath.Join(gd, n)); err == nil && touch(st.ModTime()) {
				return last
			}
		}
	}
	_ = filepath.WalkDir(path, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() && p != path && d.Name() == "node_modules" {
			return fs.SkipDir
		}
		if info, err := d.Info(); err == nil && touch(info.ModTime()) {
			return errStop
		}
		return nil
	})
	return last
}

// processCwds are the folders the user's processes work in, by their real
// paths. ok is false when that can't be told, which keeps every worktree.
func processCwds(ctx context.Context) (cwds []string, ok bool) {
	out, err := exec.CommandContext(ctx, "lsof", "-n", "-a", "-d", "cwd", "-Fn", "-u", strconv.Itoa(os.Getuid())).Output()
	if err != nil && len(out) == 0 {
		return nil, false
	}
	for l := range strings.SplitSeq(string(out), "\n") {
		if p, found := strings.CutPrefix(l, "n"); found {
			cwds = append(cwds, p)
		}
	}
	return cwds, true
}

// reap is the Clean of a worktree that can go: it looks again, as things
// may have changed since it was listed, then removes it without force, so
// git refuses anything dirty or locked too.
func reap(w fleet.Worktree, base string, idleFor time.Duration) func(context.Context) (string, error) {
	return func(ctx context.Context) (string, error) {
		cwds, ok := processCwds(ctx)
		if why := refusals(look(ctx, w, base, cwds, ok, idleFor), idleFor); len(why) > 0 {
			return "", errors.New("left alone: " + strings.Join(why, ", "))
		}
		if _, err := command(ctx, w.Repo, "git", "worktree", "remove", w.Path); err != nil {
			return "", err
		}
		if _, err := command(ctx, w.Repo, "git", "worktree", "prune"); err != nil {
			return "", err
		}
		return "Removed; its branch stays.", nil
	}
}

// keep is the Clean of a stale worktree that stays: it only says why.
func keep(why []string) func(context.Context) (string, error) {
	return func(context.Context) (string, error) {
		return "", errors.New("left alone: " + strings.Join(why, ", "))
	}
}
