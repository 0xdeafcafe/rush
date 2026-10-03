package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// press types s as keys: "ctrl+left", or plain text wrapped in quotes.
func press(buf string, pos int, keys ...string) (string, int) {
	b := []rune(buf)
	for _, k := range keys {
		msg := tea.KeyPressMsg{}
		s := k
		if len(k) > 1 && k[0] == '"' {
			msg.Text, s = k[1:len(k)-1], k[1:len(k)-1]
		}
		b, pos, _ = edit(b, pos, msg, s)
	}
	return string(b), pos
}

func TestEdit(t *testing.T) {
	cases := []struct {
		name    string
		buf     string
		pos     int
		keys    []string
		want    string
		wantPos int
	}{
		{"type in the middle", "helo", 3, []string{`"l"`}, "hello", 4},
		{"word left", "run the tests", 13, []string{"ctrl+left"}, "run the tests", 8},
		{"word left twice", "run the tests", 13, []string{"alt+left", "alt+left"}, "run the tests", 4},
		{"word right", "run the tests", 0, []string{"ctrl+right"}, "run the tests", 3},
		{"delete word back", "run the tests", 13, []string{"alt+backspace"}, "run the ", 8},
		{"delete word back over spaces", "run the   ", 10, []string{"ctrl+w"}, "run ", 4},
		{"delete word forward", "run the tests", 4, []string{"alt+delete"}, "run  tests", 4},
		{"cmd+backspace clears to line start", "one\ntwo three", 13, []string{"super+backspace"}, "one\n", 4},
		{"ctrl+u at a line start joins upward", "one\ntwo", 4, []string{"ctrl+u"}, "two", 0},
		{"ctrl+k clears to the end", "run the tests", 4, []string{"ctrl+k"}, "run ", 4},
		{"home and end", "abc", 1, []string{"home", `"x"`, "end", `"y"`}, "xabcy", 5},
		{"new line", "ab", 1, []string{"shift+enter"}, "a\nb", 2},
		{"backspace at start is a no-op", "ab", 0, []string{"backspace"}, "ab", 0},
	}
	for _, c := range cases {
		got, pos := press(c.buf, c.pos, c.keys...)
		if got != c.want || pos != c.wantPos {
			t.Errorf("%s: got %q@%d, want %q@%d", c.name, got, pos, c.want, c.wantPos)
		}
	}
}

func TestCursorSurvivesReplacement(t *testing.T) {
	m := &Model{input: []rune("hello")}
	m.setCursor(2)
	if m.cursorPos() != 2 {
		t.Fatalf("pos %d", m.cursorPos())
	}
	m.input, m.back = []rune("a new name"), 0 // what rename does
	if m.cursorPos() != len(m.input) {
		t.Fatalf("cursor should be at the end after a replacement, got %d", m.cursorPos())
	}
}

