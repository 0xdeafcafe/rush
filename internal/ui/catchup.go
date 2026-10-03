package ui

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// --- /catchup: what happened since you last wrote, in the /btw panel ---

// A catch-up is a side question with a fixed prompt: what has gone on in
// the session since the last message a person typed and sent. An agent
// left alone for hours takes other agents' messages, monitors firing and
// background tasks reporting back, all in the user's role; the catch-up
// starts from the message you wrote, not from the last of those.
//
// The model answering sees the conversation, as any side question does,
// and an index of the messages since yours, each numbered ([m7]). It
// answers in sections, each item citing the messages it comes from, and
// the panel draws every citation as a link that scrolls the conversation
// to that message and lights it.
type catchup struct {
	since   convo.Said   // your message it starts from: index[0]
	index   []convo.Said // the messages since, in the order they're numbered
	guessed bool         // the session keeps no record of what you typed: since is read off the transcript
	out     *catchupOut  // the answer in its sections; nil until it comes, or when it didn't read as them
}

// catchupOut is the answer as the prompt asks for it.
type catchupOut struct {
	Sections []catchSection `json:"sections"`
	// Report is the message that is the agent's final report of the main
	// task: linked to, not said again.
	Report string `json:"report"`
}

type catchSection struct {
	ID    string      `json:"id"`
	Items []catchItem `json:"items"`
}

type catchItem struct {
	Text string   `json:"text"`
	Refs []string `json:"refs"`
}

// catchSections are the sections in the order they're drawn, and what
// each is called.
var catchSections = []struct{ id, title, ask string }{
	{"asked", "You asked", "what I asked for, in one line"},
	{"status", "Where it stands", "where it stands now: done, in progress or stopped, in one or two lines"},
	{"facts", "Key facts", "the key facts I should pay attention to in the result"},
	{"waiting", "Waiting on you", "what is waiting on me: actions and decisions only I can take"},
	{"blocked", "Blocked or failed", "what is blocked or failed"},
	{"other", "What else happened", "what else happened since (other agents' messages, monitors, pull request babysitting), one line each"},
}

// catchMost is how many messages the index lists. Past it the agent's
// narration between steps goes, oldest first: what you sent, what was
// delivered and each turn's last words stay.
const catchMost = 150

// catchupScopeMsg is the session's record of typed messages, read off the
// UI's goroutine.
type catchupScopeMsg struct {
	key      string
	typed    []host.HumanMessage
	recorded bool
}

// openCatchup asks what happened since your last message, in the agent's
// side panel.
func (m *Model) openCatchup(c *hostConn) tea.Cmd {
	if c.client == nil && !c.sleeping {
		m.flash("that works in rush-mode sessions · /rush moves this one over", true)
		return nil
	}
	key, id := c.key, c.id
	return func() tea.Msg {
		msgs, recorded, _ := host.HumanMessages(id)
		return catchupScopeMsg{key: key, typed: msgs, recorded: recorded}
	}
}

// askCatchup asks the catch-up, now that what you typed is known.
func (m *Model) askCatchup(msg catchupScopeMsg) tea.Cmd {
	c := m.host
	if c == nil || c.key != msg.key || c.sess == nil {
		return nil // you've gone to another agent
	}
	said := c.sess.Said()
	typed := make([]convo.Typed, len(msg.typed))
	for i, t := range msg.typed {
		typed[i] = convo.Typed{At: t.At, Text: t.Text}
	}
	at := convo.LastTyped(said, typed, msg.recorded)
	if at < 0 {
		m.flash("nothing to catch up on: no message of yours is in this conversation", true)
		return nil
	}
	ct := &catchup{since: said[at], index: catchIndex(said[at:])}
	ct.guessed = !contains(convo.TypedIn(said, typed), at)
	if len(ct.index) < 2 {
		m.flash("nothing has happened since your last message", false)
		return nil
	}
	if t := m.btwFor(c.key); t != nil && !t.waiting.IsZero() {
		m.flash("one question at a time: the last one's still being answered", true)
		return nil
	}
	m.openBtw(c, "")
	t := m.btwFor(c.key)
	return t.askWith(m, c, catchPrompt(ct, time.Now()), ct)
}

func contains(xs []int, x int) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}

