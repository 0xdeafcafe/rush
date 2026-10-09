package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Up and down go from project to project in the list; enter goes into the
// one picked, on the right, and left comes back.
func TestProjectsMoveKeepsLevel(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}, w: 120, h: 40}
	for _, a := range []*fleet.Agent{
		{Key: "a1", PID: 1, Root: "/src/alpha", Repo: "/src/alpha"},
		{Key: "a2", PID: 2, Root: "/src/alpha", Repo: "/src/alpha"},
		{Key: "b1", PID: 3, Root: "/src/beta", Repo: "/src/beta"},
	} {
		a.UpdatedAt = now
		m.snap.Agents = append(m.snap.Agents, a)
	}
	m.work.projSel = "p/src/alpha"
	m.projectsBody()
	step := func(k, want string) {
		t.Helper()
		m.projectsKey(k)
		if m.work.projSel != want {
			t.Fatalf("%s: on %q, want %q", k, m.work.projSel, want)
		}
	}
	step("down", "p/src/beta")
	step("up", "p/src/alpha")
	// enter goes into the project on the right; the list keeps its place.
	m.projectsKey("enter")
	if !m.work.projIn || m.work.inSel != "aa1" {
		t.Fatalf("enter: in %v on %q, want in on aa1", m.work.projIn, m.work.inSel)
	}
	m.projectsKey("down")
	m.projectsKey("down")
	if m.work.inSel != "aa2" {
		t.Fatalf("down in alpha went to %q, want aa2", m.work.inSel)
	}
	step("left", "p/src/alpha")
	if m.work.projIn {
		t.Fatal("left didn't come back to the list")
	}
}

// Each kind has its own place in the one list: repositories, other
// folders, then Temporary, which shows /tmp and temp work on the right.
func TestProjectsSections(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}, w: 140, h: 40}
	for _, a := range []*fleet.Agent{
		{Key: "a1", PID: 1, Root: "/src/alpha", Repo: "/src/alpha"},
		{Key: "b1", Cwd: "/Users/me/Downloads", DisplayName: "Sort downloads", Temp: 2 << 20},
	} {
		a.UpdatedAt = now
		m.snap.Agents = append(m.snap.Agents, a)
	}
	m.clean.tmp = fleet.Scratch{Items: 3, Size: 4 << 20, StaleItems: 1, Stale: 1 << 20, Checked: now}
	page := ansi.Strip(strings.Join(m.projectsBody(), "\n"))
	m.pickInProjects(paneTemp)
	page += ansi.Strip(strings.Join(m.projectsBody(), "\n"))
	last := -1
	for _, s := range []string{"Projects", "◆ alpha", "OTHER FOLDERS", "◇ Downloads", "◌ Temporary", "System", "◌ /tmp", "◌ Sort downloads"} {
		i := strings.Index(page[last+1:], s)
		if i < 0 {
			t.Fatalf("%q missing or out of order:\n%s", s, page)
		}
		last += 1 + i
	}
}

// A project on the right says how big each of its worktrees is, and what
// its agents keep as scratch in it (.claude/tmp).
func TestProjectShowsSizes(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}, w: 160, h: 40}
	a := &fleet.Agent{Key: "a1", PID: 1, Root: "/src/alpha", Repo: "/src/alpha/.claude/worktrees/fix"}
	a.UpdatedAt = now
	m.snap.Agents = []*fleet.Agent{a}
	m.folders.byRoot = map[string]fleet.Folder{"/src/alpha": {Trees: map[string]fleet.GitState{"/src/alpha/.claude/worktrees/fix": {Branch: "fix"}}}}
	m.clean.wts = []fleet.Worktree{{Path: "/src/alpha/.claude/worktrees/fix", Size: 300 << 20, Checked: now}}
	m.clean.agentTmp = map[string]int64{"/src/alpha/.claude/tmp": 12 << 20}
	m.work.projSel = "p/src/alpha"
	page := ansi.Strip(strings.Join(m.projectsBody(), "\n"))
	for _, s := range []string{"WORKTREES", "fix", "300M", "AGENTS' SCRATCH", ".claude/tmp", "12M"} {
		if !strings.Contains(page, s) {
			t.Errorf("%q missing:\n%s", s, page)
		}
	}
}

// An agent untouched for hours says it can go, and x asks to close it; a
// session's temp work opens thing by thing, and x deletes just one.
func TestProjectsSayWhatCanGo(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}, w: 160, h: 40}
	old := &fleet.Agent{Key: "old", Root: "/src/alpha", Repo: "/src/alpha", DisplayName: "probe-codex"}
	old.UpdatedAt = now.Add(-5 * time.Hour)
	m.snap.Agents = []*fleet.Agent{old}
	m.work.projSel = "p/src/alpha"
	if page := ansi.Strip(strings.Join(m.projectsBody(), "\n")); !strings.Contains(page, "can go") {
		t.Fatalf("no 'can go' for an agent untouched 5h:\n%s", page)
	}
	m.projectsKey("enter")
	m.projectsKey("x")
	if m.confirm == nil || !strings.Contains(m.confirm.question, "Hide") {
		t.Fatalf("x on an agent didn't ask to hide it: %+v", m.confirm)
	}

	m.confirm = nil
	gone := &fleet.Agent{Key: "gone", Rush: true, DisplayName: "gone", Temp: 2 << 20}
	gone.ID = "gone"
	dir := host.TempDir(gone.ID)
	for _, d := range []string{"cache", "build"} {
		if err := os.MkdirAll(filepath.Join(dir, d), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	gone.Scratch = gone.TempDirs() // as the loader works it out
	m.snap.Agents = []*fleet.Agent{gone}
	m.pickInProjects(paneTemp)
	m.projectsKey("enter") // into Temporary, on the session
	runCmd(m, m.projectsKey("enter")) // opens it, listed in the background
	page :=ansi.Strip(strings.Join(m.projectsBody(), "\n"))
	if !strings.Contains(page, "cache/") || !strings.Contains(page, "build/") || !strings.Contains(page, "ended") {
		t.Fatalf("opened temp work doesn't list its things or when it ended:\n%s", page)
	}
	m.projectsKey("down")
	m.projectsKey("x")
	if m.confirm == nil || !strings.Contains(m.confirm.detail, "the rest of it stays") {
		t.Fatalf("x on one thing didn't ask to delete just it: %+v", m.confirm)
	}
}
