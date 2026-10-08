package convo

import (
	"strings"
	"sync/atomic"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/cellw"
)

func peerLabel(p event.Peer) string {
	label := p.Name
	if label == "" {
		label = p.Kind
	}
	if label == "" {
		label = "agent"
	}
	if p.Kind != "" && p.Kind != label {
		label += " · " + p.Kind
	}
	if p.SessionID != "" {
		label += " · " + p.SessionID
	}
	return label
}

// exchange has its own row; a received message's Sent and provider echo only
// attach to that row, never manufacture another user prompt.
func (s *Session) exchange(e event.Exchange, now time.Time) sentEcho {
	key := e.Direction + ":" + e.ID
	if s.exchanges == nil {
		s.exchanges = map[string]sentEcho{}
	}
	if old, ok := s.exchanges[key]; ok && e.ID != "" {
		return old
	}
	if e.Archived {
		return s.archivedExchange(e, now)
	}
	t := s.Live()
	if t == nil && (e.Direction == "sent" || e.Phase == "result") && len(s.Turns) > 0 {
		t = s.Turns[len(s.Turns)-1]
	}
	if t == nil {
		t = s.turnFor(now)
		if e.Direction == "sent" || e.Phase == "result" {
			t.Live = false
			t.End = now
			t.From = "agent exchange"
		} else {
			t.From = peerLabel(e.Sender)
		}
	}
	copy := e
	it := &Item{Kind: KExchange, Text: e.Text, Images: e.Images, Exchange: &copy, At: now}
	t.Items = append(t.Items, it)
	t.touch()
	echo := sentEcho{text: e.Text, turn: t, item: it}
	if e.ID != "" {
		s.exchanges[key] = echo
	}
	return echo
}

// peerMark is an agent kind's logo in its colour, as rush's UI marks it.
var peerMark atomic.Pointer[func(kind string) string]

// SetPeerMark is how an agent of a kind is marked: its logo, in its colour.
func SetPeerMark(f func(kind string) string) { peerMark.Store(&f) }

// exchange draws a message between agents as a card: who sent it to whom on
// its top edge, the start of what it said inside (all of it, opened), and
// what it was on the bottom edge.
func (d *drawer) exchange(it *Item) {
	if it.Exchange == nil {
		return
	}
	e := it.Exchange
	peer := e.Sender
	if e.Direction == "sent" {
		peer = e.Receiver
	}
	noun := "message"
	switch e.Phase {
	case "request":
		noun = "delegated task"
	case "result":
		noun = "result"
	}
	ref := "exchange:" + e.Direction + ":" + e.ID + ":" + peer.SessionID
	open, overridden := d.o.Open[ref]
	if !overridden {
		open = d.o.Verbose
	}
	room := max(20, min(min(d.cw, capRow)-gutter-4, 96))
	body := strings.TrimSpace(it.Text)
	var all []string
	for _, l := range strings.Split(body, "\n") {
		if strings.TrimSpace(l) == "" {
			all = append(all, "")
			continue
		}
		all = append(all, wrap(sub(expandTabs(l)), room)...)
	}
	rows := all
	if open {
		rows = d.messageMarkdown(body, room)
	}
	if !open {
		first, rest, _ := strings.Cut(body, "\n")
		rows = cardText(first, strings.Split(rest, "\n"), room)
	}
	for _, p := range it.Images {
		rows = append(rows, dim("attachment · "+p))
	}
	foot := dim(noun)
	switch {
	case !open && len(all) > len(rows)-len(it.Images):
		foot += dim(" · space expands")
	case open && len(all) > 5:
		foot += dim(" · space folds")
	}
	d.gap()
	d.box(ref, gutter, exchangeWho(e, room-4), "", rows, foot, room, cFaint)
	d.gap()
}

// exchangeWho is who sent a message to whom, each half the room: their
// logo, name and session.
func exchangeWho(e *event.Exchange, width int) string {
	half := (width - 3) / 2
	return exchangePeer(e.Sender, half) + " " + paint(cBlue, "→") + " " + exchangePeer(e.Receiver, half)
}

