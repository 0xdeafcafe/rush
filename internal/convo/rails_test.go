package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A group draws a rail beside exactly the steps it owns: ╭ on an opened
// run's header, │ down its steps, ╰ on the last; a subagent's steps hang on
// a rail under its glyph, and one inside it on another a column further in.
func TestGroupsDrawRails(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "dig"}, at(0))
	for i, id := range []string{"r1", "r2", "r3"} {
		s.Apply(toolUse(id, "Read", map[string]any{"file_path": "/work/f" + string(rune('a'+i)) + ".go"}), at(1))
		s.Apply(toolResult(id, "…", false, nil), at(1))
	}
	s.Apply(toolUse("a1", "Task", map[string]any{"subagent_type": "Explore", "description": "outer"}), at(2))
	s.Apply(headless.Message{Role: "assistant", ParentToolUseID: "a1", Blocks: []headless.Block{{Type: "tool_use", ID: "a2", Name: "Task", Input: raw(map[string]any{"subagent_type": "Explore", "description": "inner"})}}}, at(3))
	s.Apply(headless.Message{Role: "assistant", ParentToolUseID: "a2", Blocks: []headless.Block{{Type: "tool_use", ID: "g1", Name: "Grep", Input: raw(map[string]any{"pattern": "deep"})}}}, at(4))
	s.Apply(headless.Message{Role: "assistant", ParentToolUseID: "a2", Blocks: []headless.Block{{Type: "tool_use", ID: "g2", Name: "Grep", Input: raw(map[string]any{"pattern": "deeper"})}}}, at(4))
	s.Apply(headless.Result{Subtype: "success"}, at(6))
	out := plain(s.Render(Options{Width: 90, Now: at(7), Open: map[string]bool{"t1:run:0": true, "t1:s:a1": true, "t1:s:a2": true}}))
	for _, want := range []string{
		"│ ╭ ▾ hide 3 steps",
		"│ │   ✓ ◧ /work/fa.go", // a level in, clear of the rail
		"│ ╰   ✓ ◧ /work/fc.go",
		"▍   │ ◌ ⇉ Explore  inner", // the inner subagent hangs on the outer's rail
		"▍   │ │ ◌ ⌕ deep",         // and its steps on a second rail beside it
		"▍   ╰ ╰ ◌ ⌕ deeper",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
}
