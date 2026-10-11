package host

import (
	"bytes"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A client that says it reads events gets a Claude Code session as rush's
// own; one that says nothing, an older rush, gets Claude Code's lines,
// and the op it sends first is still carried out.
func TestHelloChoosesEncoding(t *testing.T) {
	bin := setup(t)
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin})
	if err != nil {
		t.Fatal(err)
	}
	old, err := dial(cfg.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer old.Close()
	if err := old.Send("hi"); err != nil { // before any hello wait is over
		t.Fatal(err)
	}
	next(t, old, func(ev any) bool { _, ok := ev.(headless.PermissionRequest); return ok })

	c, err := dial(cfg.ID, eventsFrom)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	timeout := time.After(5 * time.Second)
	var sawCall bool
	for !sawCall {
		select {
		case line, ok := <-c.Lines:
			if !ok {
				t.Fatal("connection closed")
			}
			if isClaudeLine(line) {
				t.Fatalf("a client reading events got Claude Code's line %s", line)
			}
			if !bytes.HasPrefix(line, []byte(`{"type":"`+typeEvent+`"`)) {
				continue
			}
			ev, err := Decode(line)
			if err != nil {
				t.Fatalf("decode %s: %v", line, err)
			}
			if m, ok := ev.(event.Message); ok {
				for _, p := range m.Parts {
					sawCall = sawCall || p.Call != nil && p.Call.Input.Command == "echo hi"
				}
			}
		case <-timeout:
			t.Fatal("no call came as an event")
		}
	}
}

// Claude Code doesn't always start a line with its type: a turn's result
// can come with its usage first. It's still Claude's, and goes as rush's
// own event; the host's own lines go as they are.
func TestEncoderReadsClaudeInAnyOrder(t *testing.T) {
	var e encoder
	var b bytes.Buffer
	for _, l := range []string{
		`{"duration_api_ms":3707,"session_id":"s","total_cost_usd":0.01,"type":"result","subtype":"success","is_error":false,"result":"ok"}`,
		`{"agtop_sent":true,"message":{"content":"hi","role":"user"},"type":"user"}`,
	} {
		if err := e.encode(&b, []byte(l)); err != nil {
			t.Fatal(err)
		}
	}
	lines := strings.Split(strings.TrimSpace(b.String()), "\n")
	if len(lines) != 2 {
		t.Fatalf("lines:\n%s", b.String())
	}
	if ev, err := Decode([]byte(lines[0])); err != nil || ev.(event.TurnEnd).Text != "ok" {
		t.Errorf("result: %v %+v", err, ev)
	}
	if ev, _ := Decode([]byte(lines[1])); ev.(Sent).Text != "hi" {
		t.Errorf("sent: %+v", ev)
	}
}