// catchIndex is the messages a catch-up numbers: said, cut to catchMost.
func catchIndex(said []convo.Said) []convo.Said {
	over := len(said) - catchMost
	if over <= 0 {
		return said
	}
	out := make([]convo.Said, 0, catchMost)
	for i, s := range said {
		if over > 0 && i > 0 && s.Who == "assistant" && !s.Answer {
			over--
			continue
		}
		out = append(out, s)
	}
	if n := len(out); n > catchMost {
		out = append(out[:1], out[n-catchMost+1:]...) // yours, and the newest
	}
	return out
}

// saidAt is when a message was said, as an index line and a link say it:
// the time, with the day when it wasn't today.
func saidAt(at, now time.Time) string {
	if at.IsZero() {
		return ""
	}
	at = at.Local()
	if y, m, d := at.Date(); y != now.Year() || m != now.Month() || d != now.Day() {
		return at.Format("Jan 2 15:04")
	}
	return at.Format("15:04")
}

// catchPrompt is the catch-up's question: where it starts, the index of
// what was said since, and the shape of the answer.
func catchPrompt(ct *catchup, now time.Time) string {
	var b strings.Builder
	first := oneLine(ct.since.Text)
	if r := []rune(first); len(r) > 200 {
		first = string(r[:200]) + "…"
	}
	b.WriteString("Catch me up on this session: I have been away from it. Since I sent this message: \"" + first + "\"\n\n")
	b.WriteString("Here is an index of the messages in the conversation since then, oldest first. Each has an id. [m1] is that message of mine, and the only one in the index I wrote: the others in my role were sent by a script, another agent or a background task.\n\n")
	for i, s := range ct.index {
		text := oneLine(s.Text)
		r := []rune(text)
		if len(r) > 100 {
			text = string(r[:100]) + "…"
		}
		who := s.Who
		if i > 0 && s.Yours {
			who = "in my role, not written by me"
		}
		if s.Answer {
			who += fmt.Sprintf(", the last words of a turn, %d characters", len(r))
		}
		fmt.Fprintf(&b, "[m%d] %s %s: %s\n", i+1, saidAt(s.At, now), who, text)
	}
	b.WriteString("\nAnswer with one JSON object and nothing else, no prose around it and no code fence:\n")
	b.WriteString(`{"sections":[{"id":"asked","items":[{"text":"…","refs":["m1"]}]}],"report":"m12"}` + "\n\n")
	b.WriteString("The sections, in this order. Leave out any that has nothing in it:\n")
	for _, s := range catchSections {
		b.WriteString("- " + s.id + ": " + s.ask + ".\n")
	}
	b.WriteString("\nKeep it very condensed. Plain language, short sentences, no jargon, no em-dashes, no markdown in the text. ")
	b.WriteString("Every item names in refs the messages it comes from, by their ids in the index, and no others. ")
	b.WriteString("\"report\" is the id of your long final report of the main task, when there is one: I will read it there, so link to it rather than say it again. ")
	b.WriteString("Use no tools.")
	return b.String()
}

var saidRefRe = regexp.MustCompile(`m(\d+)`)

// parseCatchup reads the answer as its sections, n being how many messages
// the index had; nil when it isn't that, and the panel shows it as written.
func parseCatchup(resp string, n int) *catchupOut {
	i, j := strings.Index(resp, "{"), strings.LastIndex(resp, "}")
	if i < 0 || j < i {
		return nil
	}
	var o catchupOut
	if jsonx.Unmarshal([]byte(resp[i:j+1]), &o) != nil {
		return nil
	}
	ref := func(s string) string {
		m := saidRefRe.FindStringSubmatch(s)
		if m == nil {
			return ""
		}
		if k, _ := strconv.Atoi(m[1]); k < 1 || k > n {
			return "" // not in the index: nowhere to go
		}
		return "m" + m[1]
	}
	kept := o.Sections[:0]
	for _, s := range o.Sections {
		items := s.Items[:0]
		for _, it := range s.Items {
			if it.Text = strings.TrimSpace(it.Text); it.Text == "" {
				continue
			}
			refs := it.Refs[:0]
			for _, r := range it.Refs {
				if r = ref(r); r != "" {
					refs = append(refs, r)
				}
			}
			it.Refs = refs
			items = append(items, it)
		}
		if s.Items = items; len(items) > 0 {
			kept = append(kept, s)
		}
	}
	if o.Sections = kept; len(kept) == 0 {
		return nil
	}
	o.Report = ref(o.Report)
	return &o
}

// saidLink is label as a link to message n of thread question qa's index.
func saidLink(qa, n int, label string) string {
	return "\x1b]8;;rush:said/" + strconv.Itoa(qa) + "/" + strconv.Itoa(n) + "\x1b\\" + label + "\x1b]8;;\x1b\\"
}

