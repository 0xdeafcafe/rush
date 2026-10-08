package fleet

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/uithread"
)

// Worktree is a linked git worktree an agent works in, and whether it can
// go without losing anything.
type Worktree struct {
	Path   string
	Repo   string // the main checkout it belongs to
	Branch string
	Agents []string // keys of the agents working in it
	Claude bool     // under a repo's .claude/worktrees: made for an agent
	Size   int64

	Changed   int  // files with uncommitted changes, untracked ones included
	Unpushed  int  // commits on no remote and not in main
	OffRemote int  // commits on no remote, main or not
	NoRemote  bool // the repo has no remote at all
	Locked    bool
	Err       string
	Checked   time.Time
}

// Safe is a worktree whose every change is committed, and on a remote or
// in the main checkout's branch: removing it loses nothing but files git
// ignores (builds, dependencies).
func (w Worktree) Safe() bool {
	return w.Err == "" && !w.Checked.IsZero() && w.Changed == 0 && w.Unpushed == 0 && !w.Locked
}

// Pushed is Safe with every commit on a remote: what the automatic
// tidy-up asks for. Work only in a local main goes only when you say so.
func (w Worktree) Pushed() bool {
	return w.Safe() && w.OffRemote == 0 && !w.NoRemote
}

// Losses says what removing it would throw away, for a confirmation.
func (w Worktree) Losses() string {
	var out []string
	if w.Changed > 0 {
		out = append(out, plural(w.Changed, "file")+" with uncommitted changes")
	}
	if w.Unpushed > 0 {
		out = append(out, plural(w.Unpushed, "commit")+" not pushed or in main")
	}
	if w.Locked {
		out = append(out, "a lock someone put on it")
	}
	return strings.Join(out, " · ")
}

func plural(n int, what string) string {
	if n == 1 {
		return "1 " + what
	}
	return strconv.Itoa(n) + " " + what + "s"
}

func git(dir string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0") // a look must not block the agent's own git
	out, err := cmd.Output()
	return strings.TrimRight(string(out), "\n"), err
}

// workDirs are the folders an agent is known to have worked in.
func (a *Agent) workDirs() []string {
	out := append([]string{}, a.Spend.Dirs...)
	for _, d := range []string{a.WorktreePath, a.Cwd} {
		if d != "" {
			out = append(out, d)
		}
	}
	return out
}

// FindWorktrees lists the linked worktrees of every repository the agents
// work in, each with the agents working in it. Nothing is checked yet.
func FindWorktrees(agents []*Agent) []Worktree {
	repos := map[string]bool{}
	seen := map[string]string{}
	for _, a := range agents {
		for _, d := range a.workDirs() {
			if main := mainCheckout(d, seen); main != "" {
				repos[main] = true
			}
		}
	}
	var out []Worktree
	for repo := range repos {
		list, err := git(repo, "worktree", "list", "--porcelain")
		if err != nil {
			continue
		}
		var w *Worktree
		flush := func() {
			if w != nil && w.Path != repo {
				out = append(out, *w)
			}
			w = nil
		}
		for _, l := range strings.Split(list, "\n") {
			switch {
			case strings.HasPrefix(l, "worktree "):
				flush()
				p := strings.TrimPrefix(l, "worktree ")
				w = &Worktree{Path: p, Repo: repo, Claude: strings.Contains(p, "/.claude/worktrees/")}
			case w == nil:
			case strings.HasPrefix(l, "branch "):
				w.Branch = strings.TrimPrefix(strings.TrimPrefix(l, "branch "), "refs/heads/")
			case l == "locked" || strings.HasPrefix(l, "locked "):
				w.Locked = true
			case l == "prunable" || strings.HasPrefix(l, "prunable "):
				w = nil // its folder is already gone
			}
		}
		flush()
	}
	dirs := make([][]string, len(agents)) // each agent's once, not once a worktree
	for i, a := range agents {
		dirs[i] = a.workDirs()
	}
	for i := range out {
		w := &out[i]
		for j, a := range agents {
			for _, d := range dirs[j] {
				if strings.HasPrefix(d, w.Path) && (len(d) == len(w.Path) || d[len(w.Path)] == '/') {
					w.Agents = append(w.Agents, a.Key)
					break
				}
			}
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Path < out[j].Path })
	return out
}

// Check looks at a worktree with git: what's uncommitted, what's unpushed,
// and how much disk it takes. It can take seconds on a big checkout.
func (w *Worktree) Check() {
	if w.checkGit() {
		w.Size = DiskUsageBackground([]TempDir{{Path: w.Path}})
	}
}

// checkGit is Check without the size: only what git says, which is all
// that says whether it's safe to remove.
func (w *Worktree) checkGit() bool {
	_, _, ok := w.look()
	return ok
}

// Doomed looks at w afresh with git and says, line by line, what removing
// it would lose: its uncommitted files as git status names them, and its
// commits nowhere else, as "sha subject". It runs git several times:
// never on the UI's goroutine.
func (w *Worktree) Doomed() (files, commits []string) {
	uithread.Forbid("fleet.Worktree.Doomed")
	files, commits, _ = w.look()
	return files, commits
}

