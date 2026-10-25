package ui

import (
	"slices"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/keymap"
)

// The keys: every key rush answers to, as you have them, over whatever is
// on the screen. F1 opens it from anywhere, ? from a list with nothing
// typed. Where you are comes first, then what works everywhere, then the
// rest; typing finds a key by its keys or what it does, and enter takes
// you to Settings, Keys, to change the one under the cursor.

// keyHelp is the open keys guide.
type keyHelp struct {
	query []rune
	pos   int
	here  []keymap.Context // where it was opened from, shown first
	rows  []keyHelpRow
	cur   int // index into rows, never a heading
	top   int // first body line shown
	snap  bool // the cursor's section to the top, as tab moved there
}

// keyHelpRow is a section's heading (id and keys empty, section set) or a
// key and what it does.
type keyHelpRow struct {
	section string
	id      string // the action, "" for a key that's not one (#, /)
	keys    []string
	title   string
	yours   bool // keys you gave it, not rush's
	heading bool
}

// openKeyHelp opens the keys guide on where the keys are now.
func (m *Model) openKeyHelp() {
	h := &keyHelp{here: m.keyHelpHere()}
	m.keyHelp = h
	m.keyHelpRebuild()
	m.didStep("keys")
}

func (m *Model) closeKeyHelp() { m.keyHelp = nil }

// keyHelpHere is the contexts the keys land in now, most particular first.
func (m *Model) keyHelpHere() []keymap.Context {
	switch {
	case m.paneFocus && m.host != nil:
		return []keymap.Context{keymap.Session}
	case m.view == placeAgents && m.mode == modeList:
		if m.inKind == inPrompt && len(m.input) > 0 {
			return []keymap.Context{keymap.Prompt, keymap.List}
		}
		return []keymap.Context{keymap.List}
	}
	return []keymap.Context{keymap.Pages}
}

// keyHelpTitle is what a section is called in the guide.
func keyHelpTitle(c keymap.Context) string {
	switch c {
	case keymap.List:
		return "Agents and the Prompt"
	case keymap.Pages:
		return "Projects, Settings and the Wall"
	}
	return c.Title()
}

// keyHelpRebuild lays out the rows that match what's typed, keeping the
// cursor on the action it was on when it's still there.
func (m *Model) keyHelpRebuild() {
	h := m.keyHelp
	was := ""
	if h.cur < len(h.rows) {
		was = h.rows[h.cur].id + "\x00" + h.rows[h.cur].section
	}
	q := strings.Fields(strings.ToLower(string(h.query)))
	km := m.keyMap()
	w, _ := m.stash()

	order := append([]keymap.Context(nil), h.here...)
	if !slices.Contains(order, keymap.Global) {
		order = append(order, keymap.Global)
	}
	for _, c := range keymap.Contexts {
		if !slices.Contains(order, c) {
			order = append(order, c)
		}
	}
	h.rows = h.rows[:0]
	for i, c := range order {
		section := keyHelpTitle(c)
		if i < len(h.here) {
			section = "Here · " + section
		}
		var rows []keyHelpRow
		for _, a := range km.Actions() {
			if a.Context != c {
				continue
			}
			r := keyHelpRow{section: section, id: a.ID, title: a.Title, yours: km.Changed(a.ID)}
			for _, s := range km.Keys(a.ID) {
				r.keys = append(r.keys, s.String())
			}
			switch a.ID {
			case "prompt.stash", "session.stash":
				r.title = w.Fill("{verb} · what's typed is back after you send")
			case "prompt.history", "session.history":
				r.title = w.Fill("{tab} · sent · cleared · replaced, to put back · {command}")
			}
			// A command or a plugin's with no key isn't a shortcut; it shows
			// only when you look for it, to give it one.
			if len(r.keys) == 0 && a.Command() && len(q) == 0 {
				continue
			}
			rows = append(rows, r)
		}
		if c == keymap.List || c == keymap.Session {
			rows = append(rows,
				keyHelpRow{section: section, keys: []string{"#"}, title: "rush's commands, typed at the start of the box"},
				keyHelpRow{section: section, keys: []string{"/"}, title: "the agent's own commands, typed at the start of the box"})
		}
		rows = slices.DeleteFunc(rows, func(r keyHelpRow) bool { return !keyHelpMatch(r, q) })
		if len(rows) == 0 {
			continue
		}
		h.rows = append(h.rows, keyHelpRow{section: section, heading: true})
		h.rows = append(h.rows, rows...)
	}
	h.cur = -1
	for i, r := range h.rows {
		if !r.heading && (h.cur < 0 || r.id+"\x00"+r.section == was) {
			h.cur = i
		}
	}
	h.cur = max(0, h.cur)
}

