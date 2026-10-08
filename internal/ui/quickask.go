package ui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// --- quick ask: the guide, a question from anywhere ---

// A quick ask is a question for rush's guide, a fast, cheap model that
// belongs to no chat: it reads the sessions as the list has them and
// rush's guide, and opens the session it points at. It floats over the top right of whatever is showing, keeps its
// thread for follow-ups, and can go into a chat's box later as one
// paste. Its threads are btw's (input, scrolling, text selection); only
// who answers and where it's drawn differ.
type quickAsk struct {
	threads []*btwThread // the last few, the one in use at cur
	cur     int
	shown   bool
	model   string        // the model that answered last, for the title
	talk    convo.Session // draws the answers' markdown
}

// quickKeep is how many threads are kept, in memory only.
const quickKeep = 5

const quickSystem = "You are rush's guide. rush is the terminal app the user runs and watches their coding agents in; " +
	"each message brings their sessions as rush's list has them, a number each. " +
	"Help them find their way: what needs them next, which session is doing what, where something is, how to do something in rush. " +
	"Sessions marked NEEDS YOU come first, then ones waiting on them, then ones that finished. " +
	"Don't make decisions for them: point, summarise in a few lines, and say what they'd do there. " +
	"For a fuller summary of one session, tell them to open it and ask its own agent. " +
	"When your answer is about one session, end with a line of its own: open: N, and rush opens it for them. " +
	"They may ask something unrelated to rush too: answer it briefly. " +
	"Be brief and direct, plain text or light markdown, no preamble. Answer rush questions from its guide, and say when it doesn't cover something."

// guideOpen is the guide's line asking rush to open a session.
var guideOpen = regexp.MustCompile(`(?mi)^\s*open:\s*\[?(\d+)\]?\s*$\n?`)

// guideFleet is the sessions as the guide reads them, numbered from 1, and
// the keys those numbers stand for.
// ponytail: the first 80 in list order; rank or filter if fleets outgrow that.
func (m *Model) guideFleet() (string, []string) {
	var b strings.Builder
	var keys []string
	now := time.Now()
	for _, a := range m.order {
		if a.Advisor || len(keys) == 80 {
			continue
		}
		keys = append(keys, a.Key)
		state := a.State
		switch {
		case a.NeedsYou():
			state = "NEEDS YOU"
		case a.Waiting():
			state = "waiting on you (seen)"
		case a.Past:
			state = "past"
		}
		fmt.Fprintf(&b, "[%d] %s · %s · %s ago", len(keys), a.DisplayName, state, a.Age(now).Round(time.Minute))
		if a.Repo != "" {
			b.WriteString(" · " + filepath.Base(a.Repo))
		}
		if a.Branch != "" {
			b.WriteString(" @" + a.Branch)
		}
		if a.Todos > 0 {
			fmt.Fprintf(&b, " · todos %d/%d", a.TodosDone, a.Todos)
		}
		for _, x := range []string{a.Needs, a.Intent, a.Detail} {
			if x = strings.Join(strings.Fields(x), " "); x != "" {
				b.WriteString(" · " + fit(x, 200))
			}
		}
		b.WriteString("\n")
	}
	if len(keys) == 0 {
		return "The user has no sessions in rush yet.", nil
	}
	if c := m.focused(); c != nil {
		b.WriteString("\nThe one selected now: " + c.DisplayName + "\n")
	}
	return "Their sessions:\n" + b.String(), keys
}

// guideOpens takes the guide's open: N off its answer and the key N stood
// for, "" when it pointed at none.
func guideOpens(answer string, keys []string) (string, string) {
	found := guideOpen.FindAllStringSubmatch(answer, -1)
	if len(found) == 0 {
		return answer, ""
	}
	answer = strings.TrimSpace(guideOpen.ReplaceAllString(answer, ""))
	n, _ := strconv.Atoi(found[len(found)-1][1])
	if n < 1 || n > len(keys) {
		return answer, ""
	}
	return answer, keys[n-1]
}