// exchangePeer is one end of a message: its agent's logo, its name, and
// the start of its session's id, which tells two sessions of one title apart.
func exchangePeer(p event.Peer, width int) string {
	mark := "◇"
	if f := peerMark.Load(); f != nil && p.Kind != "" {
		mark = (*f)(p.Kind)
	}
	name := firstNonEmpty(oneLine(p.Name), p.Kind, "agent")
	id := p.SessionID
	if len(id) > 8 {
		id = id[:8]
	}
	room := width - cellw.String(mark) - 1
	if id != "" && room-len(id)-1 >= 4 {
		room -= len(id) + 1
		id = " " + faint(id)
	} else {
		id = "" // too tight: the name first
	}
	return cellw.Truncate(mark+" "+paint(cWhite, cellw.Truncate(name, max(1, room), "…"))+id, max(0, width), "…")
}

// ExchangeText returns the complete selected exchange, including its attribution.
func (s *Session) ExchangeText(ref string) (string, bool) {
	if !strings.HasPrefix(ref, "exchange:") {
		return "", false
	}
	parts := strings.Split(ref, ":")
	if len(parts) < 3 {
		return "", false
	}
	entry, ok := s.exchanges[parts[1]+":"+parts[2]]
	if !ok || entry.item == nil || entry.item.Exchange == nil {
		return "", false
	}
	e := entry.item.Exchange
	text := peerLabel(e.Sender) + " → " + peerLabel(e.Receiver) + "\n\n" + e.Text
	for _, image := range entry.item.Images {
		text += "\n" + image
	}
	return text, true
}

// ExchangePeer resolves a foldable exchange row to its peer session, if known.
func ExchangePeer(ref string) (string, bool) {
	if !strings.HasPrefix(ref, "exchange:") {
		return "", false
	}
	i := strings.LastIndexByte(ref, ':')
	return ref[i+1:], true
}

// RestoreExchanges merges Rush's archived records into a provider transcript.
// A provider's echoed user input becomes the corresponding attributed row.
func (s *Session) RestoreExchanges(records []event.Exchange) {
	for _, e := range records {
		e.Archived = true
		s.exchange(e, e.At)
	}
}

func (s *Session) archivedExchange(e event.Exchange, now time.Time) sentEcho {
	if !e.At.IsZero() {
		now = e.At
	}
	var target *Turn
	var replace *Item
	matchedPrompt := false
	var best time.Duration = 1<<63 - 1
	for _, t := range s.Turns {
		distance := t.Start.Sub(now)
		if distance < 0 {
			distance = -distance
		}
		if e.Direction == "received" && e.Phase != "result" && !e.At.IsZero() && !t.Start.IsZero() {
			if t.Prompt == e.Text && e.Text != "" && distance <= 2*time.Minute && distance < best {
				target = t
				replace = nil
				matchedPrompt = true
				best = distance
			}
			for _, it := range t.Items {
				if it.Kind == KInterject && it.Text == e.Text && distance <= 2*time.Minute && distance < best {
					target = t
					replace = it
					matchedPrompt = false
					best = distance
				}
			}
		}
	}
	if target != nil {
		if replace == nil {
			target.Prompt = ""
			target.From = peerLabel(e.Sender)
		}
	} else {
		// Report-only records attach by time; they must never wake a past turn.
		for _, t := range s.Turns {
			if !t.Start.After(now) {
				target = t
			}
		}
		if target == nil && len(s.Turns) > 0 {
			target = s.Turns[0]
		}
	}
	if target == nil {
		target = &Turn{N: 1, Start: now, End: now, From: "agent exchange", steps: map[string]*Step{}}
		s.Turns = append(s.Turns, target)
	}
	copy := e
	it := &Item{Kind: KExchange, Text: e.Text, Images: e.Images, Exchange: &copy, At: now}
	if replace != nil {
		*replace = *it
		it = replace
	} else {
		if matchedPrompt {
			target.Items = append([]*Item{it}, target.Items...)
		} else {
			target.Items = append(target.Items, it)
		}
	}
	target.touch()
	echo := sentEcho{text: e.Text, turn: target, item: it}
	s.exchanges[e.Direction+":"+e.ID] = echo
	return echo
}
