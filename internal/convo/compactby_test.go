package convo

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestPlainText(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "fix the upload"}, now)
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "text", Text: "Looking at upload.go"}}}, now)
	got := s.PlainText()
	for _, w := range []string{"## User\nfix the upload", "## Assistant\nLooking at upload.go"} {
		if !strings.Contains(got, w) {
			t.Fatalf("no %q in\n%s", w, got)
		}
	}
}

func TestPrunedKeepsChosenStepsWholeAndRecentTurns(t *testing.T) {
	step := func(tool, in, out string) *Item {
		return &Item{Kind: KStep, Step: &Step{Tool: tool, Input: []byte(`"` + in + `"`), Output: out}}
	}
	s := New()
	s.Turns = []*Turn{
		{Prompt: "fix the upload", Items: []*Item{step("Read", "old.go", "OLD BODY"), step("Bash", "go test", "FAIL upload_test.go:12")}},
		{Prompt: "now the retry", Items: []*Item{step("Read", "retry.go", "RECENT BODY")}},
		{Prompt: "ship it", Items: []*Item{{Kind: KText, Text: "Done."}}},
	}
	goal, steps := s.PruneSteps()
	if len(steps) != 2 || !strings.Contains(steps[1], "FAIL upload_test.go:12") {
		t.Fatalf("steps %q: only turns before the latest two are judged", steps)
	}
	if !strings.HasPrefix(goal, "User:\nship it\n\nAssistant, latest:\nDone.") || !strings.Contains(goal, "User:\nnow the retry") {
		t.Fatalf("goal %q", goal)
	}
	got := s.Pruned([]bool{false, true})
	for _, w := range []string{"- Read \"old.go\" (output dropped)", "FAIL upload_test.go:12", "RECENT BODY", "## User\nship it"} {
		if !strings.Contains(got, w) {
			t.Fatalf("no %q in\n%s", w, got)
		}
	}
	if strings.Contains(got, "OLD BODY") {
		t.Fatalf("dropped step's output kept:\n%s", got)
	}
}
