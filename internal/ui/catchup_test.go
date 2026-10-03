package ui

import (
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// catchModel is a session you asked one thing of at t0, which answered,
// then took a message a script sent in your role and answered that too.
// Only the first is in the session's record of what you typed.
func catchModel(t *testing.T) (*Model, *hostConn, time.Time) {
	t.Helper()
	m, c := infoModel(t)
	home := t.TempDir()
	t.Setenv("RUSH_HOME", home)
	c.id = "abc12345"
	if err := os.MkdirAll(filepath.Join(host.Root(), c.id), 0o700); err != nil {
		t.Fatal(err)
	}
	c.sess.Info.Proto = host.Proto
	t0 := time.Now().Add(-3 * time.Hour).Truncate(time.Second)
	say := func(text string) headless.Message {
		return headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "text", Text: text}}}
	}
	for i, ev := range []any{
		host.Sent{Text: "fix the login test and open a pull request"},
		say("Looking at the test."),
		say("Fixed. The pull request is open and CI is green."),
		headless.Result{Subtype: "success"},
		host.Sent{Text: "check the review comments"},
		say("Two comments, both answered."),
		headless.Result{Subtype: "success"},
	} {
		c.sess.Apply(ev, t0.Add(time.Duration(i)*time.Minute))
	}
	if err := host.RecordHumanAt(c.id, "fix the login test and open a pull request", t0); err != nil {
		t.Fatal(err)
	}
	return m, c, t0
}

// runCatchup types /catchup and takes it as far as the question out.
func runCatchup(t *testing.T, m *Model, c *hostConn) *btwThread {
	t.Helper()
	c.input = []rune("/catchup")
	cmd := m.sendPane(c, false)
	if cmd == nil {
		t.Fatalf("/catchup did nothing: %q", m.status)
	}
	msg, ok := cmd().(catchupScopeMsg)
	if !ok {
		t.Fatalf("/catchup read %T", cmd())
	}
	m.update(msg)
	bt := m.btwFor(c.key)
	if bt == nil || len(bt.qa) == 0 || len(c.asks) != 1 {
		t.Fatalf("/catchup asked nothing: thread %+v asks %d status %q", bt, len(c.asks), m.status)
	}
	return bt
}

func replyAll(m *Model, c *hostConn, response string) {
	body, _ := jsontext.AppendQuote(nil, response)
	for id := range c.asks {
		m.onReply(c, host.Reply{ID: id, Body: jsontext.Value(`{"response":` + string(body) + `}`)})
	}
}

const catchAnswer = "```json\n" + `{"sections":[
 {"id":"status","items":[{"text":"Done. The pull request is open and CI is green.","refs":["m3"]}]},
 {"id":"asked","items":[{"text":"Fix the login test and open a pull request.","refs":["m1"]}]},
 {"id":"waiting","items":[{"text":"Merge the pull request.","refs":["m3","[m5]"]},{"text":"Nothing else.","refs":["m99"]}]},
 {"id":"blocked","items":[]},
 {"id":"other","items":[{"text":"A script asked for the review comments to be checked. Both were answered.","refs":["m4","m5"]}]}
],"report":"m3"}` + "\n```"

