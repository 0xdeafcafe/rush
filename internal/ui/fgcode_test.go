package ui

import (
	"testing"

	"github.com/0xdeafcafe/photon/theme"
	"github.com/0xdeafcafe/rush/internal/agent"
)

func TestFgCodeMatchesFG(t *testing.T) {
	for _, c := range []theme.RGB{{}, {R: 255, G: 255, B: 255}, {R: 7, G: 80, B: 199}} {
		if got, want := fgCode(int(c.R), int(c.G), int(c.B)), c.FG(); got != want {
			t.Fatalf("fgCode(%v) = %q, want %q", c, got, want)
		}
	}
}

func TestIsProviderMatchesProviders(t *testing.T) {
	ps := agent.Providers()
	if len(ps) == 0 {
		t.Skip("no agents registered")
	}
	for _, p := range ps {
		if !agent.IsProvider(p) {
			t.Fatalf("IsProvider(%q) = false", p)
		}
	}
	if agent.IsProvider("no-such-provider") {
		t.Fatal("IsProvider of an unknown name")
	}
}
