package ui

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// replierAgent takes messages from outside it, the way Claude Code's
// background daemon does, and counts them; err is what each reply returns.
type replierAgent struct {
	fakeAgent
	calls *[]string
	err   error
}

func (r replierAgent) Reply(_ agent.Profile, id, _ string) error {
	*r.calls = append(*r.calls, id)
	return r.err
}

func registerReplier(t *testing.T, err error) (agent.Kind, *[]string) {
	t.Helper()
	calls := &[]string{}
	k := agent.Kind("zreplier")
	agent.Register(replierAgent{fakeAgent: fakeAgent{kind: k, title: "ZReplier", dir: "/z/.zreplier"}, calls: calls, err: err})
	return k, calls
}

// A message for a session rush hosts goes to its host, never to the
// agent's own background service, whichever caller asked for it.
func TestReplyToAHostedSessionGoesToItsHost(t *testing.T) {
	k, calls := registerReplier(t, nil)
	// An ID no host can have: the host route refuses it at once, so the
	// test starts nothing.
	a := &fleet.Agent{Key: "h", DisplayName: "Backbase ClickHouse", Kind: string(k), Rush: true}
	msg := reply(a, "how is it going?")()
	if len(*calls) != 0 {
		t.Fatalf("a hosted session's message went to the background service: %v", *calls)
	}
	if d, ok := msg.(doneMsg); !ok || d.err == nil || !strings.Contains(d.err.Error(), "invalid session ID") {
		t.Fatalf("expected the host route's refusal, got %#v", msg)
	}
}

// When the background service refuses a message, rush logs it and says
// which agent it was for and why.
func TestReplyFailureNamesTheAgentAndIsLogged(t *testing.T) {
	home := t.TempDir()
	t.Setenv("RUSH_HOME", home)
	k, calls := registerReplier(t, errors.New("its background service isn't running"))
	a := &fleet.Agent{Key: "j", DisplayName: "clickhouse backup failure", Kind: string(k)}
	a.ID = "8c54d85d"
	msg := reply(a, "hello")()
	if len(*calls) != 1 {
		t.Fatalf("expected one reply through the background service, got %v", *calls)
	}
	d, ok := msg.(doneMsg)
	if !ok || d.err == nil {
		t.Fatalf("expected a failure, got %#v", msg)
	}
	if got := d.err.Error(); !strings.Contains(got, "clickhouse backup failure") || !strings.Contains(got, "isn't running") {
		t.Fatalf("the error should name the agent and the reason: %q", got)
	}
	events.Flush()
	b, err := os.ReadFile(filepath.Join(home, "events.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `"kind":"reply-failed"`) || !strings.Contains(string(b), "8c54d85d") {
		t.Fatalf("no reply-failed event for the agent:\n%s", b)
	}
}

// The day it went wrong: typed into a rush session at work, the message
// was offered to a Claude Code job that had finished weeks before, whose
// daemon wasn't running; enter took the offer and the message went to the
// daemon, which refused it. Only a session that can take a message now is
// offered.
func TestMisdirectNeverOffersAStoppedClaudeCodeJob(t *testing.T) {
	lw, yap, subjects := misdirectFleet()
	stale := *yap
	stale.Key, stale.Rush, stale.State, stale.PID = "stale", false, "done", 0
	text := "this history needs to use macos liquid glass ui!!"
	newModel := func(other *fleet.Agent) (*Model, *hostConn) {
		m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{lw, other}}, loader: &fleet.Loader{Temp: fleet.NewTempSizes()}, store: &state.Store{}}
		m.misdirect.subjects = subjects
		c := &hostConn{kind: "claude", key: "lw", sess: convo.New(), open: map[string]bool{}}
		c.input = []rune(text)
		return m, c
	}
	m, c := newModel(&stale)
	if m.askMisdirected(c, text, nil) {
		t.Fatalf("offered to send to a stopped Claude Code job: %q", m.confirm.question)
	}
	// One at work still is.
	live := stale
	live.State, live.PID = "working", 4242
	m, c = newModel(&live)
	if !m.askMisdirected(c, text, nil) {
		t.Fatal("a job at work should still be offered")
	}
}
