package ui

import (
	"fmt"
	"slices"
	"strings"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
)

// Onboarding teaches rush two ways: a Getting started card under the list
// that ticks off as you do each thing, and a one-time tip the first time
// something happens that a key helps with.

// steps are Getting started: each is ticked off when you do it.
var steps = []struct{ id, what, key string }{
	{"start", "Start an agent", "enter"},
	{"session", "Open its Session", "{ }"},
	{"hash", "Run a command", "#"},
	{"next", "Jump to who needs you", "ctrl+n"},
	{"rename", "Rename an agent", "ctrl+r"},
	{"pin", "Pin one to the top", "#pin"},
	{"done", "Put one away", "#done"},
	{"zen", "Try zen", "ctrl+z"},
	{"keys", "See all the keys", "?"},
	{"mackeys", "Give Terminal.app the ⌘ keys", "#mackeys on"},
}

// activeSteps are the steps that apply here: the ⌘ keys only in
// Terminal.app, which keeps them for itself.
func activeSteps() []struct{ id, what, key string } {
	if inAppleTerminal() {
		return steps
	}
	return slices.DeleteFunc(slices.Clone(steps), func(s struct{ id, what, key string }) bool { return s.id == "mackeys" })
}

// didStep ticks off a Getting started step.
func (m *Model) didStep(id string) {
	if !m.onboard {
		return
	}
	o := &m.store.Config.Onboarding
	if slices.Contains(o.Steps, id) {
		return
	}
	o.Steps = append(o.Steps, id)
	_ = m.store.SaveConfig()
	if m.stepsDone() == len(activeSteps()) && !o.Hidden {
		m.flash("✓ all set · ? for keys", false)
	}
}

func (m *Model) stepsDone() int {
	n := 0
	for _, s := range activeSteps() {
		if slices.Contains(m.store.Config.Onboarding.Steps, s.id) {
			n++
		}
	}
	return n
}

// showCard is whether Getting started is under the list: until every step
// is done, or you put it away (#tips off).
func (m *Model) showCard() bool {
	return m.onboard && !m.store.Config.Onboarding.Hidden && m.stepsDone() < len(activeSteps())
}

// startedLines is Getting started, w wide, with a blank line above.
func (m *Model) startedLines(w int) []string {
	w -= 4 // the keys end where the box's top-right label does
	done := m.store.Config.Onboarding.Steps
	active := activeSteps()
	out := []string{"", " " + paint(cOrange+bold, "✦ Getting started") + faint(fmt.Sprintf("  %d/%d", m.stepsDone(), len(active)))}
	for _, s := range active {
		gap := w - cellw.String(s.what) - cellw.String(s.key) - 2
		if slices.Contains(done, s.id) {
			out = append(out, " "+paint(cGreen, "✓ ")+faint(s.what))
			continue
		}
		l := " " + dim("○ ") + paint(cText, s.what)
		if gap >= 2 {
			l += strings.Repeat(" ", gap) + paint(cOrange+bold, s.key)
		}
		out = append(out, l)
	}
	// # is the way in to most of rush: say so, and how this card goes.
	hint := " " + paint(cOrange+bold, "#") + faint(" for commands: pin, done, group, sort… · ") + paint(cOrange+bold, "#tips off") + faint(" hides this")
	if cellw.String(ansi.Strip(hint)) > w+3 {
		hint = " " + paint(cOrange+bold, "#") + faint(" for commands · ") + paint(cOrange+bold, "#tips off") + faint(" hides this")
	}
	return append(out, "", ansi.Truncate(hint, w+3, "…"))
}

// tips show once each, the first time they'd help.
var tips = []struct {
	id, text string
	when     func(m *Model, t tally) bool
}{
	{"needs", "● needs you · ctrl+n to go there", func(m *Model, t tally) bool {
		f := m.focused()
		return t.blocked > 0 && (f == nil || !f.NeedsYou())
	}},
	{"zen", "● several need you · ctrl+z for one at a time", func(m *Model, t tally) bool {
		return t.blocked > 1 && !m.zen
	}},
	{"session", "[ ] to switch view · esc to go back", func(m *Model, t tally) bool {
		return m.paneFocus && m.host != nil && !m.zen
	}},
	{"finished", "✓ finished · #done to put it away", func(m *Model, t tally) bool {
		f := m.focused()
		return f != nil && !f.Done && f.JustFinished(m.snap.At)
	}},
	{"hash", "enter to run it on the picked agent", func(m *Model, t tally) bool {
		return m.inKind == inPrompt && typingHash(string(m.input))
	}},
}

// noteProgress ticks off the steps that are a state rather than a key
// (being in a Session, zen), and on a tick shows the first tip that's due
// when nothing else is being said.
func (m *Model) noteProgress(tick bool) {
	if !m.onboard {
		return
	}
	if m.sessionFocused() && !m.zen && m.focused() != nil {
		m.didStep("session")
	}
	if m.zen {
		m.didStep("zen")
	}
	o := &m.store.Config.Onboarding
	if !tick || len(o.Tips) == len(tips) || m.confirm != nil || m.dialog != nil || m.mode != modeList || m.snap == nil {
		return
	}
	if m.status != "" && m.snap.At.Sub(m.statusAt).Seconds() < 6 {
		return
	}
	t := m.tally()
	for _, tp := range tips {
		if !slices.Contains(o.Tips, tp.id) && tp.when(m, t) {
			o.Tips = append(o.Tips, tp.id)
			_ = m.store.SaveConfig()
			m.flash(tp.text, false)
			return
		}
	}
}

// keyRows are "key  what" rows, each key a lit keycap, the keys in a column.
func keyRows(rows [][2]string) []string {
	kw := 0
	for _, r := range rows {
		kw = max(kw, cellw.String(r[0]))
	}
	var out []string
	for _, r := range rows {
		out = append(out, keycap(r[0], true)+strings.Repeat(" ", kw-cellw.String(r[0])+2)+paint(cText, r[1]))
	}
	return out
}
