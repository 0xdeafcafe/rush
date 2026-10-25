package ui

import (
	"github.com/charmbracelet/x/ansi"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/keymap"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func pressKeys(m *Model, keys ...string) {
	for _, s := range keys {
		k, ok := keyOf(s)
		if !ok {
			panic("no key " + s)
		}
		m.key(k)
	}
}

// A chord of yours stands in for the default key of what it's bound to,
// and the default key it moved from does nothing.
func TestRemapInSession(t *testing.T) {
	m, _ := benchModel(200, 50)
	c := m.host
	if !m.paneFocus {
		pressKeys(m, "}")
	}
	m.setKeys(keymap.File{Bindings: map[string][]string{"session.verbose": {"ctrl+x v"}}})
	was := c.verbose
	pressKeys(m, "ctrl+x")
	if len(m.keys.chord) != 1 || !strings.Contains(m.status, "ctrl+x") {
		t.Fatalf("ctrl+x should begin the chord: %v %q", m.keys.chord, m.status)
	}
	pressKeys(m, "v")
	if c.verbose == was {
		t.Fatal("ctrl+x v should do what ctrl+o did")
	}
	pressKeys(m, "ctrl+o")
	if c.verbose == was {
		t.Fatal("ctrl+o was moved away: it should do nothing now")
	}
	// A chord no action finishes is said so, and its keys go nowhere.
	in := string(c.input)
	pressKeys(m, "ctrl+x", "q")
	if string(c.input) != in || !strings.Contains(m.status, "does nothing") {
		t.Fatalf("box %q, status %q", string(c.input), m.status)
	}
}

// tab, { and } go between the list and the Session; { and } only with
// nothing typed, and are text when something is.
func TestBracesSwitchFocus(t *testing.T) {
	m, _ := benchModel(200, 50)
	if !m.paneFocus {
		pressKeys(m, "}")
	}
	if !m.paneFocus {
		t.Fatal("} should go into the Session")
	}
	pressKeys(m, "tab")
	if m.paneFocus {
		t.Fatal("tab should leave the Session")
	}
	pressKeys(m, "tab")
	if !m.paneFocus {
		t.Fatal("tab should go back into the Session")
	}
	m.host.input = nil
	pressKeys(m, "a", "{")
	if got := string(m.host.input); !m.paneFocus || got != "a{" {
		t.Fatalf("typed, { is text: focus %v, box %q", m.paneFocus, got)
	}
	m.host.input = nil
	pressKeys(m, "{")
	if m.paneFocus {
		t.Fatal("{ with nothing typed should go back to Agents")
	}
}

func TestChordRunsCommand(t *testing.T) {
	m, _ := benchModel(200, 50)
	if m.paneFocus {
		pressKeys(m, "{")
	}
	m.setKeys(keymap.File{Bindings: map[string][]string{"command:help": {"ctrl+g h"}}})
	pressKeys(m, "ctrl+g", "h")
	if m.keyHelp == nil {
		t.Fatalf("ctrl+g h should run #help: mode %d", m.mode)
	}
}

func TestKeysPageTakesKeys(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(200, 50)
	m.setView(placeSettings)
	m.setSettingsPage(pageKeys)
	m.showKey("list.pr")
	pressKeys(m, "enter")
	if out := m.render(); !strings.Contains(out, "New keys for") || !strings.Contains(out, "╭") {
		t.Fatalf("taking keys should ask in a box:\n%s", out)
	}
	pressKeys(m, "ctrl+x", "p", "enter")
	if got := m.keyMap().KeyText("list.pr"); got != "ctrl+x p" {
		// ctrl+x is list.stop's: it asks first.
		if m.confirm == nil {
			t.Fatalf("list.pr has %q and nothing asked", got)
		}
		pressKeys(m, "y")
	}
	if got := m.keyMap().KeyText("list.pr"); got != "ctrl+x p" {
		t.Fatalf("list.pr has %q", got)
	}
	if got := m.keys.file.Bindings["list.pr"]; len(got) != 1 || got[0] != "ctrl+x p" {
		t.Fatalf("file has %v", got)
	}
	body := strings.Join(m.dialogBody(160), "\n")
	if !strings.Contains(body, "ctrl+x p") {
		t.Fatal("the page should show the new keys")
	}
	// A key that types can't start one.
	m.showKey("list.pin")
	pressKeys(m, "enter", "p", "enter")
	if m.keyMap().KeyText("list.pin") != "ctrl+t" {
		t.Fatal("p alone shouldn't be taken")
	}
}

