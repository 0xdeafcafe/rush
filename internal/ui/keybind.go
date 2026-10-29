package ui

import (
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/keymap"
)

// keyState is the keymap in force and a chord being typed.
type keyState struct {
	m    *keymap.Map
	file keymap.File
	// chord is the keys of a chord typed so far, and when the last came.
	chord   keymap.Seq
	chordAt time.Time
	// capture hands the next key to Settings, unmapped, to bind it.
	capture func(s string) tea.Cmd
	// page is Settings, Keys: keys being taken for an action.
	page keysPage
}

// chordWait is how long a chord waits for its next key.
const chordWait = 2 * time.Second

// keyActions are every action a key can be bound to: rush's keys, its #
// commands, and what plugins offer.
func (m *Model) keyActions() []keymap.Action {
	out := append([]keymap.Action(nil), keymap.Defaults...)
	for _, c := range fleetCommands {
		out = append(out, keymap.Action{ID: keymap.CommandID(c.Name), Context: keymap.Any, Title: "#" + c.Name + ": " + c.Description})
	}
	out = append(out, m.pluginActions()...)
	return out
}

// loadKeys reads keybindings.json, off the UI, and puts it in force.
func (m *Model) loadKeys() tea.Cmd {
	return sheetDo(keymap.Load, func(m *Model, f keymap.File, err error) tea.Cmd {
		if err != nil {
			m.fail("keybindings.json: " + err.Error())
		}
		m.setKeys(f)
		return nil
	})
}

// setKeys puts f in force.
func (m *Model) setKeys(f keymap.File) {
	m.keys.file = f
	m.keys.m = keymap.Build(m.keyActions(), f, m.pluginKeys())
}

// keyMap is the keymap in force; before keybindings.json is read, rush's
// own.
func (m *Model) keyMap() *keymap.Map {
	if m.keys.m == nil {
		m.keys.m = keymap.Build(m.keyActions(), keymap.File{}, nil)
	}
	return m.keys.m
}

// keyContexts is where a key lands now, beyond Global: nothing more while
// a sheet, the command bar or a question has the keys.
func (m *Model) keyContexts() []keymap.Context {
	if m.onPages() {
		return []keymap.Context{keymap.Pages}
	}
	if m.keyHelp != nil || m.bar != nil || m.confirm != nil || m.sheet != nil || m.picker != nil || m.dialog != nil ||
		m.embedded || m.mode != modeList || m.editingDoc() {
		return nil
	}
	if (m.paneFocus || m.quick.focused()) && m.host != nil {
		// The quick ask lets chords through only: ctrl+] i into the chat.
		return []keymap.Context{keymap.Session}
	}
	if m.inKind == inPrompt && len(m.input) > 0 {
		return []keymap.Context{keymap.Prompt, keymap.List}
	}
	return []keymap.Context{keymap.List}
}

// onPages is whether a place's pages have the keys: Settings,
// Projects or the Wall, with nothing over them and nothing being typed.
func (m *Model) onPages() bool {
	if m.bar != nil || m.confirm != nil || m.sheet != nil || m.picker != nil || m.embedded || m.editingDoc() {
		return false
	}
	if d := m.dialog; d != nil {
		return (m.view == placeSettings || m.view == placeHarnesses) && d.asking == ""
	}
	switch m.mode {
	case modeWall:
		return true
	}
	return false
}

// remapKey turns the key pressed into the one rush's handling expects, by
// the keymap. It reports true when the key is used up: a chord begun, a
// command run, or a key that no longer does anything.
func (m *Model) remapKey(k *tea.KeyPressMsg, s *string) (tea.Cmd, bool) {
	if f := m.keys.capture; f != nil {
		m.keys.capture = nil
		return f(*s), true
	}
	if m.embedded && *s == "ctrl+]" {
		return nil, false // always hands Claude Code's screen back, never a chord
	}
	if len(m.keys.chord) > 0 && time.Since(m.keys.chordAt) > chordWait {
		m.keys.chord = nil
	}
	r := m.keyMap().Resolve(m.keyContexts(), m.keys.chord, *s)
	m.keys.chord = nil
	switch {
	case len(r.Pending) > 0:
		m.keys.chord, m.keys.chordAt = r.Pending, time.Now()
		m.flash(r.Pending.String()+" …", false)
		return nil, true
	case len(r.Missed) > 0:
		m.flash(r.Missed.String()+" does nothing", true)
		return nil, true
	case r.Run != "":
		return m.runAction(r.Run), true
	case r.Key == "":
		return nil, true
	case r.Key != *s:
		nk, ok := keyOf(r.Key)
		if !ok {
			return nil, false
		}
		*k, *s = nk, r.Key
	}
	return nil, false
}

