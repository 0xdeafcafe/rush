package fleet

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
)

// leftSlack is how long after an agent was last active something it wrote
// may still change and count as its own: a clone or build it started
// finishes after the call that made it.
const leftSlack = 5 * time.Minute

// LeftItem is a place an agent wrote outside its project and scratch, as
// it is on disk now.
type LeftItem struct {
	Path  string
	Agent *Agent
	Shell bool // read from a shell command: a guess
	Size  int64
	// Temp is in a system temp folder: scratch by nature. Anything else
	// may be something you meant it to make.
	Temp bool
	Gone bool
	// Touched is changed since the agent was last active: someone else
	// uses it now.
	Touched bool
	Tracked bool // git keeps it: part of some repository
	// Before is there from before the session began, or not known to be
	// newer: the agent wrote into it, it didn't make it.
	Before  bool
	Shallow bool // home, a folder at its top or the disk's, or a temp folder itself
}

// Safe is whether it can go without asking more: scratch in a temp folder
// nothing has touched since a finished agent left it.
func (it LeftItem) Safe() bool {
	return it.Temp && it.Why() == "" && it.Agent.PID == 0
}

// Why is what holds it back from going, or "".
func (it LeftItem) Why() string {
	switch {
	case it.Gone:
		return "gone already"
	case it.Shallow:
		return "too near the top of the disk to delete from here"
	case it.Before:
		return "was there before the agent; it only wrote into it"
	case it.Tracked:
		return "git keeps it"
	case it.Touched:
		return "changed since the agent left it"
	}
	return ""
}

// LookLeft looks at what the agents wrote outside their projects, each
// place once (the outermost, the latest writer's), on disk now: off the UI.
func LookLeft(agents []*Agent) []LeftItem {
	type rec struct {
		l host.Left
		a *Agent
	}
	byPath := map[string]rec{}
	for _, a := range agents {
		for _, l := range a.Left {
			if r, ok := byPath[l.Path]; !ok || l.At.After(r.l.At) {
				byPath[l.Path] = rec{l, a}
			}
		}
	}
	paths := make([]string, 0, len(byPath))
	for p := range byPath {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	var out []LeftItem
	for i, p := range paths {
		if i > 0 && within(out, p) {
			continue // inside one already listed
		}
		r := byPath[p]
		out = append(out, lookLeft(LeftItem{Path: p, Agent: r.a, Shell: r.l.Shell}))
	}
	return out
}

func within(out []LeftItem, p string) bool {
	for _, it := range out {
		if strings.HasPrefix(p, it.Path+string(filepath.Separator)) {
			return true
		}
	}
	return false
}

// lookLeft fills in what it is on disk now.
func lookLeft(it LeftItem) LeftItem {
	it.Temp = host.IsTemp(it.Path)
	it.Shallow = shallow(it.Path)
	if _, err := os.Lstat(it.Path); err != nil {
		it.Gone = true
		return it
	}
	b, ok := born(it.Path)
	began, ok2 := born(filepath.Join(host.Root(), it.Agent.ID))
	it.Before = !ok || !ok2 || b.Before(began)
	var newest time.Time
	it.Size, newest = scratchWalk(it.Path)
	it.Touched = newest.After(it.Agent.UpdatedAt.Add(leftSlack))
	it.Tracked = tracked(it.Path)
	return it
}

// shallow is a path too near the top to delete from here, wherever an
// agent wrote: /, home, their own folders (/opt, ~/Documents), and the temp
// folders themselves.
func shallow(p string) bool {
	home, _ := os.UserHomeDir()
	for _, top := range []string{"/", home, "/tmp", "/private/tmp", "/var/folders", "/private/var/folders", filepath.Clean(os.TempDir()), host.TempRoot()} {
		if p == top {
			return true
		}
	}
	d := filepath.Dir(p)
	return d == "/" || d == home || strings.HasSuffix(d, "/var/folders") || d == "/Volumes"
}

// Again is it looked at afresh.
func (it LeftItem) Again() LeftItem { return lookLeft(it) }

// tracked is whether git keeps p, or anything in it.
func tracked(p string) bool {
	out, err := exec.Command("git", "-C", filepath.Dir(p), "ls-files", "--", filepath.Base(p)).Output()
	return err == nil && len(out) > 0
}

// RemoveLeft deletes it once looked at afresh, refusing what's running,
// gone, kept by git or touched since: you may tick what isn't scratch, but
// never what's changed under someone else.
func RemoveLeft(it LeftItem) (freed int64, err error) {
	if it.Agent.PID != 0 {
		return 0, fmt.Errorf("%s is still running; stop it first", it.Agent.DisplayName)
	}
	if !filepath.IsAbs(it.Path) || it.Path == "/" {
		return 0, fmt.Errorf("won't delete %s", it.Path)
	}
	now := lookLeft(it)
	if why := now.Why(); why != "" {
		return 0, errors.New(it.Path + ": " + why + "; it stays")
	}
	return now.Size, os.RemoveAll(it.Path)
}
