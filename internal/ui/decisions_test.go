package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

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
