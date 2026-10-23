package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/bundled/yap"
	"github.com/0xdeafcafe/rush/internal/convo"
)

func dictating(frame int) box {
	return box{w: 72, focused: true, topL: "to claude", text: []rune("check the"), cursor: 9, anchor: -1,
		lead: paint(cOrange, "❯ "), maxRows: 6,
		live: &liveText{settled: "migration replays every event", guessing: "before the upgrade", level: .7, frame: frame, hint: "listening · let go to paste"}}
}

func TestYapDictationShowsAtTheCursor(t *testing.T) {
	lines := dictating(0).lines()
	got := ansi.Strip(strings.Join(lines, "\n"))
	if !strings.Contains(got, "check the migration replays every event before the upgrade") {
		t.Fatalf("the dictation isn't after the typed text:\n%s", got)
	}
	if !strings.Contains(got, "let go to paste") {
		t.Fatalf("no hint in the bottom edge:\n%s", got)
	}
	if strings.Contains(strings.Join(lines, ""), "\x1b[7m") {
		t.Fatal("the cursor is reverse video, not the dictation's")
	}
}

func TestYapEdgeLightMoves(t *testing.T) {
	a, b := dictating(0).lines(), dictating(12).lines()
	if a[0] == b[0] && a[len(a)-1] == b[len(b)-1] {
		t.Fatal("the edge looks the same twelve frames apart")
	}
	if ansi.Strip(a[0]) != ansi.Strip(b[0]) {
		t.Fatal("the light moved the edge's text")
	}
}

func TestYapGuessedWordsShimmer(t *testing.T) {
	// Somewhere in a crossing the guessed words are lit, and elsewhere not.
	seen := map[string]bool{}
	for f := range 32 {
		seen[dictating(f).lines()[1]] = true
	}
	if len(seen) < 8 {
		t.Fatalf("the text barely changes across a crossing: %d looks", len(seen))
	}
}

func TestYapBoxWithoutDictationIsUnchanged(t *testing.T) {
	b := dictating(0)
	b.live = nil
	plain := b.lines()
	b.live, b.idle = &liveText{settled: "hi"}, true
	if strings.Join(b.lines(), "\n") != strings.Join(plain, "\n") {
		t.Fatal("an idle box drew the dictation")
	}
}

func TestYapPasteIsTheDictation(t *testing.T) {
	m := &Model{}
	m.yap.s = yap.State{Phase: "done", Settled: "ship it"}
	m.yap.final, m.yap.finalAt = "ship it", time.Now()
	if !m.yapPaste("ship it ") {
		t.Fatal("yap's own paste wasn't taken as the dictation")
	}
	if m.yap.s.Active() {
		t.Fatal("the dictation still shows after its paste")
	}
	if m.yapPaste("ship it ") {
		t.Fatal("a second paste of the same words was taken as dictated")
	}
	m.yap.final, m.yap.finalAt = "ship it", time.Now().Add(-yapPasteWait-time.Second)
	if m.yapPaste("ship it") {
		t.Fatal("a paste long after the dictation was taken as it")
	}
}

func TestYapShownOnlyForThisTerminal(t *testing.T) {
	old := yapApp
	t.Cleanup(func() { yapApp = old })
	yapApp = "com.mitchellh.ghostty"
	m := &Model{}
	m.yap.s = yap.State{Phase: "listening", App: "com.mitchellh.ghostty"}
	if !m.yapShown() {
		t.Fatal("not shown for this terminal")
	}
	m.yap.s.App = "com.apple.Notes"
	if m.yapShown() {
		t.Fatal("shown while dictating into Notes")
	}
	m.yap.s.App, m.blurred = "com.mitchellh.ghostty", true
	if m.yapShown() {
		t.Fatal("shown while the terminal hasn't the keys")
	}
}

// YAP_FRAMES=dir writes frames of the look as ANSI, to draw with
// go -C tools/shots run . -ansi dir/yap-N.ansi
func TestYapFrames(t *testing.T) {
	dir := os.Getenv("YAP_FRAMES")
	if dir == "" {
		t.Skip("YAP_FRAMES not set")
	}
	var sheet []string
	for _, f := range []int{0, 6, 12, 18, 24} {
		sheet = append(sheet, dictating(f).lines()...)
		sheet = append(sheet, "")
	}
	if err := os.WriteFile(dir+"/yap-sheet.ansi", []byte(strings.Join(sheet, "\n")), 0o644); err != nil {
		t.Fatal(err)
	}
}

// A dictation shows in the Prompt as it's heard, and its paste goes in as
// typed even when it's long enough to be folded into a chip.
func TestYapDictationIntoThePrompt(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.sel, m.preview, m.paneFocus = "", false, false
	m.Update(yapMsg{yap.State{Phase: "listening", Settled: "the deprecation", Guessing: "of the old servic"}})
	if got := ansi.Strip(strings.Join(m.promptLines(200), "\n")); !strings.Contains(got, "the deprecation of the old servic") {
		t.Fatalf("the Prompt doesn't show the dictation:\n%s", got)
	}
	said := strings.Repeat("replay every event after the migration ", 25) // past a chip's 800
	m.Update(yapMsg{yap.State{Phase: "done", Settled: said}})
	m.Update(tea.PasteMsg{Content: said + " "})
	if got := string(m.input); !strings.Contains(got, "replay every event") || convo.HasPasteChip(got) {
		t.Fatalf("the dictation's paste didn't go in as typed: %.80q", got)
	}
	if got := ansi.Strip(strings.Join(m.promptLines(200), "\n")); strings.Count(got, "replay every event") > 25 {
		t.Fatal("the dictation still shows beside its paste")
	}
}
