package convo

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
)

func TestAgentTagsLink(t *testing.T) {
	SetHandles(map[string]Handle{"custard-bean": {Name: "custard-bean", Tint: cBlue}})
	defer SetHandles(nil)
	link := "\x1b]8;;rush:agent/custard-bean\x1b\\"
	for _, s := range []string{"ask @custard-bean, then you", "ask @Custard-Bean. then you"} {
		ask := styledAsk(s, cWhite)
		if ansi.Strip(ask) != s || strings.Count(ask, link) != 1 {
			t.Fatalf("your words should link the agent once, as written: %q", ask)
		}
		said := inline(s, cText)
		if ansi.Strip(said) != s || strings.Count(said, link) != 1 {
			t.Fatalf("an answer should link the agent once: %q", said)
		}
	}
	for _, s := range []string{"mail me@custard-bean", "see @custard-bean/x.go", "ask @nobody"} {
		if strings.Contains(inline(s, cText), "rush:agent/") {
			t.Fatalf("no agent tagged in %q", s)
		}
	}
}
