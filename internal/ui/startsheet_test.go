package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	_ "github.com/0xdeafcafe/rush/internal/adapters/codex"
	_ "github.com/0xdeafcafe/rush/internal/adapters/cross"  // providers in Pi, by key
	_ "github.com/0xdeafcafe/rush/internal/adapters/ollama" // Ollama in several harnesses
	"github.com/0xdeafcafe/rush/internal/agent"
)

// What the sheet picks is what the next session starts as, and the box
// says so; without it, the folder's agent as Settings has it.
func TestStartOver(t *testing.T) {
	m, _ := benchModel(120, 40)
	dir := m.startDir()
	if got := m.nextStart(dir); got.kind != m.startKindIn(dir) {
		t.Fatalf("with nothing picked, the folder's agent: %+v", got)
	}
	m.startOver = &startOver{kind: "codex", model: "gpt-5.5", effort: "high"}
	if got := m.nextStart(dir); got.kind != "codex" || got.model != "gpt-5.5" || got.effort != "high" {
		t.Fatalf("picked: %+v", got)
	}
	if w := ansi.Strip(m.startWith(dir, true)); !strings.HasPrefix(w, "OpenAI (Codex)") || !strings.Contains(w, "high effort") {
		t.Errorf("the box should say what it starts as: %q", w)
	}
}

// Only valid combinations: a subscription in its own harness alone, an
// API key in any harness that speaks its API once the key is kept, and a
// local provider in all of its harnesses.
func TestRoutes(t *testing.T) {
	kinds := []string{"claude", "codex", "pi", "anthropic-pi", "openai-pi", "ollama", "ollama-pi"}
	say := func(rs []route) string {
		var out []string
		for _, r := range rs {
			out = append(out, r.id+"="+strings.Join(r.kinds, "+"))
		}
		return strings.Join(out, " ")
	}
	none := func(string) bool { return false }
	if got, want := say(routes(kinds, none)), "claude=claude codex=codex pi=pi ollama=ollama+ollama-pi"; got != want {
		t.Errorf("no keys:\n got %s\nwant %s", got, want)
	}
	claudeKey := func(p string) bool { return p == "claude" }
	if got, want := say(routes(kinds, claudeKey)), "claude=claude claude-key=claude+anthropic-pi codex=codex pi=pi ollama=ollama+ollama-pi"; got != want {
		t.Errorf("Anthropic's key:\n got %s\nwant %s", got, want)
	}
}

// Provider and harness are picked apart: a provider keeps the harness
// when it runs there (Ollama, then OpenAI's plan, in Pi), and a harness
// keeps the provider when it runs it.
func TestStartHarness(t *testing.T) {
	for _, k := range []agent.Kind{"ollama", "pi", "codex"} {
		if !agent.Runs(k) {
			t.Skipf("%s isn't installed here", k)
		}
	}
	m, _ := benchModel(120, 40)
	s := &startSheet{o: m.startDefaults("ollama"), row: 2}
	s.set(m, "pi")
	if s.o.kind != "ollama-pi" {
		t.Errorf("Ollama in Pi: %+v", s.o)
	}
	s.row = 1
	s.set(m, "codex")
	if s.o.kind != "openai-plan-pi" || s.o.billing != "" {
		t.Errorf("OpenAI's plan, still in Pi: %+v", s.o)
	}
	s.set(m, "ollama")
	s.row = 2
	s.set(m, "codex")
	if s.o.kind != "ollama-codex" {
		t.Errorf("Ollama, now in Codex: %+v", s.o)
	}
	// Through a harness that can't run Ollama, and back: Ollama is kept.
	s.set(m, "copilot")
	if s.set(m, "pi"); s.o.kind != "ollama-pi" {
		t.Errorf("Ollama, back in Pi: %+v", s.o)
	}
	s.set(m, "codex")
	text := ansi.Strip(strings.Join(s.body(m, 110, 40), "\n"))
	for _, want := range []string{"PROVIDER", "HARNESS", "MODEL", "same as  #new codex@ollama"} {
		if !strings.Contains(text, want) {
			t.Errorf("the sheet doesn't show %q:\n%s", want, text)
		}
	}
}

