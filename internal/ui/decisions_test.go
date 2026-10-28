package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

const decisionsMsg = "Decisions:\n" +
	"1. **Scope:** (a) the full recommendation; (b) the easier alternative.\n" +
	"2. **Answer format:** (a) `1a. Topic: label` lines; (b) keep `Q:/A:`.\n" +
	"My picks: 1a 2b."

func decisionsConn() (*Model, *hostConn) {
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{}}
	c := &hostConn{key: "k", client: &host.Client{}, sess: &convo.Session{Info: host.Info{State: "idle", Decisions: host.DecisionsOf(decisionsMsg)}}}
	return m, c
}

// A turn that ended with decisions shows them as a card with the agent's
// picks marked and under the cursor; a pick of your own, then a takes its
// picks for the rest, and the answers go as your next message.
func TestDecisionsCard(t *testing.T) {
	m, c := decisionsConn()
	if cardKind(c) != "decisions" {
		t.Fatalf("card %q", cardKind(c))
	}
	m.cardKey(c, "up", true) // onto the card
	card := ansi.Strip(strings.Join(m.questionCard(c, c.sess.Info.Decisions, 100, 0), "\n"))
	for _, want := range []string{"? decide", "2 decisions", "1. Scope", "the full recommendation  ★ its pick", "a takes its picks", "s sends nothing"} {
		if !strings.Contains(card, want) {
			t.Fatalf("card missing %q:\n%s", want, card)
		}
	}
	if c.qCursor != 0 {
		t.Errorf("cursor %d, not on its pick", c.qCursor)
	}
	m.cardKey(c, "2", true) // the easier alternative, then on to 2
	if c.qIdx != 1 || c.qCursor != 1 || !c.cardFocus {
		t.Fatalf("on to the next: idx %d cursor %d focus %v", c.qIdx, c.qCursor, c.cardFocus)
	}
	if cmd, used := m.cardKey(c, "a", true); !used || cmd == nil {
		t.Fatal("a didn't send")
	}
	if len(c.sending) != 1 || c.sending[0].text != "1b. Scope: the easier alternative\n\n2b. Answer format: keep Q:/A:" {
		t.Fatalf("sent %+v", c.sending)
	}
	if cardKind(c) != "" {
		t.Error("the card stays once answered")
	}
}

// Its host asleep, the card is still there to answer: the answers wake it.
func TestDecisionsCardAsleep(t *testing.T) {
	_, c := decisionsConn()
	c.client, c.sleeping = nil, true
	if cardKind(c) != "decisions" {
		t.Fatalf("card %q while asleep", cardKind(c))
	}
	c.sleeping = false
	if cardKind(c) != "" {
		t.Fatal("a card with no host to answer it")
	}
}

// What's typed goes as a message, not an answer, unless you chose to
// answer in your own words; s sets the card aside and sends nothing.
func TestDecisionsCardTypingAndDismiss(t *testing.T) {
	m, c := decisionsConn()
	c.input = []rune("something else entirely")
	if _, used := m.cardKey(c, "enter", false); used {
		t.Fatal("a typed message was taken for an answer")
	}
	c.input = nil
	m.cardKey(c, "up", true)
	m.cardKey(c, "down", true)
	m.cardKey(c, "down", true) // "Something else"
	m.cardKey(c, "enter", true)
	if !c.qOwn || c.cardFocus {
		t.Fatalf("own words: own %v focus %v", c.qOwn, c.cardFocus)
	}
	c.input = []rune("only the parser")
	if _, used := m.cardKey(c, "enter", false); !used || c.qAnswer["1. Scope"] != "only the parser" || c.qOwn {
		t.Fatalf("own answer %q, own %v", c.qAnswer, c.qOwn)
	}
	if cmd, used := m.cardKey(c, "s", true); !used || cmd == nil || len(c.sending) != 0 || cardKind(c) != "" {
		t.Fatalf("dismissed: used %v, sent %+v, card %q", used, c.sending, cardKind(c))
	}
}

const loaderMsg = "The lane is done.\n\nDecisions:\n" +
	"1. **Committing the loader work:** rush's permission check stopped the lane's commit, so this needs your go-ahead. (a) yes: I commit it on perf/loader as one commit per fix, nothing pushed; (b) no: leave it uncommitted for you to look at first.\n" +
	"My picks: 1a."

