package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A subagent you're watching has a queue of its own: what you send it
// waits there, where you can edit, reorder or hold it, and one message
// goes in each time it finishes a step. ctrl+enter still goes at once.

func subQKey(key, id string) string { return key + "\x00" + id }

// subQueue is the watched subagent's queue, when what you type goes
// straight to it; nil otherwise.
func (m *Model) subQueue(c *hostConn) (*localQueue, convo.Subagent) {
	sa, ok := m.relaySub(c)
	if !ok || !c.sess.Info.Inbox {
		return nil, sa
	}
	return m.localQueueOf(subQKey(c.key, sa.ID)), sa
}

// queueSub adds text to the watched subagent's queue.
func (m *Model) queueSub(c *hostConn, q *localQueue, sa convo.Subagent, text string) {
	if len(q.items) == 0 {
		// Into its inbox now, for the end of the step it's on, unless one
		// went in on this step already.
		q.since = time.Now()
		if steps := c.subSteps(sa.ID); q.sentAt.IsZero() || steps > q.at {
			q.at = steps - 1
		}
	}
	q.items = append(q.items, text)
	m.flash(fmt.Sprintf("queued for %s · it reads it when the step it's on ends", sa.Type), false)
}

func (c *hostConn) subSteps(id string) int {
	if t := c.subTails[id]; t != nil {
		return t.Sess.Totals(time.Now()).ToolCalls
	}
	return 0
}

// tellSub puts text in subagent sa's inbox, for the end of its step.
func tellSub(c *hostConn, sa convo.Subagent, text string) tea.Cmd {
	cl, key := c.client, c.key
	return func() tea.Msg {
		return subSentMsg{key: key, sub: sa.ID, name: sa.Type, text: text, how: "in its inbox · it reads it when this step ends", told: true, err: cl.Tell(sa.ID, text)}
	}
}

// viaSub sends text, meant for subagent sa, with send: how says where it
// went instead.
func viaSub(c *hostConn, sa convo.Subagent, text, how string, send func() error) tea.Cmd {
	key := c.key
	return func() tea.Msg {
		return subSentMsg{key: key, sub: sa.ID, name: sa.Type, text: text, how: how, err: send()}
	}
}

// subSentMsg says how a message meant for subagent sub went.
type subSentMsg struct {
	key, sub, name string
	text, how      string
	told           bool // into its inbox
	err            error
}

// subNote is a message meant for a subagent that its conversation doesn't
// show: sent another way, or in its inbox and not read yet.
type subNote struct {
	text, how string
	told      bool
}

// subNotesKept is how many notes each subagent keeps.
const subNotesKept = 10

func (m *Model) noteSub(key, sub string, n subNote) {
	if m.subNotes == nil {
		m.subNotes = map[string][]subNote{}
	}
	k := subQKey(key, sub)
	ns := append(m.subNotes[k], n)
	m.subNotes[k] = ns[max(0, len(ns)-subNotesKept):]
}

// onSubSent notes where a message went, or keeps one that didn't: back
// at the head of its queue, else in the box, else as a note saying why.
func (m *Model) onSubSent(msg subSentMsg) {
	if msg.err == nil {
		m.noteSub(msg.key, msg.sub, subNote{text: msg.text, how: msg.how, told: msg.told})
		m.flash("sent to "+msg.name+" · "+msg.how, false)
		return
	}
	why := "couldn't send to " + msg.name + ": " + msg.err.Error()
	switch c := m.host; {
	case msg.told:
		q := m.localQueueOf(subQKey(msg.key, msg.sub))
		q.items = append([]string{msg.text}, q.items...)
		m.flash(why+" · back in its queue", true)
	case c != nil && c.key == msg.key && len(c.input) == 0:
		c.input, c.back = []rune(msg.text), 0
		m.flash(why+" · back in the box", true)
	default:
		m.noteSub(msg.key, msg.sub, subNote{text: msg.text, how: "✗ not sent: " + msg.err.Error()})
		m.flash(why, true)
	}
}

// subNoteLines are the watched subagent's notes, the latest three, w wide;
// one in its inbox goes once its conversation shows it.
func (m *Model) subNoteLines(c *hostConn, w int) []string {
	if !m.watchingSub(c) {
		return nil
	}
	sa, live, _ := m.pickedSub(c)
	var out []string
	for _, n := range m.subNotes[subQKey(c.key, sa.ID)] {
		how := n.how
		if n.told && said(c.subTail.Sess, n.text) {
			continue
		}
		if n.told && !live {
			how = "it ended before reading it · the main session gets it"
		}
		how = dim(" · " + how)
		text := cellw.Truncate(oneLine(n.text), max(10, w-6-ansi.StringWidth(how)), "…")
		out = append(out, "  "+paint(cSub, "↪ ")+paint(cText, text)+how)
	}
	return out[max(0, len(out)-3):]
}

// said is whether s has text among your messages.
func said(s *convo.Session, text string) bool {
	for _, t := range s.Turns {
		if strings.Contains(t.Prompt, text) {
			return true
		}
		for _, it := range t.Items {
			if it.Kind == convo.KInterject && strings.Contains(it.Text, text) {
				return true
			}
		}
	}
	return false
}

// flushSubQueues sends each of the open pane's subagents the next message
// it has waiting, once it has taken a step since the last; a run that has
// finished hands what it still had to the main session.
// ponytail: only the open pane's are sent; another pane's wait for it to open.
func (m *Model) flushSubQueues() tea.Cmd {
	c := m.host
	if c == nil || c.client == nil {
		return nil
	}
	var cmds []tea.Cmd
	for _, sa := range c.subs {
		key := subQKey(c.key, sa.ID)
		q := m.localQ[key]
		if q == nil || len(q.items) == 0 {
			continue
		}
		if _, live := c.subState(sa); !live {
			mine := strings.TrimSpace(host.JoinQueue(q.items))
			text := "For your subagent " + sa.Type + ", which finished before it got this:\n\n" + mine
			delete(m.localQ, key)
			cl := c.client
			cmds = append(cmds, viaSub(c, sa, mine, "it ended first · went to the main session", func() error { return cl.Send(text) }))
			continue
		}
		if steps := c.subSteps(sa.ID); !q.held && steps > q.at {
			text := q.items[0]
			q.items, q.at, q.sentAt = q.items[1:], steps, time.Now()
			cmds = append(cmds, tellSub(c, sa, text))
		}
	}
	return tea.Batch(cmds...)
}

// sendSubQueueNow sends the watched subagent all it has queued, and extra
// after it, as one message now.
func (m *Model) sendSubQueueNow(c *hostConn, q *localQueue, sa convo.Subagent, extra string) tea.Cmd {
	items := q.items
	if extra != "" {
		items = append(items, extra)
	}
	if len(items) == 0 {
		m.flash("nothing queued", false)
		return nil
	}
	q.items, q.sentAt = nil, time.Now()
	q.at = c.subSteps(sa.ID)
	return tellSub(c, sa, strings.TrimSpace(host.JoinQueue(items)))
}
