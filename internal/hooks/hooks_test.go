package hooks

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"net"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// fakeBroker answers a client as the broker would, with state s, and
// calls answer for anything else.
func fakeBroker(t *testing.T, s plugin.UIState, answer plugin.Handler) (*Client, chan *plugin.Conn) {
	t.Helper()
	c := New()
	brokers := make(chan *plugin.Conn, 1)
	c.dial = func(h plugin.Handler) (*plugin.Conn, error) {
		a, b := net.Pipe()
		srv := plugin.NewConn(b, func(ctx context.Context, method string, params jsontext.Value) (any, error) {
			if method == "ui.attach" {
				return s, nil
			}
			return answer(ctx, method, params)
		})
		brokers <- srv
		return plugin.NewConn(a, h), nil
	}
	t.Cleanup(c.Close)
	return c, brokers
}

func waitConnected(t *testing.T, c *Client) {
	t.Helper()
	for i := 0; c.conn.Load() == nil; i++ {
		if i > 200 {
			t.Fatal("never connected")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

var eventsAndIntercept = plugin.UIState{Plugins: []plugin.UIPlugin{{Name: "p", UI: []string{"events", "input", "intercept"}}}}

func TestEmitNeverWaits(t *testing.T) {
	// A broker that never reads what it's sent.
	block := make(chan struct{})
	c, _ := fakeBroker(t, eventsAndIntercept, func(context.Context, string, jsontext.Value) (any, error) {
		<-block
		return nil, nil
	})
	defer close(block)
	c.Start()
	waitConnected(t, c)
	start := time.Now()
	for range eventQueue * 4 {
		c.Emit(plugin.UIEvent{Kind: plugin.EvTurnEnded})
	}
	if d := time.Since(start); d > 50*time.Millisecond {
		t.Fatalf("Emit took %v", d)
	}
	if c.Dropped.Load() == 0 {
		t.Fatal("a full queue should drop")
	}
}

func TestEmitSkipsWhatNobodyHears(t *testing.T) {
	c, _ := fakeBroker(t, plugin.UIState{Plugins: []plugin.UIPlugin{{Name: "p", UI: []string{"overview"}}}}, nil)
	c.Start()
	waitConnected(t, c)
	c.Emit(plugin.UIEvent{Kind: plugin.EvInputChanged, Text: "secret"})
	if len(c.events) != 0 {
		t.Fatal("an input event went to a broker with no plugin allowed to see it")
	}
}

func TestInterceptTimesOutToAllow(t *testing.T) {
	c, _ := fakeBroker(t, eventsAndIntercept, func(ctx context.Context, method string, _ jsontext.Value) (any, error) {
		<-ctx.Done()
		time.Sleep(2 * time.Second)
		return plugin.InterceptResult{Action: "block"}, nil
	})
	c.Start()
	waitConnected(t, c)
	start := time.Now()
	msg := c.Intercept(plugin.Intercept{Hook: "before-send", Text: "hi"}, func(r plugin.InterceptResult) tea.Msg { return r })()
	if r := msg.(plugin.InterceptResult); r.Action != "allow" {
		t.Fatalf("got %+v", r)
	}
	if d := time.Since(start); d > plugin.InterceptBudget+time.Second {
		t.Fatalf("waited %v", d)
	}
}

func TestInterceptRewrite(t *testing.T) {
	c, _ := fakeBroker(t, eventsAndIntercept, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		return plugin.InterceptResult{Action: "rewrite", Text: "HI"}, nil
	})
	c.Start()
	waitConnected(t, c)
	if !c.Intercepts() {
		t.Fatal("should intercept")
	}
	msg := c.Intercept(plugin.Intercept{Hook: "before-send", Text: "hi"}, func(r plugin.InterceptResult) tea.Msg { return r })()
	if r := msg.(plugin.InterceptResult); r.Action != "rewrite" || r.Text != "HI" {
		t.Fatalf("got %+v", r)
	}
}

func TestStateAndDoReachTheUI(t *testing.T) {
	c, brokers := fakeBroker(t, eventsAndIntercept, nil)
	c.Start()
	srv := <-brokers
	waitConnected(t, c)
	next := func() any {
		done := make(chan any, 1)
		go func() { done <- c.Next()() }()
		select {
		case m := <-done:
			return m
		case <-time.After(2 * time.Second):
			t.Fatal("nothing came")
		}
		return nil
	}
	if _, ok := next().(StateMsg); !ok {
		t.Fatal("attach should say the state changed")
	}
	_ = srv.Notify("ui.do", plugin.UIDo{Plugin: "p", Kind: "notify", Text: "hello"})
	_ = srv.Notify("ui.do", plugin.UIDo{Plugin: "p", UI: "someone-else", Kind: "notify", Text: "not you"})
	_ = srv.Notify("ui.do", plugin.UIDo{Plugin: "p", UI: c.ID(), Kind: "notify", Text: "you"})
	if d, ok := next().(DoMsg); !ok || d.Text != "hello" {
		t.Fatalf("got %#v", d)
	}
	if d, ok := next().(DoMsg); !ok || d.Text != "you" {
		t.Fatalf("got %#v", d)
	}
}

// An ask comes back to the window as it is, and the key chosen goes to the
// broker; an answer that can't be had holds the message back.
func TestInterceptAskAndAnswer(t *testing.T) {
	var got plugin.InterceptAnswer
	c, _ := fakeBroker(t, eventsAndIntercept, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "ui.intercept":
			return plugin.InterceptResult{Action: "ask", Plugin: "p", ID: "q", Question: "sure?"}, nil
		case "ui.intercept.answer":
			_ = jsonx.Unmarshal(params, &got)
			if got.Key == "x" {
				return nil, errors.New("gone")
			}
			return plugin.InterceptResult{Action: "rewrite", Text: "HI"}, nil
		}
		return nil, nil
	})
	c.Start()
	waitConnected(t, c)
	msg := c.Intercept(plugin.Intercept{Hook: "before-send", Text: "hi"}, func(r plugin.InterceptResult) tea.Msg { return r })()
	if r := msg.(plugin.InterceptResult); r.Action != "ask" || r.ID != "q" {
		t.Fatalf("got %+v", r)
	}
	msg = c.Answer(plugin.InterceptAnswer{Intercept: plugin.Intercept{Text: "hi"}, Plugin: "p", ID: "q", Key: "y"}, func(r plugin.InterceptResult) tea.Msg { return r })()
	if r := msg.(plugin.InterceptResult); r.Action != "rewrite" || got.Key != "y" || got.ID != "q" || got.Plugin != "p" || got.UI == "" {
		t.Fatalf("got %+v, broker had %+v", r, got)
	}
	msg = c.Answer(plugin.InterceptAnswer{Plugin: "p", Key: "x"}, func(r plugin.InterceptResult) tea.Msg { return r })()
	if r := msg.(plugin.InterceptResult); r.Action != "block" || r.Plugin != "p" {
		t.Fatalf("a failed answer = %+v", r)
	}
}
