package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/convo"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/room"
)

// #discuss sets up a room on the chat, its latest exchanges as context,
// and the verdict comes back as a card above the chat's box.
func TestDiscussFromChat(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(200, 45)
	c := m.host
	c.input = nil
	m.discuss(c, "", false)
	s, ok := m.sheet.(*roomSetup)
	if !ok || s.from != c.key || s.ctx != 3 || len(s.turns) < 3 || string(s.topic) == "" {
		t.Fatalf("set-up: %+v", m.sheet)
	}
	if b := s.brief(); strings.Count(b, "\n---\n") != 2 || !strings.Contains(b, "User: ") {
		t.Fatalf("brief:\n%s", b)
	}
	m.sheet = nil
	sum := room.Summary{Room: room.Room{ID: "r1", Topic: "the cache", From: c.key}, Verdict: true, Final: "Haiku: safe\nLuna: safe"}
	m.verdictsBack([]room.Summary{sum})
	if len(c.input) != 0 {
		t.Fatalf("the verdict went straight in the box: %q", string(c.input))
	}
	dock := ansi.Strip(strings.Join(m.paneDock(m.agentByKey(c.key), c, 200, 45), "\n"))
	for _, want := range []string{"has a verdict", "asked: the cache", "Luna: safe", "enter send it to this agent", "e edit it first", "esc drop it"} {
		if !strings.Contains(dock, want) {
			t.Fatalf("no %q in the dock:\n%s", want, dock)
		}
	}
	m.paneKey(tea.KeyPressMsg{Code: 'e', Text: "e"}, "e")
	if got := string(c.input); !strings.Contains(got, "verdict on the cache") || !strings.Contains(got, "Luna: safe") || m.verdicts[c.key].ID != "" {
		t.Fatalf("e: box %q, card %v", got, m.verdicts[c.key].ID)
	}
	c.input = nil
	m.verdictsBack([]room.Summary{sum})
	if m.verdicts[c.key].ID != "" {
		t.Fatal("a verdict came back twice")
	}

	// esc drops it; enter sends it on.
	m.verdicts = map[string]room.Summary{c.key: sum}
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyEscape}, "esc")
	if _, ok := m.verdicts[c.key]; ok || len(c.input) != 0 {
		t.Fatalf("esc: card kept or box %q", string(c.input))
	}
	m.verdicts = map[string]room.Summary{c.key: sum}
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyEnter}, "enter")
	if _, ok := m.verdicts[c.key]; ok || len(c.input) != 0 {
		t.Fatalf("enter: card kept or box not sent %q", string(c.input))
	}
}

// A room running on the chat shows above its box, a row a member, and
// enter on it opens the room.
func TestChatRoomPanel(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(200, 45)
	c := m.host
	c.input = nil
	r := room.Room{ID: "r2", Topic: "the cache", From: c.key, Members: []room.Member{{Name: "Haiku"}, {Name: "Luna"}}}
	m.rooms.list = []room.Summary{
		{Room: room.Room{ID: "other", Topic: "elsewhere", From: "nobody"}},
		{Room: r, Round: 2, Speaking: []string{"Luna"}, Last: map[string]string{"Haiku": "it's safe"}},
	}
	dock := ansi.Strip(strings.Join(m.paneDock(m.agentByKey(c.key), c, 200, 45), "\n"))
	for _, want := range []string{"room · the cache", "round 2/4 · 2 agents", "Haiku  it's safe", "Luna"} {
		if !strings.Contains(dock, want) {
			t.Fatalf("no %q in the dock:\n%s", want, dock)
		}
	}
	if strings.Contains(dock, "elsewhere") {
		t.Fatal("another chat's room shows")
	}
	refs := 0
	for _, ref := range c.panelRefs {
		if ref == dockRoomPrefix+"r2" {
			refs++
		}
	}
	if refs != 3 {
		t.Fatalf("%d rows open the room, want 3", refs)
	}
	c.sel = dockRoomPrefix + "r2"
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyEnter}, "enter")
	if m.sel != roomKeyPrefix+"r2" || m.mode != modeList || !m.preview {
		t.Fatalf("enter didn't open the room: sel %q", m.sel)
	}
	m.rooms.list[1].Over = "done"
	if dock := ansi.Strip(strings.Join(m.paneDock(m.agentByKey(c.key), c, 200, 45), "\n")); strings.Contains(dock, "room · the cache") {
		t.Fatal("a room that's over still shows")
	}
}

// In the discuss send mode, enter opens a room's set-up on what's typed.
func TestDiscussSendMode(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(200, 45)
	c := m.host
	m.sendModes = map[string]sendMode{c.key: sendDiscuss}
	c.input = []rune("should we cache it")
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyEnter}, "enter")
	s, ok := m.sheet.(*roomSetup)
	if !ok || string(s.topic) != "should we cache it" || !s.fill || s.from != c.key || len(c.input) != 0 {
		t.Fatalf("set-up %+v, box %q", m.sheet, string(c.input))
	}
}

// #intervene goes through the chat's tool calls too, and its verdict goes
// in the box.
func TestInterveneFromChat(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(200, 45)
	c := m.host
	m.intervene(c, "visual diff stalled")
	s, ok := m.sheet.(*roomSetup)
	if !ok || !s.fill || s.from != c.key || !strings.Contains(string(s.topic), "visual diff stalled") {
		t.Fatalf("set-up: %+v", m.sheet)
	}
	if b := s.brief(); !strings.Contains(b, "Tool: ") || !strings.Contains(b, "[turn ") {
		t.Fatalf("brief has no tool calls or times:\n%s", b)
	}
}

// The panel is told where the chat stands: idle since when, and its plan.
func TestChatStatus(t *testing.T) {
	now := time.Date(2026, 10, 1, 15, 0, 0, 0, time.UTC)
	s := &convo.Session{Turns: []*convo.Turn{{N: 4, End: now.Add(-3 * time.Hour)}}, Tasks: []convo.Task{{Subject: "visual diff", Status: "in_progress"}}}
	got := chatStatus(s, now)
	if !strings.Contains(got, "idle since 12:00 (3h0m0s ago)") || !strings.Contains(got, "- [in_progress] visual diff") {
		t.Fatalf("status:\n%s", got)
	}
}

// A verdict coming back takes the keys from a picked queue row: e edits
// the verdict, not what's queued, and the queue keeps its messages.
func TestVerdictGoesBeforeQueue(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(200, 45)
	c := m.host
	c.input, c.lastKeyAt = nil, time.Time{}
	c.sess.Info.Queue = []string{"first queued", "second queued"}
	c.sel = "q:1"
	m.verdicts = map[string]room.Summary{c.key: {Room: room.Room{ID: "r9", Topic: "the plan", From: c.key}, Verdict: true, Final: "Opus: go on"}}
	m.paneDock(m.agentByKey(c.key), c, 200, 45)
	if !c.cardFocus || c.sel != "" {
		t.Fatalf("the verdict should take the keys as it comes: focus %v, sel %q", c.cardFocus, c.sel)
	}
	m.paneKey(tea.KeyPressMsg{Code: 'e', Text: "e"}, "e")
	if got := string(c.input); !strings.Contains(got, "Opus: go on") || c.editQ != 0 || len(c.sess.Info.Queue) != 2 {
		t.Fatalf("e should edit the verdict: box %q, editing queue %d, queue %v", got, c.editQ, c.sess.Info.Queue)
	}
}
