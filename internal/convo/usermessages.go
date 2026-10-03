package convo

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

// sentEcho pairs the host's immediate echo with the provider's acknowledgement.
// Only unmatched sends are kept; identical messages sent twice remain distinct.
type sentEcho struct {
	text string
	turn *Turn
	item *Item
}

// id notes the provider's id for the message the echo stands for.
func (e sentEcho) id(id string) {
	switch {
	case id == "":
	case e.item != nil:
		e.item.ID = id
	case e.turn != nil:
		e.turn.PromptID = id
	}
}

func (s *Session) sent(in host.Sent, now time.Time, echo bool) sentEcho {
	if in.Exchange != nil {
		e := s.exchange(*in.Exchange, now)
		if echo {
			if len(s.echoes) == 64 {
				s.echoes = slices.Delete(s.echoes, 0, 1)
			}
			s.echoes = append(s.echoes, e)
		}
		return e
	}

	if !now.IsZero() {
		if s.First.IsZero() || now.Before(s.First) {
			s.First = now
		}
		if now.After(s.Last) {
			s.Last = now
		}
	}
	if s.light {
		in.Text = strings.Clone(firstLine(in.Text))
	}
	var e sentEcho
	if t := s.Live(); t != nil {
		it := &Item{Kind: KInterject, Text: in.Text, Images: in.Images, At: now}
		t.Items = append(t.Items, it)
		t.touch()
		e = sentEcho{in.Text, t, it}
	} else {
		s.streaming, s.woke = nil, nil
		t := &Turn{N: len(s.Turns) + 1, Prompt: in.Text, Start: now, Live: true, steps: map[string]*Step{}, Effort: s.Info.Effort, Images: in.Images}
		compactAsk(t)
		s.Turns = append(s.Turns, t)
		e = sentEcho{in.Text, t, nil}
	}
	if echo {
		// Some providers never echo. Bound their unmatched bookkeeping.
		if len(s.echoes) == 64 {
			s.echoes = slices.Delete(s.echoes, 0, 1)
		}
		s.echoes = append(s.echoes, e)
	}
	return e
}

func (s *Session) userMessage(m *event.Message, now time.Time) bool {
	if m.Role != "user" || m.Parent != "" || m.Injected || len(m.Parts) == 0 {
		return false
	}
	var texts, images []string
	var pictures []*event.ImageData
	for _, p := range m.Parts {
		switch p.Kind {
		case event.Text:
			texts = append(texts, p.Text)
		case event.Image:
			if p.Image != nil {
				pictures = append(pictures, p.Image)
				images = append(images, firstNonEmpty(p.Image.Path, "image"))
			}
		default:
			return false // tool results belong to their calls
		}
	}
	text := cleanPrompt(joinTexts(texts))
	if text == "" && len(pictures) == 0 {
		return true
	}
	if m.ID != "" {
		if s.userIDs[m.ID] {
			return true
		}
		if s.userIDs == nil {
			s.userIDs = map[string]bool{}
		}
		s.userIDs[m.ID] = true
	}
	var e sentEcho
	found := false
	for i, old := range s.echoes {
		if cleanPrompt(old.text) == text {
			e, found = old, true
			s.echoes = slices.Delete(s.echoes, i, i+1)
			break
		}
	}
	if !found {
		e = s.sent(host.Sent{Text: text, Images: images}, now, false)
	}
	e.id(m.ID)
	if len(pictures) > 0 {
		if e.item != nil {
			e.item.Pictures = pictures
			if len(e.item.Images) == 0 {
				e.item.Images = images
			}
		} else {
			e.turn.Pictures = pictures
			if len(e.turn.Images) == 0 {
				e.turn.Images = images
			}
		}
		e.turn.touch()
	}
	return true
}

// UserMessage is a complete message for reading and copying, including pastes
// and pictures. Ref identifies the same row in the conversation.
type UserMessage struct {
	Ref      string
	Text     string
	Images   []string
	Pictures []*event.ImageData
}

func (t *Turn) messageRef(it *Item) string {
	if it.userRef != "" {
		return it.userRef
	}
	for i, p := range t.Items {
		if p == it {
			it.userRef = fmt.Sprintf("t%d:u:%d", t.N, i)
			return it.userRef
		}
	}
	return ""
}

func (s *Session) UserMessages() []UserMessage {
	var out []UserMessage
	for _, t := range s.Turns {
		if t.From == "" && (t.Prompt != "" || len(t.Images) > 0 || len(t.Pictures) > 0) {
			out = append(out, UserMessage{fmt.Sprintf("t%d", t.N), t.Prompt, t.Images, t.Pictures})
		}
		for _, it := range t.Items {
			if it.Kind == KInterject {
				out = append(out, UserMessage{t.messageRef(it), it.Text, it.Images, it.Pictures})
			}
		}
	}
	return out
}

// ExpandedMessage restores pasted text while retaining its original newlines.
func ExpandedMessage(text string) string {
	return EachPaste(text, func(s string) string { return "\n" + s + "\n" })
}
