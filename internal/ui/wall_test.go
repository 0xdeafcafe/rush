package ui

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

// wallModel is a fleet of n agents in every state, each with something
// said and run, on a w×h screen showing the Wall.
func wallModel(n, w, h int) *Model {
	now := time.Now()
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: w, h: h, lastState: map[string]string{}}
	states := []string{"working", "blocked", "working", "done", "working", "idle"}
	var agents []*fleet.Agent
	for i := 0; i < n; i++ {
		a := &fleet.Agent{Key: fmt.Sprintf("k%d", i), DisplayName: fmt.Sprintf("agent number %d fixing the auth flow", i), PID: 100 + i, Repo: "/src/langwatch", Branch: "feat/wall"}
		a.ID, a.State, a.CreatedAt, a.UpdatedAt = fmt.Sprintf("id%d", i), states[i%len(states)], now.Add(-time.Duration(i)*time.Minute), now.Add(-time.Minute)
		a.Todos, a.TodosDone, a.Spend.Cost = 5, i%6, 1.5*float64(i)
		if a.State == "blocked" {
			a.Needs = "Which database should the migration target?"
		}
		agents = append(agents, a)
		m.previews[a.Key] = previewEntry{p: claude.Preview{
			Model: "claude-opus-4-5", Context: int64(90_000 * (i + 1)), At: now.Add(-time.Second), Doing: "running pnpm test",
			Recent: []agent.Line{
				{Role: "user", Text: "make the login page stop flickering when the token refreshes"},
				{Role: "assistant", Text: "Looking at how the **token** refresh is wired into the session provider first."},
				{Role: "tool", Text: "Read\x00src/auth/session.tsx"},
				{Role: "tool", Text: "Grep\x00refreshToken"},
				{Role: "assistant", Text: "The provider re-mounts on every refresh because the key changes. I'll memoise the context value and keep the key stable, then run the tests."},
				{Role: "tool", Text: "Edit\x00src/auth/session.tsx"},
				{Role: "tool", Text: "Bash\x00pnpm test --filter auth"},
			},
		}}
	}
	m.snap = &fleet.Snapshot{At: now, Agents: agents}
	m.setView(placeAgents)
	m.setAgentsPage(agentsWall)
	return m
}

func TestWallFillsTheScreen(t *testing.T) {
	for _, tc := range []struct{ n, w, h int }{{1, 120, 40}, {3, 200, 50}, {7, 160, 45}, {14, 180, 40}, {30, 100, 30}, {2, 50, 20}} {
		m := wallModel(tc.n, tc.w, tc.h)
		lines := strings.Split(m.View().Content, "\n")
		if len(lines) != tc.h {
			t.Errorf("%v: %d lines, want %d", tc, len(lines), tc.h)
		}
		for i, l := range lines {
			if cw := cellw.String(l); cw > tc.w {
				t.Errorf("%v: line %d is %d wide: %q", tc, i, cw, ansi.Strip(l))
			}
		}
		if len(m.wall.tiles) == 0 {
			t.Errorf("%v: no tiles drawn", tc)
		}
		if os.Getenv("WALL_SHOW") != "" {
			fmt.Println(m.View().Content)
		}
	}
}

func TestWallOrderAndKeys(t *testing.T) {
	m := wallModel(6, 160, 45)
	agents := m.wallAgents()
	if agents[0].State != "blocked" {
		t.Fatalf("first tile %s, want the one that needs you", agents[0].State)
	}
	cols, _, _ := wallGrid(wallGroups(m.wallItems()), m.w-4, m.wallH())
	m.wallKey("down")
	if m.wall.sel != agents[min(cols, len(agents)-1)].Key {
		t.Fatalf("down went to %s", m.wall.sel)
	}
	m.wallKey("left")
	if m.wall.sel != agents[cols-1].Key {
		t.Fatalf("left went to %s", m.wall.sel)
	}
	m.wallKey("esc")
	if m.view != placeAgents {
		t.Fatal("esc didn't go back to Agents")
	}
}

