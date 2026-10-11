package host

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// subAgent is fakeAgent with a subagent in the background that writes
// once the turn that started it has ended.
type subAgent struct{ fakeAgent }

func init() { agent.Register(subAgent{}) }

func (subAgent) Kind() agent.Kind { return "subfake" }

func (subAgent) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	c, err := fakeAgent{}.Start(ctx, o)
	if err != nil {
		return nil, err
	}
	return &subConn{fakeConn: c.(*fakeConn)}, nil
}

type subConn struct{ *fakeConn }

func (c *subConn) Answer(id, option string) error {
	if err := c.fakeConn.Answer(id, option); err != nil {
		return err
	}
	go func() {
		defer func() { _ = recover() }() // closed, stopped early
		time.Sleep(100 * time.Millisecond)
		c.events <- event.Message{Role: "assistant", ID: "sub1", Parent: "toolu_bg",
			Parts: []event.Part{{Kind: event.Text, Text: "still reading files"}}}
	}()
	return nil
}

// A subagent at work in the background isn't the agent's turn: the
// session stays idle, and what you send goes now, not into the queue.
func TestBackgroundSubagentDoesNotHoldTheQueue(t *testing.T) {
	home := filepath.Dir(setup(t))
	cfg, err := Spawn(Config{Kind: "subfake", Cwd: home, Account: agent.Profile{Kind: "subfake", Name: "sub", Dir: home}, Prompt: "hi"})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	ap := next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok }).(event.Approval)
	if err := c.Allow(ap.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	next(t, c, inState("idle"))
	next(t, c, func(ev any) bool { m, ok := ev.(event.Message); return ok && m.Parent != "" })
	if err := c.Send("next"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool {
		if i, ok := ev.(InfoEvent); ok && len(i.Info.Queue) > 0 {
			t.Fatalf("queued behind the subagent: %q", i.Info.Queue)
		}
		a, ok := ev.(event.Approval)
		return ok && a.Call.Input.Command == "echo next"
	})
	if err := c.Stop(); err != nil {
		t.Fatal(err)
	}
}
