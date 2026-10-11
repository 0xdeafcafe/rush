package host

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// bareAgent is the fake agent without a system prompt to take rush's.
type bareAgent struct{ fakeAgent }

func init() { agent.Register(bareAgent{}) }

func (bareAgent) Kind() agent.Kind { return "bare" }
func (bareAgent) Features() map[agent.Feature]agent.Support {
	return map[agent.Feature]agent.Support{agent.FeatureRun: agent.Yes}
}

// An agent without a system prompt gets rush's atop its first message.
func TestUntoldGetsThePromptFirst(t *testing.T) {
	home := filepath.Dir(setup(t))
	cfg, err := Spawn(Config{Kind: "bare", Cwd: home, Account: agent.Profile{Kind: "bare", Name: "bare", Dir: home}, Prompt: "hi"})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	cmd := next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok }).(event.Approval).Call.Input.Command
	if !strings.HasPrefix(cmd, "echo <system-reminder>\n"+tasksPrompt) || !strings.HasSuffix(cmd, "</system-reminder>\n\nhi") {
		t.Errorf("first message = %q", cmd)
	}
}
