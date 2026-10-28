package convo

import (
	"strings"

	"testing"

	"github.com/charmbracelet/x/ansi"
)

func TestMarkdownImagesAndLinks(t *testing.T) {
	got := inline("see ![menu rows, dark](/tmp/mb-dark.png) and [docs](https://x.dev/a) or https://y.dev", cText)
	plain := ansi.Strip(got)
	if want := "see ▣ menu rows, dark mb-dark.png and docs or https://y.dev"; plain != want {
		t.Fatalf("got %q, want %q", plain, want)
	}
	for _, w := range []string{"\x1b]8;;file:///tmp/mb-dark.png\x1b\\", "\x1b]8;;https://x.dev/a\x1b\\", "\x1b]8;;https://y.dev\x1b\\"} {
		if !strings.Contains(got, w) {
			t.Errorf("missing %q in %q", w, got)
		}
	}
	if p := ansi.Strip(inline("![](shot.png) [a] (b) arr[i](x y)", cText)); p != "▣ shot.png [a] (b) arr[i](x y)" {
		t.Errorf("got %q", p)
	}
}

// A link whose words are `code` links them, not the [ of the colour the
// code span put in before it.
func TestCodeInLinkLabel(t *testing.T) {
	for in, want := range map[string]string{
		"under [`$TMPDIR`](/tmp/x). Mea":  "under $TMPDIR. Mea",
		"`a` [b `c` d](https://x.y) e":    "a b c d e",
		"a `x` b [docs](https://x.dev) c": "a x b docs c",
	} {
		if got := ansi.Strip(inline(in, cSub)); got != want {
			t.Errorf("inline(%q) = %q, want %q", in, got, want)
		}
	}
}
