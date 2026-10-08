package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
)

// footModel is a Session beside a list short enough to leave room.
func footModel(t *testing.T, n int) *Model {
	t.Helper()
	m, _ := benchModel(140, 50)
	m.snap.Agents = m.snap.Agents[:n]
	m.rebuild()
	m.View()
	return m
}

func TestFooterShownOnlyWithRoom(t *testing.T) {
	if v := ansi.Strip(footModel(t, 3).View().Content); !strings.Contains(v, "next agent") {
		t.Fatalf("no footer with room to spare:\n%s", v)
	}
	if v := ansi.Strip(footModel(t, 30).View().Content); strings.Contains(v, "next agent") {
		t.Fatalf("footer drawn on a full list:\n%s", v)
	}
}

func TestRecentSetupsDedupedWithoutCurrent(t *testing.T) {
	m := footModel(t, 6)
	for i, a := range m.snap.Agents {
		a.Kind = "claude"
		a.Spend.Model = []string{"", "claude-sonnet-4-5", "claude-sonnet-4-5", "claude-haiku-4-5", "", "claude-sonnet-4-5"}[i]
	}
	m.host = nil // the Session's own setup isn't read from its host here
	cur := m.nextStart(m.startDir())
	got := m.recentSetups(cur)
	seen := map[string]bool{strings.Join(m.startWords(cur), " · "): true}
	for _, o := range got {
		w := strings.Join(m.startWords(o), " · ")
		if seen[w] {
			t.Fatalf("%q twice or the current setup, in %v", w, got)
		}
		seen[w] = true
	}
	if len(got) != 2 || got[0].model != "claude-sonnet-4-5" || got[1].model != "claude-haiku-4-5" {
		t.Fatalf("recent setups = %+v, want sonnet then haiku", got)
	}
}

func TestFooterClickSetsNextSetup(t *testing.T) {
	m := footModel(t, 3)
	m.snap.Agents[1].Kind, m.snap.Agents[1].Spend.Model = "claude", "claude-haiku-4-5"
	m.View()
	if len(m.footHits) == 0 {
		t.Fatal("no recent setups on the footer")
	}
	y := -1
	for i, k := range m.rowKeys {
		if k == footRecentKey {
			y = i + m.listTop
		}
	}
	if y < 0 {
		t.Fatal("no row keyed for the recent setups")
	}
	h := m.footHits[0]
	m.mouseClick(h.x0, y)
	if m.startOver == nil || m.startOver.model != h.o.model {
		t.Fatalf("next setup = %+v, want %+v", m.startOver, h.o)
	}
}

// The setup line names what runs it and who pays; narrow, the account
// outlasts the provider. A recent setup's chip covers its drawn cells.
func TestFooterWhoAndChips(t *testing.T) {
	m := footModel(t, 3)
	o := startOver{kind: "claude", account: "personal"}
	wide := ansi.Strip(m.footWho(o, 60))
	if !strings.Contains(wide, "Claude Code") || !strings.Contains(wide, "personal") || !strings.Contains(wide, "on ") {
		t.Fatalf("who = %q", wide)
	}
	if narrow := ansi.Strip(m.footWho(o, 26)); !strings.Contains(narrow, "personal") {
		t.Fatalf("narrow, the account should stay: %q", narrow)
	}
	if p := ansi.Strip(m.footWho(startOver{kind: "claude", profile: "work"}, 60)); !strings.Contains(p, "work") {
		t.Fatalf("a profile should be named: %q", p)
	}
	m.snap.Agents[1].Kind, m.snap.Agents[1].Spend.Model = "claude", "claude-haiku-4-5"
	lines, keys := m.footLines(60)
	for i, k := range keys {
		if k != footRecentKey {
			continue
		}
		for _, h := range m.footHits {
			chip, _ := footChip(h.o, m.nextStart(m.startDir()))
			if got := ansi.Cut(lines[i], h.x0, h.x1); ansi.Strip(got) != ansi.Strip(chip) {
				t.Fatalf("chip at %d-%d is %q, drawn %q", h.x0, h.x1, ansi.Strip(chip), ansi.Strip(got))
			}
		}
		return
	}
	t.Fatal("no recent setups drawn")
}