// loaderConn is a session whose last turn ended with loaderMsg, its card
// open on the host when open is set.
func loaderConn(open bool) (*Model, *hostConn) {
	s := convo.New()
	s.Apply(host.Sent{Text: "go"}, time.Now())
	s.Apply(event.Message{Role: "assistant", ID: "a", Parts: []event.Part{{Kind: event.Text, Text: loaderMsg}}}, time.Now())
	s.Apply(event.TurnEnd{Reason: "done"}, time.Now())
	s.Info.State = "idle"
	if open {
		s.Info.Decisions = host.DecisionsOf(loaderMsg)
	}
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{}}
	return m, &hostConn{key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
}

// The card for the block the user saw: one option a line, "no" a label
// like "yes", the pick starred; a takes it.
func TestDecisionsCardLoader(t *testing.T) {
	m, c := loaderConn(true)
	m.cardKey(c, "up", true)
	card := ansi.Strip(strings.Join(m.questionCard(c, decisionsCard(c), 100, 0), "\n"))
	for _, want := range []string{"rush's permission check stopped", "1  yes  ★ its pick", "I commit it on perf/loader", "2  no", "leave it uncommitted for you to look at first", "a takes its picks"} {
		if !strings.Contains(card, want) {
			t.Fatalf("card missing %q:\n%s", want, card)
		}
	}
	if strings.Contains(card, "no: leave") {
		t.Fatalf("no kept its detail in its label:\n%s", card)
	}
	if _, used := m.cardKey(c, "a", true); !used || len(c.sending) != 1 || c.sending[0].text != "1a. Committing the loader work: yes" {
		t.Fatalf("sent %+v", c.sending)
	}
}

// Choosing the other option sends it in the answer format.
func TestDecisionsCardChooseAnother(t *testing.T) {
	m, c := loaderConn(true)
	m.cardKey(c, "up", true)
	m.cardKey(c, "down", true) // from its pick to "no"
	if _, used := m.cardKey(c, "enter", true); !used || len(c.sending) != 1 || c.sending[0].text != "1b. Committing the loader work: no" {
		t.Fatalf("sent %+v", c.sending)
	}
}

// Words of your own go as "1: their words".
func TestDecisionsCardOwnWords(t *testing.T) {
	m, c := loaderConn(true)
	m.cardKey(c, "up", true)
	m.cardKey(c, "down", true)
	m.cardKey(c, "down", true) // "Something else"
	m.cardKey(c, "enter", true)
	c.input = []rune("commit it, but squash into one")
	if _, used := m.cardKey(c, "enter", false); !used || len(c.sending) != 1 || c.sending[0].text != "1: commit it, but squash into one" {
		t.Fatalf("sent %+v", c.sending)
	}
}

// Once answered the card is gone, and d on the block in the conversation
// opens it again to answer another way; s sets that aside with nothing sent.
func TestDecisionsDecideAgain(t *testing.T) {
	m, c := loaderConn(false)
	if cardKind(c) != "" {
		t.Fatalf("card %q with nothing open", cardKind(c))
	}
	ref := convo.DecisionsRef(c.sess.TurnRef(0))
	m.clickRef(c, ref)
	if c.sel != ref || cardKind(c) != "" {
		t.Fatalf("first click: sel %q card %q", c.sel, cardKind(c))
	}
	m.clickRef(c, ref) // again: opens it
	if cardKind(c) != "decisions" || !c.cardFocus || c.sel != "" {
		t.Fatalf("again: card %q focus %v sel %q", cardKind(c), c.cardFocus, c.sel)
	}
	if _, used := m.cardKey(c, "s", true); !used || cardKind(c) != "" || len(c.sending) != 0 {
		t.Fatalf("set aside: card %q sent %+v", cardKind(c), c.sending)
	}
	c.sel = ref
	m.host = c
	m.paneKey(tea.KeyPressMsg{Code: 'd', Text: "d"}, "d")
	if cardKind(c) != "decisions" {
		t.Fatalf("d: card %q", cardKind(c))
	}
	m.cardKey(c, "2", true)
	if len(c.sending) != 1 || c.sending[0].text != "1b. Committing the loader work: no" || cardKind(c) != "" {
		t.Fatalf("sent %+v, card %q", c.sending, cardKind(c))
	}
}

