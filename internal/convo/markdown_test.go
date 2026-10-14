package convo

import (
	"strings"
	"testing"
)

func TestMarkdownWraps(t *testing.T) {
	out := Markdown("## Summary\r\n\r\n- one thing\r\n- "+strings.Repeat("word ", 30)+"\r\n", 40)
	if len(out) < 4 {
		t.Fatalf("want the heading, a gap and a wrapped list, got %q", out)
	}
	for _, l := range out {
		if n := len([]rune(stripANSI(l))); n > 40 {
			t.Errorf("line %d wide: %q", n, l)
		}
	}
}