func TestImagePaths(t *testing.T) {
	dir := t.TempDir()
	a := dir + "/Screen Shot 1.png"
	b := dir + "/b.jpg"
	for _, p := range []string{a, b} {
		if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	escaped := strings.ReplaceAll(a, " ", `\ `)
	cases := []struct {
		paste string
		want  int
	}{
		{escaped, 1},
		{"'" + a + "'", 1},
		{escaped + " " + b, 2},
		{escaped + "\n" + b + "\n", 2},
		{"look at " + b, 0},       // ordinary text stays text
		{dir + "/missing.png", 0}, // must exist
		{dir + "/notes.txt", 0},   // must be an image
	}
	for _, c := range cases {
		if got := imagePaths(c.paste, statLook); len(got) != c.want {
			t.Errorf("%q: got %v", c.paste, got)
		}
	}
}

func TestSelection(t *testing.T) {
	type st struct {
		buf         string
		pos, anchor int
	}
	run := func(in st, keys ...string) (st, string) {
		b, p, a := []rune(in.buf), in.pos, in.anchor
		var copied string
		for _, k := range keys {
			msg := tea.KeyPressMsg{}
			s := k
			if len(k) > 1 && k[0] == '"' {
				msg.Text, s = k[1:len(k)-1], k[1:len(k)-1]
			}
			var c string
			b, p, a, c, _ = editSel(b, p, a, msg, s)
			if c != "" {
				copied = c
			}
		}
		return st{string(b), p, a}, copied
	}
	got, _ := run(st{"hello world", 11, -1}, "shift+left", "shift+left", "shift+left", "shift+left", "shift+left")
	if got.pos != 6 || got.anchor != 11 {
		t.Fatalf("select back: %+v", got)
	}
	got, _ = run(got, `"there"`)
	if got.buf != "hello there" || got.anchor != -1 {
		t.Fatalf("typing replaces the selection: %+v", got)
	}
	got, copied := run(st{"run the tests", 0, -1}, "ctrl+shift+right", "ctrl+c")
	if copied != "run" || got.buf != "run the tests" {
		t.Fatalf("copy: %q %+v", copied, got)
	}
	got, _ = run(st{"run the tests", 13, -1}, "shift+home", "backspace")
	if got.buf != "" {
		t.Fatalf("delete selection: %+v", got)
	}
	got, _ = run(st{"abc", 1, -1}, "shift+right", "left")
	if got.anchor != -1 || got.pos != 1 {
		t.Fatalf("moving clears the selection: %+v", got)
	}
	got, copied = run(st{"run the tests", 4, -1}, "super+a", "ctrl+c")
	if copied != "run the tests" || got.buf != "run the tests" {
		t.Fatalf("select all: %q %+v", copied, got)
	}
	got, copied = run(st{"run the tests", 4, -1}, "alt+c")
	if copied != "run the tests" || got.pos != 4 || got.anchor != -1 {
		t.Fatalf("copy all: %q %+v", copied, got)
	}
}

func TestBoxDragStaysInside(t *testing.T) {
	b := box{w: 24, text: []rune("one two three four five six"), lead: "❯ ", maxRows: 6}
	if p := b.near(-3, 50); p != 0 {
		t.Fatalf("above the box is its start, got %d", p)
	}
	if p := b.near(9, 0); p != len(b.text) {
		t.Fatalf("below the box is its end, got %d", p)
	}
	if p := b.near(0, 200); p != len("one two three ") {
		t.Fatalf("past the right edge is the row's end, got %d", p)
	}
	if p := b.near(1, -5); p != len("one two three ") {
		t.Fatalf("past the left edge is the row's start, got %d", p)
	}
}

func TestBoxClickMapsToText(t *testing.T) {
	b := box{w: 24, text: []rune("one two three four five six"), lead: "❯ ", maxRows: 6}
	// Inner width 20, lead 2: rows of 18 cells, word wrapped.
	segs := wrapSegs(b.text, b.w-4-b.leadW())
	if len(segs) != 2 || string(b.text[segs[0].from:segs[0].to]) != "one two three " {
		t.Fatalf("wrap: %+v", segs)
	}
	// Col 0-1 is "│ ", then the lead "❯ ": col 4 is the first character.
	if p := b.at(0, 4); p != 0 {
		t.Errorf("first char: %d", p)
	}
	if p := b.at(0, 8); p != 4 {
		t.Errorf("start of 'two': %d", p)
	}
	if p := b.at(1, 4); p != segs[1].from {
		t.Errorf("second row start: %d", p)
	}
	if p := b.at(1, 99); p != len(b.text) {
		t.Errorf("past the end: %d", p)
	}
}

func TestCleanPaste(t *testing.T) {
	in := "goroutine 1:\r\n\tmain.go:78 +0x5ec\n\x1b[1mab\tc\rd"
	want := "goroutine 1:\n    main.go:78 +0x5ec\n[1mab   c\nd"
	if got := cleanPaste(in); got != want {
		t.Fatalf("cleanPaste = %q, want %q", got, want)
	}
}

func TestImagePathsOddNames(t *testing.T) {
	dir := t.TempDir()
	name := filepath.Join(dir, "Screenshot 2026-09-23 at 23.38.19 PM.png")
	if err := os.WriteFile(name, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	esc := strings.ReplaceAll(name, " ", `\ `)
	if got := imagePaths(esc, statLook); len(got) != 1 || got[0] != name {
		t.Fatalf("escaped path with U+202F: %v", got)
	}
	u := "file://" + strings.ReplaceAll(strings.ReplaceAll(name, " ", "%20"), " ", "%E2%80%AF")
	if got := imagePaths(u, statLook); len(got) != 1 || got[0] != name {
		t.Fatalf("file URL: %v", got)
	}
}

func TestExtractImages(t *testing.T) {
	dir := t.TempDir()
	a := filepath.Join(dir, "Screenshot 2026-09-24 at 00.03.09.png")
	_ = os.WriteFile(a, []byte("x"), 0o644)
	in := strings.ReplaceAll(a, " ", `\ `) + " still no image detection?\nand /var/nope.png stays"
	rest, imgs := extractImages(in, statLook)
	if len(imgs) != 1 || imgs[0] != a {
		t.Fatalf("imgs = %v", imgs)
	}
	if rest != "still no image detection?\nand /var/nope.png stays" {
		t.Fatalf("rest = %q", rest)
	}
	if r, imgs := extractImages("no images here", statLook); imgs != nil || r != "no images here" {
		t.Fatal("plain text changed")
	}
}

func TestPasteChips(t *testing.T) {
	var p pastes
	long := "a\nb\nc\nd\ne"
	chip := p.add(long)
	if chip != "[pasted text #1 · 5 lines: a b c d e]" {
		t.Fatalf("chip = %q", chip)
	}
	draft := []rune("look at " + chip + " please")
	if got := p.expand(string(draft), false); got != "look at "+long+" please" {
		t.Fatalf("expand = %q", got)
	}
	end := len([]rune("look at " + chip))
	bs := tea.KeyPressMsg{Code: tea.KeyBackspace}
	buf, pos, _, _, _ := editChips(draft, end, -1, bs, "backspace")
	if string(buf) != "look at  please" || pos != len("look at ") {
		t.Fatalf("backspace = %q %d", string(buf), pos)
	}
	if p.lastIn(draft) != 1 {
		t.Fatal("lastIn")
	}
	got := applyEdit(&p, draft, 1, "x\ny")
	if string(got) != "look at [pasted text #1 · 2 lines: x y] please" || p.text[1] != "x\ny" {
		t.Fatalf("applyEdit = %q", string(got))
	}
	if !isLongPaste("one\ntwo") || isLongPaste("one line\n") || !isLongPaste(long) {
		t.Fatal("isLongPaste")
	}
}

func TestTypedImages(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "shot one.png")
	_ = os.WriteFile(p, []byte("x"), 0o644)
	if got := shortImages("see [image: " + p + "] ok"); got != "see ▣ shot one.png ok" {
		t.Fatalf("shortImages = %q", got)
	}
	// Typed into the new-session box, the path becomes its marker at the space.
	m, _ := benchModel(120, 40)
	m.paneFocus = false
	for _, r := range "look " + strings.ReplaceAll(p, " ", `\ `) {
		m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
	}
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeySpace, Text: " "})
	m.drain(cmd) // whether it's a file is read off the UI goroutine
	if string(m.input) != "look [Image #1] " || m.imgs.Path[1] != p {
		t.Fatalf("typed path: input %q images %v", string(m.input), m.imgs.Path)
	}
	// Sent to Claude Code, the marker is the file's path.
	if got := m.imgs.paths("look [Image #1] [Image #9]"); got != "look [image: "+p+"] [Image #9]" {
		t.Fatalf("paths = %q", got)
	}
}

