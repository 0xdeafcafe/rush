package ui

import (
	"strings"
	"testing"
	"time"
)

// ctrl+x puts a chat in Done in one press, whether it had nothing running
// or its process is up and is being stopped.
func TestDismissGoesToDone(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	m.store.Overlay.Done = map[string]time.Time{}
	for _, up := range []bool{false, true} {
		a := m.snap.Agents[1] // finished, nothing running
		delete(m.store.Overlay.Done, a.Key)
		a.Done, a.PID, a.Past, a.Interactive = false, 0, false, false
		if up {
			a.PID = -1 // its process is up
		}
		m.dismiss(a)
		if _, ok := m.store.Overlay.Done[a.Key]; !ok || m.confirm != nil {
			t.Fatalf("up=%v: not moved to Done (confirm %v)", up, m.confirm)
		}
	}
}

// ctrl+tab goes round the open chats in the list's order.
func TestStepChatCycles(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	var open []string
	for _, a := range m.order {
		if inPlay(m.groupOf[a.Key]) {
			open = append(open, a.Key)
		}
	}
	if len(open) < 2 {
		t.Skip("bench model has fewer than two open chats")
	}
	m.sel = open[0]
	m.stepChat(1)
	if m.sel != open[1] {
		t.Fatalf("next: %s, want %s", m.sel, open[1])
	}
	m.stepChat(-1)
	m.stepChat(-1)
	if m.sel != open[len(open)-1] {
		t.Fatalf("back round: %s, want %s", m.sel, open[len(open)-1])
	}
}

// A draft moves from the Session's box to the Prompt and back, appended
// or in place of what's there.
func TestDraftMovesBetweenBoxAndPrompt(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	c := m.host
	c.input = []rune("start this elsewhere")
	m.draftToPrompt()
	if string(m.input) != "start this elsewhere" || len(c.input) != 0 || m.paneFocus {
		t.Fatalf("to the Prompt: prompt %q box %q", string(m.input), string(c.input))
	}
	c.input = []rune("draft")
	m.input = []rune("more")
	m.promptToAgent(false)
	if string(c.input) != "draft\nmore" || len(m.input) != 0 || !m.paneFocus {
		t.Fatalf("append: %q", string(c.input))
	}
	chip := m.pastes.add("a long pasted log")
	m.input = []rune("see " + chip)
	m.promptToAgent(false)
	if got := c.pastes.expand(string(c.input), false); !strings.Contains(got, "a long pasted log") {
		t.Fatalf("append lost its paste: %q", got)
	}
	m.input = []rune("instead")
	m.promptToAgent(true)
	if string(c.input) != "instead" {
		t.Fatalf("replace: %q", string(c.input))
	}
}

// One put in Done stays there though subagents it left behind still count
// as running; a message to it is what brings it back.
func TestDoneHoldsUntilAMessage(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	m.store.Overlay.Done = map[string]time.Time{}
	a := m.snap.Agents[1]
	a.Done, a.PID, a.Past, a.Interactive, a.Rush = false, 0, false, false, false
	m.dismiss(a)
	a.Done, a.Subs.Direct = true, 2 // left subagents behind
	m.rebuild()
	if got := m.sectionOf(a.Key); got != sectionKey("Done") {
		t.Fatalf("with subagents left: in %q, want Done", got)
	}
	m.undone(a)
	if got := m.sectionOf(a.Key); got == sectionKey("Done") {
		t.Fatalf("after a message: still in Done")
	}
}
