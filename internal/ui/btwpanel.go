package ui

import (
	"strings"
	"time"
	"weak"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// --- /btw: side questions in a floating panel ---

// A btw thread is a side conversation with Claude about the agent's work:
// answered from the conversation so far (Claude Code's side_question),
// without adding to it or stopping the turn under way. It floats over the
// Session's top right, so the conversation and the message box stay in
// use while an answer comes; ctrl+b (for btw) moves the keys between the
// two. Pulled
// out (ctrl+f), it becomes an agent of its own: a fork of the conversation
// whose first message carries the side thread on.
//
// A follow-up goes where you say, each time: on in the side thread, or
// into the conversation, to the agent itself, with the side thread ahead
// of it as context (toMain).
type btwThread struct {
	qa      []btwQA
	input   []rune
	pos     int
	waiting time.Time              // when the question out was asked; zero when none
	asked   weak.Pointer[hostConn] // the connection it was asked on: its answer comes there; weak, so a closed one isn't kept
	err     string
	scroll  int          // rows up from the latest
	focused bool         // it has the keys, not the message box
	at      [4]int       // where it was drawn: x, y, w, h on screen
	shown   []convo.Line // each row drawn, inside the edge
	sel     textSel      // text dragged over, in rows of shown

	// links are the links drawn (a catch-up's, to the messages it cites),
	// in reading order, each on a row of the thread; pick is the one the
	// keys are on, -1 for none, and pickMoved says to scroll it into view.
	links     []btwLink
	pick      int
	pickMoved bool
	// choosing is a follow-up typed and waiting to be told where it goes;
	// toMain is the answer, kept from one follow-up to the next.
	choosing, toMain bool
}

type btwQA struct {
	Question, Response string
	catch              *catchup // a /catchup: its question is the fixed prompt
}

// asked is the question as you'd say it: a catch-up's is its command.
func (x btwQA) asked() string {
	if x.catch != nil {
		return "/catchup"
	}
	return x.Question
}

// answered is the answer as text: a catch-up's sections as markdown.
func (x btwQA) answered() string {
	if x.catch != nil {
		return x.catch.markdown(x.Response)
	}
	return x.Response
}

// hasAnswer is whether anything in the thread has been answered: from
// then on what you type is a follow-up.
func (t *btwThread) hasAnswer() bool {
	for _, x := range t.qa {
		if x.Response != "" {
			return true
		}
	}
	return false
}

// btwFor is the agent's side thread, if it has one open.
func (m *Model) btwFor(key string) *btwThread { return m.btws[key] }

// openBtw opens the agent's side thread (or the one already open), with
// the keys, and asks q when there is one.
func (m *Model) openBtw(c *hostConn, q string) tea.Cmd {
	if m.btws == nil {
		m.btws = map[string]*btwThread{}
	}
	t := m.btws[c.key]
	if t == nil {
		t = &btwThread{pick: -1}
		m.btws[c.key] = t
	}
	t.focused, m.paneFocus = true, true
	if q = strings.TrimSpace(q); q == "" {
		return nil
	}
	return t.ask(m, c, q)
}

func (t *btwThread) ask(m *Model, c *hostConn, q string) tea.Cmd { return t.askWith(m, c, q, nil) }

// askWith asks q, as the catch-up ct when it's one.
func (t *btwThread) askWith(m *Model, c *hostConn, q string, ct *catchup) tea.Cmd {
	if c.client == nil && c.sleeping {
		// Its host rests once the agent has been idle a while, which is
		// when you come back to ask: woken, it answers.
		return m.wakeHostThen(c, func(m *Model, c *hostConn) tea.Cmd { return t.askWith(m, c, q, ct) })
	}
	var history []map[string]string
	for _, x := range t.qa {
		if x.Response != "" {
			history = append(history, map[string]string{"question": x.Question, "response": x.Response})
		}
	}
	if history == nil {
		history = []map[string]string{}
	}
	cmd := m.askClaude(c, map[string]any{"subtype": "side_question", "question": q, "history": history}, func(m *Model, r host.Reply) tea.Cmd {
		t.waiting, t.sel = time.Time{}, textSel{} // the rows under it moved
		var a struct {
			Response string `json:"response"`
		}
		switch {
		case r.Error != "":
			t.err = r.Error
		case jsonx.Unmarshal(r.Body, &a) != nil || strings.TrimSpace(a.Response) == "":
			t.err = "no answer came back: ask again, or ask in the conversation"
		default:
			t.qa[len(t.qa)-1].Response = a.Response
			if ct != nil {
				ct.out, ct.at = parseCatchup(a.Response, len(ct.index)), time.Now()
				t.scroll = 1 << 20 // read from its top, not from its last line
			}
			t.keep(c.id) // a catch-up, or a follow-up under one
		}
		return nil
	})
	if cmd == nil {
		return nil // askClaude said why
	}
	t.qa = append(t.qa, btwQA{Question: q, catch: ct})
	t.waiting, t.asked, t.err, t.scroll, t.sel = time.Now(), weak.Make(c), "", 0, textSel{}
	t.pick, t.choosing = -1, false
	return cmd
}

// btwKey handles keys meant for the side thread: all of them while it has
// the keys, and ctrl+b, which moves them there, while it hasn't (bar in a
// memory file, where it's bold).
func (m *Model) btwKey(c *hostConn, k tea.KeyPressMsg, s string) (tea.Cmd, bool) {
	t := m.btwFor(c.key)
	if t == nil || !t.focused {
		switch {
		case s == "ctrl+b" && !c.memEdit:
			return m.openBtw(c, ""), true
		case s == "ctrl+d" && t != nil && len(c.input) == 0:
			delete(m.btws, c.key) // tucked away, it closes from the chat too
			return nil, true
		}
		return nil, false
	}
	if t.choosing {
		// A follow-up waits to be told where it goes.
		switch s {
		case "left", "right", "up", "down", "tab", "shift+tab":
			t.toMain = !t.toMain
			return nil, true
		case "esc":
			t.choosing = false
			return nil, true
		case "enter":
		default:
			t.choosing = false // typing on: it's asked again at the next enter
		}
	}
	switch s {
	case "super+c", "ctrl+c":
		// A selection in the panel is copied; ctrl+c with none still
		// reaches the Session.
		if m.copyBtwSel(t) || s == "super+c" {
			return nil, true
		}
		return nil, false
	case "esc", "ctrl+b":
		// Back to the conversation. A thread with something in it stays,
		// tucked away; one never asked anything goes.
		t.focused, t.sel = false, textSel{}
		if len(t.qa) == 0 {
			delete(m.btws, c.key)
		}
	case "ctrl+d":
		delete(m.btws, c.key)
	case "enter":
		q := strings.TrimSpace(string(t.input))
		if q == "" {
			if t.pick >= 0 && t.pick < len(t.links) {
				m.followSaid(c, t, t.links[t.pick].target)
			}
			return nil, true
		}
		if !t.waiting.IsZero() {
			m.flash("one question at a time: the last one's still being answered", true)
			return nil, true
		}
		if t.hasAnswer() && !t.choosing {
			t.choosing = true // a follow-up: here, or to the agent itself
			return nil, true
		}
		if t.choosing && t.toMain {
			t.choosing = false
			cmd := m.toMain(c, t, q)
			if m.btwFor(c.key) == nil || len(c.input) > 0 {
				t.input, t.pos = nil, 0
			}
			return cmd, true
		}
		t.choosing = false
		t.input, t.pos = nil, 0
		return t.ask(m, c, q), true

	case "ctrl+f":
		return m.forkBtw(c, t), true
	case "ctrl+r":
		// A catch-up asked again, whatever is kept of the last one.
		if t.lastCatch() < 0 {
			return nil, false
		}
		if !t.waiting.IsZero() {
			m.flash("one question at a time: the last one's still being answered", true)
			return nil, true
		}
		return m.openCatchup(c, true), true
	case "ctrl+s":
		// The last answer into the message box, to send after all.
		if n := len(t.qa); n > 0 && t.qa[n-1].Response != "" {
			last := t.qa[n-1]
			c.input, c.back = []rune("About my side question (\""+last.asked()+"\"), you said:\n\n"+last.answered()+"\n\n"), 0
			t.focused, t.sel = false, textSel{}
		}
	case "up", "down":
		// With links drawn, the arrows move between them, the panel
		// scrolling to follow; the page keys scroll.
		if len(t.links) > 0 {
			t.movePick(map[string]int{"down": 1, "up": -1}[s])
			t.sel = textSel{}
			return nil, true
		}
		if s == "up" {
			t.scroll++
		} else {
			t.scroll = max(0, t.scroll-1)
		}
		t.sel = textSel{}
	case "pgup":
		t.scroll += 8
		t.sel = textSel{}
	case "pgdown":
		t.scroll = max(0, t.scroll-8)
		t.sel = textSel{}
	default:
		if in, pos, ok := edit(t.input, t.pos, k, s); ok {
			t.input, t.pos = in, pos
			return nil, true
		}
		return nil, false // ctrl+x and the like still reach the Session
	}
	return nil, true
}

// forkBtw pulls the side thread out into an agent of its own: /fork of the
// whole conversation, its first message the side thread so far.
func (m *Model) forkBtw(c *hostConn, t *btwThread) tea.Cmd {
	a := m.agentByKey(c.key)
	if a == nil || len(t.qa) == 0 {
		return nil
	}
	name := oneLine(t.qa[0].asked())
	if r := []rune(name); len(r) > 40 {
		name = string(r[:39]) + "…"
	}
	m.openFork(c, a, "btw: "+name)
	f, ok := m.sheet.(*forkSheet)
	if !ok {
		return nil
	}
	var b strings.Builder
	b.WriteString("While you were working I asked some side questions. Let's carry that thread on here.\n")
	b.WriteString(t.carried())
	f.first, f.row = []rune(b.String()), fkFirst
	f.firstPos = len(f.first)
	t.focused = false
	return nil
}

// clickBtw gives the side thread the keys when it's clicked, and starts a
// drag over its text there. A click elsewhere drops what was dragged over.
func (m *Model) clickBtw(c *hostConn, x, y int) bool {
	t := m.btwFor(c.key)
	if t == nil {
		return false
	}
	if t.at[2] == 0 || x < t.at[0] || x >= t.at[0]+t.at[2] || y < t.at[1] || y >= t.at[1]+t.at[3] {
		t.sel = textSel{}
		return false
	}
	at := t.cellAt(x, y)
	if at.row < len(t.shown) && m.followSaid(c, t, linkAt(t.shown[at.row].Text, at.col)) {
		return true // a link to a message: the conversation goes there
	}
	t.focused, m.paneFocus = true, true
	t.sel = textSel{drag: true, a: at, b: at}
	if m.dbl && at.row < len(t.shown) {
		t.sel.selectWord(t.shown[at.row].Text)
	}
	return true
}

// cellAt is the cell of the panel's text under the pointer, clamped to it.
func (t *btwThread) cellAt(x, y int) cell {
	return cell{
		row: min(max(y-t.at[1], 0), max(0, t.at[3]-1)),
		col: min(max(x-t.at[0]-2, 0), max(0, t.at[2]-3)), // past the edge and its space
	}
}

// drag moves the end of a drag in the panel to the pointer.
func (t *btwThread) drag(x, y int) {
	if at := t.cellAt(x, y); at != t.sel.b {
		t.sel.b = at
		t.sel.moved, t.sel.on = true, true
	}
}

// endBtwDrag finishes a drag in the panel: what it covered goes to the
// clipboard, or, with CopyOnSelect off, stays selected for cmd+c.
func (m *Model) endBtwDrag(t *btwThread) {
	t.sel.drag = false
	if !t.sel.moved {
		t.sel = textSel{}
		return
	}
	if m.store.Config.CopiesOnSelect() {
		m.copyBtwSel(t)
	}
}

// copyBtwSel copies the text dragged over in the panel, reporting whether
// there was any.
func (m *Model) copyBtwSel(t *btwThread) bool {
	if !t.sel.on {
		return false
	}
	if txt := selectedText(t.shown, t.sel.a, t.sel.b, t.at[2]-3); txt != "" {
		m.copyText(txt)
	}
	return true
}

// btwDragging is the side thread a drag is under way in, if any, or the
// quick ask's.
func (m *Model) btwDragging() *btwThread {
	if t := m.quick.thread(); t != nil && t.sel.drag {
		return t // the quick ask's, which is btw's too
	}
	if m.host == nil {
		return nil
	}
	if t := m.btwFor(m.host.key); t != nil && t.sel.drag {
		return t
	}
	return nil
}

var bgBtw string // a raised surface: it floats over the conversation

// btwOverlay draws the agent's side thread over the pane's rows from top
// down, at its right; w is the pane's width and room how many rows it may
// take.
func (m *Model) btwOverlay(c *hostConn, out []string, top, room, w int) {
	t := m.btwFor(c.key)
	if t == nil {
		return
	}
	if !t.focused && len(t.qa) == 0 {
		delete(m.btws, c.key) // nothing asked, and you've gone back to the chat
		return
	}
	if !t.waiting.IsZero() && t.asked.Value() != c {
		// Asked on a connection since closed (you went to another agent):
		// its answer went with it.
		t.waiting, t.err = time.Time{}, "the answer was lost when you left this agent: ask again"
	}
	pw := min(w-2, max(44, w*45/100))
	if w < 70 {
		pw = w - 2
	}
	maxH := room - 1
	if !t.focused {
		maxH = min(room-1, 11)
	}
	panel := t.lines(c, pw, maxH, m.paneFocus)
	if len(panel) == 0 || room < 5 {
		return
	}
	leftW := w - pw - 1
	for i, p := range panel {
		r := top + i
		if r >= len(out) {
			break
		}
		left := fit(ansi.Truncate(out[r], leftW, ""), leftW)
		out[r] = left + reset + " " + p
	}
	t.at = [4]int{m.paneX() + leftW + 1, m.paneTop + top, pw, len(panel)}
}

// lines draws the panel pw wide and at most maxH tall: a title, the
// thread (the latest at the bottom), the question box and its keys.
func (t *btwThread) lines(c *hostConn, pw, maxH int, paneFocused bool) []string {
	on := t.focused && paneFocused
	edge := faint("▏")
	if on {
		edge = paint(cOrange, "▍")
	}
	iw := pw - 3 // inside the edge and a space either side
	row := func(s string) string { return onBg(bgBtw, edge+" "+s, pw) }

	var thread []convo.Line
	add := func(s string) { thread = append(thread, convo.Line{Text: s}) }
	qa := t.qa
	if !t.focused && len(qa) > 1 {
		qa = qa[len(qa)-1:] // tucked away, it shows the latest only
	}
	now := time.Now()
	for i, x := range qa {
		if i > 0 {
			add("")
		}
		at := len(t.qa) - len(qa) + i // its place in the whole thread
		for j, l := range wrap(x.asked(), iw-2) {
			lead := paint(cOrange, "❯ ")
			if j > 0 {
				lead = "  "
			}
			add(lead + paint(cText+bold, l))
		}
		if x.catch != nil {
			for _, l := range wrap(x.catch.scope(now), iw-2) {
				add("  " + dim(l))
			}
		}
		last := i == len(qa)-1
		switch {
		case x.catch != nil && x.catch.out != nil:
			add("")
			thread = append(thread, x.catch.rows(at, iw, now)...)
		case x.Response != "":
			rows := c.sess.Answer(x.Response, iw)
			if k := t.catchBefore(at); k >= 0 {
				rows = citeLinks(rows, t.qa[k].catch, at)
			}
			thread = append(thread, rows...)
		case last && !t.waiting.IsZero():
			add(dim("  thinking · " + dur(time.Since(t.waiting))))
		case last && t.err != "":
			for _, l := range wrap(t.err, iw-2) {
				add("  " + paint(cRed, l))
			}
		}
	}
	if len(t.qa) == 0 {
		add(dim("ask about what's going on: it doesn't go into the conversation"))
	}

	// The links drawn, and the one the keys are on.
	t.links = findLinks(thread)
	if t.pick >= len(t.links) {
		t.pick = -1
	}
	pickRow := -1
	if t.pick >= 0 && t.focused {
		l := t.links[t.pick]
		pickRow, thread[l.row].Text = l.row, litLink(thread[l.row].Text, l.nth)
	}

	head := paint(cOrange+bold, "btw") + dim("  side question · not in the conversation")
	if len(t.qa) > 0 && t.qa[0].catch != nil {
		head = paint(cOrange+bold, "catchup") + dim("  since your last message · not in the conversation")
	}
	var foot []string
	switch {
	case t.focused && t.choosing:
		foot = append(foot, paint(cOrange, "❯ ")+textField(t.input, t.pos, false, "ask…", iw-2))
		opt := func(on bool, label string) string {
			if on {
				return paint(cOrange, "● ") + paint(cText+bold, label)
			}
			return faint("○ ") + dim(label)
		}
		foot = append(foot, dim("where does this follow-up go?"))
		foot = append(foot, opt(!t.toMain, "here, in the side chat"))
		foot = append(foot, opt(t.toMain, "the main chat, with this side chat as context"))
		foot = append(foot, keysFit(iw, "↑↓", "choose", "enter", "send", "esc", "keep typing"))
	case t.focused:
		foot = append(foot, paint(cOrange, "❯ ")+textField(t.input, t.pos, on, "ask…", iw-2))
		var again []string // first on its row: the last to go when it's narrow
		if t.lastCatch() >= 0 {
			again = []string{"ctrl+r", "catch up again"}
		}
		if len(t.links) > 0 {
			foot = append(foot, keysFit(iw, "↑↓", "links", "enter", "go to the message", "pgup pgdn", "scroll"))
			foot = append(foot, keysFit(iw, "enter", "ask what you typed", "esc", "back to the chat"))
		} else {
			foot = append(foot, keysFit(iw, "enter", "ask", "↑↓", "scroll", "esc", "back to the chat"))
		}
		if len(t.qa) > 0 {
			foot = append(foot, keysFit(iw, append(again, "ctrl+f", "its own chat", "ctrl+s", "into the box", "ctrl+d", "close")...))
		} else {
			foot = append(foot, keysFit(iw, "ctrl+d", "close"))
		}
	default:
		foot = append(foot, keysFit(iw, "ctrl+b", "ask more", "ctrl+d", "close"))
	}

	room := max(1, maxH-2-len(foot))
	switch {
	case len(thread) <= room:
	case !t.focused:
		// Tucked away: the latest question and the start of its answer.
		thread = append(thread[:room-1], convo.Line{Text: dim("  … ctrl+b reads the rest")})
	default:
		// Anchored to its latest line; ↑ scrolls back. A link the keys just
		// moved to is scrolled into view.
		if end := len(thread) - t.scroll; t.pickMoved && pickRow >= 0 && (pickRow < end-room || pickRow >= end) {
			t.scroll = len(thread) - min(len(thread), pickRow+max(1, room/2)+1)
		}
		t.scroll = max(0, min(t.scroll, len(thread)-room))
		end := len(thread) - t.scroll
		thread = thread[max(0, end-room):end]
	}
	t.pickMoved = false
	t.shown = append([]convo.Line{{Text: head}, {}}, thread...)
	for _, l := range foot {
		t.shown = append(t.shown, convo.Line{Text: l})
	}
	out := make([]string, len(t.shown))
	for i, l := range t.shown {
		out[i] = row(l.Text)
		if from, to, ok := t.sel.cols(i, iw); ok {
			out[i] = paintCols(out[i], from+2, to+2) // past the edge and its space
		}
	}
	return out
}
