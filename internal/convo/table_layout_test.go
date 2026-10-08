package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
)

func TestCompactTableKeepsAllColumnsOnTinyPane(t *testing.T) {
	s := New()
	markdown := "| A | B | C | D | E | F |\n|---|---|---|---|---|---|\n| 1 | 2 | 3 | 4 | 5 | 6 |"
	for _, width := range []int{14, 32, 80} {
		lines := s.Answer(markdown, width)
		out := plain(lines)
		for _, value := range []string{"A", "B", "C", "D", "E", "F", "1", "2", "3", "4", "5", "6"} {
			if !strings.Contains(out, value) {
				t.Fatalf("width %d lost %s: %s", width, value, out)
			}
		}
		for _, line := range lines {
			if cellw.String(line.Text) > width {
				t.Fatalf("width %d overflow", width)
			}
		}
		if strings.ContainsAny(out, "┌┐└┘│") {
			t.Fatalf("table has a frame: %s", out)
		}
	}
}
