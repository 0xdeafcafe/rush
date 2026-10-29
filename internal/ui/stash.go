package ui

import (
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/bundled/drafts"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// --- undo ---

// undoStack is an input box's undo: what the box held before each change.
// A run of typing is one step, as in any editor.
type undoStack struct {
	past, future []undoState
	typing       bool // the last step saved was typing, so more joins it
}

type undoState struct {
	text []rune
	back int
}

const maxUndo = 200

// save keeps text as a state to come back to. Typing straight after
// typing joins the step before it.
func (u *undoStack) save(text []rune, back int, typing bool) {
	if typing && u.typing {
		return
	}
	u.typing = typing
	u.past = append(u.past, undoState{slices.Clone(text), back})
	if len(u.past) > maxUndo {
		u.past = u.past[len(u.past)-maxUndo:]
	}
	u.future = nil
}

// undo swaps the box's text for the one before it; redo goes the other way.
func (u *undoStack) undo(text []rune, back int) ([]rune, int, bool) {
	return u.step(&u.past, &u.future, text, back)
}

func (u *undoStack) redo(text []rune, back int) ([]rune, int, bool) {
	return u.step(&u.future, &u.past, text, back)
}

func (u *undoStack) step(from, to *[]undoState, text []rune, back int) ([]rune, int, bool) {
	u.typing = false
	if len(*from) == 0 {
		return text, back, false
	}
	st := (*from)[len(*from)-1]
	*from = (*from)[:len(*from)-1]
	*to = append(*to, undoState{slices.Clone(text), back})
	return st.text, min(st.back, len(st.text)), true
}

func isUndo(s string) bool { return s == "super+z" || s == "ctrl+_" || s == "ctrl+/" }
func isRedo(s string) bool { return s == "shift+super+z" || s == "super+shift+z" || s == "ctrl+y" }

// undoKey runs undo or redo on the Session's box.
func (m *Model) undoKey(c *hostConn, s string) bool {
	var buf []rune
	var back int
	var ok bool
	switch {
	case isUndo(s):
		buf, back, ok = c.undo.undo(c.input, c.back)
		if !ok {
			m.flash(m.stashSaid("nothing to undo · {history} has what you typed before"), false)
		}
	case isRedo(s):
		buf, back, ok = c.undo.redo(c.input, c.back)
	default:
		return false
	}
	if ok {
		c.input, c.back, c.anchor = buf, back, 0
	}
	return true
}

// --- the stash ---

// The stash is the bundled drafts plugin: it keeps each box until it's
// sent, sets one aside, and keeps what was sent, cleared and replaced, to
// put back. rush gives it keys, says where text went, and calls it what
// you chose in its settings.

// stashPlugin is the bundled plugin that is the stash.
var stashPlugin = drafts.Manifest.Name

// stash is what the stash is called, and whether it's running: all from
// what the broker last said, so cheap enough for every frame.
func (m *Model) stash() (drafts.Words, bool) {
	if m.hooks != nil {
		for _, p := range m.hooks.State().Plugins {
			if p.Name == stashPlugin {
				return drafts.WordsFor(p.Values["called"]), true
			}
		}
	}
	return drafts.WordsFor(""), false
}

// stashKeys are the stash and history keys where the keys are now. An
// empty Prompt's are the list's own, so there the history is its command.
func (m *Model) stashKeys() (aside, history string) {
	w, _ := m.stash()
	aside, history = m.boundKey("prompt.stash"), m.boundKey("prompt.history")
	if m.paneFocus && m.host != nil {
		aside, history = m.boundKey("session.stash"), m.boundKey("session.history")
	} else if len(m.input) == 0 || m.inKind != inPrompt {
		history = ""
	}
	return firstNonEmpty(aside, "the "+w.Title+" key"), firstNonEmpty(history, w.Command())
}

// stashSaid fills in the keys the stash names, as you have them.
func (m *Model) stashSaid(s string) string {
	aside, history := m.stashKeys()
	return strings.NewReplacer("{aside}", aside, "{history}", history).Replace(s)
}

// wentTo is what a flash says of text taken out of a box: what happened,
// and the history's tab that has it, when the stash is running to keep it.
func (m *Model) wentTo(did, tab string) string {
	if _, on := m.stash(); !on {
		return did
	}
	return did + m.stashSaid(" · {history} › "+tab+" has it")
}

// hasStash is whether a box has a stash waiting, as its edge says.
func (m *Model) hasStash(key string) bool {
	if m.hooks == nil {
		return false
	}
	return slices.ContainsFunc(m.hooks.State().Notes[key], func(n plugin.UIStatus) bool { return n.Plugin == stashPlugin })
}

// stashCommand runs the stash's command on the box with the keys: stash,
// or history.
func (m *Model) stashCommand(cmd string) tea.Cmd {
	w, on := m.stash()
	if !on {
		m.flash(w.Title+" isn't running · Settings, Plugins has it, as "+stashPlugin, true)
		return nil
	}
	a := m.selected()
	if m.paneFocus && m.host != nil {
		a = m.focused()
	}
	in, box, _ := m.boxState()
	return m.hooks.Command(stashPlugin, cmd, m.uiSession(a), box, in, func(err error) tea.Msg {
		return sheetMsg{apply: func(m *Model) tea.Cmd {
			if err != nil {
				m.fail(w.Title + ": " + err.Error())
			}
			return nil
		}}
	})
}

// wipeBox clears the Session's box, keeping what was in it to undo and in
// the history.
func (m *Model) wipeBox(c *hostConn) {
	if len(c.input) == 0 {
		return
	}
	c.undo.save(c.input, c.back, false)
	m.emitBox(plugin.EvInputCleared, c.key, string(c.input))
	c.input, c.back, c.anchor = nil, 0, 0
	c.clearedAt = time.Now()
	m.flash(m.wentTo("cleared · "+undoHint+" brings it back", "Cleared"), false)
}

const undoHint = "cmd+z or ctrl+/"

// wipePrompt is wipeBox for the Prompt under Agents.
func (m *Model) wipePrompt() {
	if len(m.input) == 0 {
		return
	}
	m.emitBox(plugin.EvInputCleared, "", string(m.input))
	m.undo.save(m.input, m.back, false)
	m.input, m.back, m.anchor = nil, 0, 0
	m.flash(m.wentTo("cleared · "+undoHint+" brings it back", "Cleared"), false)
}

// keepSent tells the stash a message went, for its history, and says so:
// with the box's stash, if it has one, coming back into it.
func (m *Model) keepSent(c *hostConn, text string) {
	if strings.TrimSpace(text) == "" {
		return
	}
	did := "sent"
	if m.hasStash(c.key) {
		w, _ := m.stash()
		did = "sent · your " + w.Noun + " is back in the box"
	}
	m.emitBox(plugin.EvInputSent, c.key, strings.TrimSpace(text))
	m.flash(m.wentTo(did, "Sent"), false)
}

// clearPrompt empties the Prompt under Agents; a message being written
// there is kept in the history first.
func (m *Model) clearPrompt() {
	if m.inKind == inPrompt || m.inKind == inReply {
		m.wipePrompt()
		return
	}
	m.input, m.back, m.anchor = m.input[:0], 0, 0
}

// boxVert moves the Session box's cursor a row up or down through the
// text as it's wrapped, keeping its column. Past the first row it goes to
// the start; past the last, the end. shift extends the selection.
func (m *Model) boxVert(c *hostConn, d int, shift bool) {
	b := c.box
	b.text = c.input
	segs := b.segs()
	pos := len(c.input) - c.back
	row := len(segs) - 1
	for i, sg := range segs {
		if pos >= sg.from && pos <= sg.to {
			row = i
			break
		}
	}
	if shift && c.anchor == 0 {
		c.anchor = pos + 1
	} else if !shift {
		c.anchor = 0
	}
	switch to := row + d; {
	case to < 0:
		pos = 0
	case to >= len(segs):
		pos = len(c.input)
	default:
		col := 0
		for p := segs[row].from; p < pos; p++ {
			col += runeW(c.input[p])
		}
		sg, width := segs[to], 0
		pos = sg.from
		for pos < sg.to && width+runeW(c.input[pos]) <= col {
			width += runeW(c.input[pos])
			pos++
		}
	}
	c.back = len(c.input) - pos
}
