package ui

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func TestAskCold(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	c.sess.Context = 120_000
	sent := 0
	send := func() tea.Cmd { sent++; return nil }

	c.sess.Requests = []convo.Request{{At: time.Now().Add(-10 * time.Minute)}}
	if m.askCold(c, "hi", send) {
		t.Fatal("a warm cache shouldn't ask")
	}

	c.sess.Requests = []convo.Request{{At: time.Now().Add(-3 * time.Hour)}}
	c.sess.Info.State = "working"
	if m.askCold(c, "hi", send) {
		t.Fatal("a turn under way keeps the cache warm")
	}
	c.sess.Info.State = "idle"
	if m.askCold(c, "/clear", send) {
		t.Fatal("/clear re-reads nothing")
	}
	if !m.askCold(c, "hi", send) || m.confirm == nil || sent != 0 {
		t.Fatalf("a cold cache should ask first: confirm=%v sent=%d", m.confirm, sent)
	}
	m.confirmKey("y")
	if sent != 1 {
		t.Fatal("y should send")
	}
	if m.askCold(c, "again", send) {
		t.Fatal("once you've said send, the same cold cache doesn't ask again")
	}

	c2 := &hostConn{kind: "claude", key: "k2", sess: convo.New()}
	c2.sess.Requests = []convo.Request{{At: time.Now().Add(-3 * time.Hour)}}
	c2.sess.Context = 20_000
	if m.askCold(c2, "hi", send) {
		t.Fatal("a small context isn't worth asking about")
	}
	c2.sess.Requests = nil
	c2.sess.Context = 0
	if m.askCold(c2, "hi", send) {
		t.Fatal("a session with no request yet has no cache to lose")
	}
}

// A send that stops to ask keeps the box's pastes, so the chip goes out as
// the text it stands for once you say yes.
func TestAskColdKeepsPastes(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	c.sess.Context = 120_000
	c.sess.Requests = []convo.Request{{At: time.Now().Add(-3 * time.Hour)}}
	c.input = []rune(c.pastes.add("one\ntwo\n"))
	m.sendPane(c, false)
	if m.confirm == nil || c.pastes.text[1] != "one\ntwo\n" {
		t.Fatalf("asking first lost the paste: confirm=%v pastes=%v", m.confirm, c.pastes.text)
	}
	m.confirmKey("y")
	if len(c.input) != 0 || len(c.pastes.text) != 0 {
		t.Fatalf("sent, the box should be empty: %q %v", string(c.input), c.pastes.text)
	}
}

// A cold cache can be compacted by a cheaper model first, the message
// carried into the fresh conversation's box when it came from the pane's.
func TestAskColdOffersCheaperCompact(t *testing.T) {
	defer func(old func(func() tea.Msg) tea.Cmd) { cmdOff = old }(cmdOff)
	cmdOff = func(f func() tea.Msg) tea.Cmd { return f }
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{Agents: []*fleet.Agent{{Key: "k", ID: "k", Kind: "claude"}}}}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	c.sess.Info = host.Info{Proto: 9, State: "idle", Kind: "claude"}
	c.sess.Context = 120_000
	c.sess.Requests = []convo.Request{{At: time.Now().Add(-3 * time.Hour)}}
	c.input = []rune("carry on")
	if !m.askCold(c, "carry on", func() tea.Cmd { return nil }) || len(m.confirm.more) != 1 || m.confirm.more[0].key != "c" {
		t.Fatalf("no cheaper-compact choice: %+v", m.confirm)
	}
	if m.confirmKey("c") == nil || !c.sess.Compacting() || m.confirm != nil {
		t.Fatal("c should start compacting and close the question")
	}
	c.sess.Info.Proto = 8
	if !m.askCold(c, "carry on", func() tea.Cmd { return nil }) || len(m.confirm.more) != 0 {
		t.Fatal("a host that can't carry on fresh can only send or cancel")
	}
}
