package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

// The first turn opens on a card: when the session began and who began
// it, what it runs as, and its commands, listed once opened.
func TestOpeningCard(t *testing.T) {
	s := session()
	s.Apply(host.InfoEvent{Info: host.Info{StartedAt: at(0), Model: "claude-opus-5-5[1m]", Effort: "high", Meta: map[string]string{"spawnedBy": "p1"}}}, at(0))
	s.Commands = []event.Command{{Name: "review"}, {Name: "ship"}}
	out := plain(s.Render(Options{Width: 110, Now: at(40), History: HistoryOpen, Parent: "fix login"}))
	for _, w := range []string{"◆ session · started", "by fix login, as its subagent", "high", "2 commands and skills ▸ list"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in:\n%s", w, out)
		}
	}
	if strings.Count(out, "◆ session") != 1 {
		t.Errorf("card drawn other than once:\n%s", out)
	}
	if strings.Index(out, "◆ session") > strings.Index(out, "▍ the ux") {
		t.Errorf("card not above the first prompt:\n%s", out)
	}
	out = plain(s.Render(Options{Width: 110, Now: at(40), History: HistoryOpen, Parent: "fix login", Open: map[string]bool{"t1:opening:skills": true}}))
	if !strings.Contains(out, "/review  /ship") {
		t.Errorf("opened, the commands aren't listed:\n%s", out)
	}
}

// A hosted session's card says what was added to its agent's prompt, and
// opens on the brief it was started with.
func TestOpeningBrief(t *testing.T) {
	s := session()
	s.Apply(host.InfoEvent{Info: host.Info{StartedAt: at(0), Model: "sonnet"}}, at(0))
	o := Options{Width: 110, Now: at(40), History: HistoryOpen, Hosted: true}
	if out := plain(s.Render(o)); !strings.Contains(out, "prompt: rush's own\n") {
		t.Errorf("no prompt row:\n%s", out)
	}
	o.Brief = "Work on the login bug.\nReport back."
	if out := plain(s.Render(o)); !strings.Contains(out, "and its brief · 2 lines ▸ show") || strings.Contains(out, "Report back.") {
		t.Errorf("brief not folded:\n%s", out)
	}
	o.Open = map[string]bool{"t1:opening:brief": true}
	if out := plain(s.Render(o)); !strings.Contains(out, "Report back.") {
		t.Errorf("opened, the brief isn't shown:\n%s", out)
	}
}