// /catchup is a side question with a fixed prompt: it starts from the last
// message a person typed, not the last in the user's role, and gives the
// model an index of what was said since to cite from.
func TestCatchupStartsFromWhatYouTyped(t *testing.T) {
	m, c, _ := catchModel(t)
	bt := runCatchup(t, m, c)
	ct := bt.qa[0].catch
	if ct == nil || ct.since.Text != "fix the login test and open a pull request" || ct.guessed || len(ct.index) != 5 {
		t.Fatalf("scope: %+v", ct)
	}
	q := bt.qa[0].Question
	for _, want := range []string{
		`Since I sent this message: "fix the login test and open a pull request"`,
		"[m1] ", " me: fix the login test",
		"[m3] ", "assistant, the last words of a turn, 48 characters: Fixed. The pull request is open",
		"[m4] ", "in my role, not written by me: check the review comments",
		`"report"`, "- waiting: ", "no em-dashes",
	} {
		if !strings.Contains(q, want) {
			t.Errorf("the question lacks %q:\n%s", want, q)
		}
	}
	if !bt.focused || m.sheet != nil {
		t.Fatalf("it opens in the side panel, with the keys: %+v sheet %T", bt, m.sheet)
	}
	panel := func() string { return ansi.Strip(strings.Join(bt.lines(c, 64, 40, true), "\n")) }
	if p := panel(); !strings.Contains(p, "/catchup") || !strings.Contains(p, `since "fix the login test`) || !strings.Contains(p, "thinking") || strings.Contains(p, "Catch me up") {
		t.Fatalf("waiting:\n%s", p)
	}

	// The answer draws as its sections, in their own order, empty ones left out.
	replyAll(m, c, catchAnswer)
	p := panel()
	at := func(s string) int { return strings.Index(p, s) }
	if at("You asked") < 0 || at("You asked") > at("Where it stands") || at("Where it stands") > at("Waiting on you") || at("Waiting on you") > at("What else happened") {
		t.Fatalf("sections out of order:\n%s", p)
	}
	if strings.Contains(p, "Blocked") || strings.Contains(p, "Key facts") || strings.Contains(p, "sections") || !strings.Contains(p, "▸ Full report") {
		t.Fatalf("drawn:\n%s", p)
	}
	// Every citation in the index is a link; one that isn't in it is dropped.
	var targets []string
	for _, l := range bt.links {
		targets = append(targets, l.target)
	}
	if got := strings.Join(targets, " "); got != "rush:said/0/1 rush:said/0/3 rush:said/0/3 rush:said/0/3 rush:said/0/5 rush:said/0/4 rush:said/0/5" {
		t.Fatalf("links: %s\n%s", got, p)
	}
}

// A session from before the record of typed messages falls back to the
// transcript: the last message in your role nothing marks as delivered.
func TestCatchupWithoutARecord(t *testing.T) {
	m, c, _ := catchModel(t)
	if err := os.Remove(host.HumanPath(c.id)); err != nil {
		t.Fatal(err)
	}
	bt := runCatchup(t, m, c)
	ct := bt.qa[0].catch
	if ct.since.Text != "check the review comments" || !ct.guessed || len(ct.index) != 2 {
		t.Fatalf("scope: %+v", ct)
	}
	if p := ansi.Strip(strings.Join(bt.lines(c, 64, 40, true), "\n")); !strings.Contains(p, "no record of what you typed") {
		t.Fatalf("it should say it guessed:\n%s", p)
	}
	// Nothing since your message: nothing to ask.
	m2, c2, _ := catchModel(t)
	c2.sess.Turns = c2.sess.Turns[:1]
	c2.sess.Turns[0].Items = nil
	c2.input = []rune("/catchup")
	m2.update(m2.sendPane(c2, false)())
	if m2.btwFor(c2.key) != nil || !strings.Contains(m2.status, "nothing has happened") {
		t.Fatalf("nothing since: %q", m2.status)
	}
}

// A link goes to the message it cites: the conversation scrolls to it and
// lights its rows, and the panel tucks away. ↑↓ move between links,
// enter follows the one the keys are on, and a click follows one too.
func TestCatchupLinksGoToTheMessage(t *testing.T) {
	m, c, _ := catchModel(t)
	bt := runCatchup(t, m, c)
	replyAll(m, c, catchAnswer)
	bt.lines(c, 64, 40, true)
	m.paneFocus = true
	m.paneKey(tea.KeyPressMsg{}, "down")
	m.paneKey(tea.KeyPressMsg{}, "down")
	if bt.pick != 1 {
		t.Fatalf("down moved to link %d", bt.pick)
	}
	bt.lines(c, 64, 40, true)
	m.paneKey(tea.KeyPressMsg{}, "enter")
	report := bt.qa[0].catch.index[2]
	if c.litSaid != report.Ref || c.goSaid != report.Ref || c.goTurn != "t1" || bt.focused || !c.open["t1"] {
		t.Fatalf("followed to %q (go %q in %q), panel focused %v", c.litSaid, c.goSaid, c.goTurn, bt.focused)
	}
	if m.btwFor(c.key) == nil || len(c.asks) != 0 {
		t.Fatal("following a link asks nothing and keeps the panel")
	}
	// A key that isn't scrolling puts the light out.
	m.paneKey(tea.KeyPressMsg{}, "down")
	if c.litSaid == "" {
		t.Fatal("scrolling put the light out")
	}
	m.paneKey(tea.KeyPressMsg{Code: 'x', Text: "x"}, "x")
	if c.litSaid != "" {
		t.Fatal("typing left the message lit")
	}
	c.input = nil

	// A click on a link in the panel follows it.
	m.paneKey(tea.KeyPressMsg{}, "ctrl+b")
	rows := make([]string, 50)
	for i := range rows {
		rows[i] = strings.Repeat(" ", 140)
	}
	m.btwOverlay(c, rows, 2, 46, 140)
	x, y := -1, -1
	for i, l := range bt.shown {
		if strings.Contains(l.Text, "rush:said/0/4") {
			plain := ansi.Strip(strings.Split(l.Text, "\x1b]8;;rush:said/0/4")[0])
			x, y = bt.at[0]+2+ansi.StringWidth(plain), bt.at[1]+i
		}
	}
	if x < 0 || !m.clickBtw(c, x, y) {
		t.Fatalf("no link to click at %d,%d", x, y)
	}
	if want := bt.qa[0].catch.index[3].Ref; c.litSaid != want || want != "t2" || bt.focused {
		t.Fatalf("clicked to %q, want %q", c.litSaid, want)
	}
}

