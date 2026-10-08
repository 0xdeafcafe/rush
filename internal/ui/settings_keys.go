package ui

import (
	"maps"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/keymap"
)

// Settings, Keys: every action rush's keys do and the keys it has, one
// place at a time (1-9 or ← → pick it). enter takes the keys you press
// next (a chord is several, ended with enter), a adds another, x takes
// them all away, r puts rush's back. What you change goes in
// keybindings.json.

// keysPage is the page's own state: keys being taken for an action.
type keysPage struct {
	taking  string     // the action keys are being taken for
	adding  bool       // added to its keys, rather than replacing them
	pressed keymap.Seq // the keys pressed so far
}

// keyPlaces are the contexts with actions, in the order the strip
// shows them.
func (m *Model) keyPlaces() []keymap.Context {
	km := m.keyMap()
	var out []keymap.Context
	for _, c := range keymap.Contexts {
		for _, a := range km.Actions() {
			if a.Context == c {
				out = append(out, c)
				break
			}
		}
	}
	return out
}

// keyContext is the context Keys shows.
func (m *Model) keyContext() keymap.Context {
	cs := m.keyPlaces()
	if len(cs) == 0 {
		return ""
	}
	return cs[min(m.dialog.keyCtx, len(cs)-1)]
}

// keyRows are the actions of the context Keys shows, rush's before
// plugins'.
func (m *Model) keyRows() []keymap.Action {
	c := m.keyContext()
	var out []keymap.Action
	for _, a := range m.keyMap().Actions() {
		if a.Context == c {
			out = append(out, a)
		}
	}
	return out
}

// showKey puts Keys on the action id: its context, and the cursor on it.
func (m *Model) showKey(id string) {
	a, ok := m.keyMap().Action(id)
	if !ok {
		return
	}
	d := m.dialog
	d.keyCtx = max(0, slices.Index(m.keyPlaces(), a.Context))
	for i, r := range m.keyRows() {
		if r.ID == id {
			d.cursor = i
		}
	}
}

// contextWhere is where a context's keys work, for the page to say.
func contextWhere(c keymap.Context) string {
	switch c {
	case keymap.Global:
		return "anywhere in rush, over sheets and the command bar too"
	case keymap.List:
		return "in the Agents list and the Prompt under it"
	case keymap.Session:
		return "in an agent's Session: its conversation and message box"
	case keymap.Prompt:
		return "in the Agents prompt while a draft is typed"
	case keymap.Pages:
		return "on Settings, Projects and Wall pages"
	case keymap.Any:
		return "in the Agents list or a Session"
	}
	return ""
}

func (m *Model) keysLen() int { return len(m.keyRows()) }

// keyCaps draws an action's keys as keycaps, a chord in one cap, the
// alternatives side by side. lit, when it matches one, lights it: what's
// being practiced.
func keyCaps(seqs []keymap.Seq, lit string) string {
	if len(seqs) == 0 {
		return paint(cText, "Unbound")
	}
	var out []string
	for _, s := range seqs {
		out = append(out, strings.ReplaceAll(keycap(s.String(), lit != "" && s.String() == lit), cSub, cText))
	}
	return strings.Join(out, paint(cText, " or "))
}

// keysControls wraps whole controls; none disappear when the page is narrow.
func keysControls(w int, pairs ...string) []string {
	var out []string
	line := ""
	for i := 0; i+1 < len(pairs); i += 2 {
		item := paint(cText+bold, pairs[i]) + " " + paint(cText, pairs[i+1])
		if line != "" && cellw.String(line)+3+cellw.String(item) > w {
			out = append(out, line)
			line = ""
		}
		if line != "" {
			line += " · "
		}
		line += item
	}
	if line != "" {
		out = append(out, line)
	}
	return out
}

