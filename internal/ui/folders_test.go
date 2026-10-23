package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Split by project, each status section keeps its place and its rows sit
// together by project, a worktree's agents under the repository it came
// from, after the main checkout's, headed by the worktree; each project's
// line says what git says. Off, the sections are as they were.
func TestProjectSplit(t *testing.T) {
	now := time.Now()
	st := &state.Store{}
	st.Config.GroupBy, st.Config.ListLook = "status", "classic" // the rail look has no project headings
	m := &Model{store: st, snap: &fleet.Snapshot{At: now}}
	add := func(name, repo, root, cwd, state string, age time.Duration) {
		a := &fleet.Agent{Key: name, DisplayName: name, Repo: repo, Root: root}
		a.Cwd, a.State, a.UpdatedAt = cwd, state, now.Add(-age)
		if state == "working" || state == "blocked" {
			a.PID = 1
		}
		m.snap.Agents = append(m.snap.Agents, a)
	}
	const app, wt = "/src/app", "/src/app/.claude/worktrees/fix"
	add("zed", "/src/zed", "/src/zed", "/src/zed", "stopped", time.Hour)
	add("in-tree", wt, app, wt, "working", time.Minute)
	add("on-main", app, app, app+"/web", "working", time.Minute)
	add("scratchy", "", "", "/tmp/x", "stopped", time.Hour)
	add("old", app, app, app, "stopped", 48*time.Hour)
	add("done-here", app, app, app, "stopped", 2*time.Hour)
	lines := func() string {
		m.rebuild()
		var got []string
		for _, l := range m.lines {
			switch l.kind {
			case lineSection:
				got = append(got, "§"+l.title)
			case lineProject:
				got = append(got, "▪"+l.title)
			case lineTree:
				got = append(got, "⎇"+l.root)
			case lineAgent:
				got = append(got, l.agent.Key)
			}
		}
		return strings.Join(got, " ")
	}
	want := "§Working ▪app on-main ⎇" + wt + " in-tree §Just left zed §Today done-here §Earlier §Scratch" // what finished goes by time, no headings
	if s := lines(); s != want {
		t.Fatalf("split:\n got %s\nwant %s", s, want)
	}

	m.folders.byRoot = map[string]fleet.Folder{app: {
		Root: app, Worktrees: 3,
		Git:   fleet.GitState{Branch: "main", Upstream: true, Ahead: 2, Changed: 4, Commit: "abc1234", Target: "origin/release"},
		Trees: map[string]fleet.GitState{wt: {Branch: "fix", Changed: 1}},
	}}
	m.snap.Agents[2].PRs = []agent.PR{{Number: 12, State: "OPEN"}} // on-main's, named once on its heading
	out := ansi.Strip(strings.Join(m.listLines(140, 40), "\n"))
	if n := strings.Count(out, "#12 open"); n != 1 || strings.Count(out, "#12") != n {
		t.Errorf("the PR belongs on its project's headings, not its row:\n%s", out)
	}
	for _, s := range []string{"app  main ↑2  #12 open  → origin/release  abc1234  ±4  3 worktrees", "↳ fix  ±1"} {
		if !strings.Contains(out, s) {
			t.Errorf("list lacks %q:\n%s", s, out)
		}
	}
	if strings.Count(out, "main ↑2") != 1 {
		t.Errorf("git should head the project once, in Working; Today has no headings:\n%s", out)
	}

	st.Config.SplitBy = "none"
	// Today's rows go by when each stopped, the latest first.
	if s, want := lines(), "§Working in-tree on-main §Just left zed §Today done-here §Earlier §Scratch"; s != want {
		t.Fatalf("unsplit:\n got %s\nwant %s", s, want)
	}
}

// Two folders of one name are told apart by the folders they're in.
func TestFolderTitles(t *testing.T) {
	got := folderTitles(map[string]bool{"/src/lw/langwatch": true, "/tmp/w/langwatch": true, "/src/rush": true, scratchSection: true})
	want := map[string]string{"/src/lw/langwatch": "lw/langwatch", "/tmp/w/langwatch": "w/langwatch", "/src/rush": "rush", scratchSection: scratchSection}
	for k, v := range want {
		if got[k] != v {
			t.Errorf("%s: %q, want %q", k, got[k], v)
		}
	}
}