// The pane scrolls to a message followed to and lights the rows it's on,
// under the tucked panel; nothing else is lit.
func TestCatchupLightsTheMessageInThePane(t *testing.T) {
	m, _ := benchModel(160, 44)
	c := m.host
	c.input = nil
	said := c.sess.Said()
	var target convo.Said
	for _, s := range said {
		if s.Turn == "t120" && s.Answer {
			target = s
		}
	}
	if target.Ref == "" {
		t.Fatal("no answer in turn 120")
	}
	bt := &btwThread{pick: -1, focused: true, qa: []btwQA{{Question: "q", Response: "{}", catch: &catchup{since: said[0], index: []convo.Said{said[0], target}}}}}
	m.btws = map[string]*btwThread{c.key: bt}
	if !m.followSaid(c, bt, "rush:said/0/2") {
		t.Fatal("not followed")
	}
	rows := strings.Split(m.render(), "\n")
	if d := os.Getenv("CATCHUP_DUMP"); d != "" {
		os.WriteFile(d, []byte(strings.Join(rows, "\n")), 0o644)
	}
	lit, first := 0, -1
	for i, r := range rows {
		if strings.Contains(r, selBG) && strings.Contains(ansi.Strip(r), "What changed") {
			first = i
		}
		if strings.Contains(r, selBG) && !strings.Contains(ansi.Strip(r), "more · end follows") {
			lit++
		}
	}
	if first < 0 {
		t.Fatalf("the answer of turn 120 isn't lit on screen:\n%s", ansi.Strip(strings.Join(rows, "\n")))
	}
	if under := bt.at[1] + bt.at[3]; first < under || first > under+btwTucked {
		t.Errorf("the message starts at row %d: it should sit just under the tucked panel, which ends at row %d", first, under)
	}
	if bt.focused {
		t.Error("the panel kept the keys")
	}
	if lit < 4 || lit > 14 {
		t.Errorf("%d rows lit: one message's rows, no more", lit)
	}
	if c.goSaid != "" || c.litSaid != target.Ref {
		t.Errorf("after the frame: go %q lit %q", c.goSaid, c.litSaid)
	}
	if v := ansi.Strip(strings.Join(rows, "\n")); !strings.Contains(v, "turn 120:") {
		t.Errorf("turn 120 isn't on screen")
	}
}

// An answer that doesn't read as sections is shown as written, and the
// citations in it are links all the same.
func TestCatchupFallsBackToMarkdown(t *testing.T) {
	m, c, _ := catchModel(t)
	bt := runCatchup(t, m, c)
	replyAll(m, c, "**Done.** The pull request is open [m3]. A script asked about the review [m4] [m9].")
	if bt.qa[0].catch.out != nil {
		t.Fatal("prose read as sections")
	}
	p := ansi.Strip(strings.Join(bt.lines(c, 64, 40, true), "\n"))
	if !strings.Contains(p, "Done. The pull request is open [m3].") {
		t.Fatalf("as written:\n%s", p)
	}
	if len(bt.links) != 2 || bt.links[0].target != "rush:said/0/3" || bt.links[1].target != "rush:said/0/4" {
		t.Fatalf("links: %+v", bt.links)
	}
}

