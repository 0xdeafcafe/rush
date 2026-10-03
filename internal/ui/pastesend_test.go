package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

const pasteText = "first line of the paste\nsecond line\nthird line"

// pasteBox is a Session's box holding a chip for pasteText between two lines
// typed around it.
func pasteBox(t *testing.T) (*Model, *hostConn) {
	m, c := infoModel(t)
	c.input = []rune("look at this:\n" + c.pastes.add(pasteText) + "\nthanks")
	return m, c
}

func sentText(t *testing.T, c *hostConn) string {
	t.Helper()
	if len(c.sending) != 1 {
		t.Fatalf("want one message sent, got %d", len(c.sending))
	}
	return c.sending[0].text
}

func wantPasteIn(t *testing.T, text string) {
	t.Helper()
	if !strings.Contains(text, pasteText) || convo.PasteChipRe.MatchString(text) {
		t.Fatalf("the paste should go out whole, not as its chip:\n%s", text)
	}
}

func TestSendExpandsPastes(t *testing.T) {
	m, c := pasteBox(t)
	m.sendPane(c, false)
	text := sentText(t, c)
	wantPasteIn(t, text)
	if !strings.Contains(text, "<pasted_content") {
		t.Errorf("a rush session gets the paste tagged as Claude Code tags one:\n%s", text)
	}
	if len(c.input) != 0 || len(c.pastes.text) != 0 {
		t.Errorf("a sent box lets its pastes go: input %q pastes %v", string(c.input), c.pastes.text)
	}
}

// A send to a cold cache asks first; saying yes sends the same box again,
// and its chips must still stand for their pastes then.
func TestColdCacheSendExpandsPastes(t *testing.T) {
	m, c := pasteBox(t)
	c.sess.Context = 120_000
	c.sess.Requests = []convo.Request{{At: time.Now().Add(-3 * time.Hour)}}
	m.sendPane(c, false)
	if m.confirm == nil || len(c.sending) != 0 {
		t.Fatalf("a cold cache should ask first: confirm=%v sent=%d", m.confirm, len(c.sending))
	}
	if len(c.pastes.text) != 1 {
		t.Fatalf("asking keeps the box's pastes: %v", c.pastes.text)
	}
	m.confirmKey("y")
	wantPasteIn(t, sentText(t, c))
}

// A box answering a question sends its pastes, not their chips.
func TestQuestionAnswerExpandsPastes(t *testing.T) {
	m := &Model{}
	c := &hostConn{}
	c.input = []rune("start with " + c.pastes.add(pasteText))
	m.questionKey(c, askReq(), "enter", false)
	got := c.qAnswer["Which rules first?"]
	wantPasteIn(t, got)
	if strings.Contains(got, "<pasted_content") {
		t.Errorf("an answer carries the paste untagged:\n%s", got)
	}
	if len(c.pastes.text) != 0 {
		t.Errorf("an answered box lets its pastes go: %v", c.pastes.text)
	}
}

// The Prompt replying into a cold session asks with its pastes kept, for
// yes to send them.
func TestPromptColdReplyExpandsPastes(t *testing.T) {
	m, c := infoModel(t)
	c.sess.Context = 120_000
	c.sess.Requests = []convo.Request{{At: time.Now().Add(-3 * time.Hour)}}
	m.order, m.sel = m.snap.Agents, c.key
	m.inKind = inReply
	m.input = []rune("see " + m.pastes.add(pasteText))
	m.submit()
	if m.confirm == nil {
		t.Fatal("a cold cache should ask first")
	}
	if len(m.pastes.text) != 1 {
		t.Fatalf("asking keeps the Prompt's pastes: %v", m.pastes.text)
	}
}

// A rewound prompt comes back in the box with its pastes as chips, to go
// out whole again.
func TestRewoundDraftKeepsPastes(t *testing.T) {
	m, c := infoModel(t)
	c.client = &host.Client{}
	m.host = nil
	m.hostOpening = c.key
	m.rewound = map[string]string{c.key: "again:\n\n<pasted_content id=\"00ab\">\n" + pasteText + "\n</pasted_content id=\"00ab\">\n"}
	m.onHostOpen(hostOpenMsg{key: c.key, c: c})
	if !convo.PasteChipRe.MatchString(string(c.input)) {
		t.Fatalf("the paste should come back as a chip: %q", string(c.input))
	}
	m.sendPane(c, false)
	wantPasteIn(t, sentText(t, c))
}
