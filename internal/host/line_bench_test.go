package host

import (
	"bytes"
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// benchTurn is a turn's output as Claude Code writes it: streamed deltas,
// the whole message they built, a tool call and its result.
func benchTurn(n int) [][]byte {
	var out [][]byte
	for i := range 40 {
		out = append(out, fmt.Appendf(nil, `{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"word %d "}},"session_id":"s"}`, i))
	}
	text := strings.Repeat("Looking at how the pane draws its rows. ", 40)
	out = append(out, fmt.Appendf(nil, `{"type":"assistant","message":{"id":"m%d","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":%q},{"type":"tool_use","id":"t%d","name":"Bash","input":{"command":"go test ./...","description":"Run the tests"}}],"usage":{"input_tokens":10,"output_tokens":200,"cache_read_input_tokens":50000}},"session_id":"s"}`, n, text, n))
	res := strings.Repeat("ok  \tgithub.com/x/y/pkg\t0.123s\n", 200)
	out = append(out, fmt.Appendf(nil, `{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t%d","content":%q}]},"session_id":"s"}`, n, res))
	return out
}

// tapConn is a session whose lines reach the host through tap.
type tapConn struct{ agent.Conn }

func (tapConn) Taps() bool { return true }

// BenchmarkHostTurn is a host taking in one turn's output: each line into
// the replay ring and to a client, the ones it looks at decoded and acted
// on (a whole message publishes the session's info).
func BenchmarkHostTurn(b *testing.B) {
	b.Setenv("RUSH_HOME", b.TempDir())
	s := &server{cfg: Config{ID: "bench"}, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	tc := tapConn{}
	var n headless.Neutral
	_ = os.MkdirAll(dir(s.cfg.ID), 0o700)
	c := &conn{out: make(chan []byte, 1<<16), gone: make(chan struct{})}
	s.clients[c] = struct{}{}
	turns := make([][][]byte, 64)
	for i := range turns {
		turns[i] = benchTurn(i)
	}
	i := 0
	b.ReportAllocs()
	for b.Loop() {
		for _, l := range turns[i%len(turns)] {
			s.tap(l)
			if bytes.HasPrefix(l, []byte(`{"type":"stream_event"`)) || bytes.HasPrefix(l, []byte(`{"type":"user"`)) {
				continue // what a conn started Lightly leaves out
			}
			ev, err := headless.Decode(l)
			if err != nil {
				b.Fatal(err)
			}
			s.mu.Lock()
			for _, e := range n.Event(ev) {
				s.onAgentEvent(tc, e)
			}
			s.mu.Unlock()
		}
		for len(c.out) > 0 {
			<-c.out
		}
		i++
	}
	b.ReportMetric(float64(s.ringN)/(1<<20), "ringMB")
}

// The info line publish sends is what marshalling it as a map would give.
func TestPublishLine(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	s := &server{cfg: Config{ID: "pub"}, clients: map[*conn]struct{}{}}
	_ = os.MkdirAll(dir(s.cfg.ID), 0o700)
	c := &conn{out: make(chan []byte, 4), gone: make(chan struct{})}
	s.clients[c] = struct{}{}
	s.info.Detail = "a <b> & c"
	s.publish()
	got := <-c.out
	want, _ := jsonx.Marshal(map[string]any{"type": typeInfo, "info": s.info})
	if string(got) != string(want) {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
	if ev, err := Decode(got); err != nil || ev.(InfoEvent).Info.Detail != s.info.Detail {
		t.Fatalf("decoded %v %v", ev, err)
	}
}