// openGuide is #guide: the float with the keys, asking q when there is one.
func (m *Model) openGuide(q string) tea.Cmd {
	t := m.quick.thread()
	if t == nil || q != "" {
		t = m.quick.newThread()
	}
	m.quick.shown, t.focused = true, true
	if q = strings.TrimSpace(q); q != "" {
		return m.askQuick(t, q)
	}
	return nil
}

// quickAsker answers prompt with the cheapest model rush can run without
// a session (Claude Code's haiku, else a local Ollama model), and says
// which. A variable so tests don't spend a model call.
var quickAsker = func(prompt string) (string, string, error) {
	model, out, err := askCheap(quickSystem+"\n\n<rush-guide>\n"+rush.Guide+"\n</rush-guide>", prompt, "")
	if err != nil && strings.Contains(err.Error(), "too small") {
		// A local model that can't hold the guide answers without it.
		return askCheap(quickSystem, prompt, "")
	}
	return model, out, err
}

// askCheap asks the cheapest model rush can run without a session, the
// first of its models whose name has prefer when one does.
func askCheap(system, prompt, prefer string) (string, string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	for _, k := range []agent.Kind{agent.LegacyKind, "ollama"} {
		a, ok := agent.Get(k)
		sm, can := a.(agent.Summarizer)
		if !ok || !can || !agent.Installed(k) {
			continue
		}
		ms, err := sm.SummaryModels(ctx)
		if err != nil || len(ms) == 0 {
			continue
		}
		pick := ms[0]
		for _, x := range ms {
			if prefer != "" && strings.Contains(x, prefer) {
				pick = x
				break
			}
		}
		out, err := sm.Summarize(ctx, pick, system, prompt)
		return pick, out, err
	}
	return "", "", errors.New("no model to ask: it runs on Claude Code or Ollama")
}

// thread is the thread in use, nil before the first.
func (q *quickAsk) thread() *btwThread {
	if q.cur < len(q.threads) {
		return q.threads[q.cur]
	}
	return nil
}

// focused is whether the float has the keys.
func (q *quickAsk) focused() bool {
	t := q.thread()
	return q.shown && t != nil && t.focused
}

// newThread starts a thread, dropping the oldest past quickKeep; an empty
// one in use is kept as it is.
func (q *quickAsk) newThread() *btwThread {
	if t := q.thread(); t != nil && len(t.qa) == 0 {
		return t
	}
	q.threads = append(q.threads, &btwThread{pick: -1})
	if len(q.threads) > quickKeep {
		q.threads = q.threads[1:]
	}
	q.cur = len(q.threads) - 1
	return q.threads[q.cur]
}

// toggleQuick is quick.ask: opens the float with the keys, else moves the
// keys between it and what's under it.
func (m *Model) toggleQuick() tea.Cmd {
	q := &m.quick
	t := q.thread()
	if t == nil {
		t = q.newThread()
	}
	if !q.shown {
		q.shown, t.focused = true, true
		return nil
	}
	t.focused, t.sel = !t.focused, textSel{}
	return nil
}

// quickThread is a thread's answered questions as text.
func quickThread(qa []btwQA) string {
	var b strings.Builder
	for _, x := range qa {
		if x.Response != "" {
			b.WriteString("Q: " + x.Question + "\nA: " + x.Response + "\n\n")
		}
	}
	return b.String()
}