// A follow-up says where it goes, each time: on in the side chat, or to
// the agent itself with the side chat ahead of it as context. /btw asks
// the same.
func TestFollowUpChoosesWhereItGoes(t *testing.T) {
	m, c, _ := catchModel(t)
	bt := runCatchup(t, m, c)
	replyAll(m, c, catchAnswer)
	m.paneFocus = true
	typeIn := func(s string) {
		for _, r := range s {
			m.paneKey(tea.KeyPressMsg{Code: r, Text: string(r)}, string(r))
		}
	}
	panel := func() string { return ansi.Strip(strings.Join(bt.lines(c, 64, 40, true), "\n")) }
	typeIn("which comments?")
	m.paneKey(tea.KeyPressMsg{}, "enter")
	if !bt.choosing || len(c.asks) != 0 || len(bt.qa) != 1 {
		t.Fatalf("a follow-up should ask where it goes first: %+v", bt)
	}
	if p := panel(); !strings.Contains(p, "where does this follow-up go?") || !strings.Contains(p, "● here, in the side chat") || !strings.Contains(p, "○ the main chat, with this side chat as context") {
		t.Fatalf("the choice:\n%s", p)
	}
	// Here: asked in the side chat, the thread carried as its history.
	m.paneKey(tea.KeyPressMsg{}, "enter")
	if bt.choosing || len(bt.qa) != 2 || len(c.asks) != 1 || bt.qa[1].Question != "which comments?" || len(c.sending) != 0 {
		t.Fatalf("here: %+v asks %d", bt.qa, len(c.asks))
	}
	replyAll(m, c, "The two on the test file [m5].")
	if panel(); len(bt.links) == 0 || bt.links[len(bt.links)-1].target != "rush:said/1/5" {
		t.Fatalf("a follow-up's citations link into the catch-up's index: %+v", bt.links)
	}
	// esc goes back to typing; typing on closes the choice.
	typeIn("merge it")
	m.paneKey(tea.KeyPressMsg{}, "enter")
	m.paneKey(tea.KeyPressMsg{}, "esc")
	if bt.choosing || !bt.focused || string(bt.input) != "merge it" {
		t.Fatalf("esc: %+v", bt)
	}
	// The main chat: the side chat goes ahead of the reply, to the agent.
	m.paneKey(tea.KeyPressMsg{}, "enter")
	m.paneKey(tea.KeyPressMsg{}, "down")
	if p := panel(); !strings.Contains(p, "● the main chat, with this side chat as context") {
		t.Fatalf("the choice moved:\n%s", p)
	}
	if cmd := m.paneKey(tea.KeyPressMsg{}, "enter"); cmd == nil {
		t.Fatal("nothing sent to the main chat")
	}
	if m.btwFor(c.key) != nil {
		t.Fatal("the side chat moved to the main chat: its panel closes")
	}
	if len(c.sending) != 1 || len(c.asks) != 0 {
		t.Fatalf("sent: %+v asks %d", c.sending, len(c.asks))
	}
	sent := c.sending[0].text
	for _, want := range []string{
		"a side chat I had about this conversation",
		"Me: /catchup\nYou: You asked:\n- Fix the login test and open a pull request.",
		"Waiting on you:\n- Merge the pull request.",
		"Me: which comments?\nYou: The two on the test file [m5].",
		"My reply, to carry on from here:\nmerge it",
	} {
		if !strings.Contains(sent, want) {
			t.Errorf("sent to the main chat lacks %q:\n%s", want, sent)
		}
	}
	if strings.Contains(sent, "Catch me up on this session") || strings.Contains(sent, `"sections"`) {
		t.Errorf("the catch-up's own prompt or JSON went to the main chat:\n%s", sent)
	}

	// /btw: the first question goes straight; a follow-up asks.
	m.openBtw(c, "what's left?")
	b2 := m.btwFor(c.key)
	replyAll(m, c, "Only the tests.")
	typeIn("and then?")
	m.paneKey(tea.KeyPressMsg{}, "enter")
	if !b2.choosing || b2.toMain {
		t.Fatalf("a /btw follow-up should ask where it goes: %+v", b2)
	}
	m.paneKey(tea.KeyPressMsg{}, "enter")
	if len(b2.qa) != 2 || len(c.asks) != 1 {
		t.Fatalf("/btw follow-up, here: %+v", b2.qa)
	}
}

