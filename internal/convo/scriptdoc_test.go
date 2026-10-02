package convo

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
)

// A call run as a script draws as the file it ran: numbered, the line it's
// on marked with how long, what each line before took, its breakpoints.
func TestScriptDoc(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "build"}, at(0))
	cmd := "cd /work\ngo build ./...\ngo vet ./...\ngo test ./..."
	s.Apply(toolUse("a", "Bash", map[string]any{"command": cmd}), at(1))
	v := &ScriptView{At: 2, Since: at(3), Took: map[int]time.Duration{1: 2 * time.Second}, Breaks: map[int]bool{4: true}}
	o := Options{Width: 100, Now: at(10), Verbose: true, Scripts: map[string]*ScriptView{"a": v}}
	out := plain(s.Render(o))
	for _, w := range []string{"ran as a script", "  1   cd /work", "2.0s", "  2 ▸ go build ./...", "7s", "● 4   go test"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	var refs []string
	for _, l := range s.Render(o) {
		if _, n, ok := ScriptLine(l.Ref); ok && n == 3 {
			refs = append(refs, l.Ref)
		}
	}
	if len(refs) != 1 {
		t.Errorf("line 3's row is its own to click: %v", refs)
	}
	v.Held, v.At = true, 4
	if out := plain(s.Render(o)); !strings.Contains(out, "⏸ stopped at line 4") {
		t.Errorf("held:\n%s", out)
	}
}