// askQuick asks q in thread t, with the thread so far as context.
func (m *Model) askQuick(t *btwThread, q string) tea.Cmd {
	sessions, keys := m.guideFleet()
	prompt := q
	if before := quickThread(t.qa); before != "" {
		prompt = "Earlier in this thread:\n\n" + before + "Now: " + q
	}
	prompt = sessions + "\n" + prompt
	t.qa = append(t.qa, btwQA{Question: q})
	t.waiting, t.err, t.scroll, t.sel = time.Now(), "", 0, textSel{}
	return sheetDo(func() ([2]string, error) {
		model, out, err := quickAsker(prompt)
		return [2]string{model, out}, err
	}, func(m *Model, r [2]string, err error) tea.Cmd {
		t.waiting, t.sel = time.Time{}, textSel{} // the rows under it moved
		switch {
		case err != nil:
			t.err = err.Error()
		case strings.TrimSpace(r[1]) == "":
			t.err = "no answer came back: ask again"
		default:
			answer, key := guideOpens(strings.TrimSpace(r[1]), keys)
			m.quick.model = r[0]
			for _, a := range m.order {
				if a.Key == key {
					t.qa[len(t.qa)-1].Response = answer + "\n\n→ opened " + a.DisplayName
					return m.goAgent(a)
				}
			}
			t.qa[len(t.qa)-1].Response = answer
		}
		return nil
	})
}

// insertQuick is session.quickask.insert: the thread in use into the
// chat's box as one paste, to add a line to and send.
func (m *Model) insertQuick() tea.Cmd {
	c, t := m.host, m.quick.thread()
	switch {
	case c == nil:
		m.flash("open a chat first: the quick ask goes into its box", true)
		return nil
	case t == nil || quickThread(t.qa) == "":
		m.flash("nothing answered yet: "+m.boundKey("quick.ask")+" asks", true)
		return nil
	}
	chip := c.pastes.add("A quick question I asked on the side:\n\n" + strings.TrimSpace(quickThread(t.qa)) + "\n")
	pos := max(0, len(c.input)-c.back)
	c.undo.save(c.input, c.back, false)
	c.input = insert(c.input, pos, []rune(chip+" "))
	m.quick.shown, t.focused, t.sel = false, false, textSel{}
	m.paneFocus = true
	m.flash("the quick ask is in the box: add a line and send", false)
	return nil
}

// quickKey handles keys while the float has them: all but chords, which
// the keymap reads (ctrl+] q, ctrl+] i), quitting and the command bar.
func (m *Model) quickKey(k tea.KeyPressMsg, s string) (tea.Cmd, bool) {
	q := &m.quick
	if !q.focused() || m.bar != nil || m.confirm != nil || s == "ctrl+]" || len(m.keys.chord) > 0 {
		return nil, false
	}
	t := q.thread()
	switch s {
	case "ctrl+q", "ctrl+k", "super+k", "alt+/":
		return nil, false
	case "super+c", "ctrl+c":
		if m.copyBtwSel(t) || s == "super+c" {
			return nil, true
		}
		return nil, false
	case "esc":
		// Hidden, the thread stays for ctrl+] q.
		q.shown, t.focused, t.sel = false, false, textSel{}
	case "tab":
		t.focused, t.sel = false, textSel{} // the keys to what's under it
	case "enter":
		text := strings.TrimSpace(string(t.input))
		if text == "" {
			return nil, true
		}
		if !t.waiting.IsZero() {
			m.flash("one question at a time: the last one's still being answered", true)
			return nil, true
		}
		t.input, t.pos = nil, 0
		return m.askQuick(t, text), true
	case "ctrl+n":
		q.newThread().focused = true
	case "ctrl+p":
		// The thread before, round to the latest.
		if n := len(q.threads); n > 1 {
			t.focused, t.sel = false, textSel{}
			q.cur = (q.cur + n - 1) % n
			q.thread().focused = true
		}
	case "up", "pgup":
		t.scroll += map[string]int{"up": 1, "pgup": 8}[s]
		t.sel = textSel{}
	case "down", "pgdown":
		t.scroll = max(0, t.scroll-map[string]int{"down": 1, "pgdown": 8}[s])
		t.sel = textSel{}
	default:
		if in, pos, ok := edit(t.input, t.pos, k, s); ok {
			t.input, t.pos = in, pos
		}
	}
	return nil, true // nothing reaches the box under it
}