var (
	saidLinkRe = regexp.MustCompile(`\x1b\]8;;(rush:said/\d+/\d+)\x1b\\(.*?)\x1b\]8;;\x1b\\`)
	saidCiteRe = regexp.MustCompile(`\[m(\d+)\]`)
)

// link is the link to message ref ("m7") of question qa's index: an arrow
// and when it was said.
func (ct *catchup) link(qa int, ref string, now time.Time) string {
	n, _ := strconv.Atoi(strings.TrimPrefix(ref, "m"))
	if n < 1 || n > len(ct.index) {
		return ""
	}
	label := firstNonEmpty(saidAt(ct.index[n-1].At, now), ref)
	return saidLink(qa, n, paint(cBlue, "↗ "+label))
}

// rows draws the answer's sections iw wide, as question qa of its thread.
func (ct *catchup) rows(qa, iw int, now time.Time) []convo.Line {
	var out []convo.Line
	add := func(s string) { out = append(out, convo.Line{Text: s}) }
	// item is its text wrapped under lead, its links after its last word,
	// or on a row of their own when they don't fit there.
	item := func(lead, text, colour string, refs []string) {
		var links []string
		for _, r := range refs {
			if l := ct.link(qa, r, now); l != "" {
				links = append(links, l)
			}
		}
		tail := strings.Join(links, " ")
		pad := blanks(cellw.String(lead))
		lines := wrap(text, max(8, iw-cellw.String(lead)))
		for i, l := range lines {
			row := pad + paint(colour, l)
			if i == 0 {
				row = lead + paint(colour, l)
			}
			if i == len(lines)-1 && tail != "" {
				if cellw.String(row)+2+cellw.String(tail) <= iw {
					row += "  " + tail
				} else {
					add(row)
					row = pad + tail
				}
			}
			add(row)
		}
	}
	for _, sec := range catchSections {
		for _, s := range ct.out.Sections {
			if s.ID != sec.id {
				continue
			}
			if len(out) > 0 {
				add("")
			}
			add(paint(cOrange+bold, sec.title))
			lead := "  " + faint("· ")
			if len(s.Items) == 1 {
				lead = "  "
			}
			for _, it := range s.Items {
				item(lead, it.Text, cText, it.Refs)
			}
			if sec.id == "status" && ct.out.Report != "" {
				item("  "+paint(cOrange, "▸ "), "Full report", cText+bold, []string{ct.out.Report})
			}
		}
	}
	return out
}

// markdown is the catch-up as plain markdown: what it reads as when it's
// carried into another chat.
func (ct *catchup) markdown(resp string) string {
	if ct.out == nil {
		return resp
	}
	var b strings.Builder
	for _, sec := range catchSections {
		for _, s := range ct.out.Sections {
			if s.ID != sec.id {
				continue
			}
			b.WriteString(sec.title + ":\n")
			for _, it := range s.Items {
				b.WriteString("- " + it.Text + "\n")
			}
		}
	}
	return strings.TrimSpace(b.String())
}

// scope says where the catch-up starts, under its question.
func (ct *catchup) scope(now time.Time) string {
	first := oneLine(ct.since.Text)
	if r := []rune(first); len(r) > 60 {
		first = string(r[:60]) + "…"
	}
	s := "since \"" + first + "\""
	if at := saidAt(ct.since.At, now); at != "" {
		s += " · " + at
	}
	if ct.guessed {
		s += " · read off the transcript: this session has no record of what you typed"
	}
	return s
}

// citeLinks makes links of the citations ([m7]) in drawn rows, as written
// answers carry them: the same words, so no row grows.
func citeLinks(rows []convo.Line, ct *catchup, qa int) []convo.Line {
	out := make([]convo.Line, len(rows))
	for i, l := range rows {
		l.Text = saidCiteRe.ReplaceAllStringFunc(l.Text, func(s string) string {
			n, _ := strconv.Atoi(s[2 : len(s)-1])
			if n < 1 || n > len(ct.index) {
				return s
			}
			return saidLink(qa, n, s)
		})
		out[i] = l
	}
	return out
}

// btwLink is a link drawn in the panel: the thread row it's on, which of
// that row's links it is, and where it goes.
type btwLink struct {
	row, nth int
	target   string
}

