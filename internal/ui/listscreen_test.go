package ui

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

// manyModel is a list far longer than the screen, in the look given.
func manyModel(look string, n int) *Model {
	now := time.Now()
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 30, lastState: map[string]string{}}
	m.store.Config.ListLook = look
	snap := &fleet.Snapshot{At: now}
	for i := range n {
		a := &fleet.Agent{Key: fmt.Sprintf("default/m:%d", i), DisplayName: fmt.Sprintf("task %03d", i), Root: "/src/x", Repo: "/src/x", Branch: "main"}
		a.UpdatedAt, a.State = now.Add(-time.Duration(i)*time.Minute), []string{"working", "blocked", "done"}[i%3]
		if a.State != "done" {
			a.PID = 10 + i
		}
		snap.Agents = append(snap.Agents, a)
	}
	m.snap = snap
	m.rebuild()
	return m
}

// The list draws only what's on screen: what it shows is what the whole
// list drawn would show there, the selected row scrolled into sight.
func TestListDrawsOnScreenRows(t *testing.T) {
	for _, look := range []string{"", "classic"} {
		m := manyModel(look, 80)
		whole := m.listLines(110, 10000)
		m.scroll = 0
		part := m.listLines(110, 20)
		for i, l := range part[:len(part)-1] {
			if l != whole[i] {
				t.Fatalf("%q: row %d is %q, want %q", look, i, ansi.Strip(l), ansi.Strip(whole[i]))
			}
		}
		if last := ansi.Strip(part[len(part)-1]); !strings.Contains(last, fmt.Sprintf("↓ %d more lines", len(whole)-20)) {
			t.Fatalf("%q: last row %q", look, last)
		}
		k := m.order[len(m.order)-1].Key
		m.sel = k
		part = m.listLines(110, 20)
		at := -1
		for i, rk := range m.rowKeys {
			if rk == k {
				at = i
				break
			}
		}
		if at < 0 || !strings.Contains(ansi.Strip(part[at]), m.order[len(m.order)-1].DisplayName) {
			t.Fatalf("%q: the selected last agent isn't on screen: %q", look, ansi.Strip(strings.Join(part, "\n")))
		}
	}
}