// keyHelpMatch is whether every word typed is in the row's keys, what it
// does or its section.
func keyHelpMatch(r keyHelpRow, q []string) bool {
	hay := strings.ToLower(strings.Join(r.keys, " ") + " " + keyText(r.keys) + " " + r.title + " " + r.section + " " + r.id)
	for _, w := range q {
		if !strings.Contains(hay, w) {
			return false
		}
	}
	return true
}

// keyText is keys as the guide writes them: ⌘ for super, ⌥ for alt on a
// Mac, one key from the next with a dot.
func keyText(keys []string) string {
	out := make([]string, len(keys))
	for i, k := range keys {
		out[i] = prettyKey(k)
	}
	return strings.Join(out, " · ")
}

func prettyKey(k string) string {
	parts := strings.Fields(k)
	for i, p := range parts {
		mods, name := "", p
		if j := strings.LastIndex(p, "+"); j > 0 && j < len(p)-1 {
			mods, name = p[:j+1], p[j+1:]
		}
		if a, ok := arrowKeys[name]; ok {
			name = a
		}
		parts[i] = mods + name
	}
	return strings.ReplaceAll(convo.KeyWord(strings.Join(parts, " ")), "super+", "⌘")
}

var arrowKeys = map[string]string{"up": "↑", "down": "↓", "left": "←", "right": "→"}

// keyHelpKey is a key pressed while the guide is open.
func (m *Model) keyHelpKey(k tea.KeyPressMsg, s string) tea.Cmd {
	h := m.keyHelp
	switch s {
	case "esc", "ctrl+c":
		if len(h.query) > 0 {
			h.query, h.pos = nil, 0
			m.keyHelpRebuild()
			return nil
		}
		m.closeKeyHelp()
		return nil
	case "f1":
		m.closeKeyHelp()
		return nil
	case "up", "ctrl+p":
		h.step(-1)
		return nil
	case "down", "ctrl+n":
		h.step(1)
		return nil
	case "pgup":
		for range 10 {
			h.step(-1)
		}
		return nil
	case "pgdown":
		for range 10 {
			h.step(1)
		}
		return nil
	case "tab", "shift+tab":
		h.nextSection(map[string]int{"tab": 1, "shift+tab": -1}[s])
		return nil
	case "enter":
		if h.cur >= len(h.rows) || h.rows[h.cur].id == "" {
			return nil
		}
		id := h.rows[h.cur].id
		m.closeKeyHelp()
		m.mode = modeList
		m.setView(placeSettings)
		m.setSettingsPage(pageKeys)
		if m.dialog != nil {
			m.showKey(id)
		}
		return nil
	}
	buf, pos, ok := edit(h.query, h.pos, k, s)
	if !ok || strings.Contains(string(buf), "\n") {
		return nil
	}
	changed := string(buf) != string(h.query)
	h.query, h.pos = buf, pos
	if changed {
		m.keyHelpRebuild()
	}
	return nil
}

// step moves the cursor d rows, over headings, stopping at either end.
func (h *keyHelp) step(d int) {
	for i := h.cur + d; i >= 0 && i < len(h.rows); i += d {
		if !h.rows[i].heading {
			h.cur = i
			return
		}
	}
}

// nextSection puts the cursor on the first row of the next section (or
// the one before), round.
func (h *keyHelp) nextSection(d int) {
	var starts []int
	for i, r := range h.rows {
		if r.heading && i+1 < len(h.rows) {
			starts = append(starts, i+1)
		}
	}
	if len(starts) == 0 {
		return
	}
	at := 0
	for i, s := range starts {
		if s <= h.cur {
			at = i
		}
	}
	h.cur = starts[(at+d+len(starts))%len(starts)]
	h.snap = true
}

