package plugin

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"io"
	"net"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

func pair(t *testing.T, a, b Handler) (*Conn, *Conn) {
	t.Helper()
	x, y := net.Pipe()
	ca, cb := NewConn(x, a), NewConn(y, b)
	t.Cleanup(func() { ca.Close(); cb.Close() })
	return ca, cb
}

func TestCallsBothWays(t *testing.T) {
	echo := func(_ context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "echo":
			return params, nil
		case "fail":
			return nil, &Error{Code: 7, Message: "no"}
		case "boom":
			return nil, errors.New("broke")
		}
		return nil, &Error{Code: CodeNoMethod, Message: "method not found: " + method}
	}
	a, b := pair(t, echo, echo)
	ctx := context.Background()
	var got map[string]int
	if err := a.Call(ctx, "echo", map[string]int{"n": 1}, &got); err != nil || got["n"] != 1 {
		t.Fatalf("a→b echo = %v, %v", got, err)
	}
	if err := b.Call(ctx, "echo", map[string]int{"n": 2}, &got); err != nil || got["n"] != 2 {
		t.Fatalf("b→a echo = %v, %v", got, err)
	}
	var e *Error
	if err := a.Call(ctx, "fail", nil, nil); !errors.As(err, &e) || e.Code != 7 {
		t.Fatalf("fail = %v", err)
	}
	if err := a.Call(ctx, "boom", nil, nil); !errors.As(err, &e) || e.Code != CodeServer || e.Message != "broke" {
		t.Fatalf("boom = %v", err)
	}
	if err := a.Call(ctx, "nope", nil, nil); !errors.As(err, &e) || e.Code != CodeNoMethod {
		t.Fatalf("nope = %v", err)
	}
}

func TestConcurrentCallsMatchTheirReplies(t *testing.T) {
	slowEcho := func(_ context.Context, _ string, params jsontext.Value) (any, error) {
		var n int
		_ = jsonx.Unmarshal(params, &n)
		time.Sleep(time.Duration(50-n) * time.Millisecond) // later calls answer first
		return n, nil
	}
	a, _ := pair(t, nil, slowEcho)
	var wg sync.WaitGroup
	for i := range 50 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			var got int
			if err := a.Call(context.Background(), "n", i, &got); err != nil || got != i {
				t.Errorf("call %d got %d, %v", i, got, err)
			}
		}()
	}
	wg.Wait()
}

func TestNotificationsArriveInOrder(t *testing.T) {
	var mu sync.Mutex
	var seen []int
	done := make(chan struct{})
	a, _ := pair(t, nil, func(_ context.Context, _ string, params jsontext.Value) (any, error) {
		var n int
		_ = jsonx.Unmarshal(params, &n)
		mu.Lock()
		seen = append(seen, n)
		if len(seen) == 100 {
			close(done)
		}
		mu.Unlock()
		return nil, nil
	})
	for i := range 100 {
		if err := a.Notify("ev", i); err != nil {
			t.Fatal(err)
		}
	}
	<-done
	for i, n := range seen {
		if n != i {
			t.Fatalf("notification %d arrived as %d", n, i)
		}
	}
}

func TestCallFailsWhenTheOtherSideGoes(t *testing.T) {
	block := make(chan struct{})
	a, b := pair(t, nil, func(context.Context, string, jsontext.Value) (any, error) { <-block; return nil, nil })
	errc := make(chan error)
	go func() { errc <- a.Call(context.Background(), "wait", nil, nil) }()
	time.Sleep(20 * time.Millisecond)
	b.Close()
	if err := <-errc; !errors.Is(err, ErrClosed) {
		t.Fatalf("call on closed conn = %v, want ErrClosed", err)
	}
	close(block)
	if err := a.Call(context.Background(), "again", nil, nil); err == nil {
		t.Fatal("call after close succeeded")
	}
}

func TestOversizedFrameClosesTheConnection(t *testing.T) {
	x, y := net.Pipe()
	c := NewConn(x, nil)
	defer c.Close()
	go func() { _, _ = y.Write([]byte{0xff, 0xff, 0xff, 0xff}) }()
	select {
	case <-c.Done():
	case <-time.After(time.Second):
		t.Fatal("a 4 GB frame header didn't close the connection")
	}
	if c.Err() == nil || !strings.Contains(c.Err().Error(), "over") {
		t.Fatalf("err = %v", c.Err())
	}
}

func TestLineConnSpeaksMCPStdio(t *testing.T) {
	// The server side reads lines on its stdin and writes lines to stdout.
	inR, inW := io.Pipe()
	outR, outW := io.Pipe()
	server := NewLineConn(inR, outW, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		return map[string]string{"method": method}, nil
	})
	client := NewLineConn(outR, inW, nil)
	defer server.Close()
	defer client.Close()
	var got map[string]string
	if err := client.Call(context.Background(), "tools/list", nil, &got); err != nil || got["method"] != "tools/list" {
		t.Fatalf("got %v, %v", got, err)
	}
	// A long line crosses the reader's buffer several times.
	long := strings.Repeat("x", 300<<10)
	var echoed map[string]string
	if err := client.Call(context.Background(), long, nil, &echoed); err != nil || echoed["method"] != long {
		t.Fatalf("long line: %v", err)
	}
}