// One name for a setup to type: <provider>-<harness>:<account>, or :key
// for its provider's API key. The names from before still work.
func TestSetupName(t *testing.T) {
	for _, c := range []struct {
		k                agent.Kind
		billing, account string
		want, old        string
	}{
		{"claude", "", "Alex Work", "anthropic-claudecode:alex-work", "claudecode:alex-work"},
		{"claude", "key", "", "anthropic-claudecode:key", "claudecode:key"},
		{"codex", "", "alex", "openai-codex:alex", "codex:alex"},
		{"codex", "", "", "openai-codex", "codex"},
		{"ollama", "", "", "ollama-claudecode", "claudecode:ollama"},
		{"ollama-pi", "", "", "ollama-pi", "pi:ollama"},
		{"anthropic-pi", "key", "", "anthropic-pi:key", "pi:claude-key"},
	} {
		if got := setupName(c.k, c.billing, c.account); got != c.want {
			t.Errorf("%s %q %q: got %s, want %s", c.k, c.billing, c.account, got, c.want)
		}
		if got := oldSetupName(c.k, c.billing, c.account); got != c.old {
			t.Errorf("%s %q %q: old name %s, want %s", c.k, c.billing, c.account, got, c.old)
		}
	}
}

// #use takes a setup's name or another name for it, then an effort of
// its agent's if you like.
func TestPickSetup(t *testing.T) {
	ss := []setup{
		{"codex", []string{"codex"}, startOver{kind: "codex"}},
		{"codex:alex", []string{"codex:alex"}, startOver{kind: "codex", account: "alex"}},
		{"claudecode:alex", []string{"claude:alex"}, startOver{kind: "claude", account: "alex"}},
	}
	efforts := func(string) []agent.Choice { return []agent.Choice{{ID: "low"}, {ID: "high"}} }
	for arg, want := range map[string]startOver{
		"codex:alex":      {kind: "codex", account: "alex"},
		"Claude:Alex":     {kind: "claude", account: "alex"},
		"codex:alex:high": {kind: "codex", account: "alex", effort: "high"},
		"codex:low":       {kind: "codex", effort: "low"},
	} {
		if got, err := pickSetup(ss, arg, efforts); err != nil || got != want {
			t.Errorf("%s: got %+v, %v; want %+v", arg, got, err, want)
		}
	}
	for _, arg := range []string{"nope", "codex:alex:ultra", "pi", ":high"} {
		if _, err := pickSetup(ss, arg, efforts); err == nil {
			t.Errorf("%s should say there's no such setup or effort", arg)
		}
	}
}

// A click takes what it lands on: a value in a column is picked, where
// before it went to the generic click, whose ↑↓ change a column's value
// rather than move to the row clicked, so it did nothing.
func TestStartSheetClicks(t *testing.T) {
	m, _ := benchModel(120, 40)
	m.openStartSheet()
	s, ok := m.sheet.(*startSheet)
	if !ok {
		t.Fatal("no start sheet")
	}
	draw := func() []string { return s.body(m, 100, 34) }
	lines := draw()
	picks := 0
	for _, h := range s.hits {
		if !h.pick {
			continue
		}
		picks++
		cell := ansi.Strip(ansi.Cut(lines[h.y], h.x0, h.x1))
		word := ansi.Strip(s.word(m, h.row, h.v))
		if !strings.Contains(cell, word[:min(len(word), 6)]) {
			t.Errorf("row %d %q is drawn as %q", h.row, h.v, cell)
		}
	}
	if picks == 0 {
		t.Fatal("no values to click")
	}
	// Another model, by a click on it.
	clicked := false
	for _, h := range s.hits {
		if h.pick && h.row == 4 && h.v != s.o.model {
			s.mouse(m, mousePress, h.x0+1, h.y)
			if s.o.model != h.v || s.row != 4 {
				t.Fatalf("clicked model %q: now %q, row %d", h.v, s.o.model, s.row)
			}
			clicked = true
			break
		}
	}
	if !clicked {
		t.Fatal("no other model to click")
	}
	// Effort: a click puts it in focus, another moves it on.
	draw()
	for _, h := range s.hits {
		if h.row == 5 && !h.pick {
			before := s.o.effort
			s.mouse(m, mousePress, h.x0+1, h.y)
			if s.row != 5 || s.o.effort != before {
				t.Fatalf("first click focuses effort: row %d, %q", s.row, s.o.effort)
			}
			draw()
			s.mouse(m, mousePress, h.x0+1, h.y)
			if len(s.choices(m, 5)) > 1 && s.o.effort == before {
				t.Fatal("a second click moves effort on")
			}
			return
		}
	}
	t.Fatal("no effort to click")
}
