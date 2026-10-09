package ui

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The two sessions of the day it went wrong: a langwatch coordinator and a
// yap session whose newest commit was its History window.
func misdirectFleet() (lw, yap *fleet.Agent, subjects map[string][]string) {
	lw = &fleet.Agent{Key: "lw", DisplayName: "coordinator", Repo: "langwatch", Root: "/src/langwatch", Branch: "feat/strict-feature-layout-v0", Rush: true,
		Name: "coordinator", Intent: "collect lanes and commit slices", TodoItems: []agent.Todo{{Label: "Collect dashboard lane"}, {Label: "Run lint on identity module"}}}
	yap = &fleet.Agent{Key: "yap", DisplayName: "yap", Repo: "yap", Root: "/src/yap", Branch: "main", Rush: true,
		Name: "dictation app", Intent: "build the dictation app"}
	subjects = map[string][]string{
		"/src/langwatch": {"fix(dashboard): an invalid widget definition is a 422 refusal", "refactor(identity): auth commands go through AuthApi"},
		"/src/yap":       {"feat: History window lists past dictations", "feat(macos): menu bar icon"},
	}
	return lw, yap, subjects
}

func TestMisdirectedRoutesToTheBetterMatch(t *testing.T) {
	lw, yap, subjects := misdirectFleet()
	for _, text := range []string{
		"this history needs to use macos liquid glass ui!!",
		"in yap, make the window wider",
	} {
		to, words := misdirected(text, lw, []*fleet.Agent{yap}, subjects)
		if to != yap {
			t.Fatalf("%q should look meant for yap, got %v", text, to)
		}
		if len(words) == 0 {
			t.Fatalf("%q: no words named as the reason", text)
		}
	}
}

func TestMisdirectedLeavesOrdinaryMessagesAlone(t *testing.T) {
	lw, yap, subjects := misdirectFleet()
	for _, text := range []string{
		"keep going",
		"commit",
		"yes",
		"looks good, ship it",
		"fix the failing test",
		"why did the dashboard widget refuse with a 422?",
		"use astra for all ui stuff",
		"",
	} {
		if to, _ := misdirected(text, lw, []*fleet.Agent{yap}, subjects); to != nil {
			t.Errorf("%q shouldn't fire, went to %s", text, to.DisplayName)
		}
	}
	// A message as much about here as there stays here.
	if to, _ := misdirected("the dashboard history window", lw, []*fleet.Agent{yap}, subjects); to != nil {
		t.Error("a message that matches here too shouldn't fire")
	}
	// Two sessions as likely as each other: nothing stands out.
	yap2 := *yap
	yap2.Key = "yap2"
	if to, _ := misdirected("in yap, make the window wider", lw, []*fleet.Agent{yap, &yap2}, subjects); to != nil {
		t.Error("a tie shouldn't pick one")
	}
}

func TestAskMisdirectedKeys(t *testing.T) {
	lw, yap, subjects := misdirectFleet()
	newModel := func() (*Model, *hostConn) {
		m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{lw, yap}}, loader: &fleet.Loader{Temp: fleet.NewTempSizes()}, store: &state.Store{}}
		m.misdirect.subjects = subjects
		c := &hostConn{kind: "claude", key: "lw", sess: convo.New(), open: map[string]bool{}}
		c.input = []rune("this history needs to use macos liquid glass ui!!")
		return m, c
	}
	text := "this history needs to use macos liquid glass ui!!"

	m, c := newModel()
	sent := 0
	send := func() tea.Cmd { sent++; return nil }
	if m.askMisdirected(c, "keep going", send) || m.confirm != nil {
		t.Fatal("an ordinary message shouldn't ask")
	}
	if !m.askMisdirected(c, text, send) || m.confirm == nil {
		t.Fatal("the yap message should ask")
	}
	// esc: nothing sent, the box keeps the text to edit.
	m.confirmKey("esc")
	if m.confirm != nil || sent != 0 || len(c.input) == 0 {
		t.Fatalf("esc should leave it in the box: confirm=%v sent=%d input=%q", m.confirm, sent, string(c.input))
	}

	// s: sent here, and the send it retries isn't asked about again.
	m.askMisdirected(c, text, func() tea.Cmd {
		if m.askMisdirected(c, text, send) {
			t.Fatal("send here anyway shouldn't ask again")
		}
		return send()
	})
	m.confirmKey("s")
	if sent != 1 {
		t.Fatalf("s should send here, sent=%d", sent)
	}
	if !m.askMisdirected(c, text, send) {
		t.Fatal("the next send should be checked again")
	}

	// enter: forwarded to yap, the box emptied, nothing sent here.
	m, c = newModel()
	sent = 0
	m.askMisdirected(c, text, send)
	if cmd := m.confirmKey("enter"); cmd == nil {
		t.Fatal("enter should forward to yap")
	}
	if sent != 0 || len(c.input) != 0 || m.confirm != nil {
		t.Fatalf("enter should send there, not here: sent=%d input=%q", sent, string(c.input))
	}

	// space says yes too; the question sits on the pane's box with its keys.
	m, c = newModel()
	m.askMisdirected(c, text, send)
	if m.confirm.box != c {
		t.Fatal("the question should be asked on the pane's own box")
	}
	line := ansi.Strip(m.confirm.boxLine())
	for _, want := range []string{"Looks meant for yap (yap)", "enter send there", "s send here anyway", "esc edit"} {
		if !strings.Contains(line, want) {
			t.Errorf("box line %q lacks %q", line, want)
		}
	}
	if cmd := m.confirmKey("space"); cmd == nil || len(c.input) != 0 {
		t.Fatal("space should forward to yap")
	}
}
