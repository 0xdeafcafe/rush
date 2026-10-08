package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/0xdeafcafe/rush/internal/statusline"
)

// A fork up to turn 2 is the transcript cut where turn 3 starts, under a
// session id of its own.
func TestForkCutsTheTranscript(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "old.jsonl")
	var b strings.Builder
	for i, p := range []string{"first", "second", "third"} {
		fmt.Fprintf(&b, `{"type":"user","sessionId":"old","timestamp":"2026-09-01T10:0%d:00Z","message":{"role":"user","content":%q}}`+"\n", i, p)
		fmt.Fprintf(&b, `{"type":"assistant","sessionId":"old","timestamp":"2026-09-01T10:0%d:30Z","message":{"id":"m%d","role":"assistant","model":"claude-x","content":[{"type":"text","text":"done %d"}],"usage":{"input_tokens":1,"output_tokens":1}}}`+"\n", i, i, i)
	}
	if err := os.WriteFile(src, []byte(b.String()), 0o600); err != nil {
		t.Fatal(err)
	}
	starts, err := convo.TurnStarts(src)
	if err != nil || len(starts) != 3 || starts[2].Prompt != "third" {
		t.Fatalf("turn starts: %+v %v", starts, err)
	}
	off, err := cutAfter(src, 2, "third")
	if err != nil || off != starts[2].Offset {
		t.Fatalf("cut after 2: %d %v (want %d)", off, err, starts[2].Offset)
	}
	// The prompt wins over a numbering that's off by one.
	if off2, _ := cutAfter(src, 1, "third"); off2 != off {
		t.Fatalf("matched by prompt: %d, want %d", off2, off)
	}
	dst := filepath.Join(dir, "new.jsonl")
	if err := claude.CopyTranscript(src, dst, "old", "new", off); err != nil {
		t.Fatal(err)
	}
	got, _ := os.ReadFile(dst)
	if strings.Contains(string(got), "third") || strings.Contains(string(got), `"sessionId":"old"`) || !strings.Contains(string(got), `"sessionId":"new"`) {
		t.Fatalf("copy:\n%s", got)
	}
	if sess := convo.History(dst, time.Now()); len(sess.Turns) != 2 {
		t.Fatalf("the copy holds %d turns, want 2", len(sess.Turns))
	}
	if err := claude.CopyTranscript(src, dst, "old", "new", -1); err == nil {
		t.Fatal("an existing conversation is never overwritten")
	}
}

func TestPluginSheet(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, w: 140, h: 44}
	p := &pluginSheet{loaded: true, acct: agent.Profile{Name: "work"}, cwd: "/x", costs: map[string]string{"lw@m": "~108 tok"}, costAsked: map[string]bool{"lw@m": true}}
	p.installed = []agent.Plugin{{ID: "lw@m", Name: "lw", Marketplace: "m", Enabled: true, Scope: "user", Description: "records things",
		Parts: agent.PluginParts{Skills: []string{"lw"}, Hooks: []string{"SessionStart", "Stop"}}}}
	p.available = []agent.Plugin{{ID: "a@m", Name: "alpha", Installs: 1_300_000, Description: "front ends"}, {ID: "b@m", Name: "beta", Installs: 12, Description: "tests"}}
	p.markets = []agent.Marketplace{{Name: "m", Source: "github", Repo: "o/m"}}
	m.sheet = p
	text := func() string { return ansi.Strip(strings.Join(p.body(m, 120, 40), "\n")) }
	for _, want := range []string{"Installed 1/1 on", "Discover 2", "● lw", "1 skill: lw", "2 hooks: SessionStart, Stop", "~108 tok in every session"} {
		if !strings.Contains(text(), want) {
			t.Errorf("installed tab missing %q:\n%s", want, text())
		}
	}
	// x asks in a dialog first; it doesn't remove.
	p.key(m, tea.KeyPressMsg{}, "x")
	if m.confirm == nil || p.busy != "" || !strings.Contains(m.confirm.question, "lw") {
		t.Fatalf("x should ask first: confirm=%v busy=%q", m.confirm, p.busy)
	}
	m.confirm = nil
	p.key(m, tea.KeyPressMsg{}, "]")
	for _, r := range "tes" {
		p.key(m, tea.KeyPressMsg{Text: string(r)}, string(r))
	}
	if got := p.shown(); len(got) != 1 || got[0].Name != "beta" {
		t.Fatalf("search: %v", got)
	}
	if !strings.Contains(text(), "beta") || strings.Contains(text(), "alpha ") {
		t.Fatalf("discover:\n%s", text())
	}
	p.key(m, tea.KeyPressMsg{}, "]")
	if !strings.Contains(text(), "o/m") {
		t.Fatalf("marketplaces:\n%s", text())
	}
	p.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil {
		t.Fatal("esc closes")
	}
}

