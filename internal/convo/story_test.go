package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A finished turn tells its story: a strip of how it went, its words,
// each batch of steps as a row, red when one failed, and its reply. While
// it runs, every step draws as it always has.
func TestStory(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	evs := []any{
		host.Sent{Text: "fix it"},
		say("Looking first."),
		toolUse("a", "Bash", map[string]any{"command": "go build ./..."}),
		toolResult("a", "", false, map[string]any{"stdout": "", "stderr": ""}),
		toolUse("b", "Bash", map[string]any{"command": "go vet ./..."}),
		toolResult("b", "exit 1", true, nil),
		say("The vet failed; trying again."),
		toolUse("c", "Bash", map[string]any{"command": "go test ./..."}),
		toolResult("c", "", false, map[string]any{"stdout": "", "stderr": ""}),
		toolUse("d", "Bash", map[string]any{"command": "git status"}),
		toolResult("d", "", false, map[string]any{"stdout": "", "stderr": ""}),
	}
	for i, e := range evs {
		s.Apply(e, at(i))
	}
	live := plain(s.Render(Options{Width: 100, Now: at(20)}))
	if strings.Contains(live, "▸ 2 steps") || !strings.Contains(live, "$ go vet ./...") {
		t.Errorf("a running turn should draw step by step:\n%s", live)
	}
	s.Apply(say("Fixed."), at(30))
	s.Apply(headless.Result{Subtype: "success"}, at(31))
	out := plain(s.Render(Options{Width: 100, Now: at(40), Agent: "@fixer"}))
	for _, w := range []string{"agent · @fixer", "Looking first.", "▸ 2 steps  go build, go vet", "The vet failed; trying again.", "▸ 2 steps  go test, git status", "Fixed."} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if strings.Contains(out, "$ go vet ./...") {
		t.Errorf("a failed step should fold into its batch:\n%s", out)
	}
	for _, l := range s.Render(Options{Width: 100, Now: at(40)}) {
		if strings.Contains(stripANSI(l.Text), "go vet") && !strings.Contains(l.Text, cRed) {
			t.Errorf("the failed batch isn't red: %q", l.Text)
		}
	}
}

// An error a turn stopped on says what kind it is and what gets past it.
func TestErrAdvice(t *testing.T) {
	for err, want := range map[string]string{
		"You've hit your session limit · resets 5am": "Usage limit reached",
		"API Error: 401 OAuth token has expired":     "Signed out",
		"Unable to connect to API (ECONNREFUSED)":    "Can't reach the API",
		"API Error: 529 Overloaded":                  "The API failed",
		"claude exited mid-turn: exit status 1":      "The agent stopped mid-turn",
		"something nobody planned for":               "The turn stopped on an error",
		"during execution":                           "Claude Code couldn't run the turn",
		"Not logged in · Please run /login":          "Signed out",
	} {
		if got, _ := errAdvice(err); got != want {
			t.Errorf("%q: %q, want %q", err, got, want)
		}
	}
}

// A document a finished turn wrote shows its head, by the write tool or a
// shell's heredoc, its last write only; code it wrote stays in the batch.
func TestStoryShowsDocs(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	evs := []any{
		host.Sent{Text: "write it up"},
		toolUse("a", "Write", map[string]any{"file_path": "/work/notes.md", "content": "# Draft\n\nfirst go\n"}),
		toolResult("a", "ok", false, map[string]any{"type": "create", "filePath": "/work/notes.md"}),
		toolUse("b", "Write", map[string]any{"file_path": "/work/notes.md", "content": "# Handoff\n\nwhere it stands\n"}),
		toolResult("b", "ok", false, map[string]any{"type": "update", "filePath": "/work/notes.md"}),
		toolUse("c", "Write", map[string]any{"file_path": "/work/main.go", "content": "package main\n"}),
		toolResult("c", "ok", false, map[string]any{"type": "create", "filePath": "/work/main.go"}),
		toolUse("d", "Bash", map[string]any{"command": "cat > report.md <<'EOF'\n# Report\nall clear\nEOF"}),
		toolResult("d", "", false, map[string]any{"stdout": "", "stderr": ""}),
		say("Done."),
		headless.Result{Subtype: "success"},
	}
	for i, e := range evs {
		s.Apply(e, at(i))
	}
	out := plain(s.Render(Options{Width: 100, Now: at(40)}))
	for _, w := range []string{"Handoff", "where it stands", "Report", "all clear"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if strings.Contains(out, "first go") || strings.Contains(out, "package main") {
		t.Errorf("only a document's last write shows:\n%s", out)
	}
}

// Looking back, a test run a later one in the turn tells again shows no
// card: only how the last came out.
func TestStoryLastTestsOnly(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	fail := "--- FAIL: TestOld (0.01s)\n    a_test.go:9: broke\nFAIL\nFAIL\tgithub.com/x/a\t0.2s\n"
	evs := []any{
		host.Sent{Text: "fix the tests"},
		toolUse("a", "Bash", map[string]any{"command": "go test ./..."}),
		toolResult("a", "", false, map[string]any{"stdout": fail, "stderr": ""}),
		toolUse("b", "Bash", map[string]any{"command": "go vet ./..."}),
		toolResult("b", "", false, map[string]any{"stdout": "", "stderr": ""}),
		say("Fixing it."),
		toolUse("c", "Bash", map[string]any{"command": "go test ./..."}),
		toolResult("c", "", false, map[string]any{"stdout": "ok  \tgithub.com/x/a\t0.2s\n", "stderr": ""}),
		toolUse("d", "Bash", map[string]any{"command": "git status"}),
		toolResult("d", "", false, map[string]any{"stdout": "", "stderr": ""}),
		say("Fixed."),
		headless.Result{Subtype: "success"},
		host.Sent{Text: "thanks"},
	}
	for i, e := range evs {
		s.Apply(e, at(i))
	}
	out := plain(s.Render(Options{Width: 100, Now: at(40)}))
	if strings.Contains(out, "TestOld") || !strings.Contains(out, "✗ 1 failed") {
		t.Errorf("the fixed run's card should go, its batch still red:\n%s", out)
	}
}