// A paste of more than one line folds into a chip in either box, however
// its lines arrive: LF, the CR Terminal.app pastes, or text in one key
// press from a terminal that didn't bracket the paste.
func TestLongPasteFolds(t *testing.T) {
	text := "╭─ rush ─╮\n│ ✓ loaded │\n╰──────────╯"
	for _, msg := range []tea.Msg{
		tea.PasteMsg{Content: text},
		tea.PasteMsg{Content: strings.ReplaceAll(text, "\n", "\r")},
		tea.KeyPressMsg{Code: tea.KeyExtended, Text: text},
	} {
		for _, pane := range []bool{false, true} {
			m, _ := benchModel(120, 40)
			m.paneFocus = pane
			m.host.input = nil
			m.fastPending = true // the live turn's pulse would batch its tick with the paste's command
			// Whether it names files is asked of the disk first, and the
			// paste comes back.
			_, cmd := m.Update(msg)
			for cmd != nil {
				back := cmd()
				if a, ok := back.(applyMsg); ok {
					back = a.applyTo(m)()
				}
				_, cmd = m.Update(back)
			}
			box, ps := m.input, &m.pastes
			if pane {
				box, ps = m.host.input, &m.host.pastes
			}
			want := "[pasted text #1 · 3 lines: ╭─ rush … ───────╯]"
			if string(box) != want || ps.expand(string(box), false) != text {
				t.Errorf("%T in pane=%v: box %q", msg, pane, string(box))
			}
		}
	}
}

