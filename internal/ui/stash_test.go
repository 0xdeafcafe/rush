package ui

import (
	"os"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/0xdeafcafe/rush/internal/testhome"
)

// TestMain keeps what the tests save (config, say) out of your own
// home. It's HOME rather than RUSH_HOME so a test can still have a home
// of its own with t.Setenv("HOME", …).
func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "rush-ui-test")
	if err != nil {
		panic(err)
	}
	os.Setenv("HOME", dir)
	code := testhome.Run(m)
	os.RemoveAll(dir)
	os.Exit(code)
}

func draftModel() (*Model, *hostConn) {
	a := &fleet.Agent{Key: "k", DisplayName: "fix the tests"}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}, paneFocus: true}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	c.box = box{w: 40, lead: "❯ "}
	m.host = c
	return m, c
}

func typeInBox(m *Model, text string) {
	for _, r := range text {
		s := string(r)
		if r == ' ' {
			s = "space"
		}
		m.paneKey(tea.KeyPressMsg{Text: string(r), Code: r}, s)
	}
}

// A wiped box comes back with undo, and the flash says where else it is.
func TestWipeUndo(t *testing.T) {
	m, c := draftModel()
	m.hooks = stashRunning("")
	typeInBox(m, "refactor the parser")
	m.paneKey(tea.KeyPressMsg{}, "esc")
	if len(c.input) != 0 {
		t.Fatalf("esc should clear the box: %q", string(c.input))
	}
	if !strings.Contains(m.status, "ctrl+r › Cleared has it") {
		t.Fatalf("the flash should say where it went: %q", m.status)
	}
	m.paneKey(tea.KeyPressMsg{}, "super+z")
	if got := string(c.input); got != "refactor the parser" {
		t.Fatalf("undo after esc: %q", got)
	}
	// Typing undoes a word at a time, not a letter.
	m.paneKey(tea.KeyPressMsg{}, "ctrl+_")
	if got := string(c.input); got != "refactor the " {
		t.Fatalf("undo a word: %q", got)
	}
	m.paneKey(tea.KeyPressMsg{}, "super+shift+z")
	if got := string(c.input); got != "refactor the parser" {
		t.Fatalf("redo: %q", got)
	}
}

// stashRunning is the broker saying the stash runs, called called.
func stashRunning(called string) *hooks.Client {
	return hooks.Static(plugin.UIState{Plugins: []plugin.UIPlugin{{Name: stashPlugin, Values: map[string]string{"called": called}}}})
}

// ctrl+p and ctrl+r are the stash and history in a Session's box, typed
// in or not; in the Prompt only with something typed, and the list's own
// split and rename without.
func TestStashKeys(t *testing.T) {
	m, c := draftModel()
	m.hooks = stashRunning("")
	for _, s := range []string{"ctrl+p", "ctrl+r"} {
		if cmd := m.key(mustKey(s)); cmd == nil {
			t.Fatalf("%s in an empty Session box should run the stash", s)
		}
	}
	typeInBox(m, "half a thought")
	if cmd := m.key(mustKey("ctrl+p")); cmd == nil || string(c.input) != "half a thought" {
		t.Fatal("ctrl+p should ask the stash, which sets the box")
	}

	m, _ = benchModel(200, 50)
	m.hooks = stashRunning("")
	m.paneFocus, m.inKind = false, inPrompt
	was := m.splitProjects()
	if m.key(mustKey("ctrl+p")); m.splitProjects() == was {
		t.Fatal("ctrl+p in an empty Prompt should split the list")
	}
	was = m.splitProjects()
	m.input = []rune("a task")
	if cmd := m.key(mustKey("ctrl+p")); cmd == nil || m.splitProjects() != was {
		t.Fatal("ctrl+p with something typed should stash it, not split the list")
	}
	if cmd := m.key(mustKey("ctrl+r")); cmd == nil || m.inKind != inPrompt {
		t.Fatal("ctrl+r with something typed should open the history, not rename")
	}
}

// What the stash is called is said everywhere: its flashes, the history's
// title, the box's keys and the guide.
func TestStashCalledDrafts(t *testing.T) {
	m, c := draftModel()
	m.hooks = stashRunning("Drafts")
	m.pluginDo(plugin.UIDo{Plugin: stashPlugin, Kind: "notify", Text: "kept as a draft · {aside} brings it back now"})
	if m.status != "kept as a draft · ctrl+p brings it back now" {
		t.Fatalf("flash %q", m.status)
	}
	c.input = []rune("typed")
	if keys := strings.Join(m.boxKeys(c, &fleet.Agent{}, c.sess), " "); !strings.Contains(keys, "ctrl+p keep as a draft") || !strings.Contains(keys, "ctrl+r drafts · sent · cleared") {
		t.Fatalf("box keys: %s", keys)
	}
	m.helpPage = 0
	help := ansi.Strip(strings.Join(m.helpBody(), "\n"))
	if !strings.Contains(help, "keep as a draft · what's typed") || !strings.Contains(help, "#drafts") || strings.Contains(help, "stash") {
		t.Fatalf("guide:\n%s", help)
	}
	// The Prompt, emptied, has the list's keys: the history is its command.
	m.paneFocus = false
	m.wipePrompt()
	m.input = []rune("x")
	m.wipePrompt()
	if !strings.Contains(m.status, "#drafts › Cleared has it") {
		t.Fatalf("flash %q", m.status)
	}
}

// A hint after a strip of tabs goes first when the row is too narrow.
func TestTabHintDropsFirst(t *testing.T) {
	if got := ansi.Strip(withTabHint("  a b", "[ ]", "views", "", 40)); got != "  a b   [ ] views" {
		t.Fatalf("wide: %q", got)
	}
	if got := ansi.Strip(withTabHint("  a b", "[ ]", "views", " zen", 12)); got != "  a b zen" {
		t.Fatalf("narrow: %q", got)
	}
}

// ↑ and ↓ move through a message's rows, keeping the column.
func TestBoxVert(t *testing.T) {
	m, c := draftModel()
	c.input = []rune("first line\nsecond line\nthird")
	c.back = len("third") - 2 // on "th|ird"
	m.paneKey(tea.KeyPressMsg{}, "up")
	if pos := len(c.input) - c.back; pos != len("first line\nse") {
		t.Fatalf("up: at %d", pos)
	}
	m.paneKey(tea.KeyPressMsg{}, "up")
	m.paneKey(tea.KeyPressMsg{}, "up")
	if c.back != len(c.input) {
		t.Fatalf("up past the first row should go to the start, back=%d", c.back)
	}
	m.paneKey(tea.KeyPressMsg{}, "super+down")
	if c.back != 0 {
		t.Fatalf("cmd+↓ should go to the end, back=%d", c.back)
	}
}

// cmd+← and → reach the line's ends however the terminal sends cmd.
func TestCmdArrows(t *testing.T) {
	for _, s := range []string{"super+left", "ctrl+a", "home"} {
		if _, pos := press("one\ntwo three", 13, s); pos != 4 {
			t.Errorf("%s: at %d, want 4", s, pos)
		}
	}
	m, c := draftModel()
	c.input, c.back = []rune("hello"), 2
	m.key(tea.KeyPressMsg{Code: tea.KeyLeft, Mod: tea.ModMeta})
	if c.back != 5 {
		t.Errorf("meta+left should go to the start, back=%d", c.back)
	}
}