func TestPluginsInTheScreen(t *testing.T) {
	m, _ := benchModel(200, 50)
	c := m.host
	m.hooks = hooks.Static(plugin.UIState{
		Plugins: []plugin.UIPlugin{{Name: "haven", UI: []string{"overview", "notify", "input"},
			Commands: []plugin.CommandSpec{{Name: "open", Description: "open the stack's home", Key: "alt+u"}},
			Settings: []plugin.SettingSpec{{Key: "logs", Title: "Show log errors", Type: "bool", Default: "true"}}}},
		Sections: map[string][]plugin.UISection{c.key: {{Plugin: "haven", ID: "stack", Title: "Stack", Lines: []plugin.Line{{Text: "app up", Tone: "good"}}}}},
	})
	m.setKeys(keymap.File{})
	if got := m.keyMap().KeyText("plugin:haven.open"); got != "alt+u" {
		t.Fatalf("the plugin's free key should be taken: %q", got)
	}
	lines := m.pluginOverview(c.key)
	text := make([]string, 0, len(lines))
	for _, l := range lines {
		text = append(text, l.Text)
	}
	if s := strings.Join(text, "\n"); !strings.Contains(s, "Stack") || !strings.Contains(s, "app up") {
		t.Fatalf("overview: %q", s)
	}
	m.pluginDo(plugin.UIDo{Plugin: "haven", Kind: "input.set", Session: c.key, Text: "from haven"})
	if string(c.input) != "from haven" {
		t.Fatalf("box %q", string(c.input))
	}
	m.pluginDo(plugin.UIDo{Plugin: "haven", Kind: "notify", Text: "stack \x1b[31mup"})
	if strings.Contains(m.status, "\x1b") || !strings.Contains(m.status, "haven: stack") {
		t.Fatalf("status %q", m.status)
	}
	m.pluginDo(plugin.UIDo{Plugin: "haven", Kind: "notify", Session: c.key, Text: "stack down"})
	if a := m.agentByKey(c.key); a == nil || !strings.Contains(m.status, "haven · "+a.DisplayName+": stack down") {
		t.Fatalf("a notice about a session should name it: %q", m.status)
	}
	m.setView(placeSettings)
	m.setSettingsPage(pagePlugins)
	<-m.dialog.pluginsRead.done
	if body := strings.Join(m.dialogBody(160), "\n"); !strings.Contains(body, "haven") || strings.Contains(body, "Show log errors") {
		t.Fatalf("Plugins should list plugins, not their settings:\n%s", body)
	}
	for i, r := range flat(m.pluginSections()) {
		if r.label == "haven" {
			m.dialog.cursor = i
		}
	}
	m.dialogKey(tea.KeyPressMsg{}, "enter")
	if body := strings.Join(m.dialogBody(160), "\n"); !strings.Contains(body, "Show log errors") || !strings.Contains(body, "alt+u") {
		t.Fatalf("Plugins page:\n%s", body)
	}
	if m.dialogKey(tea.KeyPressMsg{}, "esc"); m.dialog == nil || m.dialog.plugin != "" || m.view != placeSettings {
		t.Fatal("esc on an open plugin goes back to the list")
	}
}

// What plugins say before a message goes: held back keeps the box, a
// box changed meanwhile isn't sent.
func TestIntercepted(t *testing.T) {
	m, _ := benchModel(200, 50)
	c := m.host
	c.input = []rune("rm -rf everything")
	c.intercepting = true
	m.onIntercepted(interceptedMsg{key: c.key, was: "rm -rf everything", r: plugin.InterceptResult{Action: "block", Plugin: "guard", Reason: "not that"}})
	if string(c.input) != "rm -rf everything" || !strings.Contains(m.status, "guard: not that") || c.intercepting {
		t.Fatalf("box %q status %q", string(c.input), m.status)
	}
	c.intercepting = true
	c.input = []rune("changed")
	m.onIntercepted(interceptedMsg{key: c.key, was: "before", r: plugin.InterceptResult{Action: "allow"}})
	if string(c.input) != "changed" || !strings.Contains(m.status, "changed while") {
		t.Fatalf("box %q status %q", string(c.input), m.status)
	}
	_ = tea.KeyPressMsg{}
}

func TestPluginHashCommands(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.hooks = hooks.Static(plugin.UIState{Plugins: []plugin.UIPlugin{{Name: "haven",
		Commands: []plugin.CommandSpec{{Name: "open", Description: "open the stack's home"}}}}})
	got := m.hashMatches([]rune("#hav"), 0)
	if len(got) != 1 || got[0].Name != "haven.open" {
		t.Fatalf("#hav offers %+v", got)
	}
	if !m.isPluginCommand("haven.open") || m.isPluginCommand("haven") {
		t.Fatal("haven.open is the plugin's command, haven isn't")
	}
}

// A stop rush continues itself says so, so a plugin needn't too.
func TestHaltSaysRushRetries(t *testing.T) {
	now := time.Now()
	stopped := func(kind, text string, hosted bool) *plugin.UIError {
		a := &fleet.Agent{Key: "a", PID: 7, Rush: hosted}
		a.State, a.UpdatedAt = "done", now.Add(-time.Minute)
		a.Spend.Halt = &claude.Halt{Kind: kind, Text: text, At: now.Add(-time.Minute)}
		return haltKind(a)
	}
	for _, tc := range []struct {
		kind, text string
		hosted     bool
		want       string
		retrying   bool
	}{
		{"server_error", "API Error: Unable to connect to API (ENOTFOUND)", false, "offline", true},
		{"server_error", "API Error: Connection dropped (ECONNRESET)", false, "retryable", true},
		{"server_error", "API Error: Connection dropped (ECONNRESET)", true, "retryable", true},
		{"rate_limit", "You've hit your limit", false, "limit", false},
		{"server_error", "API Error: something else entirely", false, "other", false},
	} {
		e := stopped(tc.kind, tc.text, tc.hosted)
		if e.Kind != tc.want || e.Retrying != tc.retrying {
			t.Errorf("%s hosted=%v: got %s retrying=%v, want %s retrying=%v", tc.text, tc.hosted, e.Kind, e.Retrying, tc.want, tc.retrying)
		}
	}
}

