package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func TestOptionalFound(t *testing.T) {
	b := func(name string, optional bool, req plugin.Requires) plugin.Bundle {
		return plugin.Bundle{Optional: optional, Manifest: plugin.Manifest{Name: name, Category: "machine", Requires: req}}
	}
	bundles := []plugin.Bundle{
		b("always", false, plugin.Requires{}),
		b("asked", true, plugin.Requires{}),
		b("fresh", true, plugin.Requires{}),
		b("running", true, plugin.Requires{}),
		b("elsewhere", true, plugin.Requires{OS: []string{"plan9"}}),
	}
	got := optionalFound([]string{"asked"}, bundles, map[string]bool{"running": true})
	if len(got.picks) != 1 || got.picks[0].name != "fresh" || got.picks[0].category != "Your machine" {
		t.Fatalf("picks: %+v", got.picks)
	}
	if len(got.alreadyOn) != 1 || got.alreadyOn[0] != "running" {
		t.Fatalf("already on: %v", got.alreadyOn)
	}
}

func TestOptionalSheet(t *testing.T) {
	s := &optionalSheet{first: true, picks: []optPick{
		{name: "cleanup", about: "offers a cleanup agent", category: "Your machine"},
		{name: "remote", about: "keeps rush serve running", category: "Other tools"},
	}}
	out := strings.Join(s.body(&Model{}, 100, 40), "\n")
	for _, want := range []string{"Welcome to rush", "cleanup", "remote", "Your machine", "Other tools", "space"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	m := &Model{}
	m.sheet = s
	s.key(m, key("down"), "down")
	s.key(m, key("space"), "space")
	if s.picks[0].on || !s.picks[1].on {
		t.Fatalf("space ticks the row under the cursor: %+v", s.picks)
	}
}

func TestOptionalSheetKeepsItsHeight(t *testing.T) {
	long := strings.Repeat("a long description that wraps onto more lines ", 6)
	var picks []optPick
	for i, c := range []string{"Your machine", "Your machine", "Other tools", "Other tools", "Other tools", "Agents", "Agents", "Agents"} {
		about := "short"
		if i == 2 {
			about = long
		}
		picks = append(picks, optPick{name: string(rune('a' + i)), about: about, category: c})
	}
	for _, h := range []int{40, 16} {
		s := &optionalSheet{picks: picks}
		want := len(s.body(&Model{}, 100, h))
		for range picks {
			s.key(&Model{}, key("down"), "down")
			if got := len(s.body(&Model{}, 100, h)); got != want {
				t.Fatalf("h=%d: at row %d the sheet is %d lines, was %d", h, s.cur, got, want)
			}
		}
	}
}