// findLinks lists the links in the thread's rows, in reading order.
func findLinks(thread []convo.Line) []btwLink {
	var out []btwLink
	for r, l := range thread {
		if !strings.Contains(l.Text, "rush:said/") {
			continue
		}
		for k, m := range saidLinkRe.FindAllStringSubmatch(l.Text, -1) {
			out = append(out, btwLink{row: r, nth: k, target: m[1]})
		}
	}
	return out
}

// litLink draws the nth link of row as the one the keys are on.
func litLink(row string, nth int) string {
	k := -1
	return saidLinkRe.ReplaceAllStringFunc(row, func(s string) string {
		if k++; k != nth {
			return s
		}
		m := saidLinkRe.FindStringSubmatch(s)
		return "\x1b]8;;" + m[1] + "\x1b\\" + reset + selBlue + ansi.Strip(m[2]) + reset + "\x1b]8;;\x1b\\"
	})
}

// catchBefore is the catch-up a question's citations point into: its own,
// or the nearest asked before it in the thread; -1 when there's none.
func (t *btwThread) catchBefore(qa int) int {
	for i := min(qa, len(t.qa)-1); i >= 0; i-- {
		if t.qa[i].catch != nil {
			return i
		}
	}
	return -1
}

// movePick moves the keys' link by d (the arrows), round the ends.
func (t *btwThread) movePick(d int) {
	n := len(t.links)
	if n == 0 {
		return
	}
	switch {
	case t.pick < 0 && d > 0:
		t.pick = 0
	case t.pick < 0:
		t.pick = n - 1
	default:
		t.pick = (t.pick + d + n) % n
	}
	t.pickMoved = true
}

// followSaid goes to the message a link in the panel names: the
// conversation scrolls to it and lights it, and the panel tucks away over
// it. It reports whether target was such a link.
func (m *Model) followSaid(c *hostConn, t *btwThread, target string) bool {
	rest, ok := strings.CutPrefix(target, "rush:said/")
	if !ok {
		return false
	}
	q, k, _ := strings.Cut(rest, "/")
	qa, _ := strconv.Atoi(q)
	n, _ := strconv.Atoi(k)
	if qa < 0 || qa >= len(t.qa) || t.qa[qa].catch == nil || n < 1 || n > len(t.qa[qa].catch.index) {
		return true
	}
	s := t.qa[qa].catch.index[n-1]
	if c.sess.TurnOf(s.Turn) < 0 {
		m.flash("that message is no longer in the conversation as it's read here", true)
		return true
	}
	if c.open == nil {
		c.open = map[string]bool{}
	}
	c.open[s.Turn] = true
	m.showView(c, "conversation")
	c.sel, c.subSel = "", ""
	c.goSaid, c.goTurn, c.litSaid = s.Ref, s.Turn, s.Ref
	c.scrollOnly, c.pinTop = false, false
	t.focused, t.sel, t.choosing = false, textSel{}, false
	return true
}

// saidRow is the first drawn row of the message ref names, or -1.
func saidRow(body []convo.Line, base int, ref string) int {
	for i, l := range body {
		if l.Ref == ref || l.Said == ref {
			return base + i
		}
	}
	return -1
}

// btwTucked is how many rows the side panel takes tucked away: a message
// gone to shows under it.
const btwTucked = 11

// carried is the side thread as text for another chat to read: each
// question and its answer.
func (t *btwThread) carried() string {
	var b strings.Builder
	for _, x := range t.qa {
		b.WriteString("\nMe: " + x.asked() + "\n")
		if x.Response != "" {
			b.WriteString("You: " + x.answered() + "\n")
		}
	}
	return b.String()
}

// toMain carries the side thread into the conversation and sends q there,
// to the agent itself: the thread goes ahead of it as context, and the
// panel closes, the chat having moved.
func (m *Model) toMain(c *hostConn, t *btwThread, q string) tea.Cmd {
	if len(c.input) > 0 {
		m.flash("the message box has a draft: send or clear it first", true)
		return nil
	}
	text := "For context, here is a side chat I had about this conversation while you worked. It is not in the conversation itself.\n" +
		t.carried() + "\nMy reply, to carry on from here:\n" + q
	c.input, c.back = []rune(text), 0
	cmd := m.sendPane(c, false)
	if len(c.input) > 0 {
		// It stopped to ask first (a cold cache, a plugin's question): the
		// message waits in the box, and the thread stays until it goes.
		t.focused, t.choosing = false, false
		return cmd
	}
	delete(m.btws, c.key)
	return cmd
}