func TestPasteTaggedRoundTrip(t *testing.T) {
	var p pastes
	long := "a\nb\nc\nd"
	draft := "look at " + p.add(long) + " please"
	sent := p.expand(draft, true)
	if !strings.Contains(sent, "<pasted_content id=") || !strings.Contains(sent, "\na\nb\nc\nd\n</pasted_content id=") {
		t.Fatalf("tagged = %q", sent)
	}
	var q pastes
	back := string(q.unfold(sent))
	if back != "look at [pasted text #1 · 4 lines: a b c d] please" || q.expand(back, false) != "look at a\nb\nc\nd please" {
		t.Fatalf("unfold = %q", back)
	}
}

// Files dropped onto the terminal go to the box under the mouse, whichever
// has the keys; text pasted goes where the keys are.
func TestDropGoesWhereItFalls(t *testing.T) {
	dir := t.TempDir()
	f := filepath.Join(dir, "notes.txt")
	if err := os.WriteFile(f, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if !isDrop(f+" ", statLook) || !isDrop("file://"+f, statLook) || isDrop("hello "+f, statLook) || isDrop("", statLook) || isDrop("notes.txt", statLook) {
		t.Fatal("isDrop doesn't tell a drop from a paste")
	}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, listW: 40, mode: modeList}
	m.focusAt(60, 10)
	if !m.paneFocus {
		t.Fatal("a drop on the Session didn't give it the keys")
	}
	m.focusAt(10, 10)
	if m.paneFocus {
		t.Fatal("a drop on Agents left the keys with the Session")
	}
	m.focusAt(40, 10) // the edge between them
	if m.paneFocus {
		t.Fatal("a drop on the edge moved the keys")
	}
}

// The pointer coming onto the Session or Agents gives it the keys as it
// crosses; moving about inside one doesn't take them back after tab.
func TestPointerOntoSessionFocusesIt(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, listW: 40, mode: modeList}
	m.update(tea.MouseMotionMsg{X: 10, Y: 5})
	if m.paneFocus {
		t.Fatal("moving over Agents gave the Session the keys")
	}
	m.update(tea.MouseMotionMsg{X: 60, Y: 5})
	if !m.paneFocus {
		t.Fatal("the pointer onto the Session didn't give it the keys")
	}
	m.paneFocus = false // tab back to Agents
	m.update(tea.MouseMotionMsg{X: 62, Y: 6})
	if m.paneFocus {
		t.Fatal("moving within the Session took the keys back from Agents")
	}
	m.update(tea.MouseMotionMsg{X: 60, Y: 5})
	m.update(tea.MouseMotionMsg{X: 10, Y: 5})
	if m.paneFocus {
		t.Fatal("the pointer onto Agents didn't give it the keys")
	}
	m.paneFocus = true // tab to the Session
	m.update(tea.MouseMotionMsg{X: 12, Y: 6})
	if !m.paneFocus {
		t.Fatal("moving within Agents took the keys back from the Session")
	}
}