func (m *Model) keysBody(w int) []string {
	d, km := m.dialog, m.keyMap()
	ctxs, ctx, rows := m.keyPlaces(), m.keyContext(), m.keyRows()
	d.cursor = min(d.cursor, max(0, len(rows)-1))
	budget := max(1, m.h-len(m.header())-6) // frame chrome plus page title
	out := []string{paint(cText+bold, ctx.Title()) + dim(" · "+strconv.Itoa(d.keyCtx+1)+"/"+strconv.Itoa(len(ctxs))) + "  " + paint(cText, "← → change context")}
	guidance := wrap(paint(cText, contextWhere(ctx)), w)
	out = append(out, guidance...)
	footer := keysControls(w, "enter", "Edit binding", "a", "Add alternative", "x", "Unbind", "r", "Reset default")
	footer = append(footer, keysControls(w, "↑ ↓", "Select action", "[ ]", "Settings page", "esc", "Back")...)
	if len(rows) > 0 {
		a := rows[d.cursor]
		detail := wrap(paint(cText, a.Title)+" · "+keyCaps(km.Keys(a.ID), ""), w)
		footer = append(detail, footer...)
	}
	if budget >= 20 {
		footer = append([]string{dim("Try a shortcut to find its action. • marks a custom binding.")}, footer...)
	}
	if problems := km.Problems(); len(problems) > 0 {
		out = append(out, wrap(paint(cYellow, strconv.Itoa(len(problems))+" invalid bindings: "+problems[0].String()), w)...)
	}
	practicing := ""
	if len(m.practiced(d.practice)) > 0 {
		practicing = d.practice.String()
	}
	keysOf := func(a keymap.Action) string {
		if kp := m.keysTaking(); kp != nil && kp.taking == a.ID {
			return paint(cOrange, "press keys…")
		}
		return keyCaps(km.Keys(a.ID), practicing)
	}
	// Keys first, as wide as the widest, so the eye runs down them.
	keyW, titleW := 8, 6
	for _, a := range rows {
		keyW = max(keyW, cellw.String(keysOf(a)))
		titleW = max(titleW, cellw.String(a.Title))
	}
	keyW = min(keyW, max(12, min(30, w/3)))
	actionW := max(8, w-keyW-5)
	tableW := min(w, 2+keyW+3+titleW)
	// The keyboard never takes an action row: beside the table where the
	// screen is wide, else under it when every row already shows.
	kb := m.keysBoard(rows, practicing, w-tableW-4)
	side := w-tableW-4 >= kbWide
	rowW := w
	if side {
		actionW, rowW = titleW, tableW
	}
	out = append(out, paint(cText+bold, "  "+fit("BINDING", keyW)+"   ACTION"))
	// On very short screens preserve the action and editing controls first.
	if len(out)+len(footer)+2 > budget && len(out) > 2 {
		out = append(out[:1], out[len(out)-1])
	}
	if len(out)+len(footer)+2 > budget && len(footer) > 2 {
		footer = footer[len(footer)-(max(1, budget-len(out)-2)):]
	}
	// Reserve footer space first. The action window gets everything remaining.
	n := max(1, budget-len(out)-len(footer)-1)
	from, to := window(len(rows), d.cursor, n)
	d.keyTop, d.keyFrom, d.keyTo = len(out), from, to
	for i := from; i < to; i++ {
		a := rows[i]
		mark := "  "
		if km.Changed(a.ID) {
			mark = paint(cBlue, "• ")
		}
		line := mark + fit(keysOf(a), keyW) + "   " + fit(paint(cText, a.Title), actionW)
		switch {
		case i == d.cursor:
			line = highlight(line, rowW)
		case i == d.keyHover-1:
			line = hoverBG + strings.ReplaceAll(fit(line, rowW), reset, reset+hoverBG) + reset
		}
		out = append(out, line)
	}
	rangeText := strconv.Itoa(from+1) + "–" + strconv.Itoa(to) + " of " + strconv.Itoa(len(rows)) + " actions"
	out = append(out, dim(rangeText))
	if side && to-from+2 >= len(kb) {
		for j, l := range kb {
			y := d.keyTop - 1 + j
			out[y] = fit(out[y], tableW) + "    " + l
		}
	} else if from == 0 && to == len(rows) && len(out)+len(kb)+1+len(footer) <= budget && w >= kbWide+2 {
		out = append(out, "")
		for _, l := range kb {
			out = append(out, "  "+l)
		}
	}
	out = append(out, footer...)
	for i := range out {
		out[i] = fit(out[i], w)
	}
	return out
}