func TestStatusSheet(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, w: 140, h: 44}
	a := &fleet.Agent{Key: "k", Cwd: "/src/rush", Acct: claude.Account{Name: "work", ConfigDir: t.TempDir()}.Profile()}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	c.sess.Model = "Opus 5.5"
	m.openStatusLine(c, a)
	st := m.sheet.(*statusSheet)
	st.tab = stClaude
	text := func() string { return ansi.Strip(strings.Join(st.body(m, 120, 40), "\n")) }
	if !strings.Contains(text(), "  Opus 5.5 · rush") || strings.Contains(text(), "Your own line") {
		t.Fatalf("preview:\n%s", text())
	}
	// Folder moves up above Model, then down past the end of line 1 onto
	// line 2, then Model comes out of the line.
	st.cur = 1
	st.key(m, tea.KeyPressMsg{}, "shift+up")
	if st.agentLay.Lines[0][0] != "folder" || st.cur != 0 {
		t.Fatalf("move: %v cur=%d", st.agentLay.Lines, st.cur)
	}
	st.key(m, tea.KeyPressMsg{}, "2")
	if len(st.agentLay.Lines[1]) != 1 || st.agentLay.Lines[1][0] != "folder" {
		t.Fatalf("to line 2: %v", st.agentLay.Lines)
	}
	st.cur = 0
	st.key(m, tea.KeyPressMsg{}, "space")
	if st.agentLay.Shown("model") || !strings.Contains(text(), "   ⎇") && !strings.Contains(text(), "   ctx") {
		t.Fatalf("hide: %v\n%s", st.agentLay.Lines, text())
	}
	if !strings.Contains(text(), "Line 3") || !strings.Contains(text(), "Not shown") {
		t.Fatalf("headings:\n%s", text())
	}
	// Saving writes the layout and points settings.json at rush.
	os.WriteFile(filepath.Join(a.Acct.Dir, "settings.json"), []byte(`{"model":"opus"}`), 0o600)
	st.key(m, tea.KeyPressMsg{}, "enter")
	b, _ := os.ReadFile(filepath.Join(a.Acct.Dir, "settings.json"))
	if !strings.Contains(string(b), "statusline") || !strings.Contains(string(b), `"model": "opus"`) || m.sheet != nil {
		t.Fatalf("settings.json after save:\n%s", b)
	}
}

// /plugins is Claude Code's /plugin by another name: enter must open it,
// not /reload-plugins, which also contains the word.
func TestSlashAliasComesFirst(t *testing.T) {
	c := &hostConn{kind: "claude", sess: convo.New(), open: map[string]bool{}}
	c.sess.Commands = []event.Command{{Name: "reload-plugins"}, {Name: "compact"}}
	c.input = []rune("/plugins")
	if got := slashMatches(c); len(got) < 2 || got[0].Name != "plugin" {
		t.Fatalf("/plugins: %v", got)
	}
	c.input = []rune("/compact")
	if got := slashMatches(c); got[0].Name != "compact" {
		t.Fatalf("exact first: %v", got)
	}
}

// Segments drag to a new place with the mouse, through the screen's own
// mouse messages: onto another, it takes that one's place; onto a line's
// heading from below, it goes to the end of the line above.
func TestStatusSheetDrag(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, w: 140, h: 50}
	// Use an explicit detailed layout so drag coverage is independent of defaults.
	m.bars.Top = statusline.Layout{Lines: [][]string{{"today", "usage"}, {"ram", "tokens", "cpu", "net", "disk", "battery", "tmp"}}, Sep: " · "}
	m.openTopBar(nil)
	st := m.sheet.(*statusSheet)
	if w := m.sheetWidth(); w != m.w-6 {
		t.Fatalf("the top bar's sheet should be as wide as it can: %d", w)
	}
	at := func(text string) (int, int) {
		lines := strings.Split(ansi.Strip(m.sheetView(strings.Repeat("\n", m.h-1))), "\n")
		for y, l := range lines {
			if x := strings.Index(l, text); x >= 0 {
				return ansi.StringWidth(l[:x]), y
			}
		}
		t.Fatalf("no %q on screen", text)
		return 0, 0
	}
	drag := func(what, onto string) {
		x, y := at(what)
		m.Update(tea.MouseClickMsg{X: x, Y: y, Button: tea.MouseLeft})
		x, y = at(onto)
		m.Update(tea.MouseMotionMsg{X: x, Y: y, Button: tea.MouseLeft})
		m.Update(tea.MouseReleaseMsg{X: x, Y: y, Button: tea.MouseLeft})
	}
	drag("Spend today", "CPU")
	if got := st.bars.Top.Lines; !slices.Equal(got[0], []string{"usage"}) || !slices.Equal(got[1], []string{"ram", "tokens", "cpu", "today", "net", "disk", "battery", "tmp"}) {
		t.Fatalf("onto CPU: %v", got)
	}
	drag("Clock", "Line 2")
	if got := st.bars.Top.Lines[0]; !slices.Equal(got, []string{"usage", "clock"}) || st.drag != "" {
		t.Fatalf("onto Line 2's heading: %v", got)
	}
	x, y := at("Claude Code  ")
	m.Update(tea.MouseClickMsg{X: x, Y: y, Button: tea.MouseLeft})
	if st.tab != stClaude {
		t.Fatalf("clicking a tab: %d", st.tab)
	}
}

