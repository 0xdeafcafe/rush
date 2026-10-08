package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Zen is only the agent that needs you: no header, no Session strip, only
// the keys to leave, and the screen still exactly filled.
func TestZenHidesTheUI(t *testing.T) {
	m, _ := benchModel(160, 40)
	for _, a := range m.snap.Agents {
		a.State, a.Needs = "done", ""
	}
	m.snap.Agents[0].State, m.snap.Agents[0].Needs = "blocked", "which way?"
	m.setZen(true)
	frame := ansi.Strip(m.listView())
	if rows := strings.Count(frame, "\n") + 1; rows != m.h {
		t.Fatalf("zen should fill the screen exactly, got %d rows of %d", rows, m.h)
	}
	for _, ui := range []string{"Settings", "ctrl+n", "find in chat", "back to the list", "views"} {
		if strings.Contains(frame, ui) {
			t.Errorf("zen should hide %q:\n%s", ui, frame)
		}
	}
	if !strings.Contains(frame, "ctrl+z leave zen") {
		t.Errorf("zen should say how to leave it:\n%s", frame)
	}
	if !strings.Contains(frame, "needs you") || !strings.Contains(frame, "halfway through typing") {
		t.Fatalf("zen should still show the agent and its box:\n%s", frame)
	}

	for _, a := range m.snap.Agents {
		a.State, a.Needs = "done", ""
	}
	frame = ansi.Strip(m.listView())
	if !strings.Contains(frame, "answered") || !strings.Contains(frame, "halfway through typing") {
		t.Fatalf("an answered agent should stay on screen until it's marked done:\n%s", frame)
	}
	m.snap.Agents[0].Done = true
	frame = ansi.Strip(m.listView())
	if !strings.Contains(frame, "nothing needs you") || !strings.Contains(frame, "ctrl+z leave zen") {
		t.Fatalf("zen with nothing waiting should only say so:\n%s", frame)
	}
}

// In zen, tab held peeks at what's working and letting go comes back; a
// tap peeks until the next key, which goes no further.
func TestZenPeek(t *testing.T) {
	m, _ := benchModel(160, 40)
	for _, a := range m.snap.Agents {
		a.State, a.Needs = "done", ""
	}
	m.snap.Agents[0].State, m.snap.Agents[0].Needs = "blocked", "which way?"
	m.snap.Agents[1].State = "working"
	m.setZen(true)
	tab := tea.KeyPressMsg{Code: tea.KeyTab}

	m.key(tab)
	frame := ansi.Strip(m.listView())
	if !m.peek.on || !strings.Contains(frame, "what's working") || !strings.Contains(frame, "tab again") {
		t.Fatalf("tab in zen should peek at what's working:\n%s", frame)
	}
	m.key(tab) // the keyboard's repeat: still held
	if !m.peek.on || !m.peek.held {
		t.Fatalf("a repeat should keep the peek open, held")
	}
	m.peek.at = time.Now().Add(-time.Second) // the repeats stopped
	m.peekCheck()
	if m.peek.on {
		t.Fatalf("letting go of tab should end the peek")
	}

	m.key(tab)
	m.peek.at = time.Now().Add(-2 * time.Second) // a tap, then a while
	before := string(m.host.input)
	m.key(tea.KeyPressMsg{Code: 'x', Text: "x"})
	if m.peek.on || string(m.host.input) != before {
		t.Fatalf("a key after a tapped peek should only end it (peek %v, input %q)", m.peek.on, string(m.host.input))
	}
}

// A question asked in zen is on screen, once: its keys aren't swallowed.
func TestZenShowsAQuestion(t *testing.T) {
	m, _ := benchModel(160, 40)
	m.snap.Agents[0].State, m.snap.Agents[0].Needs = "blocked", "which way?"
	m.setZen(true)
	m.host = &hostConn{kind: "claude", key: m.selected().Key, client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m.confirm = &confirmation{question: "Stop the agent?"}
	frame := ansi.Strip(m.render())
	if n := strings.Count(frame, "Stop the agent?"); n != 1 {
		t.Fatalf("the question is on screen %d times:\n%s", n, frame)
	}
	if rows := strings.Count(frame, "\n") + 1; rows != m.h {
		t.Fatalf("zen should still fill the screen exactly, got %d rows of %d", rows, m.h)
	}
}
