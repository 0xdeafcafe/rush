package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// linesAgent's history is a prompt and an answer per line of its file.
type linesAgent struct{ installedAgent }

func init() { agent.Register(linesAgent{}) }

func (linesAgent) Kind() agent.Kind { return "lines" }

func (linesAgent) History(s agent.Session, _ time.Time) ([]event.Event, error) {
	b, err := os.ReadFile(s.Transcript)
	if err != nil {
		return nil, err
	}
	var out []event.Event
	for _, l := range strings.Fields(string(b)) {
		out = append(out, event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: l}}},
			event.TurnEnd{Reason: "done"})
	}
	return out, nil
}

func TestAHistoryIsFollowedAsItGrows(t *testing.T) {
	path := filepath.Join(t.TempDir(), "rollout.jsonl")
	if err := os.WriteFile(path, []byte("one\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	h := &history{kind: "lines", s: agent.Session{Transcript: path}}
	h.stat()
	c := &hostConn{kind: "claude", sess: agentHistory(h.kind, h.s, time.Time{}), hist: h}
	if n := len(c.sess.Turns); n != 1 {
		t.Fatalf("read %d turns, want 1", n)
	}
	if err := os.WriteFile(path, []byte("one\ntwo\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	c.followHistory()
	if n := len(c.sess.Turns); n != 2 {
		t.Errorf("after it grew: %d turns, want 2", n)
	}
	was := c.sess
	c.followHistory()
	if c.sess != was {
		t.Error("read again with nothing changed")
	}
}

// A stopped rush session of an agent whose transcripts rush doesn't tail
// opens from its history, in the folder its host ran it in: it never
// waits on a connection that won't come.
func TestStoppedHostedOpensFromHistory(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	d := filepath.Join(host.Root(), "abcd1234")
	if err := os.MkdirAll(d, 0o700); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(d, "config.json"), []byte(`{"id":"abcd1234","account":{"name":"x","configDir":"/work/.lines"}}`), 0o600)
	a := &fleet.Agent{Key: "k", Rush: true, Kind: "lines"}
	a.ID, a.SessionID = "abcd1234", "thread-1"
	msg, ok := openTail(a)().(hostOpenMsg)
	if !ok || msg.err != nil || msg.c == nil || msg.c.hist == nil || msg.c.hist.s.Profile.Dir != "/work/.lines" {
		t.Fatalf("opened %+v", msg)
	}
}

func TestStoppedHistoryRestoresAgentExchanges(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	id := "abcd1234"
	dir := filepath.Join(host.Root(), id)
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	e := event.Exchange{ID: "kept", Direction: "sent", Phase: "message", Text: "Persistent review", Sender: event.Peer{SessionID: id, Kind: "kimi"}, Receiver: event.Peer{SessionID: "other", Kind: "codex"}, At: time.Now()}
	b, err := jsonx.Marshal(e)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(dir, "exchanges.jsonl"), append(b, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
	for _, native := range []bool{false, true} {
		a := &fleet.Agent{Key: "k", Rush: true, Kind: "lines"}
		a.ID = id
		if native {
			a.Kind = "claude"
			a.TranscriptPath = filepath.Join(t.TempDir(), "history.jsonl")
			if err := os.WriteFile(a.TranscriptPath, []byte(`{"type":"user","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"original task"}}`+"\n"), 0600); err != nil {
				t.Fatal(err)
			}
		}
		msg := openTail(a)().(hostOpenMsg)
		if msg.err != nil {
			t.Fatal(msg.err)
		}
		ref := "exchange:sent:kept:other"
		if text, ok := msg.c.sess.ExchangeText(ref); !ok || !strings.Contains(text, e.Text) {
			t.Fatalf("native=%v missing stored exchange: %s", native, text)
		}
		if native {
			whole := msg.c.readWhole(convo.Options{})().(wholeMsg)
			if _, ok := whole.tail.Sess.ExchangeText(ref); !ok {
				t.Fatal("full transcript discarded exchange")
			}
		} else {
			s := msg.c.hist.read(&msg.c.closed)
			if _, ok := s.ExchangeText(ref); !ok {
				t.Fatal("history refresh discarded exchange")
			}
		}
	}
}