// cmd+c copies a selection in a box, as ctrl+c does.
func TestSuperCCopiesTheSelection(t *testing.T) {
	buf := []rune("hello world")
	_, _, anchor, copied, ok := editSel(buf, 5, 0, tea.KeyPressMsg{}, "super+c")
	if !ok || copied != "hello" || anchor != -1 {
		t.Fatalf("copied %q, anchor %d, ok %v", copied, anchor, ok)
	}
}

// With copy on select off, a drag in the box leaves its selection for
// cmd+c rather than copying.
func TestBoxDragKeepsTheSelection(t *testing.T) {
	off := false
	m := &Model{store: &state.Store{}}
	m.store.Config.CopyOnSelect = &off
	m.input, m.back, m.anchor, m.boxDrag = []rune("hello world"), 6, 1, 2
	m.endBoxDrag()
	if m.pendingCopy != "" || m.anchor != 1 {
		t.Fatalf("copied %q, anchor %d", m.pendingCopy, m.anchor)
	}
	m.store.Config.CopyOnSelect = nil
	m.boxDrag = 2
	m.endBoxDrag()
	if m.pendingCopy != "hello" {
		t.Fatalf("on, it copies: %q", m.pendingCopy)
	}
}

// The same text pasted again right away opens its chip in place to edit, as
// in Claude Code, in either box. Once anything follows the chip, or the
// clipboard holds something else, a paste folds into a chip of its own.
func TestPastedAgainOpensTheChip(t *testing.T) {
	text := "one\ntwo\nthree"
	other := "four\nfive"
	paste := func(m *Model, s string) {
		_, cmd := m.Update(tea.PasteMsg{Content: s})
		for cmd != nil {
			back := cmd()
			if a, ok := back.(applyMsg); ok {
				back = a.applyTo(m)()
			}
			_, cmd = m.Update(back)
		}
	}
	for _, pane := range []bool{false, true} {
		m, _ := benchModel(120, 40)
		m.paneFocus = pane
		m.host.input = nil
		m.fastPending = true
		box := func() string {
			if pane {
				return string(m.host.input)
			}
			return string(m.input)
		}
		chip := chipFor(1, text)

		paste(m, text)
		if box() != chip {
			t.Fatalf("pane=%v: first paste %q", pane, box())
		}
		paste(m, text)
		if box() != text {
			t.Fatalf("pane=%v: pasted again %q", pane, box())
		}
		// A third time it's a chip again, after the opened text.
		paste(m, text)
		if box() != text+chipFor(2, text) {
			t.Fatalf("pane=%v: third paste %q", pane, box())
		}

		// Something else on the clipboard is a paste of its own.
		m, _ = benchModel(120, 40)
		m.paneFocus = pane
		m.host.input = nil
		m.fastPending = true
		paste(m, text)
		paste(m, other)
		if box() != chip+chipFor(2, other) {
			t.Fatalf("pane=%v: another paste %q", pane, box())
		}

		// Typed after the chip, the same paste is a new chip.
		m, _ = benchModel(120, 40)
		m.paneFocus = pane
		m.host.input = nil
		m.fastPending = true
		paste(m, text)
		m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
		paste(m, text)
		if box() != chip+"x"+chipFor(2, text) {
			t.Fatalf("pane=%v: paste after typing %q", pane, box())
		}
	}
}