// look is checkGit, with the files and commits behind its counts.
func (w *Worktree) look() (files, commits []string, ok bool) {
	w.Checked, w.Err = time.Now(), ""
	status, err := git(w.Path, "status", "--porcelain", "--untracked-files=normal")
	if err != nil {
		w.Err = "git status failed"
		return nil, nil, false
	}
	w.Changed = 0
	if status != "" {
		files = strings.Split(status, "\n")
		w.Changed = len(files)
	}
	remotes, _ := git(w.Path, "remote")
	w.NoRemote = strings.TrimSpace(remotes) == ""
	commits, off, err := unkept(w.Path, w.Repo, w.NoRemote)
	if err != nil {
		w.Err = "couldn't compare with the remote"
		return files, nil, false
	}
	w.Unpushed, w.OffRemote = len(commits), off
	return files, commits, true
}

// unkept is the worktree's commits that are nowhere else, as "sha
// subject": on no remote, and not in the main checkout's branch, as
// themselves or cherry-picked. Work an agent's branch handed back to main
// is kept there, pushed or not. off counts those on no remote, in main or
// not.
func unkept(path, repo string, noRemote bool) (lost []string, off int, err error) {
	args := []string{"rev-list", "HEAD"}
	if !noRemote {
		args = append(args, "--not", "--remotes")
	}
	out, err := git(path, args...)
	if err != nil || out == "" {
		return nil, 0, err
	}
	shas := strings.Fields(out)
	short := func(s string) string { return s[:min(7, len(s))] }
	all := func() []string {
		for i, s := range shas {
			shas[i] = short(s)
		}
		return shas
	}
	base, err := git(repo, "rev-parse", "HEAD")
	if err != nil {
		return all(), len(shas), nil
	}
	// git cherry marks with + what base has no equivalent of, and doesn't
	// list what base already contains.
	cherry, err := git(path, "cherry", "-v", base, "HEAD")
	if err != nil {
		return all(), len(shas), nil
	}
	unique := map[string]string{}
	for l := range strings.SplitSeq(cherry, "\n") {
		if rest, ok := strings.CutPrefix(l, "+ "); ok {
			sha, subject, _ := strings.Cut(rest, " ")
			unique[sha] = subject
		}
	}
	for _, s := range shas {
		if subject, ok := unique[s]; ok {
			lost = append(lost, short(s)+" "+subject)
		}
	}
	return lost, len(shas), nil
}

// RemoveWorktree removes a worktree through git, which also forgets it in
// the main checkout; its branch stays. Unless force is set, git refuses one
// with uncommitted changes, and this refuses one with unpushed commits.
func RemoveWorktree(w Worktree, force bool) error {
	_, err := removeWorktree(w, force, false)
	return err
}

// TidyWorktree is RemoveWorktree for the automatic clean-up: only a
// worktree whose every commit is pushed goes. It says how much disk went,
// measured only once git has said the worktree is safe to go, since walking
// a big checkout takes seconds.
func TidyWorktree(w Worktree) (int64, error) { return removeWorktree(w, false, true) }

// removeWorktree removes w; measure is the automatic clean-up, which asks
// for Pushed rather than Safe.
func removeWorktree(w Worktree, force, measure bool) (int64, error) {
	if err := linkedWorktree(w); err != nil {
		return 0, err
	}
	if !force {
		c := w
		c.checkGit()
		if !c.Safe() {
			return 0, fmt.Errorf("%s isn't safe to remove: %s", filepath.Base(w.Path), firstNonEmpty(c.Losses(), c.Err))
		}
		if measure && !c.Pushed() {
			return 0, fmt.Errorf("%s isn't safe to remove: %s", filepath.Base(w.Path), "its commits are only in a local main")
		}
	}
	var size int64
	if measure {
		size = DiskUsage([]TempDir{{Path: w.Path}})
	}
	args := []string{"worktree", "remove", w.Path}
	if force {
		args = []string{"worktree", "remove", "--force", "--force", w.Path}
	}
	if _, err := git(w.Repo, args...); err != nil {
		var ee *exec.ExitError
		if errorsAs(err, &ee) && len(ee.Stderr) > 0 {
			return 0, fmt.Errorf("git: %s", strings.TrimSpace(string(ee.Stderr)))
		}
		return 0, err
	}
	return size, nil
}

func firstNonEmpty(s ...string) string {
	for _, x := range s {
		if x != "" {
			return x
		}
	}
	return ""
}

func errorsAs(err error, target **exec.ExitError) bool { return errors.As(err, target) }

// MainCheckout is the main checkout of the repository dir is in, through
// any linked worktree; dir itself when it isn't in one.
func MainCheckout(dir string) string {
	return firstNonEmpty(mainCheckout(dir, map[string]string{}), dir)
}