// runAction runs a command action: a # command on the agent in view, a
// plugin's command, or one of the Prompt's.
func (m *Model) runAction(id string) tea.Cmd {
	a := m.selected()
	if m.paneFocus && m.host != nil {
		a = m.focused()
	}
	if name, ok := strings.CutPrefix(id, "command:"); ok {
		return m.command(a, "#"+name)
	}
	if rest, ok := strings.CutPrefix(id, "plugin:"); ok {
		return m.runPluginCommand(rest, a)
	}
	switch id {
	case "session.history.open", "session.history.close":
		m.setHistoryFold(id == "session.history.open")
		return nil
	case "chat.next", "chat.prev":
		return m.stepChat(map[bool]int{true: 1, false: -1}[id == "chat.next"])
	case "session.depth.0", "session.depth.1", "session.depth.2", "session.depth.3":
		if c := m.host; c != nil {
			n := id[len(id)-1] - '0'
			c.depth, c.verbose = []convo.Depth{convo.DepthProse, convo.DepthRuns, convo.DepthDefault, convo.DepthDefault}[n], n == 3
			m.flash(m.depthName(n), false)
		}
		return nil
	case "grid.beside", "grid.below":
		return m.gridPin(id == "grid.below")
	case "grid.unpin":
		m.gridUnpin(false)
		return nil
	case "agent.new":
		m.toPrompt()
		return nil
	case "session.toprompt":
		return m.draftToPrompt()
	case "session.carry":
		return m.carryDraft()
	case "prompt.append", "prompt.replace":
		return m.promptToAgent(id == "prompt.replace")
	case "list.close", "session.close":
		return m.askClose(a)
	case "session.setup":
		if m.host != nil {
			return m.openSwitchSheet(m.host)
		}
		return nil
	case "session.discuss":
		if c := m.host; c != nil && !isRoomKey(c.key) {
			return m.discuss(c, "", false)
		}
		return nil
	case "session.mode":
		return m.cycleSessionPermission()
	case "guide.open":
		if m.keyHelp != nil {
			m.closeKeyHelp()
		} else {
			m.openKeyHelp()
		}
		return nil
	case "quick.ask":
		return m.toggleQuick()
	case "session.quickask.insert":
		return m.insertQuick()
	case "session.recall":
		return m.toggleRecall()
	case "prompt.stash":
		return m.stashCommand("stash")
	case "prompt.history":
		return m.stashCommand("history")
	}
	return nil
}

func (m *Model) cycleSessionPermission() tea.Cmd {
	return m.permissionCommand(m.host, "", false)
}

// depthName says what depth n shows.
func (m *Model) depthName(n byte) string {
	return []string{"m0 · only what's said", "m1 · every run of steps folded", "m2 · as usual", "m3 · everything open"}[n]
}

// draftToPrompt moves what's typed in the Session's box to Agents' Prompt,
// to start a new agent with.
func (m *Model) draftToPrompt() tea.Cmd {
	c := m.host
	if c == nil || len(c.input) == 0 {
		m.flash("nothing typed to move", false)
		return nil
	}
	text, ps := c.input, c.pastes
	c.input, c.back, c.pastes = nil, 0, pastes{}
	m.toPrompt()
	m.input, m.back, m.pastes = text, 0, ps
	m.flash("moved to the Prompt · enter starts a new agent with it", false)
	return nil
}

