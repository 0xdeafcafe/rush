package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A Markdown file printed at the head of a chain reads as Markdown, and
// its fenced blocks each in their own language; a fence with none stays
// plain code.
func TestMarkdownOutputFences(t *testing.T) {
	s := New()
	s.Info.Cwd = "/w"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Verbose: true, Open: map[string]bool{}}, cw: 120}
	cmd := "cat .claude/coordinator/handoff-template.md | head -80 && git -C .worktrees/nx-correctness log --oneline 3e37e1b4a9..HEAD | cat"
	out := "# Handoff template\n\nCopy to `.claude/handoffs/<task-id>.md` and fill in.\n\n---\n\n" +
		"```markdown\n# Handoff: <task-id>\n\n## 1. Identity\n```\n\n" +
		"```go\nfunc main() { return }\n```\n\n```\nplain words\n```\n" +
		"3e37e1b feat: a commit\n"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		got = append(got, l.Text)
	}
	all := strings.Join(got, "\n")
	for _, want := range []string{hlKw + "# Handoff template", hlKw + "## 1. Identity", hlKw + "func", hlKw + "return", hlStr + "plain words"} {
		if !strings.Contains(all, want) {
			t.Errorf("output should have %q:\n%q", want, all)
		}
	}
}

// Narration closes up its paragraphs but not around a heading or a table,
// and a table too wide for the room wraps its cells instead of cutting them.
func TestProseTableWraps(t *testing.T) {
	d := &drawer{s: New(), t: &Turn{}, o: Options{Width: 60, Open: map[string]bool{}}, cw: 60}
	d.prose("Intro.\n## Head\n| Where | The fix |\n|---|---|\n| a/very/long/path/to/some/file.ts | keep only the generic type and let each module own its own |\nAfter.", gutter, cSub)
	var got []string
	for _, l := range d.lines {
		got = append(got, strings.TrimSpace(stripANSI(l.Text)))
	}
	all := strings.Join(got, "\n")
	if strings.Contains(all, "›") || !strings.Contains(all, "its own") {
		t.Errorf("cells should wrap, not be cut:\n%s", all)
	}
	if !strings.Contains(all, "Intro.\n\nHead") || !strings.Contains(all, "\n\nAfter.") {
		t.Errorf("a heading and a table should stand a row clear:\n%s", all)
	}
}