// SessionProject is the main checkout a rush session works in; "" when
// its config has gone.
func SessionProject(id string) string {
	if cfg, err := host.ReadConfig(id); err == nil && cfg.Cwd != "" {
		return MainCheckout(cfg.Cwd)
	}
	return ""
}

// mainCheckout finds the main checkout of the repository dir is in, by
// looking for .git upwards: stats only, no git. A linked worktree's .git
// is a file naming <main>/.git/worktrees/<name>. seen remembers answers.
func mainCheckout(dir string, seen map[string]string) string {
	var walked []string
	main := ""
	for d := dir; d != "/" && d != "." && d != ""; d = filepath.Dir(d) {
		if m, ok := seen[d]; ok {
			main = m
			break
		}
		walked = append(walked, d)
		p := filepath.Join(d, ".git")
		st, err := os.Stat(p)
		if err != nil {
			continue
		}
		if st.IsDir() {
			main = d
			break
		}
		b, _ := os.ReadFile(p)
		g := strings.TrimSpace(strings.TrimPrefix(string(b), "gitdir:"))
		if !filepath.IsAbs(g) {
			g = filepath.Join(d, g)
		}
		if i := strings.Index(g, "/.git/worktrees/"); i >= 0 {
			main = g[:i]
		}
		break
	}
	if main != "" {
		for _, d := range walked {
			seen[d] = main
		}
	}
	return main
}

// linkedWorktree refuses anything that isn't a linked worktree: a repo's
// own checkout (its .git is a folder), a folder holding the repo, or one
// whose .git doesn't point back into the repo's worktrees. Only a linked
// worktree is ever removed, forced or not.
func linkedWorktree(w Worktree) error {
	path, repo := filepath.Clean(w.Path), filepath.Clean(w.Repo)
	if path == repo || strings.HasPrefix(repo+"/", path+"/") || path == "/" || repo == "" {
		return fmt.Errorf("%s is the repository itself, not a worktree; it's never removed", path)
	}
	st, err := os.Lstat(filepath.Join(path, ".git"))
	if err != nil || st.IsDir() {
		return fmt.Errorf("%s isn't a linked worktree; it's never removed", path)
	}
	b, err := os.ReadFile(filepath.Join(path, ".git"))
	if err != nil || !strings.HasPrefix(strings.TrimSpace(strings.TrimPrefix(string(b), "gitdir:")), filepath.Join(repo, ".git", "worktrees")+"/") {
		return fmt.Errorf("%s doesn't belong to %s as a worktree; it's never removed", path, repo)
	}
	return nil
}

// cwds are transcripts' folders as their heads say: one never changes.
var cwds sync.Map

var headBufs = sync.Pool{New: func() any { b := make([]byte, 64<<10); return &b }}

var cwdMark = []byte(`"cwd":"`)

// TranscriptCwd is the folder a transcript's head says its agent ran in:
// Claude Code's lines and Codex's session_meta both say.
func TranscriptCwd(path string) string {
	if v, ok := cwds.Load(path); ok {
		return v.(string)
	}
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()
	// Its first line says it, after the prompt: mostly in the first few
	// kilobytes, read into a buffer kept for the next.
	bp := headBufs.Get().(*[]byte)
	defer headBufs.Put(bp)
	b := *bp
	n, _ := io.ReadFull(f, b[:16<<10])
	i := bytes.Index(b[:n], cwdMark)
	if i < 0 && n == 16<<10 {
		more, _ := io.ReadFull(f, b[n:])
		n += more
		i = bytes.Index(b[:n], cwdMark)
	}
	b = b[:n]
	if i < 0 {
		return ""
	}
	j := bytes.IndexByte(b[i+len(cwdMark):], '"')
	var cwd string
	if j < 0 || jsonx.Unmarshal(b[i+len(cwdMark)-1:i+len(cwdMark)+j+1], &cwd) != nil || cwd == "" {
		return ""
	}
	cwds.Store(path, cwd)
	return cwd
}

// SubWorktree is the checkout a subagent's transcript says it worked in,
// by its branch (or folder), when that isn't its session's, whose folder
// is cwd: "" when it's the same. ok is false while that can't be told.
func SubWorktree(transcript, cwd string) (label string, ok bool) {
	label, ok = SubWorktrees([]string{transcript}, cwd)[transcript]
	return label, ok
}

// SubWorktrees is SubWorktree for many runs of one session, asking git
// about each folder once: by transcript, those that can be told.
func SubWorktrees(transcripts []string, cwd string) map[string]string {
	out := map[string]string{}
	if cwd == "" {
		return out
	}
	own, _ := gitAt(cwd)
	type checkout struct{ repo, branch string }
	seen := map[string]checkout{}
	for _, t := range transcripts {
		sub := TranscriptCwd(t)
		if sub == "" {
			continue
		}
		g, ok := seen[sub]
		if !ok {
			g.repo, g.branch = gitAt(sub)
			seen[sub] = g
		}
		out[t] = ""
		if g.repo != "" && g.repo != own {
			out[t] = firstNonEmpty(g.branch, filepath.Base(g.repo))
		}
	}
	return out
}
