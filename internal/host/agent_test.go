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
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// fakeAgent is an agent other than Claude Code: each message runs a
// command it asks to run, then ends the turn. Its starts are logged to
// starts.log beside its profile, with the session it resumed.
type fakeAgent struct{}

func init() { agent.Register(fakeAgent{}) }

func (fakeAgent) Kind() agent.Kind   { return "fake" }
func (fakeAgent) Name() string       { return "Fake" }
func (fakeAgent) Level() agent.Level { return agent.LevelPreview }
func (fakeAgent) Features() map[agent.Feature]agent.Support {
	return map[agent.Feature]agent.Support{agent.FeatureRun: agent.Yes, agent.FeaturePrompt: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeatureRewind: agent.Yes}
}
func (fakeAgent) Profiles() []agent.Profile { return nil }

func (fakeAgent) Start(_ context.Context, o agent.StartOptions) (agent.Conn, error) {
	f, _ := os.OpenFile(filepath.Join(o.Profile.Dir, "starts.log"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	resumed := ""
	if o.Resume {
		resumed = o.SessionID
	}
	f.WriteString(strings.TrimSpace("start "+resumed) + "\n")
	f.Close()
	c := &fakeConn{events: make(chan event.Event, 32)}
	c.events <- event.Init{SessionID: "fake-session", Model: "fake-1"}
	return c, nil
}

type fakeConn struct {
	events chan event.Event
	once   sync.Once
}

func (c *fakeConn) Events() <-chan event.Event { return c.events }

func (c *fakeConn) Send(in agent.Input) error {
	call := tool.Call{ID: "c1", Name: "shell", Kind: tool.Shell, Input: tool.Input{Command: "echo " + in.Text}}
	c.events <- event.MessageStart{ID: "m1"}
	c.events <- event.Delta{Kind: event.Text, Text: "On it"}
	c.events <- event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{{Kind: event.Text, Text: "On it"}, {Kind: event.ToolCall, Call: &call}}}
	c.events <- event.Approval{ID: "ap1", Call: call, Options: []event.Option{{ID: "yes", Kind: event.AllowOnce}, {ID: "no", Kind: event.RejectOnce}}}
	return nil
}

func (c *fakeConn) Answer(id, option string) error {
	exit := 0
	if option == "no" {
		exit = 1
	}
	c.events <- event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: "c1", Text: "answered " + option, Exit: &exit, IsError: exit != 0}}}}
	c.events <- event.TurnEnd{Reason: "done", Cost: 0.02}
	return nil
}

func (c *fakeConn) Interrupt() error      { return nil }
func (c *fakeConn) SetModel(string) error { return nil }
func (c *fakeConn) SetMode(string) error  { return nil }
func (c *fakeConn) Close() error          { c.once.Do(func() { close(c.events) }); return nil }

func TestHostRunsAnyAgent(t *testing.T) {
	home := filepath.Dir(setup(t))
	cfg, err := Spawn(Config{Owner: os.Getpid(), Kind: "fake", Cwd: home, Account: agent.Profile{Kind: "fake", Name: "fake", Dir: home}, Prompt: "hi"})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()

	ap := next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok }).(event.Approval)
	if ap.Call.Input.Command != "echo hi" {
		t.Errorf("approval = %+v", ap)
	}
	if info := next(t, c, inState("blocked")).(InfoEvent).Info; info.Kind != "fake" || !strings.Contains(info.Needs, "echo hi") {
		t.Errorf("while asking: %+v", info)
	}
	if err := c.Allow(ap.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	out := next(t, c, func(ev any) bool {
		m, ok := ev.(event.Message)
		return ok && m.Role == "user"
	}).(event.Message)
	if o := out.Parts[0].Output; o.Text != "answered yes" {
		t.Errorf("the host answered with %q, want yes", o.Text)
	}
	idle := next(t, c, inState("idle")).(InfoEvent).Info
	if idle.CostUSD != 0.02 || idle.SessionID != "fake-session" {
		t.Errorf("idle: %+v", idle)
	}

	// Idle well past the old 3s rest, its agent keeps running: rush ends
	// an agent only when you ask, and the next message goes to the same one.
	time.Sleep(4 * time.Second)
	if i, err := ReadInfo(cfg.ID); err != nil || i.Sleeping || !Alive(i.HostPID) {
		t.Fatalf("idle agent was stopped: %+v (%v)", i, err)
	}
	if err := c.Send("again"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { a, ok := ev.(event.Approval); return ok && a.Call.Input.Command == "echo again" })
	starts, _ := os.ReadFile(filepath.Join(home, "starts.log"))
	if got := strings.Split(strings.TrimSpace(string(starts)), "\n"); len(got) != 1 || got[0] != "start" {
		t.Errorf("starts:\n%s", starts)
	}
	if err := c.Stop(); err != nil {
		t.Fatal(err)
	}
}