// paneText is the session pane as drawn for c, colours stripped.
func paneText(m *Model, c *hostConn) string {
	m.host = c
	return ansi.Strip(strings.Join(m.rushPane(110, 45), "\n"))
}

func loaderPane(open bool) (*Model, *hostConn) {
	m, _ := benchModel(110, 45)
	_, c := loaderConn(open)
	c.kind, c.key, c.id, c.ready = m.host.kind, m.host.key, m.host.id, true
	return m, c
}

// While its card is up the block is one quiet row, the options shown once,
// on the card; once the card closes, the list is back in the same pane.
func TestDecisionsListHiddenWhileCardOpen(t *testing.T) {
	m, c := loaderPane(true)
	list := "★ yes  I commit it on perf/loader"
	got := paneText(m, c)
	if !strings.Contains(got, "Decisions · 1 decision · on the card below") || strings.Contains(got, list) || !strings.Contains(got, "yours to decide") {
		t.Fatalf("card open:\n%s", got)
	}
	c.cardFocus = true
	if _, used := m.cardKey(c, "s", true); !used {
		t.Fatal("s unused")
	}
	// Set aside, it's back as a list that says it wasn't answered.
	if got = paneText(m, c); !strings.Contains(got, "☆ yes  I commit it on perf/loader") || !strings.Contains(got, "not answered") || strings.Contains(got, "on the card below") || strings.Contains(got, "yours to decide") {
		t.Fatalf("card closed:\n%s", got)
	}
	// Opened again, hidden again; answered, back.
	m.decideAgain(c, convo.DecisionsRef(c.sess.TurnRef(0)))
	if got = paneText(m, c); strings.Contains(got, list) || !strings.Contains(got, "on the card below") {
		t.Fatalf("reopened:\n%s", got)
	}
	m.cardKey(c, "a", true)
	if got = paneText(m, c); !strings.Contains(got, list) || strings.Contains(got, "not answered") {
		t.Fatalf("answered:\n%s", got)
	}
}

// Enter on a picked block opens its card, as d does.
func TestDecisionsEnterOpensCard(t *testing.T) {
	m, c := loaderPane(false)
	paneText(m, c)
	ref := convo.DecisionsRef(c.sess.TurnRef(0))
	c.sel = ref
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyEnter}, "enter")
	if cardKind(c) != "decisions" || !c.cardFocus || c.open[ref] {
		t.Fatalf("enter: card %q focus %v open %v", cardKind(c), c.cardFocus, c.open[ref])
	}
}

// A decisions card coming up doesn't take the keys: a message starting
// with a, s or a digit is a message, and only ↑ puts you on the card.
func TestDecisionsCardWaitsForUp(t *testing.T) {
	m, c := decisionsConn()
	m.cardModal(c)
	if c.cardFocus {
		t.Fatal("a decisions card should not take the keys as it comes")
	}
	for _, k := range []string{"a", "s", "1"} {
		if cmd, _ := m.cardKey(c, k, true); cmd != nil || c.cardFocus {
			t.Fatalf("%q answered a card you hadn't moved onto", k)
		}
	}
	m.cardKey(c, "up", true)
	if !c.cardFocus {
		t.Fatal("↑ should put you on the card")
	}
}

// A card that comes while you type never takes the keys: enter sends
// what's typed, as a message, and nothing else. While that message is on
// its way the block stays one row (no flicker back to the list under the
// card), and once the message has passed the card over, the block says it
// went unanswered rather than drawing the agent's picks as if chosen.
func TestDecisionsPassedOverByMessage(t *testing.T) {
	m, c := loaderPane(false)
	c.input, c.lastKeyAt = []rune("something else"), time.Now()
	paneText(m, c)
	c.sess.Info.Decisions = host.DecisionsOf(loaderMsg) // the card arrives mid-word
	got := paneText(m, c)
	if c.cardFocus || !strings.Contains(got, "on the card below") {
		t.Fatalf("card took the keys (%v) or the block isn't one row:\n%s", c.cardFocus, got)
	}
	if _, used := m.cardKey(c, "enter", false); used {
		t.Fatal("the typed message was taken by the card")
	}
	// The message lands as a turn before the host says the card is gone.
	c.sess.Apply(host.Sent{Text: "something else"}, time.Now())
	if got = paneText(m, c); strings.Contains(got, "★ yes") {
		t.Fatalf("the block flickered back to its list under the card:\n%s", got)
	}
	c.sess.Info.Decisions = nil
	got = paneText(m, c)
	if !strings.Contains(got, "not answered") || strings.Contains(got, "on the card below") {
		t.Fatalf("passed over:\n%s", got)
	}
	if strings.Contains(got, "its pick") {
		t.Fatalf("its picks drawn as chosen:\n%s", got)
	}
}