func TestWallGrid(t *testing.T) {
	for _, tc := range []struct{ n, w, h, cols int }{{1, 200, 40, 1}, {2, 200, 40, 2}, {4, 200, 40, 2}, {9, 200, 40, 3}} {
		c, r, th := wallGrid(ones(tc.n), tc.w, tc.h)
		if c != tc.cols || th < wallMinH || c*r < tc.n {
			t.Errorf("%v: %d cols × %d rows, %d tall", tc, c, r, th)
		}
	}
	// Too many to fit pages them rather than squashing.
	if _, r, th := wallGrid(ones(40), 120, 30); th < wallMinH || r*wallMinH > 30 {
		t.Errorf("40 tiles: %d rows %d tall", r, th)
	}
}

func ones(n int) []int {
	gs := make([]int, n)
	for i := range gs {
		gs[i] = 1
	}
	return gs
}

// A group that fits a row sits side by side, joined, starting a row of its
// own when what's left is too little; a bigger one has a row to itself,
// the agent down the first column and its subagents stacked in the rest.
func TestWallLayout(t *testing.T) {
	shape := func(rows []wallRow) string {
		var out []string
		for _, r := range rows {
			var cells []string
			for _, c := range r.cells {
				cells = append(cells, fmt.Sprintf("%d:c%d/%d/%d", c.item, c.col, c.k, c.n))
			}
			out = append(out, strings.Join(cells, " ")+fmt.Sprint(r.joined))
		}
		return strings.Join(out, " | ")
	}
	// 1 alone, then a group of 3 that doesn't fit what's left, then 1.
	got := shape(wallLayout([]int{1, 3, 1}, 3, 2))
	want := "0:c0/0/1[false false false] | 1:c0/0/1 2:c1/0/1 3:c2/0/1[true true false] | 4:c0/0/1[false false false]"
	if got != want {
		t.Errorf("side by side:\n got %s\nwant %s", got, want)
	}
	// An agent with 5 subagents on 3 columns, 3 stacked at most: 3 in one
	// column, 2 in the other, all on the agent's row.
	got = shape(wallLayout([]int{6}, 3, 3))
	want = "0:c0/0/1 1:c1/0/3 2:c1/1/3 3:c1/2/3 4:c2/0/2 5:c2/1/2[true true false]"
	if got != want {
		t.Errorf("stacked:\n got %s\nwant %s", got, want)
	}
	// Too many to stack: the rest go on in the row under, beside nothing.
	got = shape(wallLayout([]int{6}, 3, 1))
	want = "0:c0/0/1 1:c1/0/1 2:c2/0/1[true true false] | 3:c1/0/1 4:c2/0/1[false true false] | 5:c1/0/1[false false false]"
	if got != want {
		t.Errorf("spilling:\n got %s\nwant %s", got, want)
	}
}

// Up, down and across go by where tiles are: from a stacked subagent up
// to the one above it, across to the agent beside.
func TestWallStep(t *testing.T) {
	geo := wallGeos(wallLayout([]int{4, 1}, 2, 3), 5)
	if j := wallStep(geo, 2, 0, -1); j != 1 {
		t.Errorf("up from the second stacked subagent: %d", j)
	}
	if j := wallStep(geo, 2, -1, 0); j != 0 {
		t.Errorf("left from a subagent: %d, want its agent", j)
	}
	if j := wallStep(geo, 0, 1, 0); j != 1 {
		t.Errorf("right from the agent: %d, want its top subagent", j)
	}
	if j := wallStep(geo, 3, 0, 1); j != 4 {
		t.Errorf("down from the last subagent: %d", j)
	}
}

func TestWallKeepsSubagentsWithTheirAgent(t *testing.T) {
	m := wallModel(4, 200, 50)
	agents := m.wallAgents()
	agents[1].Subagents = []fleet.SubagentTile{{ID: "s1", Description: "a"}, {ID: "s2", Description: "b"}}
	m.View()
	byKey := map[string]wallTile{}
	for _, t := range m.wall.tiles {
		byKey[t.key] = t
	}
	p, s := byKey[agents[1].Key], byKey[agents[1].Key+"\x00s2"]
	if p.y != s.y {
		t.Errorf("subagent on row %d, its agent on %d", s.y, p.y)
	}
}