// clickQuick gives the float the keys when it's clicked, and starts a drag
// over its text there. A click elsewhere hands the keys back.
func (m *Model) clickQuick(x, y int) bool {
	t := m.quick.thread()
	if !m.quick.shown || t == nil || t.at[2] == 0 {
		return false
	}
	if x < t.at[0] || x >= t.at[0]+t.at[2] || y < t.at[1]-1 || y > t.at[1]+t.at[3] {
		t.focused, t.sel = false, textSel{}
		return false
	}
	t.focused = true
	at := t.cellAt(x, y)
	t.sel = textSel{drag: true, a: at, b: at}
	if m.dbl && at.row < len(t.shown) {
		t.sel.selectWord(t.shown[at.row].Text)
	}
	return true
}

// quickOver draws the float over screen's top right, while it's shown.
func (m *Model) quickOver(screen string) string {
	q := &m.quick
	t := q.thread()
	if t != nil {
		t.at = [4]int{}
	}
	if !q.shown || t == nil || m.w < 30 || m.h < 8 {
		return screen
	}
	pw := min(m.w-2, max(40, m.w*2/5))
	iw := pw - 4 // inside the edges and a space either side

	var thread []convo.Line
	add := func(s string) { thread = append(thread, convo.Line{Text: s}) }
	for i, x := range t.qa {
		if i > 0 {
			add("")
		}
		for j, l := range wrap(x.Question, iw-2) {
			lead := paint(cOrange, "› ")
			if j > 0 {
				lead = "  "
			}
			add(lead + paint(cText+bold, l))
		}
		last := i == len(t.qa)-1
		switch {
		case x.Response != "":
			thread = append(thread, q.talk.Answer(x.Response, iw)...)
		case last && !t.waiting.IsZero():
			add(dim("thinking · " + dur(time.Since(t.waiting))))
		case last && t.err != "":
			for _, l := range wrap(t.err, iw) {
				add(paint(cRed, l))
			}
		}
	}
	if len(t.qa) == 0 {
		add(dim("ask the guide: what's next? which one needs me? where's the login work?"))
		add(dim("it sees your sessions, opens the one it means, and answers anything quick"))
	}
	var foot []string
	if t.focused {
		foot = []string{"", paint(cOrange, "› ") + textField(t.input, t.pos, true, "ask…", iw-2)}
	}
	room := max(1, m.h/2-2-len(foot))
	if len(thread) > room {
		// Anchored to its latest line; ↑ scrolls back.
		t.scroll = max(0, min(t.scroll, len(thread)-room))
		end := len(thread) - t.scroll
		thread = thread[max(0, end-room):end]
	}
	t.shown = thread
	for _, l := range foot {
		t.shown = append(t.shown, convo.Line{Text: l})
	}

	edge := cDim
	if t.focused {
		edge = cOrange
	}
	title := paint(cOrange+bold, "guide")
	if q.model != "" {
		title += dim(" · " + q.model)
	}
	hint := m.boundKey("quick.ask") + " to type here · esc"
	switch {
	case t.focused && m.host != nil && quickThread(t.qa) != "":
		hint = m.boundKey("session.quickask.insert") + " into a chat · esc"
	case t.focused:
		hint = "enter asks · ctrl+n new · tab back · esc"
	}
	line := func(l, r, mid string) string {
		room := pw - cellw.String(l) - 1
		w := cellw.String(mid)
		if w > room {
			mid, w = fit(mid, room), room
		}
		return paint(edge, l) + mid + paint(edge, strings.Repeat("─", room-w)+r)
	}
	box := []string{line("╭─ ", "╮", title+" ")}
	for i, l := range t.shown {
		row := paint(edge, "│") + onBg(bgBtw, " "+l.Text, pw-2) + paint(edge, "│")
		if from, to, ok := t.sel.cols(i, iw); ok {
			row = paintCols(row, from+2, to+2) // past the edge and its space
		}
		box = append(box, row)
	}
	box = append(box, line("╰ ", "╯", dim(hint)+" "))
	left, top := m.w-pw-1, 1
	t.at = [4]int{left, top + 1, pw, len(t.shown)}
	return strings.Join(pasteAt(strings.Split(screen, "\n"), box, top, left), "\n")
}