const prLayoutMsg = "Two ways to show several pull requests.\n\nDecisions:\n" +
	"1. **PR card layout:** An agent can have several pull requests, and the card shows one. (a) tabs: one PR at a time, ← → moves; (b) stacked: every PR at once, a row each, taller.\n" +
	"   (a) tabs:\n" +
	"   ```\n" +
	"   [ #118 ✗ ]  #121 ✓   #124 ◌\n" +
	"   PR #118 can't merge yet: unit fails\n" +
	"   ✓ pushed  ✓ opened  ✗ checks  ○ review\n" +
	"   ```\n" +
	"   (b) stacked:\n" +
	"   ```\n" +
	"   #118  ✗ unit fails          f fix\n" +
	"   #121  ✓ ready to merge      m merge\n" +
	"   #124  ◌ checks 3 of 5\n" +
	"   ```\n" +
	"My picks: 1a."

// A decision whose options have previews: the card is titled by its topic
// and shows the preview of the option under the cursor beside the options;
// the block in the conversation shows each under its option.
func TestDecisionsCardPreviews(t *testing.T) {
	s := convo.New()
	s.Apply(host.Sent{Text: "go"}, time.Now())
	s.Apply(event.Message{Role: "assistant", ID: "a", Parts: []event.Part{{Kind: event.Text, Text: prLayoutMsg}}}, time.Now())
	s.Apply(event.TurnEnd{Reason: "done"}, time.Now())
	s.Info.State = "idle"
	s.Info.Decisions = host.DecisionsOf(prLayoutMsg)
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{}}
	c := &hostConn{key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m.cardKey(c, "up", true)
	card := ansi.Strip(strings.Join(m.questionCard(c, decisionsCard(c), 120, 0), "\n"))
	for _, want := range []string{"? PR card layout", "yours to decide", "[ #118 ✗ ]  #121 ✓", "1  tabs  ★ its pick", "2  stacked"} {
		if !strings.Contains(card, want) {
			t.Fatalf("card missing %q:\n%s", want, card)
		}
	}
	t.Log("\n" + card)
	m.cardKey(c, "down", true)
	card = ansi.Strip(strings.Join(m.questionCard(c, decisionsCard(c), 120, 0), "\n"))
	if !strings.Contains(card, "#121  ✓ ready to merge") {
		t.Fatalf("stacked's preview not shown on it:\n%s", card)
	}
	t.Log("\n" + card)
}

// An option too long to split into a label and what it does wraps on the
// card, under its own start, rather than running off the edge.
func TestDecisionsCardLongLabelWraps(t *testing.T) {
	long := "keep the popup and add device code only as the fallback when the popup is blocked"
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{}}
	c := &hostConn{key: "k", client: &host.Client{}, sess: &convo.Session{Info: host.Info{State: "idle", Decisions: host.DecisionsOf(
		"Decisions:\n1. **Sign-in:** (a) " + long + " for most readers, a code for the rest, and it never fails; (b) device code only.\nMy picks: 1a.")}}}
	m.cardKey(c, "up", true)
	lines := m.questionCard(c, c.sess.Info.Decisions, 70, 0)
	card := ansi.Strip(strings.Join(lines, "\n"))
	if strings.Contains(card, "…") {
		t.Fatalf("label cut short:\n%s", card)
	}
	if !strings.Contains(strings.Join(strings.Fields(card), " "), "never fails") {
		t.Fatalf("label's end missing:\n%s", card)
	}
	for _, l := range lines {
		if w := ansi.StringWidth(l); w > 70 {
			t.Fatalf("line %d wide: %q", w, ansi.Strip(l))
		}
	}
}
