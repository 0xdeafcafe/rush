package ui

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// writeSession leaves a stopped rush-mode session behind, as a host that
// ended would.
func writeSession(t *testing.T, id, name string) {
	t.Helper()
	d := filepath.Join(host.Root(), id)
	if err := os.MkdirAll(d, 0o700); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	b, _ := jsonx.Marshal(host.Info{ID: id, SessionID: id + "-0000-4000-8000-000000000000", Cwd: t.TempDir(), Name: name,
		State: "stopped", StartedAt: now, UpdatedAt: now})
	if err := os.WriteFile(filepath.Join(d, "info.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
}

func hostedPress(m *Model, s string) tea.Cmd {
	k := tea.KeyPressMsg{Code: []rune(s)[0], Text: s}
	switch s {
	case "tab":
		k = tea.KeyPressMsg{Code: tea.KeyTab}
	case "esc":
		k = tea.KeyPressMsg{Code: tea.KeyEscape}
	case "left":
		k = tea.KeyPressMsg{Code: tea.KeyLeft}
	case "ctrl+n", "ctrl+z", "ctrl+\\", "ctrl+k", "ctrl+6":
		k = tea.KeyPressMsg{Code: rune(s[5]), Mod: tea.ModCtrl}
	}
	_, cmd := m.Update(k)
	return cmd
}

// Hosted shows the one session at the whole width under rush's header: no
// list, and the keys that go to other agents do nothing.
func TestHostedShowsOneSession(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	writeSession(t, "bbbb2222", "another session")

	m := loaded(NewHosted(state.Load(), "test", "aaaa1111"))
	if len(m.snap.Agents) != 1 || m.hostedKey == "" {
		t.Fatalf("hosted snapshot: %d agents, key %q", len(m.snap.Agents), m.hostedKey)
	}
	out := m.Frame(160, 45)
	for _, not := range []string{"another session", "esc back to the list", "describe a task", "ctrl+z zen", "  , ."} {
		if strings.Contains(out, not) {
			t.Errorf("hosted frame shows %q:\n%s", not, out)
		}
	}
	// The header is rush's, counting every agent, not only this one.
	for _, want := range []string{"rush", "finished", "Agents", "Settings", "ctrl+\\"} {
		if !strings.Contains(out, want) {
			t.Errorf("hosted frame lacks %q:\n%s", want, out)
		}
	}
	if l, p, _ := m.layout(); l != 0 || p != 160 || m.topH() != m.headH()+2 {
		t.Fatalf("hosted layout: list %d pane %d top %d", l, p, m.topH())
	}

	c := &hostConn{kind: "claude", key: m.hostedKey, sess: convo.New(), open: map[string]bool{}}
	m.host = c
	for _, k := range []string{"tab", "{", "}", "ctrl+n", "ctrl+z", "left", "ctrl+k"} {
		hostedPress(m, k)
		if m.sel != m.hostedKey || m.view != placeAgents || m.zen || m.mode != modeList || !m.paneFocus || !m.full || m.bar != nil {
			t.Fatalf("after %s: sel %q view %d zen %v mode %d focus %v bar %v", k, m.sel, m.view, m.zen, m.mode, m.paneFocus, m.bar != nil)
		}
	}
	if out := m.render(); strings.Contains(out, "another session") {
		t.Fatalf("hosted frame after keys:\n%s", out)
	}
	// , . < > are text, first or not.
	for _, k := range []string{">", ",", ".", "<", "a"} {
		hostedPress(m, k)
	}
	if got := string(c.input); got != ">,.<a" || m.view != placeAgents {
		t.Fatalf("typed %q in view %d, want >,.<a", got, m.view)
	}
	hostedPress(m, "esc") // clears the box
	if len(c.input) != 0 {
		t.Fatalf("esc left %q", string(c.input))
	}
	// esc on the empty box takes the keys off it, and never quits; with no
	// turn running, esc again does nothing.
	for range 3 {
		if cmd := m.key(tea.KeyPressMsg{Code: tea.KeyEscape}); cmd != nil && isQuit(cmd) {
			t.Fatal("esc quit hosted")
		}
		if m.paneFocus || !m.hostedAway || m.sel != m.hostedKey || m.view != placeAgents {
			t.Fatalf("after esc: focus %v away %v sel %q view %d", m.paneFocus, m.hostedAway, m.sel, m.view)
		}
	}
	if out := ansi.Strip(m.render()); !strings.Contains(out, "ctrl+q quit") {
		t.Fatalf("off the box, the hint should say how to quit:\n%s", out)
	}
	// Typing goes back to the box, the key with it.
	hostedPress(m, "b")
	if !m.paneFocus || m.hostedAway || string(c.input) != "b" {
		t.Fatalf("typing off the box: focus %v away %v input %q", m.paneFocus, m.hostedAway, string(c.input))
	}
	hostedPress(m, "esc")
	hostedPress(m, "esc")
	m.key(tea.KeyPressMsg{Code: tea.KeyEnter})
	if !m.paneFocus || m.hostedAway || len(c.input) != 0 {
		t.Fatalf("enter off the box: focus %v away %v input %q", m.paneFocus, m.hostedAway, string(c.input))
	}
	cmd := m.key(tea.KeyPressMsg{Code: 'q', Mod: tea.ModCtrl})
	if cmd == nil || !isQuit(cmd) {
		t.Fatal("ctrl+q should quit")
	}
}

// Still opening, esc does nothing: the view stays.
func TestHostedEscWhileOpening(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	m := NewHosted(state.Load(), "test", "aaaa1111")
	if cmd := m.key(tea.KeyPressMsg{Code: tea.KeyEscape}); cmd != nil && isQuit(cmd) {
		t.Fatal("esc while opening quit hosted")
	}
}

// In hosted, ctrl+\\ opens the other places as usual, and Agents is the one
// session again, never the list.
func TestHostedPlaces(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	writeSession(t, "bbbb2222", "another session")
	m := loaded(NewHosted(state.Load(), "test", "aaaa1111"))
	m.Frame(160, 45)
	m.host = &hostConn{kind: "claude", key: m.hostedKey, sess: convo.New(), open: map[string]bool{}}

	want := []int{placeHarnesses, placeSettings, placeAgents}
	for i, place := range want {
		hostedPress(m, "ctrl+\\")
		if m.view != place {
			t.Fatalf("ctrl+\\ %d: view %d, want %d", i+1, m.view, place)
		}
	}
	if m.sel != m.hostedKey || !m.paneFocus || !m.full || m.mode != modeList {
		t.Fatalf("back in Agents: sel %q focus %v full %v mode %d", m.sel, m.paneFocus, m.full, m.mode)
	}
	hostedPress(m, "ctrl+\\")
	hostedPress(m, "esc")
	if m.view != placeAgents || m.sel != m.hostedKey {
		t.Fatalf("esc from Efficiency: view %d sel %q", m.view, m.sel)
	}
	if out := m.render(); strings.Contains(out, "another session") {
		t.Fatalf("the list showed in hosted:\n%s", out)
	}
}

// Hosted's Session runs to the right edge of a wide screen, past the width
// a Session beside the list or alone in the full view stops at.
func TestHostedFillsTheWidth(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	m := loaded(NewHosted(state.Load(), "test", "aaaa1111"))
	m.Frame(208, 45)
	m.host = &hostConn{kind: "claude", key: m.hostedKey, sess: convo.New(), open: map[string]bool{}}
	out := m.Frame(208, 45)
	var head string
	for _, l := range strings.Split(out, "\n") {
		// The header's second row ends with the connection, on the right.
		if strings.Contains(ansi.Strip(l), "send to resume") {
			head = ansi.Strip(l)
			break
		}
	}
	if head == "" {
		t.Fatalf("no session header:\n%s", out)
	}
	if w := cellw.String(strings.TrimRight(head, " ")); w < 200 {
		t.Fatalf("the session header stops at column %d of 208: %q", w, head)
	}
	if l, p, _ := m.layout(); l != 0 || p != 208 {
		t.Fatalf("layout: list %d pane %d", l, p)
	}
}

// ctrl+6 shows the list beside hosted's session: every agent, to pick and
// read. Again, or esc from the list, and the view is hosted's own session.
func TestHostedListToggle(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	writeSession(t, "bbbb2222", "another session")
	m := loaded(NewHosted(state.Load(), "test", "aaaa1111"))
	m.Frame(200, 45)
	m.host = &hostConn{key: m.hostedKey, sess: convo.New(), open: map[string]bool{}}
	if out := m.render(); strings.Contains(out, "another session") || !strings.Contains(out, "ctrl+6") {
		t.Fatalf("hosted alone shows the list, or not the key:\n%s", out)
	}
	for _, key := range []tea.KeyPressMsg{{Code: '6', Mod: tea.ModCtrl}, {Code: '^', Mod: tea.ModCtrl}} {
		if !listToggleKey(key.String()) {
			t.Fatalf("%q isn't the list toggle key", key.String())
		}
		m.Update(key)
		out := m.Frame(200, 45)
		if !m.hostedList || m.listW == 0 || m.paneFocus {
			t.Fatalf("%s: list %v width %d focus %v", key.String(), m.hostedList, m.listW, m.paneFocus)
		}
		if !strings.Contains(out, "another session") || !strings.Contains(out, "the hosted session") || !strings.Contains(out, "ctrl+6") {
			t.Fatalf("the list isn't shown:\n%s", out)
		}
		// Another agent can be picked, and stays picked.
		other := m.keyOf(sid("bbbb2222"))
		m.sel = other
		m.Update(tickMsg(time.Now()))
		if m.sel != other || m.focused() == nil || m.focused().Key != other {
			t.Fatalf("picking another agent: sel %q", m.sel)
		}
		m.Update(key)
		if m.hostedList || m.sel != m.hostedKey || !m.paneFocus || len(m.snap.Agents) != 1 {
			t.Fatalf("hidden again: list %v sel %q focus %v agents %d", m.hostedList, m.sel, m.paneFocus, len(m.snap.Agents))
		}
		if out := m.Frame(200, 45); strings.Contains(out, "another session") {
			t.Fatalf("the list stayed:\n%s", out)
		}
	}
	// esc from the list hides it too.
	hostedPress(m, "ctrl+6")
	hostedPress(m, "esc")
	if m.hostedList || m.sel != m.hostedKey {
		t.Fatalf("esc from the list: list %v sel %q", m.hostedList, m.sel)
	}
}

// Outside hosted, ctrl+6 hides the list beside an open Session, and shows
// it again.
func TestListToggle(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.View()
	if m.listW == 0 {
		t.Fatal("the bench model should open split")
	}
	key := tea.KeyPressMsg{Code: '6', Mod: tea.ModCtrl}
	m.Update(key)
	m.View()
	if m.listW != 0 || !m.paneFocus {
		t.Fatalf("hiding: list %d focus %v", m.listW, m.paneFocus)
	}
	m.Update(key)
	m.View()
	if m.listW == 0 || m.paneFocus {
		t.Fatalf("showing: list %d focus %v", m.listW, m.paneFocus)
	}
}

// Outside hosted the same keys do move: the test above means something.
func TestPlacesMoveOutsideHosted(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	m := loaded(New(state.Load(), "test"))
	m.Frame(160, 45)
	hostedPress(m, ".")
	if m.view == placeAgents {
		t.Fatal(". should move to the next place outside hosted")
	}
}

// A question asked while the Session has the whole width is drawn under
// its box: the list's hint row, where it's asked otherwise, isn't there.
func TestHostedShowsAQuestion(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "the hosted session")
	m := NewHosted(state.Load(), "test", "aaaa1111")
	m.host = &hostConn{kind: "claude", key: m.hostedKey, sess: convo.New(), open: map[string]bool{}}
	m.confirm = &confirmation{question: "Send to a cold cache?"}
	if out := ansi.Strip(m.Frame(160, 45)); !strings.Contains(out, "Send to a cold cache?") {
		t.Fatalf("the question isn't on screen:\n%s", out)
	}
}

// isQuit runs a command, batches included, looking for tea.Quit.
func isQuit(cmd tea.Cmd) bool {
	switch msg := cmd().(type) {
	case tea.QuitMsg:
		return true
	case tea.BatchMsg:
		for _, c := range msg {
			if c != nil && isQuit(c) {
				return true
			}
		}
	}
	return false
}

// esc on a running turn stops it at once; esc again soon after asks to
// close it, with restart beside yes.
func TestEscStopsThenAsksToClose(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	a := m.snap.Agents[0]
	c := &hostConn{kind: "claude", key: a.Key, client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	c.sess.Turns = append(c.sess.Turns, &convo.Turn{Live: true})
	if !canInterrupt(c) {
		t.Skip("claude can't be interrupted here")
	}
	m.stopTurn(c)
	if m.confirm != nil || canInterrupt(c) || m.escKey != c.key {
		t.Fatalf("esc on a running turn should stop it straight away, and arm esc again")
	}
	m.askClose(a)
	if m.confirm == nil || m.confirm.onYes == nil {
		t.Fatalf("closing should ask")
	}
	var keys []string
	for _, ch := range m.confirm.more {
		keys = append(keys, ch.key)
	}
	if !slices.Contains(keys, "r") || !strings.Contains(m.confirm.keys(), "restart") {
		t.Errorf("close offers %v, want r to restart", keys)
	}
	// A stop, not a hide, comes with it: z stops the run and keeps the agent
	// in the list rather than hiding it for good.
	if m.confirm.yesText != "hide" || !strings.Contains(m.confirm.keys(), "stop") {
		t.Errorf("close offers %q, want a z to stop alongside y to hide", m.confirm.keys())
	}
}