// keysBoard is the keyboard for rows: the hovered or selected action's
// keys lit, the key being practiced brightest, every key bound here plain.
func (m *Model) keysBoard(rows []keymap.Action, practicing string, w int) []string {
	d, km := m.dialog, m.keyMap()
	var lit []keymap.Seq
	switch {
	case d.keyHover > 0 && d.keyHover <= len(rows):
		lit = km.Keys(rows[d.keyHover-1].ID)
	case d.cursor < len(rows):
		lit = km.Keys(rows[d.cursor].ID)
	}
	hit, on, used := map[string]bool{}, map[string]bool{}, map[string]bool{}
	if practicing != "" {
		hit = keyParts(d.practice)
	}
	for _, s := range lit {
		maps.Copy(on, keyParts(s))
	}
	for _, a := range rows {
		for _, s := range km.Keys(a.ID) {
			maps.Copy(used, keyParts(s))
		}
	}
	return append(keyboard(hit, on, used), "", fit(kbLegend(), max(kbWide, w)))
}

// keysModal asks for the keys being taken in a box over base.
func (m *Model) keysModal(base string) string {
	kp := m.keysTaking()
	a, _ := m.keyMap().Action(kp.taking)
	bw := min(m.w-4, 64)
	verb := "New keys for "
	if kp.adding {
		verb = "Another key for "
	}
	pressed := faint("…")
	if len(kp.pressed) > 0 {
		pressed = keycap(kp.pressed.String(), true)
	}
	body := []string{paint(cText+bold, verb+a.Title), faint(a.ID), "",
		dim(fit("Now", 10)) + keyCaps(m.keyMap().Keys(a.ID), ""),
		dim(fit("Pressed", 10)) + pressed, ""}
	for _, l := range wrap("Press the key, or up to three for a chord (ctrl+x then p, say).", bw-4) {
		body = append(body, dim(l))
	}
	if a.Context != keymap.Global {
		for _, l := range wrap("A key that types a character, p say, can't be one on its own here: start a chord with it instead.", bw-4) {
			body = append(body, dim(l))
		}
	}
	body = append(body, "", keys("enter", "keep", "esc", "cancel"))
	return m.modalOver(base, body, bw, cOrange)
}

// keysTaking is the keys being taken, if they are.
func (m *Model) keysTaking() *keysPage {
	if m.keys.page.taking == "" {
		return nil
	}
	return &m.keys.page
}

func (m *Model) keysKey(s string) tea.Cmd {
	rows := m.keyRows()
	d := m.dialog
	if d.cursor >= len(rows) {
		return nil
	}
	a := rows[d.cursor]
	switch s {
	case "left", "right", "h", "l":
		n := len(m.keyPlaces())
		d.keyCtx = (min(d.keyCtx, n-1) + map[bool]int{true: -1, false: 1}[s == "left" || s == "h"] + n) % n
		d.cursor = 0
	case "1", "2", "3", "4", "5", "6", "7", "8", "9":
		if i := int(s[0] - '1'); i < len(m.keyPlaces()) {
			d.keyCtx, d.cursor = i, 0
		}
	case "enter", "a":
		m.keys.page = keysPage{taking: a.ID, adding: s == "a"}
		m.keys.capture = m.takeKey
		return nil
	case "x", "backspace", "delete":
		return m.saveBinding(a.ID, []string{})
	case "r":
		return m.saveBinding(a.ID, nil)
	default:
		return m.practiceKey(s)
	}
	return nil
}

// takeKey is each key pressed while keys are being taken: every key comes
// here first, unmapped.
func (m *Model) takeKey(s string) tea.Cmd {
	kp := &m.keys.page
	done := func() tea.Cmd {
		id, seq, adding := kp.taking, kp.pressed, kp.adding
		m.keys.page = keysPage{}
		if len(seq) == 0 {
			return nil
		}
		var keys []string
		if adding {
			for _, k := range m.keyMap().Keys(id) {
				keys = append(keys, k.String())
			}
		}
		keys = append(keys, seq.String())
		if a, _ := m.keyMap().Action(id); keymap.Types(seq[0]) && a.Context != keymap.Global && !slices.Contains(a.Keys, seq[0]) {
			m.flash(seq[0]+" types a character, so it can't start a key: make it a chord, ctrl+x then "+seq[0]+" say", true)
			return nil
		}
		if c := m.keyMap().Conflicts(id, seq); len(c) > 0 {
			m.confirmThen(seq.String()+" is "+strings.Join(c, ", ")+"'s now: take it?", func() tea.Cmd { return m.saveBinding(id, keys) })
			return nil
		}
		return m.saveBinding(id, keys)
	}
	switch {
	case s == "esc":
		m.keys.page = keysPage{}
		return nil
	case s == "enter" && len(kp.pressed) > 0:
		return done()
	}
	kp.pressed = append(kp.pressed, s)
	if len(kp.pressed) == 3 {
		return done()
	}
	m.keys.capture = m.takeKey // the next key too
	return nil
}

