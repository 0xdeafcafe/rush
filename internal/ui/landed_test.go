package ui

import (
	"fmt"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The tidy-up tries only rush's own worktrees whose sessions are all due,
// and keeps each repository's newest three finished ones however old.
func TestTidyable(t *testing.T) {
	now := time.Now()
	due, finished := map[string]bool{}, map[string]time.Time{}
	wts := make([]fleet.Worktree, 0, 6)
	for i := range 5 {
		k := fmt.Sprintf("s%d", i)
		due[k], finished[k] = true, now.Add(-time.Duration(i)*time.Hour)
		wts = append(wts, fleet.Worktree{Path: "/r/.claude/worktrees/" + k, Repo: "/r", Own: true, Agents: []string{k}})
	}
	due["x"], finished["x"] = true, now.Add(-9*time.Hour)
	wts = append(wts, fleet.Worktree{Path: "/r/.claude/worktrees/agent-x", Repo: "/r", Agents: []string{"x"}}) // not rush's
	got := tidyable(wts, due, finished, nil)
	if len(got) != 2 || got[0].Path != "/r/.claude/worktrees/s3" || got[1].Path != "/r/.claude/worktrees/s4" {
		t.Fatalf("tidyable %+v", got)
	}
}

// A session whose work landed is marked done, as ctrl+d would, once: moved
// back, it stays back.
func TestLandedMarksDone(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	st := &state.Store{}
	st.Overlay.Done = map[string]time.Time{}
	m := &Model{store: st, snap: &fleet.Snapshot{At: time.Now()}}
	m.onLanded(landedMsg{done: map[string]string{"a": "pull request #9 merged"}, did: []string{"x: removed abc"}, stays: map[string]string{}})
	if _, ok := st.Overlay.Done["a"]; !ok || !m.clean.settled["a"] {
		t.Fatal("not marked done")
	}
	delete(st.Overlay.Done, "a")
	m.onLanded(landedMsg{done: map[string]string{"a": "pull request #9 merged"}, stays: map[string]string{}})
	if _, ok := st.Overlay.Done["a"]; ok {
		t.Fatal("marked done again after it was moved back")
	}
}

// One whose pull request merged while it still asks you something isn't
// put away: the question would sit unseen in Done.
func TestLandedSkipsAQuestion(t *testing.T) {
	a := &fleet.Agent{Key: "a", State: "blocked", Decides: true, PR: &fleet.PR{State: "merged", HeadOid: "abc"}, Root: "/r", Repo: "/r/.claude/worktrees/a"}
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: time.Now(), Agents: []*fleet.Agent{a}}, tick: 15}
	if m.settleLanded() != nil {
		t.Fatal("an agent with an open question was looked at to be marked done")
	}
	a.State = "done"
	if m.settleLanded() == nil {
		t.Fatal("once answered, it isn't looked at")
	}
}

// A worktree of rush's whose session is gone goes too, first of all, and
// one of a session still known stays till its session is due.
func TestTidyableTakesTheGoneSessions(t *testing.T) {
	wts := []fleet.Worktree{
		{Path: "/r/.claude/worktrees/gone", Repo: "/r", Own: true},
		{Path: "/r/.claude/worktrees/kept", Repo: "/r", Own: true},
		{Path: "/r/.worktrees/mine", Repo: "/r"}, // not rush's: by hand only
	}
	gone := func(w *fleet.Worktree) bool { return w.Path != "/r/.claude/worktrees/kept" }
	// Three finished ones newer than it, so it's past those kept.
	now := time.Now()
	due, finished := map[string]bool{}, map[string]time.Time{}
	for i := range 3 {
		k := fmt.Sprintf("n%d", i)
		due[k], finished[k] = true, now
		wts = append(wts, fleet.Worktree{Path: "/r/.claude/worktrees/" + k, Repo: "/r", Own: true, Agents: []string{k}})
	}
	got := tidyable(wts, due, finished, gone)
	if len(got) != 1 || got[0].Path != "/r/.claude/worktrees/gone" {
		t.Fatalf("tidyable %+v", got)
	}
}