// A status line of your own becomes a segment, and survives saving.
func TestStatusSheetKeepsYourOwn(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("RUSH_HOME", t.TempDir())
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, w: 140, h: 44}
	a := &fleet.Agent{Key: "k", Cwd: t.TempDir(), Acct: claude.Account{Name: "work", ConfigDir: t.TempDir()}.Profile()}
	os.WriteFile(filepath.Join(a.Acct.Dir, "settings.json"), []byte(`{"statusLine":{"type":"command","command":"cat >/dev/null; echo mine"}}`), 0o600)
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.openStatusLine(c, a)
	st := m.sheet.(*statusSheet)
	st.tab = stClaude
	// Your own command runs off to the side: its output shows once it's in.
	shows := func() bool { return strings.Contains(ansi.Strip(strings.Join(st.body(m, 120, 40), "\n")), "   mine") }
	for end := time.Now().Add(3 * time.Second); !shows() && time.Now().Before(end); {
		time.Sleep(20 * time.Millisecond)
	}
	if st.agentLay.Lines[0][0] != "custom" || !shows() {
		t.Fatalf("your own line should lead: %v", st.agentLay.Lines)
	}
	st.key(m, tea.KeyPressMsg{}, "enter")
	if l := statusline.Load(); l.Custom == "" || l.Lines[0][0] != "custom" || len(l.Lines) != 2 {
		t.Fatalf("saved: %+v", l)
	}
}

// /permissions adds a rule to the file you pick, and takes one out only
// on the second x; other keys in the file are kept.
func TestPermSheet(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, w: 140, h: 44}
	cwd := t.TempDir()
	a := &fleet.Agent{Key: "k", Cwd: cwd, Acct: claude.Account{Name: "work", ConfigDir: t.TempDir()}.Profile()}
	user := filepath.Join(a.Acct.Dir, "settings.json")
	os.WriteFile(user, []byte(`{"model":"opus","permissions":{"allow":["Read"],"defaultMode":"auto"}}`), 0o600)
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.openPermissions(c, a)
	p := m.sheet.(*permSheet)
	if len(p.rules[0]) != 1 || p.rules[0][0].file.label != "yours" || !strings.Contains(p.mode, "auto") {
		t.Fatalf("loaded: %+v mode %q", p.rules, p.mode)
	}
	p.key(m, tea.KeyPressMsg{}, "a")
	for _, r := range "Bash(ls:*)" {
		p.key(m, tea.KeyPressMsg{Text: string(r)}, string(r))
	}
	p.key(m, tea.KeyPressMsg{}, "enter") // into the project's just-for-you file
	local, _ := os.ReadFile(filepath.Join(cwd, ".claude", "settings.local.json"))
	if !strings.Contains(string(local), "Bash(ls:*)") || len(p.rules[0]) != 2 {
		t.Fatalf("added: %s %+v", local, p.rules[0])
	}
	p.cur[0] = 0
	p.key(m, tea.KeyPressMsg{}, "x")
	if len(p.rules[0]) != 2 || m.confirm == nil {
		t.Fatal("x only asks")
	}
	m.confirmKey("y")
	b, _ := os.ReadFile(user)
	if strings.Contains(string(b), `"Read"`) || !strings.Contains(string(b), `"model": "opus"`) || !strings.Contains(string(b), "auto") {
		t.Fatalf("took out:\n%s", b)
	}
	hs := parseHooks([]byte(`{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}`), settingsFile{"yours", user})
	if len(hs) != 1 || hs[0].matcher != "Bash" || hs[0].command != "echo hi" {
		t.Fatalf("hooks: %+v", hs)
	}
}
