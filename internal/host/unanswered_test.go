package host

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// lateAgent is fakeAgent that, sent "late", first ends a turn of its own
// (a notice it had queued) and only answers after the rest would come.
type lateAgent struct{ fakeAgent }

func init() { agent.Register(lateAgent{}) }

func (lateAgent) Kind() agent.Kind { return "late" }

func (lateAgent) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	c, err := fakeAgent{}.Start(ctx, o)
	if err != nil {
		return nil, err
	}
	return &lateConn{c.(*fakeConn)}, nil
}

type lateConn struct{ *fakeConn }

func (c *lateConn) Send(in agent.Input) error {
	if in.Text != "late" {
		return c.fakeConn.Send(in)
	}
	c.events <- event.TurnEnd{Reason: "done"}
	go func() {
		time.Sleep(600 * time.Millisecond)
		defer func() { _ = recover() }() // closed: rested early
		c.events <- event.Message{Role: "assistant", ID: "m2", Parts: []event.Part{{Kind: event.Text, Text: "answered late"}}}
		c.events <- event.TurnEnd{Reason: "done"}
	}()
	return nil
}

// An agent isn't rested between a turn it ends of its own and its answer
// to the message it was sent.
func TestUnansweredKeepsItUp(t *testing.T) {
	home := filepath.Dir(setup(t))
	cfg, err := Spawn(Config{Kind: "late", Cwd: home, Account: agent.Profile{Kind: "late", Name: "late", Dir: home}, Prompt: "late"})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	next(t, c, func(ev any) bool {
		m, ok := ev.(event.Message)
		return ok && m.Role == "assistant" && len(m.Parts) > 0 && m.Parts[0].Text == "answered late"
	})
}
