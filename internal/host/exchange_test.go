package host

import (
	"errors"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"testing"
)

type rejectExchangeConn struct{ sentConn }

func (*rejectExchangeConn) Send(agent.Input) error { return errors.New("rejected") }

func TestExchangeGuideAndRejectedDelivery(t *testing.T) {
	setup(t)
	e := event.Exchange{ID: "x", Text: "check this", Sender: event.Peer{SessionID: "source", Kind: "codex"}}
	c := &sentConn{}
	s := &server{cfg: Config{ID: "dest", Kind: "claude"}, conn: c, clients: map[*conn]struct{}{}}
	s.info.State = "working"
	if err := s.do(op{Op: "send", Text: e.Text, Guide: true, Exchange: &e}); err != nil {
		t.Fatal(err)
	}
	if len(c.sent) != 1 || c.stops != 0 || len(s.info.Queue) != 0 {
		t.Fatalf("guide changed semantics: %+v %+v", c, s.info)
	}
	var decoder Decoder
	var got []event.Exchange
	for _, line := range s.ring {
		events, _ := decoder.Decode(line)
		for _, ev := range events {
			if ex, ok := ev.(event.Exchange); ok {
				got = append(got, ex)
			}
		}
	}
	if len(got) != 1 || got[0].Sender.Kind != "codex" || got[0].Receiver.Kind != "claude" {
		t.Fatalf("lost origin: %+v", got)
	}
	s = &server{cfg: Config{ID: "reject", Kind: "claude"}, conn: &rejectExchangeConn{}, clients: map[*conn]struct{}{}}
	if err := s.deliverExchange(e.Text, nil, nil, &e, ""); err == nil {
		t.Fatal("accepted rejected delivery")
	}
	if len(s.ring) != 0 {
		t.Fatal("failed delivery recorded as success")
	}
}

func TestExchangeQueueRetainsAttribution(t *testing.T) {
	setup(t)
	e := event.Exchange{ID: "queued", Text: "agent input", Sender: event.Peer{SessionID: "source", Kind: "kimi"}}
	s := &server{cfg: Config{ID: "queue", Kind: "nothing-guides"}, conn: &sentConn{}, clients: map[*conn]struct{}{}}
	s.info.State = "working"
	if err := s.do(op{Op: "send", Text: e.Text, Guide: true, Exchange: &e}); err != nil {
		t.Fatal(err)
	}
	if len(s.info.QueueExchanges) != 1 || s.info.QueueExchanges[0].ID != e.ID {
		t.Fatalf("lost queued origin: %+v", s.info)
	}
	if err := s.do(op{Op: "queue_edit", Index: 0, Text: "edited"}); err != nil {
		t.Fatal(err)
	}
	if s.info.QueueExchanges[0].Text != "edited" || e.Text != "agent input" {
		t.Fatal("edit corrupted queued metadata")
	}
}
