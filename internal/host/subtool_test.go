package host

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agtools"
)

// echoAgent answers every message with it, and ends the turn.
type echoAgent struct{}

func init() { agent.Register(echoAgent{}) }

func (echoAgent) Kind() agent.Kind   { return "echo" }
func (echoAgent) Name() string       { return "Echo" }
func (echoAgent) Level() agent.Level { return agent.LevelPreview }
func (echoAgent) Features() map[agent.Feature]agent.Support {
	return map[agent.Feature]agent.Support{agent.FeatureRun: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeaturePrompt: agent.Yes}
}
func (echoAgent) Profiles() []agent.Profile { return nil }

func (echoAgent) Start(_ context.Context, _ agent.StartOptions) (agent.Conn, error) {
	c := &echoConn{events: make(chan event.Event, 32)}
	c.events <- event.Init{SessionID: "echo-session", Model: "echo-1"}
	return c, nil
}

type echoConn struct {
	events chan event.Event
	once   sync.Once
}

func (c *echoConn) Events() <-chan event.Event { return c.events }
func (c *echoConn) Send(in agent.Input) error {
	c.events <- event.Message{Role: "assistant", ID: "m", Parts: []event.Part{{Kind: event.Text, Text: "echo: " + in.Text}}}
	c.events <- event.TurnEnd{Reason: "done"}
	return nil
}
func (c *echoConn) Answer(string, string) error { return nil }
func (c *echoConn) Interrupt() error            { return nil }
func (c *echoConn) SetModel(string) error       { return nil }
func (c *echoConn) SetMode(string) error        { return nil }
func (c *echoConn) Close() error                { c.once.Do(func() { close(c.events) }); return nil }

// A spawned agent's host keeps its answer for whoever waits on it, and one
// asked to report sends it to its parent as a message from itself.
func TestSpawnedAgentAnswers(t *testing.T) {
	home := filepath.Dir(setup(t))
	start := func(cfg Config) Config {
		cfg.Kind, cfg.Cwd, cfg.Account = "echo", home, agent.Profile{Kind: "echo", Name: "echo", Dir: home}
		got, err := Spawn(cfg)
		if err != nil {
			t.Fatal(err)
		}
		return got
	}
	parent := start(Config{Owner: os.Getpid()})
	sa := subagents{parent: parent.ID, kind: "echo"}
	child := Config{Owner: os.Getpid(), Prompt: "find the bug", Meta: map[string]string{"spawnedBy": parent.ID}}
	child.fillIDs()
	child.PromptExchange = sa.exchange(child.ID, "request", child.Prompt)
	if err := report(child.ID, parent.ID); err != nil {
		t.Fatal(err)
	}
	since := time.Now()
	start(child)
	got, err := awaitAnswer(child.ID, "", since, 5*time.Second)
	if err != nil || !strings.Contains(got, "echo: find the bug") {
		t.Fatalf("answer: %q %v", got, err)
	}
	// The parent is sent it, as a message from the child.
	var ex event.Exchange
	for deadline := time.Now().Add(5 * time.Second); ex.ID == "" && time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		for _, e := range ReadExchanges(parent.ID) {
			if e.Phase == "result" {
				ex = e
			}
		}
	}
	if ex.Sender.SessionID != child.ID || !strings.Contains(ex.Text, "echo: find the bug") {
		t.Errorf("parent got %+v", ex)
	}
	// A waited follow-up is answered to the caller, and no longer reported.
	got, err = sa.Send(agtools.SendInput{ID: child.ID, Prompt: "and the fix?"})
	if err != nil || !strings.Contains(got, "echo: and the fix?") {
		t.Fatalf("follow-up: %q %v", got, err)
	}
	if _, err := os.Stat(reportPath(child.ID)); !os.IsNotExist(err) {
		t.Errorf("still reporting: %v", err)
	}
	if _, err := sa.Result(agtools.ResultInput{ID: parent.ID}); err == nil {
		t.Error("an agent it didn't start answered")
	}
}

// An answer rush will try again isn't the last; a turn that ends with a
// message queued is answered with the next, its error kept though it ended
// "done"; and the session it reports to is sent it once.
func TestAnswerLifecycle(t *testing.T) {
	setup(t)
	id := "answer01"
	if err := os.MkdirAll(dir(id), 0o700); err != nil {
		t.Fatal(err)
	}
	s := &server{cfg: Config{ID: id, Meta: map[string]string{"spawnedBy": "nobody00"}}, info: Info{State: "idle", Queue: []string{"next"}}}
	s.lastSaid = "first"
	s.turnDone(event.TurnEnd{Reason: "done"})
	if _, done := finished(id, s.info, time.Time{}); done {
		t.Error("finished with a message queued")
	}
	for _, info := range []Info{{State: "idle", Retry: &Retry{}}, {State: "idle", Limit: &Limit{Continue: true}}} {
		if _, done := finished(id, info, time.Time{}); done {
			t.Errorf("finished while rush tries again: %+v", info)
		}
	}
	if err := report(id, "nobody00"); err != nil {
		t.Fatal(err)
	}
	s.info.Queue, s.lastSaid = nil, "second"
	s.turnDone(event.TurnEnd{Reason: "done", Err: "is_error"})
	a, done := finished(id, s.info, time.Time{})
	if !done || a.Text != "first\n\nsecond" || a.Err != "is_error" {
		t.Errorf("answer %+v, done %v", a, done)
	}
	if _, err := os.Stat(reportPath(id)); !os.IsNotExist(err) {
		t.Error("reported, but would report again")
	}
}
