package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func TestParseAway(t *testing.T) {
	now := time.Now()
	a, err := parseAway("3h every 15m", false, now)
	if err != nil || a.Until != now.Add(3*time.Hour) || a.Every != 15*time.Minute || a.Loop {
		t.Fatal(a, err)
	}
	if a, err = parseAway("90m 10m", false, now); err != nil || a.Every != 10*time.Minute {
		t.Fatal(a, err)
	}
	if a, err = parseAway("", true, now); err != nil || !a.Until.IsZero() || a.Every != 30*time.Minute || !a.Loop {
		t.Fatal("a loop needs no time", a, err)
	}
	if a, err = parseAway("every 15m", true, now); err != nil || !a.Until.IsZero() || a.Every != 15*time.Minute {
		t.Fatal(a, err)
	}
	for _, bad := range []string{"", "soon", "3h 10s", "-1h", "every 15m", "3h every"} {
		if _, err := parseAway(bad, false, now); err == nil {
			t.Fatal(bad)
		}
	}
}

// An away that ended while you were gone opens the back sheet once you're
// in its session, once; enter puts what it saved in the box as one reply.
func TestBackFromAway(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}}
	away := &host.Away{From: now.Add(-3 * time.Hour), Until: now, Ended: now, Nudges: 5,
		Held: []host.Held{{Kind: "question", Text: "Which DB?"}, {Kind: "permission", Text: "Bash rm -rf build"}}}
	c := &hostConn{key: "k", client: &host.Client{}, sess: &convo.Session{Info: host.Info{Proto: host.Proto, State: "idle", Detail: "All done: shipped", Away: away}}}
	m.host = c
	m.backFromAway(c)
	s, ok := m.sheet.(*backSheet)
	if !ok {
		t.Fatal("no back sheet")
	}
	if body := strings.Join(s.body(m, 100, 30), "\n"); !strings.Contains(body, "away 3h00m · 5 check-ins") || !strings.Contains(body, "Which DB?") || !strings.Contains(body, "All done: shipped") {
		t.Fatal(body)
	}
	s.key(m, tea.KeyPressMsg{}, "esc")
	m.backFromAway(c)
	if m.sheet != nil {
		t.Fatal("kept, it opened again")
	}
	m.sheet = s
	if cmd := s.key(m, tea.KeyPressMsg{}, "enter"); cmd == nil || m.sheet != nil {
		t.Fatal("enter didn't end it")
	}
	if in := string(c.input); !strings.Contains(in, "- Which DB? → ") || !strings.Contains(in, "- you asked to Bash rm -rf build → ") {
		t.Fatal(in)
	}

	// #away alone, or #back, while still away opens it too.
	m.sheet, c.sess.Info.Away = nil, &host.Away{From: now, Until: now.Add(time.Hour)}
	if m.awayCommand(c, "away", ""); m.sheet == nil {
		t.Fatal("#away alone while away didn't open the back sheet")
	}
}

func TestAwayChip(t *testing.T) {
	now := time.Now()
	a := &host.Away{From: now, Until: now.Add(time.Hour), Next: now.Add(10 * time.Minute), Held: []host.Held{{}}}
	if c := ansi.Strip(awayChip(a, now, false)); c != "✈ away 1h00m" {
		t.Fatal(c)
	}
	if c := ansi.Strip(awayChip(a, now, true)); !strings.Contains(c, "next check-in 10m · 0 sent · 1 saved") {
		t.Fatal(c)
	}
	if c := ansi.Strip(awayChip(&host.Away{Loop: true, Next: now}, now, false)); c != "⟳ loop" {
		t.Fatal(c)
	}
	a.Ended = now
	if c := ansi.Strip(awayChip(a, now, false)); !strings.HasPrefix(c, "✈ back · 1 saved") {
		t.Fatal(c)
	}
}