// ctrl+] begins rush's chords for the keys ⌥ has, but always hands Claude
// Code's own screen back.
func TestCtrlBracketLeavesTheScreen(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.paneFocus, m.embedded = false, true
	pressKeys(m, "ctrl+]")
	if m.embedded || len(m.keys.chord) != 0 {
		t.Fatalf("embedded %v, chord %v", m.embedded, m.keys.chord)
	}
	pressKeys(m, "ctrl+]")
	if len(m.keys.chord) != 1 {
		t.Fatal("ctrl+] off the screen should begin a chord")
	}
}

// ctrl+d moves the agent to Done, from the list as from its Session.
func TestCtrlDMarksDone(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.paneFocus = false
	a := m.selected()
	if a == nil {
		t.Skip("no agent selected")
	}
	_, was := m.store.Overlay.Done[a.Key]
	pressKeys(m, "ctrl+d")
	if _, now := m.store.Overlay.Done[a.Key]; now == was && m.confirm == nil {
		t.Fatal("ctrl+d should move the agent to Done, or ask first")
	}
}

// The keys guide opens over a draft without touching it, puts where you
// are first, shows the keys as you have them, finds a key by what it does
// and takes you to change it.
func TestContextualShortcutGuide(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.paneFocus = true
	m.host.input = []rune("unfinished message")
	m.setKeys(keymap.File{Bindings: map[string][]string{
		"session.first": {"ctrl+x h"},
		"session.last":  {},
	}})
	pressKeys(m, "f1")
	if m.keyHelp == nil || m.keyHelp.rows[0].section != "Here · A Session" {
		t.Fatalf("F1 should open the keys on the Session's: %+v", m.keyHelp)
	}
	body := ansi.Strip(strings.Join(m.keyHelpBox(196, 400), "\n"))
	for _, want := range []string{"ctrl+x h", "no key", "full user message", "the command bar", "Everywhere"} {
		if !strings.Contains(body, want) {
			t.Fatalf("guide missing %q: %s", want, body)
		}
	}
	pressKeys(m, "f1")
	if m.keyHelp != nil || string(m.host.input) != "unfinished message" {
		t.Fatal("closing the keys must keep the draft and go back to the Session")
	}
	m.paneFocus = false
	m.input = []rune("list draft")
	pressKeys(m, "f1")
	if m.keyHelp == nil || m.keyHelp.rows[0].section != "Here · The Prompt, with something typed" || string(m.input) != "list draft" {
		t.Fatal("F1 must also open the keys while typing in the Prompt")
	}
	pressKeys(m, "f", "u", "l", "l", "space", "u", "s", "e", "r")
	if r := m.keyHelp.rows[m.keyHelp.cur]; r.id != "session.message" {
		t.Fatalf("typing should find the key: on %q", r.id)
	}
	pressKeys(m, "enter")
	if m.keyHelp != nil || m.view != placeSettings || m.dialog == nil || m.keyContext() != keymap.Session {
		t.Fatal("enter should open Settings, Keys on the key")
	}
	for _, v := range []int{placeProjects, placeHarnesses, placeSettings} {
		m.goView(v)
		pressKeys(m, "f1")
		if m.keyHelp == nil {
			t.Fatalf("F1 should open the keys in place %d", v)
		}
		pressKeys(m, "esc")
	}
}

func TestPluginShortcutCannotSilentlyReplaceQueueAction(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.hooks = hooks.Static(plugin.UIState{Plugins: []plugin.UIPlugin{{Name: "haven",
		Commands: []plugin.CommandSpec{{Name: "open", Key: "alt+o"}},
	}}})
	m.setKeys(keymap.File{})
	if got := m.keyMap().KeyText("plugin:haven.open"); got != "" {
		t.Fatalf("plugin suggestion stole the queue shortcut: %q", got)
	}
	if got := m.keyMap().Resolve([]keymap.Context{keymap.Session}, nil, "alt+o"); got.Key != "alt+o" {
		t.Fatalf("queue shortcut should remain usable: %+v", got)
	}
	// An explicit user binding can override the default, as Settings promises.
	m.setKeys(keymap.File{Bindings: map[string][]string{"plugin:haven.open": {"alt+o"}}})
	if got := m.keyMap().Resolve([]keymap.Context{keymap.Session}, nil, "alt+o"); got.Run != "plugin:haven.open" {
		t.Fatalf("explicit plugin binding not honored: %+v", got)
	}
}
