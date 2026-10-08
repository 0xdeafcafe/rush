package ui

import (
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A message sent shows as sending until the agent has it, each of several
// in turn; a send that fails stops showing.
func TestSendingUntilItArrives(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	m.markSending(c, "first")
	m.markSending(c, "second")
	if got := ansi.Strip(strings.Join(m.sendingLines(c, 80), "\n")); !strings.Contains(got, "sending  first") || !strings.Contains(got, "second") {
		t.Fatalf("sending rows: %q", got)
	}
	c.sess.Apply(host.Sent{Text: "first"}, time.Now())
	if l := m.sendingLines(c, 80); len(l) != 1 || !strings.Contains(l[0], "second") {
		t.Fatalf("after the first arrived: %q", l)
	}
	// Mid-turn, it arrives as an interjection.
	c.sess.Apply(host.Sent{Text: "second"}, time.Now())
	if l := m.sendingLines(c, 80); len(l) != 0 {
		t.Fatalf("after both arrived: %q", l)
	}
	m.markSending(c, "third")
	m.sendFailed(sendFailedMsg{key: "k", err: errors.New("host gone")})
	if len(c.sending) != 0 {
		t.Fatal("a failed send still shows as sending")
	}
}

// A message the conversation shows stops showing as sending, even when the
// conversation it arrived in holds fewer of yours than when it was sent.
func TestSendingSeenInShorterConversation(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	c.sess.Apply(host.Sent{Text: "old"}, time.Now())
	c.sess.Apply(host.Sent{Text: "older"}, time.Now())
	m.markSending(c, "does exist??")
	c.sess = convo.New()
	c.sess.Apply(host.Sent{Text: "does exist??"}, time.Now())
	if l := m.sendingLines(c, 80); len(l) != 0 {
		t.Fatalf("still sending once it arrived: %q", l)
	}
}

// The same text sent twice waits for both to arrive.
func TestSendingSameTextTwice(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	c.sess.Apply(host.Sent{Text: "old"}, time.Now())
	m.markSending(c, "yes")
	m.markSending(c, "yes")
	c.sess = convo.New()
	c.sess.Apply(host.Sent{Text: "yes"}, time.Now())
	if l := m.sendingLines(c, 80); len(l) != 1 {
		t.Fatalf("after one of two arrived: %q", l)
	}
	c.sess.Apply(host.Sent{Text: "yes"}, time.Now())
	if l := m.sendingLines(c, 80); len(l) != 0 {
		t.Fatalf("after both arrived: %q", l)
	}
}
