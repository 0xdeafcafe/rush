package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// A session stopped signed out waits on a card, even for "Not logged in",
// which says no 401; carrying on from it takes the card away at once,
// and a session busy again has none.
func TestSignedOutCard(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, c := draftModel()
	c.client = &host.Client{}
	c.sess.Info.State = "idle"
	c.sess.Info.Error = host.AuthStopped + "Not logged in · Please run /login"
	if cardKind(c) != "signin" {
		t.Fatalf("card is %q", cardKind(c))
	}
	text := ansi.Strip(strings.Join(m.cardRows(m.agentByKey(c.key), c, 100, 30), "\n"))
	for _, want := range []string{"Signed out", "Not logged in", "Carry on as", "in use"} {
		if !strings.Contains(text, want) {
			t.Fatalf("missing %q:\n%s", want, text)
		}
	}
	c.cardFocus = true
	if chs := m.signinChoices(c); len(chs) > 0 && !chs[0].here {
		t.Fatalf("the account it's on should come first: %+v", chs[0])
	}
	if cmd, used := m.cardKey(c, "enter", true); !used || cmd == nil {
		t.Fatal("enter should carry it on")
	}
	if cardKind(c) != "" {
		t.Fatal("the card should go once a continue is sent")
	}
	c.sess.Info.Error = host.AuthStopped + "401 again"
	c.sess.Info.State = "working"
	if cardKind(c) != "" {
		t.Fatal("a session at work has no signed-out card")
	}
}
