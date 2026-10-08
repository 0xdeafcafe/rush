package ui

import (
	"os"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
)

// A second line that runs over keeps what tells you most: the file a long
// path ends on, or whole words, then one ellipsis; never a cut mid-word.
func TestTidyFit(t *testing.T) {
	home, _ := os.UserHomeDir()
	path := "Write " + home + "/.config/agtop/claude/a4bb3b77-4f15/projects/rush/memory/convo-note.md"
	got := strings.TrimRight(tidyFit(path, 50), " ")
	if !strings.HasSuffix(got, "memory/convo-note.md") || !strings.HasPrefix(got, "Write ~/.config/") || cellw.String(got) > 50 {
		t.Errorf("path: %q", got)
	}
	got = strings.TrimRight(tidyFit("waiting on you to approve the migration before it runs", 30), " ")
	if got != "waiting on you to approve the…" {
		t.Errorf("words: %q", got)
	}
	if got := tidyFit("short", 10); got != "short     " {
		t.Errorf("fits: %q", got)
	}
}
