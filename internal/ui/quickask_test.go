package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
)

// A quick ask opens from anywhere, takes the keys, is answered by the
// cheap model with the thread so far, and hides and comes back on its keys.
func TestQuickAskThread(t *testing.T) {
	var asked []string
	was := quickAsker
	quickAsker = func(p string) (string, string, error) {
		asked = append(asked, p)
		return "luna", "yes: KeyTab with ModCtrl", nil
	}
	defer func() { quickAsker = was }()

	m, _ := benchModel(160, 40)
	m.paneFocus = false // from the Agents list
	box := string(m.host.input)
	ask := func(q string) {
		pressKeys(m, strings.Split(q, "")...)
		k, _ := keyOf("enter")
		msg := m.key(k)()
		m.Update(msg)
	}
	pressKeys(m, "ctrl+]", "q")
	if !m.quick.focused() {
		t.Fatal("ctrl+] q should open the float with the keys")
	}
	ask("ctrl+tab?")
	ask("kitty?")
	if string(m.host.input) != box {
		t.Fatalf("keys reached the box under: %q", string(m.host.input))
	}
	if len(asked) != 2 || !strings.Contains(asked[1], "Q: ctrl+tab?\nA: yes") || !strings.HasSuffix(asked[1], "Now: kitty?") {
		t.Fatalf("follow-up should carry the thread: %q", asked)
	}
	if v := ansi.Strip(m.render()); !strings.Contains(v, "guide · luna") || !strings.Contains(v, "› kitty?") {
		t.Fatalf("float not drawn:\n%s", v)
	}
	pressKeys(m, "esc")
	if m.quick.shown || strings.Contains(ansi.Strip(m.render()), "guide · luna") {
		t.Fatal("esc should hide the float")
	}
	pressKeys(m, "ctrl+]", "q")
	if !m.quick.focused() || len(m.quick.thread().qa) != 2 {
		t.Fatal("ctrl+] q should bring the same thread back")
	}
	pressKeys(m, "ctrl+]", "q")
	if !m.quick.shown || m.quick.focused() {
		t.Fatal("ctrl+] q again should hand the keys back, the float still shown")
	}
}

// The guide reads the sessions numbered, and its open: N opens the Nth,
// the line itself taken off the answer.
func TestGuideOpens(t *testing.T) {
	m, _ := benchModel(160, 40)
	m.paneFocus = false
	want := m.order[2]
	var asked string
	was := quickAsker
	quickAsker = func(p string) (string, string, error) {
		asked = p
		return "luna", "That one's waiting on you.\nopen: 3", nil
	}
	defer func() { quickAsker = was }()
	m.Update(m.openGuide("what's next?")())
	if !strings.Contains(asked, "[3] "+want.DisplayName) || !strings.HasSuffix(asked, "what's next?") {
		t.Fatalf("the guide should see the sessions numbered: %q", asked)
	}
	if m.sel != want.Key {
		t.Fatalf("open: 3 should select %q, got %q", want.Key, m.sel)
	}
	if r := m.quick.thread().qa[0].Response; strings.Contains(r, "open:") || !strings.Contains(r, "opened "+want.DisplayName) {
		t.Fatalf("answer should drop the open line and say what opened: %q", r)
	}
	if a, k := guideOpens("no.\nopen: 99", []string{"x"}); a != "no." || k != "" {
		t.Fatalf("out of range opens nothing: %q %q", a, k)
	}
}

// ctrl+] i in a Session puts the thread into the box as one paste chip.
func TestQuickAskInsert(t *testing.T) {
	m, _ := benchModel(160, 40)
	c := m.host
	c.input, c.back = nil, 0
	pressKeys(m, "ctrl+]", "i")
	if len(c.input) != 0 || len(m.said) == 0 || m.said[len(m.said)-1].kind != sayRefused {
		t.Fatal("nothing answered: nothing goes in, and it's said why")
	}
	m.quick.newThread().qa = []btwQA{{Question: "ctrl+tab?", Response: "yes"}}
	m.quick.shown = true
	pressKeys(m, "ctrl+]", "i")
	if !pasteRe.MatchString(string(c.input)) {
		t.Fatalf("box should hold a paste chip: %q", string(c.input))
	}
	if got := c.pastes.expand(string(c.input), false); !strings.Contains(got, "Q: ctrl+tab?\nA: yes") {
		t.Fatalf("the chip should carry the thread: %q", got)
	}
	if m.quick.shown || m.statusErr || !strings.Contains(m.status, "in the box") {
		t.Fatalf("float should hide and a flash say so: %q", m.status)
	}
}