// keyHelpBox is the guide's body, w wide and at most rows tall: the box
// you type in, then the rows that fit, by section.
func (m *Model) keyHelpBox(w, rows int) []string {
	h := m.keyHelp
	cur := paint(cOrange, "▏")
	field := paint(cText, string(h.query[:h.pos])) + cur + paint(cText, string(h.query[h.pos:]))
	if len(h.query) == 0 {
		field = cur + dim("find a key by its keys or what it does")
	}
	n := 0
	for _, r := range h.rows {
		if !r.heading {
			n++
		}
	}
	count := dim(strconv.Itoa(n) + " key" + plural(n))
	out := []string{spread(paint(cOrange+bold, "❯ ")+field, count, w), faint(strings.Repeat("─", w))}

	// The keys line up in one column, as wide as the widest that fits.
	kw := 0
	for _, r := range h.rows {
		if !r.heading {
			kw = max(kw, min(cellw.String(keyText(r.keys)), 20, w/4))
		}
	}
	kw = max(kw, 8)
	type line struct {
		text    string
		row     int
		section string
	}
	var lines []line
	for i, r := range h.rows {
		if r.heading {
			if i > 0 {
				lines = append(lines, line{"", -1, ""})
			}
			lines = append(lines, line{rule(r.section, "", w), -1, ""})
			continue
		}
		for _, l := range keyHelpLine(r, w, kw, i == h.cur) {
			lines = append(lines, line{l, i, r.section})
		}
	}
	if len(h.rows) == 0 {
		lines = append(lines, line{"", -1, ""}, line{dim("  no key matches · esc clears what's typed"), -1, ""})
	}
	avail := max(3, rows-len(out)-2)
	first, last := 0, 0
	for i, l := range lines {
		if l.row == h.cur {
			if first == 0 || lines[first].row != h.cur {
				first = i
			}
			last = i
		}
	}
	// Keep the cursor on screen, with its heading when it's near the top;
	// a section tab moved to starts at the top.
	if h.snap {
		h.top, h.snap = first-1, false
	}
	if first < h.top+1 {
		h.top = max(0, first-2)
	}
	if last >= h.top+avail {
		h.top = last - avail + 1
	}
	h.top = max(0, min(h.top, len(lines)-avail))
	for i := h.top; i < min(len(lines), h.top+avail); i++ {
		l := lines[i].text
		if i == h.top && lines[i].section != "" {
			l = rule(lines[i].section, "", w) // the section scrolled into stays named
		}
		out = append(out, l)
	}
	for range avail - (min(len(lines), h.top+avail) - h.top) {
		out = append(out, "") // the box stays one size as you type
	}
	out = append(out, "", keysFit(w, "↑↓", "choose", "enter", "change its keys", "tab", "next section", "esc", "close"))
	return out
}

// keyHelpLine is a row: its keys in their column, what it does beside
// them. The row under the cursor shows all of what it does, wrapped.
func keyHelpLine(r keyHelpRow, w, kw int, sel bool) []string {
	var k string
	switch {
	case len(r.keys) == 0:
		k = faint("no key")
	default:
		// As many of its keys as fit, the first brightest.
		k = paint(cText+bold, prettyKey(r.keys[0]))
		used := cellw.String(prettyKey(r.keys[0]))
		for _, o := range r.keys[1:] {
			add := 3 + cellw.String(prettyKey(o))
			if used+add > kw {
				break
			}
			k += faint(" · ") + paint(cSub, prettyKey(o))
			used += add
		}
		if used > kw {
			k = ansi.Truncate(k, kw, "…")
		}
	}
	if r.yours {
		k = paint(cOrange, "•") + k
	} else {
		k = " " + k
	}
	lead := " " + k + strings.Repeat(" ", max(0, kw-cellw.String(ansi.Strip(k))+1)) + "  "
	dw := max(10, w-cellw.String(lead))
	var out []string
	if sel {
		for i, l := range wrap(r.title, dw) {
			if i == 0 {
				out = append(out, highlight(paint(cOrange, "▍")+lead[1:]+paint(cText, l), w))
			} else {
				out = append(out, highlight(strings.Repeat(" ", cellw.String(lead))+paint(cText, l), w))
			}
		}
		return out
	}
	return []string{lead + paint(cSub, ansi.Truncate(r.title, dw, "…"))}
}

// overlayKeyHelp draws the guide over the screen, in the command bar's
// frame.
func (m *Model) overlayKeyHelp(base string) string {
	lines := strings.Split(base, "\n")
	bw := min(m.w-6, 104)
	body := m.keyHelpBox(bw-4, max(10, m.h*4/5)-2)
	e := barEdge{w: bw, h: len(body) + 2, glint: -1}
	note := ""
	if len(m.keyMap().Problems()) > 0 {
		note = paint(cYellow, " keybindings.json has problems: Settings, Keys ")
	}
	return m.dropBox(lines, body, e, paint(cText+bold, " ⌨ Keys "), dim(" "+prettyKey(firstNonEmpty(m.boundKey("guide.open"), "f1"))+" "), note)
}