// /catchup asked again while the session has said nothing since shows the
// one kept, at once and with its follow-ups, and asks no model; it's kept
// in the session's folder, so a closed panel or another UI finds it. A new
// message in the session, or ctrl+r in the panel, asks anew.
func TestCatchupReopensWhatWasKept(t *testing.T) {
	m, c, t0 := catchModel(t)
	bt := runCatchup(t, m, c)
	replyAll(m, c, catchAnswer)
	// A follow-up in the side chat is kept with it.
	bt.input = []rune("is CI green?")
	m.btwKey(c, tea.KeyPressMsg{}, "enter")
	m.btwKey(c, tea.KeyPressMsg{}, "enter")
	if len(c.asks) != 1 {
		t.Fatalf("the follow-up wasn't asked: %d", len(c.asks))
	}
	replyAll(m, c, "Yes [m3].")
	panel := func(bt *btwThread) string { return ansi.Strip(strings.Join(bt.lines(c, 90, 60, true), "\n")) }
	before := panel(bt)
	if !strings.Contains(before, "ctrl+r catch up again") {
		t.Fatalf("the panel doesn't say how to ask again:\n%s", before)
	}

	again := func(fresh bool) {
		t.Helper()
		var cmd tea.Cmd
		if fresh {
			cmd, _ = m.btwKey(c, tea.KeyPressMsg{}, "ctrl+r")
		} else {
			c.input = []rune("/catchup")
			cmd = m.sendPane(c, false)
		}
		if cmd == nil {
			t.Fatalf("it did nothing: %q", m.status)
		}
		m.update(cmd())
	}

	// The panel closed, as after a restart of the UI: the file brings it back.
	delete(m.btws, c.key)
	again(false)
	bt = m.btwFor(c.key)
	if bt == nil || len(bt.qa) != 2 || len(c.asks) != 0 || !bt.waiting.IsZero() || !bt.focused {
		t.Fatalf("it should be back with no question out: %+v asks %d", bt, len(c.asks))
	}
	after := panel(bt)
	if !strings.Contains(after, "· answered ") || !strings.Contains(after, "is CI green?") {
		t.Fatalf("kept:\n%s", after)
	}
	strip := func(s string) string {
		i := strings.Index(s, "You asked")
		return s[i:]
	}
	if strip(after) != strip(before) {
		t.Fatalf("it isn't the same catch-up:\n%s\n---\n%s", before, after)
	}
	if len(bt.links) == 0 || bt.links[0].target != "rush:said/0/1" {
		t.Fatalf("its links are gone: %+v", bt.links)
	}
	// Asked again with the panel open: still nothing asked, nothing added.
	again(false)
	if len(bt.qa) != 2 || len(c.asks) != 0 {
		t.Fatalf("asked again with the panel open: qa %d asks %d", len(bt.qa), len(c.asks))
	}

	// ctrl+r asks anew, in the old one's place.
	again(true)
	if len(bt.qa) != 1 || len(c.asks) != 1 || bt.waiting.IsZero() || bt.qa[0].catch.kept {
		t.Fatalf("ctrl+r should ask again: qa %d asks %d", len(bt.qa), len(c.asks))
	}
	replyAll(m, c, catchAnswer)

	// The session says something more: the next /catchup is a new one.
	c.sess.Apply(host.Sent{Text: "rebase on main"}, t0.Add(time.Hour))
	c.sess.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "text", Text: "Rebased."}}}, t0.Add(time.Hour+time.Minute))
	c.sess.Apply(headless.Result{Subtype: "success"}, t0.Add(time.Hour+2*time.Minute))
	delete(m.btws, c.key)
	again(false)
	bt = m.btwFor(c.key)
	if bt == nil || len(c.asks) != 1 || bt.waiting.IsZero() || len(bt.qa[0].catch.index) != 7 {
		t.Fatalf("new messages should ask a new one: %+v asks %d", bt, len(c.asks))
	}
}
