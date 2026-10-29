package ui

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A link to a pull request in any repository opens in rush once GitHub
// has said of it: a click opens its sheet, and its menu offers opening it
// there and merging it before the browser.
func TestPRLinkOpensInRush(t *testing.T) {
	was := ghpr.RunGH
	t.Cleanup(func() { ghpr.RunGH = was; convo.LookLinks(false) })
	ghpr.RunGH = func(_ context.Context, _ string, args ...string) ([]byte, error) {
		if len(args) > 1 && args[0] == "pr" && args[1] == "view" && strings.Contains(strings.Join(args, " "), "comments") && !strings.Contains(strings.Join(args, " "), "title") {
			return []byte(`{"comments":[]}`), nil
		}
		return []byte(`{"number":33,"title":"Emoji fix","url":"https://github.com/acme/linkopens/pull/33","state":"OPEN","headRefName":"emoji","baseRefName":"main"}`), nil
	}
	convo.LookLinks(true)
	u := "https://github.com/acme/linkopens/pull/33"
	for end := time.Now().Add(5 * time.Second); ; time.Sleep(time.Millisecond) {
		if _, _, _, ok := convo.LinkInfo(u); ok {
			break
		}
		if time.Now().After(end) {
			t.Fatal("GitHub never answered")
		}
	}

	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: time.Now()}, w: 160, h: 50}
	if _, ok := m.openKnownLink(u); !ok {
		t.Fatal("a click on the link didn't open it in rush")
	}
	s, ok := m.sheet.(*prSheet)
	if !ok || s.url != u || s.num != 33 || s.pr(m).Title != "Emoji fix" {
		t.Fatalf("sheet = %#v", m.sheet)
	}
	if k := s.keys(m, s.pr(m), 200); !strings.Contains(k, "merge") {
		t.Errorf("a linked open pull request can't be merged: %q", k)
	}

	m.sheet = nil
	if !m.openLinkMenu(u) {
		t.Fatal("no menu")
	}
	labels := make([]string, 0, len(m.picker.acts))
	for _, a := range m.picker.acts {
		labels = append(labels, a.label)
	}
	if got := strings.Join(labels, ", "); got != "Open in rush, Merge…, Open in browser, Copy link" {
		t.Errorf("menu = %s", got)
	}

	// Any other link is left to the terminal.
	if _, ok := m.openKnownLink("https://example.com/x"); ok {
		t.Error("opened a link no kind knows")
	}
}