// saveBinding sets id's keys (nil for rush's) and writes
// keybindings.json, off the UI.
func (m *Model) saveBinding(id string, keys []string) tea.Cmd {
	f := m.keys.file.With(id, keys)
	m.setKeys(f)
	switch {
	case keys == nil:
		m.flash(id+": rush's keys again", false)
	case len(keys) == 0:
		m.flash(id+": no key", false)
	default:
		m.flash(id+": "+m.keyMap().KeyText(id), false)
	}
	return sheetDo(func() (struct{}, error) { return struct{}{}, keymap.Save(f) }, func(m *Model, _ struct{}, err error) tea.Cmd {
		if err != nil {
			m.flash("keybindings.json: "+err.Error(), true)
		}
		return nil
	})
}

// practiceKey is a key pressed on Keys that isn't the page's own: tried
// against every action's keys (a chord's first key waits for the rest, up
// to chordWait), so a key can be pressed to see, live, what it's bound to.
// A match jumps the page to it and lights its chip; a miss clears back to
// nothing, ready for the next key tried.
func (m *Model) practiceKey(s string) tea.Cmd {
	d := m.dialog
	if time.Since(d.practiceAt) > chordWait {
		d.practice = nil
	}
	d.practiceAt = time.Now()
	d.tried, d.triedAt = keymap.Seq{s}, d.practiceAt
	d.keyHover = 0 // the key pressed shows over the row the pointer rests on, till it moves
	try := func(seq keymap.Seq) bool {
		if a := m.practiced(seq); len(a) > 0 {
			d.practice = seq
			m.showKey(a[0].ID)
			return true
		}
		if len(seq) < 3 && m.practicedPrefix(seq) {
			d.practice = seq
			return true
		}
		return false
	}
	if seq := append(slices.Clone(d.practice), s); try(seq) {
		d.tried = seq
	} else if !try(keymap.Seq{s}) {
		d.practice = nil
	}
	return tea.Tick(kbFlash, func(time.Time) tea.Msg { return kbFrameMsg{} }) // its flash goes out
}

// kbFlash is how long the key just pressed stays lit on the keyboard.
const kbFlash = 600 * time.Millisecond

// kbFrameMsg expires the practice highlight; no animation loop is needed.
type kbFrameMsg struct{}

func (m *Model) onKbFrame() tea.Cmd { return nil }

// practiced are the actions whose keys are seq, exactly.
func (m *Model) practiced(seq keymap.Seq) []keymap.Action {
	if len(seq) == 0 {
		return nil
	}
	s := seq.String()
	var out []keymap.Action
	for _, a := range m.keyMap().Actions() {
		for _, k := range m.keyMap().Keys(a.ID) {
			if k.String() == s {
				out = append(out, a)
				break
			}
		}
	}
	return out
}

// practicedPrefix is whether seq begins a longer chord bound to something.
func (m *Model) practicedPrefix(seq keymap.Seq) bool {
	s := seq.String()
	for _, a := range m.keyMap().Actions() {
		for _, k := range m.keyMap().Keys(a.ID) {
			if full := k.String(); len(full) > len(s) && strings.HasPrefix(full, s) && full[len(s)] == ' ' {
				return true
			}
		}
	}
	return false
}

// keysHover finds the row of Keys under the pointer at y, reporting
// whether that changed.
func (m *Model) keysHover(y int) bool {
	d := m.dialog
	i := y - m.headH() - 4 - d.keyTop + d.keyFrom // under the header, the pages and the page's name
	hover := 0
	if i >= d.keyFrom && i < d.keyTo {
		hover = i + 1
	}
	changed := hover != d.keyHover
	d.keyHover = hover
	return changed
}
