package convo

import (
	"strconv"
	"strings"
	"time"
)

// Said is one message of the conversation, as an index of it lists it:
// what you sent, what the agent wrote, what another agent or a background
// task delivered. Steps aren't messages and aren't in it.
type Said struct {
	// Ref finds its rows in the drawn conversation: a Line's Ref, or for
	// the agent's words a Line's Said. Turn is the turn it's in ("t13").
	Ref, Turn string
	At        time.Time
	// Who said it: "me" for what came as your own message, "assistant" for
	// the agent's words, else who delivered it ("background task ·
	// completed", "message from rush-8a").
	Who  string
	Text string
	// ID is the provider's id for a message of yours, where it gives one.
	ID string
	// Yours is a message in your role that nothing marks as delivered by
	// someone else. Whether a person typed it only its sender knows.
	Yours bool
	// Answer is the agent's final words of a turn: its report.
	Answer bool
}

// saidRef is the anchor of item it's words in turn t, made once.
func (t *Turn) saidRef(ref string, it *Item) string {
	if it.saidRef == "" {
		for i, p := range t.Items {
			if p == it {
				it.saidRef = ref + ":m:" + strconv.Itoa(i)
				break
			}
		}
	}
	return it.saidRef
}

// said marks the rows drawn since from as it's words, the blank rows
// either side of them left out.
func (d *drawer) said(from int, it *Item) {
	to := len(d.lines)
	for from < to && d.isBlank(from) {
		from++
	}
	for to > from && d.isBlank(to-1) {
		to--
	}
	if from >= to {
		return
	}
	a := d.t.saidRef(d.s.turnRef(d.t), it)
	for i := from; i < to; i++ {
		d.lines[i].Said = a
	}
}

// Said lists the conversation's messages, oldest first.
func (s *Session) Said() []Said {
	var out []Said
	for _, t := range s.Turns {
		ref := s.turnRef(t)
		_, _, wake := woke(t)
		switch {
		case strings.TrimSpace(t.Prompt) == "" && len(t.Images) == 0 && len(t.Pictures) == 0:
			if t.Cause != "" {
				out = append(out, Said{Ref: ref, Turn: ref, At: t.Start, Who: firstNonEmpty(t.From, "woken"), Text: t.Cause})
			}
		case t.From == "" && !wake && !switched(t):
			out = append(out, Said{Ref: ref, Turn: ref, At: t.Start, Who: "me", Text: t.Prompt, ID: t.PromptID, Yours: true})
		default:
			out = append(out, Said{Ref: ref, Turn: ref, At: t.Start, Who: firstNonEmpty(t.From, "rush"), Text: firstNonEmpty(t.Prompt, t.Cause), ID: t.PromptID})
		}
		at := t.Start
		for _, it := range t.Items {
			if !it.At.IsZero() {
				at = it.At
			} else if it.Kind == KStep && it.Step != nil && !it.Step.Start.IsZero() {
				at = it.Step.Start
			}
			switch it.Kind {
			case KText:
				if strings.TrimSpace(it.Text) != "" {
					out = append(out, Said{Ref: t.saidRef(ref, it), Turn: ref, At: at, Who: "assistant", Text: it.Text, Answer: it.Answer})
				}
			case KInterject:
				out = append(out, Said{Ref: t.messageRef(it), Turn: ref, At: at, Who: "me", Text: it.Text, ID: it.ID, Yours: true})
			case KExchange:
				e := it.Exchange
				if e == nil {
					continue
				}
				peer, who := e.Sender, "message from "
				if e.Direction == "sent" {
					peer, who = e.Receiver, "the agent's message to "
				}
				out = append(out, Said{Ref: "exchange:" + e.Direction + ":" + e.ID + ":" + peer.SessionID, Turn: ref, At: at,
					Who: who + peerLabel(peer), Text: it.Text})
			}
		}
	}
	return out
}

// Typed is a message a person typed, as its sender wrote it down.
type Typed struct {
	At   time.Time
	Text string
}

// carries is whether the message text holds typed: it is it, or a queue
// sent as one message (host.JoinQueue) with it among them.
func carries(text, typed string) bool {
	text, typed = strings.TrimSpace(text), strings.TrimSpace(typed)
	if typed == "" {
		return false
	}
	if text == typed || cleanPrompt(text) == cleanPrompt(typed) {
		return true
	}
	if !strings.Contains(text, " messages, queued while you worked.") {
		return false
	}
	i := strings.Index(text, "]\n"+typed)
	if i < 0 {
		return false
	}
	rest := text[i+2+len(typed):]
	return rest == "" || strings.HasPrefix(rest, "\n\n[Message ")
}

// TypedIn pairs each typed message with the message of said that carried
// it, by index, or -1 where none has yet: the first in your role at or
// after it was sent that holds its words. One queued goes later than it
// was sent, never earlier.
func TypedIn(said []Said, typed []Typed) []int {
	out := make([]int, len(typed))
	for k, ty := range typed {
		out[k] = -1
		for i, m := range said {
			if !m.Yours || !m.At.IsZero() && m.At.Before(ty.At.Add(-5*time.Second)) {
				continue
			}
			if carries(m.Text, ty.Text) {
				out[k] = i
				break
			}
		}
	}
	return out
}

// LastTyped is the index in said of the last message a person typed, or
// -1: the newest typed message found there when the session keeps a
// record of them, else (a session from before the record, or one whose
// typed messages aren't in what's read of it) the last in your role that
// nothing marks as delivered by someone else.
func LastTyped(said []Said, typed []Typed, recorded bool) int {
	if recorded {
		at := TypedIn(said, typed)
		for k := len(at) - 1; k >= 0; k-- {
			if at[k] >= 0 {
				return at[k]
			}
		}
	}
	for i := len(said) - 1; i >= 0; i-- {
		if said[i].Yours {
			return i
		}
	}
	return -1
}
