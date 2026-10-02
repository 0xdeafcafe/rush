package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/script"
)

// A long call rush ran as a script shows the line it's on from its trace,
// how long the lines before took, and a click on a line's number sets a
// breakpoint there.
func TestScriptViewsFollowTheTrace(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if script.Bash() == "" {
		t.Skip("no bash 4 to run scripts with")
	}
	cmd := "cd /tmp\necho a\nsleep 1\necho b"
	now := time.Now()
	s := convo.New()
	s.Apply(host.Sent{Text: "go"}, now)
	s.Apply(event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "b1", Name: "Bash", Kind: tool.Shell, Input: tool.Input{Command: cmd}}},
	}}, now)
	if _, err := script.Wrap("b1", cmd); err != nil {
		t.Fatal(err)
	}
	t0 := float64(now.Add(-3*time.Second).UnixNano()) / 1e9
	trace := fmt.Sprintf("pid 1 %d\n1 %.6f\n2 %.6f\n3 %.6f\n", int(t0), t0, t0+0.5, t0+1)
	if err := os.WriteFile(filepath.Join(script.Dir(), "b1.trace"), []byte(trace), 0o644); err != nil {
		t.Fatal(err)
	}
	c := &hostConn{key: "k", sess: s}
	v := c.scriptViews(now)["b1"]
	if v == nil || v.At != 3 || v.Took[1] != 500*time.Millisecond || v.Held {
		t.Fatalf("the script should be on line 3, line 1 having taken 500ms: %+v", v)
	}
	m := &Model{host: c}
	if !m.toggleScriptBreak(c, convo.ScriptLineRef("t0:s:b1", 4)) || !slices.Equal(script.Breaks("b1"), []int{4}) {
		t.Fatalf("clicking line 4 should stop the script there: %v", script.Breaks("b1"))
	}
	m.toggleScriptBreak(c, convo.ScriptLineRef("t0:s:b1", 4))
	if len(script.Breaks("b1")) != 0 {
		t.Fatalf("clicking it again should take it away: %v", script.Breaks("b1"))
	}
}