// carryDraft lifts what's typed in the Session's box into Agents' Prompt
// to take to another session, typed in the wrong one: ↑↓ picks it in the
// list, enter puts it in its box and esc brings it back (carryKey).
func (m *Model) carryDraft() tea.Cmd {
	c := m.host
	if c == nil || len(c.input) == 0 {
		m.flash("nothing typed to move", false)
		return nil
	}
	if len(m.input) > 0 && m.inKind == inPrompt {
		m.flash("the Prompt has something typed in it already: clear it first", true)
		return nil
	}
	text, ps := c.input, c.pastes
	c.input, c.back, c.pastes = nil, 0, pastes{}
	m.toPrompt()
	m.input, m.back, m.pastes = text, 0, ps
	m.carryFrom = c.key
	m.flash("↑↓ pick the session · enter puts it in its box · esc brings it back", false)
	return nil
}

// carrying is whether the Prompt holds a draft lifted from a session.
func (m *Model) carrying() bool {
	if m.carryFrom != "" && (len(m.input) == 0 || m.inKind != inPrompt) {
		m.carryFrom = "" // sent, cleared or put down some other way
	}
	return m.carryFrom != ""
}

// carryKey is enter and esc in the list while carrying a draft: enter
// puts it in the picked session's box, esc in the one it came from.
func (m *Model) carryKey(s string) (tea.Cmd, bool) {
	if !m.carrying() {
		return nil, false
	}
	var a *fleet.Agent
	switch s {
	case "enter":
		if a = m.selected(); a == nil {
			m.flash("pick a session to put it in", true)
			return nil, true
		}
	case "esc":
		if a = m.agentByKey(m.carryFrom); a == nil {
			m.carryFrom = "" // gone: the Prompt keeps it, esc clears it as usual
			m.flash("the session it came from has gone · it stays in the Prompt", true)
			return nil, true
		}
	default:
		return nil, false
	}
	if _, ok := m.roomOf(a); ok {
		m.flash("a room's box takes no draft: pick a session", true)
		return nil, true
	}
	d := draftIn{text: m.input, pastes: m.pastes}
	m.input, m.back, m.pastes, m.carryFrom = m.input[:0], 0, pastes{}, ""
	m.sel = a.Key
	cmd := m.focusPane(a)
	if c := m.host; c != nil && c.key == a.Key {
		putDraft(c, d)
	} else {
		if m.dropIn == nil {
			m.dropIn = map[string]draftIn{}
		}
		m.dropIn[a.Key] = d // the box once its session opens
	}
	m.flash("moved to "+oneLine(a.DisplayName), false)
	return cmd, true
}

// draftIn is text on its way into a session's box, with its pasted chips.
type draftIn struct {
	text   []rune
	pastes pastes
}

// putDraft puts d in c's box: in place of nothing, or after what's there.
func putDraft(c *hostConn, d draftIn) {
	if len(c.input) == 0 {
		c.input, c.pastes = d.text, d.pastes
	} else {
		// Its pasted chips come along, numbered again in the box's own.
		moved := c.pastes.unfold(d.pastes.expand(string(d.text), true))
		c.input = append(append(c.input, '\n'), moved...)
	}
	c.back = 0
}

// promptToAgent puts what's typed in Agents' Prompt into the open agent's
// box, after its draft or in place of it, and the keys go there.
func (m *Model) promptToAgent(replace bool) tea.Cmd {
	c := m.host
	if c == nil {
		m.flash("no agent open to send it to", true)
		return nil
	}
	if len(m.input) == 0 {
		m.flash("nothing typed to move", false)
		return nil
	}
	switch {
	case replace || len(c.input) == 0:
		c.input, c.pastes = m.input, m.pastes
	default:
		// Its pasted chips come along, numbered again in the box's own.
		moved := c.pastes.unfold(m.pastes.expand(string(m.input), true))
		c.input = append(append(c.input, '\n'), moved...)
	}
	c.back = 0
	m.input, m.back, m.pastes = m.input[:0], 0, pastes{}
	m.preview, m.paneFocus = true, true
	return nil
}
