package ui

import (
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func infoModel(t *testing.T) (*Model, *hostConn) {
	t.Setenv("HOME", t.TempDir())
	now := time.Now()
	acct := claude.Account{Name: "work", ConfigDir: t.TempDir()}
	a := &fleet.Agent{Key: "k", Kind: "claude", Cwd: "/src/rush", Acct: acct.Profile()}
	usage := claude.Usage{Email: "me@x", Plan: "Max", FiveHour: claude.Window{Present: true, Percent: 42, ResetsAt: now.Add(2 * time.Hour)}, SevenDay: claude.Window{Present: true, Percent: 12, ResetsAt: now.Add(3 * 24 * time.Hour)}}.Reading()
	m := &Model{store: &state.Store{}, w: 140, h: 50, snap: &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a},
		Accounts: []fleet.AccountView{{Folder: state.Folder(acct), Usage: usage, Quota: usage.Quota(""), Today: 3.5, Current: true}}}}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	c.sess.Model, c.sess.Version = "claude-opus-5-5", "2.1.280"
	c.sess.MCP = []event.MCPServer{{Name: "rush", Status: "connected"}, {Name: "linear", Status: "failed"}}
	c.sess.Commands = []event.Command{{Name: "compact"}}
	m.host = c
	return m, c
}

func TestStatusAndUsageAreRushSheets(t *testing.T) {
	m, c := infoModel(t)
	c.input = []rune("/status")
	m.sendPane(c, false)
	st, ok := m.sheet.(*infoSheet)
	if !ok || st.tab != infoStatus || len(c.input) != 0 {
		t.Fatalf("/status opened %T", m.sheet)
	}
	text := ansi.Strip(strings.Join(st.body(m, 96, 60), "\n"))
	if !strings.Contains(text, "Status  ·  Context  ·  Usage  ·  History  ·  Settings") {
		t.Errorf("tabs:\n%s", text)
	}
	for _, want := range []string{"2.1.280", "● linear failed", "● rush connected", "me@x", "Max", "/src/rush"} {
		if !strings.Contains(text, want) {
			t.Errorf("/status missing %q:\n%s", want, text)
		}
	}
	for _, cmd := range []string{"/usage", "/cost"} {
		m.sheet = nil
		c.input = []rune(cmd)
		m.sendPane(c, false)
		u, ok := m.sheet.(*infoSheet)
		if !ok || u.tab != infoUsage {
			t.Fatalf("%s opened %T", cmd, m.sheet)
		}
		text = ansi.Strip(strings.Join(u.body(m, 96, 60), "\n"))
		for _, want := range []string{"Plan · work · Max", "5-hour", "42%", "weekly", "12%", "$3.50 today"} {
			if !strings.Contains(text, want) {
				t.Errorf("%s missing %q:\n%s", cmd, want, text)
			}
		}
	}
}

func TestUnknownCommandAsks(t *testing.T) {
	m, c := infoModel(t)
	// One the session knows goes to Claude as it is; a path isn't a command.
	for _, text := range []string{"/compact", "/src/rush has a bug"} {
		m.sheet = nil
		c.input = []rune(text)
		m.sendPane(c, false)
		if m.sheet != nil {
			t.Fatalf("%q asked: %T", text, m.sheet)
		}
	}
	c.input = []rune("/brand-new-screen now")
	m.sendPane(c, false)
	u, ok := m.sheet.(*unknownSheet)
	if !ok || u.line != "brand-new-screen now" || string(c.input) != "/brand-new-screen now" {
		t.Fatalf("unknown: %T %+v input %q", m.sheet, u, string(c.input))
	}
	if text := ansi.Strip(strings.Join(u.body(m, 68, 30), "\n")); !strings.Contains(text, "isn't a command rush knows") {
		t.Fatalf("dialog:\n%s", text)
	}
	u.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil || string(c.input) != "/brand-new-screen now" {
		t.Fatalf("esc keeps what you typed: %q", string(c.input))
	}
	// The second choice sends it to Claude after all, without asking again.
	m.sendPane(c, false)
	m.sheet.(*unknownSheet).key(m, tea.KeyPressMsg{}, "down")
	m.sheet.(*unknownSheet).key(m, tea.KeyPressMsg{}, "enter")
	if m.sheet != nil || len(c.input) != 0 || c.sendRaw {
		t.Fatalf("send anyway: %T input %q", m.sheet, string(c.input))
	}
}

func TestInfoTabs(t *testing.T) {
	m, c := infoModel(t)
	acct := m.snap.Agents[0].Acct
	os.WriteFile(filepath.Join(acct.Dir, "stats-cache.json"), []byte(`{"lastComputedDate":"2026-09-23","totalSessions":302,"totalMessages":278004,
		"dailyActivity":[{"date":"2026-09-22","messageCount":15769,"sessionCount":12,"toolCallCount":9000},{"date":"2026-09-21","messageCount":4297,"sessionCount":10,"toolCallCount":9392}],
		"dailyModelTokens":[{"date":"2026-09-22","tokensByModel":{"claude-opus-5":1500000}}],
		"modelUsage":{"claude-opus-5":{"inputTokens":1,"outputTokens":2,"cacheReadInputTokens":3,"cacheCreationInputTokens":4}},
		"hourCounts":{"14":38,"21":39}}`), 0o600)
	os.WriteFile(filepath.Join(acct.Dir, "settings.json"), []byte(`{"model":"opus","permissions":{"allow":["Bash(ls)","Read"],"deny":["Bash(rm:*)"]},"env":{"A":"1"}}`), 0o600)
	m.openInfo(c, infoStatus)
	k := m.sheet.(*infoSheet)
	text := func() string { return ansi.Strip(strings.Join(k.body(m, 96, 50), "\n")) }
	k.key(m, tea.KeyPressMsg{}, "right")
	k.key(m, tea.KeyPressMsg{}, "right")
	k.key(m, tea.KeyPressMsg{}, "right")
	if k.tab != infoHistory {
		t.Fatalf("tab %d", k.tab)
	}
	for _, want := range []string{"302 · 278,004 messages", "Mon 21 Sep", "Tue 22 Sep", "16k msgs", "By model"} {
		if !strings.Contains(text(), want) {
			t.Errorf("history missing %q:\n%s", want, text())
		}
	}
	k.key(m, tea.KeyPressMsg{}, "]")
	for _, want := range []string{"Claude Code", "opus · 1 env var", "2 allow · 1 deny", "claude code ↗"} {
		if !strings.Contains(text(), want) {
			t.Errorf("settings missing %q:\n%s", want, text())
		}
	}
	// enter opens the row's own sheet: Permissions.
	k.key(m, tea.KeyPressMsg{}, "down")
	k.key(m, tea.KeyPressMsg{}, "enter")
	if _, ok := m.sheet.(*permSheet); !ok {
		t.Fatalf("enter on Permissions opened %T", m.sheet)
	}
	// Wrapping round from Status goes to Settings.
	m.openInfo(c, infoStatus)
	m.sheet.key(m, tea.KeyPressMsg{}, "left")
	if m.sheet.(*infoSheet).tab != infoSettings {
		t.Fatal("left from Status")
	}
}

func TestContextTab(t *testing.T) {
	m, c := infoModel(t)
	m.openInfo(c, infoContext)
	k := m.sheet.(*infoSheet)
	text := func() string { return ansi.Strip(strings.Join(k.body(m, 96, 50), "\n")) }
	if !strings.Contains(text(), "the breakdown") {
		t.Fatalf("no count yet:\n%s", text())
	}
	c.sess.Apply(host.Context{Usage: usage.Context{Total: 15801, Max: 1_000_000, Model: "claude-opus-5-5[1m]", AutoCompact: true, AutoCompactAt: 967000, At: time.Now(),
		Parts: []usage.ContextPart{{Name: "System prompt", Tokens: 2163, Kind: "used"}, {Name: "System tools", Tokens: 9537, Kind: "used"},
			{Name: "System tools (deferred)", Tokens: 21578, Kind: "deferred", Deferred: true}, {Name: "Skills", Tokens: 3938, Kind: "used"},
			{Name: "Messages", Tokens: 163, Kind: "used"}, {Name: "Autocompact buffer", Tokens: 33000, Kind: "buffer"}, {Name: "Free space", Tokens: 951199, Kind: "free"}}}}, time.Now())
	for _, want := range []string{"16k of 1M", "auto-compacts at 967k", "System prompt", "Skills", "Auto-compact buffer", "Free"} {
		if !strings.Contains(text(), want) {
			t.Errorf("context missing %q:\n%s", want, text())
		}
	}
	if strings.Contains(text(), "deferred") {
		t.Errorf("deferred tools aren't in the window:\n%s", text())
	}
	// The overview shows it too.
	var ov []string
	for _, l := range c.sess.Overview(convo.Options{Width: 100, Now: time.Now()}) {
		ov = append(ov, ansi.Strip(l.Text))
	}
	if o := strings.Join(ov, "\n"); !strings.Contains(o, "Context") || !strings.Contains(o, "System tools") {
		t.Fatalf("overview:\n%s", o)
	}
}

func TestClaudeCommandsRouting(t *testing.T) {
	m, c := infoModel(t)
	run := func(text string) {
		m.sheet, m.status = nil, ""
		c.input = []rune(text)
		m.sendPane(c, false)
	}
	// Claude Code's own terminal and the odds and ends are off.
	for _, cmd := range []string{"/theme", "/release-notes", "/loops"} {
		run(cmd)
		if m.sheet != nil || !strings.Contains(m.status, "isn't in rush") {
			t.Errorf("%s: sheet %T status %q", cmd, m.sheet, m.status)
		}
	}
	// Account and cloud ones say they open a real Claude Code first.
	for _, cmd := range []string{"/login", "/claude:login", "/tp"} {
		run(cmd)
		k, ok := m.sheet.(*claudeSheet)
		if !ok {
			t.Fatalf("%s opened %T", cmd, m.sheet)
		}
		if text := ansi.Strip(strings.Join(k.body(m, 68, 30), "\n")); !strings.Contains(text, "This opens a real Claude Code") {
			t.Fatalf("%s:\n%s", cmd, text)
		}
	}
	// Ours: /context and /version open the sheet's tabs; /diff the changes view.
	run("/context")
	if k, ok := m.sheet.(*infoSheet); !ok || k.tab != infoContext {
		t.Fatalf("/context opened %T", m.sheet)
	}
	run("/version")
	if k, ok := m.sheet.(*infoSheet); !ok || k.tab != infoStatus {
		t.Fatalf("/version opened %T", m.sheet)
	}
	run("/diff")
	if m.views(c)[c.view] != "changes" {
		t.Fatalf("/diff: view %s", m.views(c)[c.view])
	}
	// The picker offers claude:login, and not the ones that are off.
	c.input = []rune("/log")
	got := slashMatches(c)
	if len(got) == 0 || got[0].Name != "claude:login" {
		t.Fatalf("/log: %v", got)
	}
	c.input = []rune("/them")
	if got := slashMatches(c); len(got) != 0 {
		t.Fatalf("/them: %v", got)
	}
}

func TestBtwExportSubtask(t *testing.T) {
	m, c := infoModel(t)
	c.sess.Info.Proto = 2
	c.sess.Info.Cwd = t.TempDir()
	run := func(text string) tea.Cmd {
		m.sheet, m.status = nil, ""
		c.input = []rune(text)
		return m.sendPane(c, false)
	}
	reply := func(body string) {
		for id := range c.asks {
			m.onReply(c, host.Reply{ID: id, Body: jsontext.Value(body)})
		}
	}
	// /btw asks at once, in a panel over the Session: no sheet takes the keys.
	if cmd := run("/btw what's left to do?"); cmd == nil {
		t.Fatal("/btw sent nothing")
	}
	bt := m.btwFor(c.key)
	if bt == nil || m.sheet != nil || !bt.focused || len(c.asks) != 1 {
		t.Fatalf("/btw: thread %+v sheet %T asks %d", bt, m.sheet, len(c.asks))
	}
	panel := func() string { return ansi.Strip(strings.Join(bt.lines(c, 60, 20, true), "\n")) }
	if !strings.Contains(panel(), "what's left to do?") || !strings.Contains(panel(), "thinking") {
		t.Fatalf("waiting:\n%s", panel())
	}
	// esc hands the keys back to the chat while it thinks: typing goes to
	// the message box.
	m.paneKey(tea.KeyPressMsg{}, "esc")
	m.paneKey(tea.KeyPressMsg{Code: 'x', Text: "x"}, "x")
	if bt.focused || string(c.input) != "x" {
		t.Fatalf("back to the chat: focused %v input %q", bt.focused, string(c.input))
	}
	c.input = nil
	reply(`{"response":"Only the **tests**.","synthetic":false}`)
	if !strings.Contains(panel(), "Only the tests.") || strings.Contains(panel(), "thinking") {
		t.Fatalf("answered:\n%s", panel())
	}
	// It floats over the conversation's top right.
	rows := make([]string, 30)
	for i := range rows {
		rows[i] = strings.Repeat("c", 100)
	}
	m.btwOverlay(c, rows, 2, 26, 100)
	if !strings.HasPrefix(ansi.Strip(rows[2]), "ccc") || !strings.Contains(ansi.Strip(rows[2]), "btw") || ansi.StringWidth(rows[2]) != 100 {
		t.Fatalf("overlay row: %q", ansi.Strip(rows[2]))
	}
	// ctrl+b goes back to it; a follow-up carries the thread.
	m.paneKey(tea.KeyPressMsg{}, "ctrl+b")
	for _, r := range "and then?" {
		m.paneKey(tea.KeyPressMsg{Code: r, Text: string(r)}, string(r))
	}
	if !bt.focused || string(bt.input) != "and then?" {
		t.Fatalf("follow-up typed: %q", string(bt.input))
	}
	// A follow-up says where it goes: enter again keeps it in the panel.
	m.paneKey(tea.KeyPressMsg{}, "enter")
	if !bt.choosing || len(bt.qa) != 1 {
		t.Fatalf("follow-up should ask where it goes: %+v", bt)
	}
	m.paneKey(tea.KeyPressMsg{}, "enter")
	if len(bt.qa) != 2 || len(c.asks) != 1 {
		t.Fatalf("follow-up asked: %+v", bt.qa)
	}
	reply(`{"response":"Ship it.","synthetic":false}`)
	// ctrl+f pulls it out: /fork, its first message the side thread.
	c.sess.Turns = append(c.sess.Turns, &convo.Turn{N: 1, Prompt: "first"})
	c.sess.Info.SessionID = "0123456789abcdef"
	m.paneKey(tea.KeyPressMsg{}, "ctrl+f")
	f, ok := m.sheet.(*forkSheet)
	if !ok || !strings.HasPrefix(string(f.name), "btw: what's left") || !strings.Contains(string(f.first), "You: Ship it.") {
		t.Fatalf("fork out: %T %+v", m.sheet, f)
	}
	m.sheet = nil
	m.paneKey(tea.KeyPressMsg{}, "ctrl+b")
	m.paneKey(tea.KeyPressMsg{}, "ctrl+d")
	if m.btwFor(c.key) != nil {
		t.Fatal("ctrl+d closes it")
	}
	// Tucked away, ctrl+d from the chat closes it too.
	m.openBtw(c, "one more?")
	reply(`{"response":"Sure."}`)
	m.paneKey(tea.KeyPressMsg{}, "esc")
	if bt := m.btwFor(c.key); bt == nil || bt.focused {
		t.Fatal("esc tucks an answered thread away")
	}
	m.paneKey(tea.KeyPressMsg{}, "ctrl+d")
	if m.btwFor(c.key) != nil {
		t.Fatal("ctrl+d from the chat closes it")
	}
	// Opened and left without asking anything, it goes.
	m.paneKey(tea.KeyPressMsg{}, "ctrl+b")
	m.paneKey(tea.KeyPressMsg{}, "esc")
	if m.btwFor(c.key) != nil {
		t.Fatal("an empty thread goes when you leave it")
	}

	// /export: copy, or save next to the agent.
	run("/export")
	e, ok := m.sheet.(*exportSheet)
	if !ok {
		t.Fatalf("/export opened %T", m.sheet)
	}
	reply(`{"text":"> hello\n\nhi","default_filename":"conversation-x.txt"}`)
	e.key(m, tea.KeyPressMsg{}, "down")
	e.key(m, tea.KeyPressMsg{}, "enter")
	if b, err := os.ReadFile(filepath.Join(c.sess.Info.Cwd, "conversation-x.txt")); err != nil || string(b) != "> hello\n\nhi" {
		t.Fatalf("saved: %q %v", b, err)
	}
	run("/export notes.txt")
	reply(`{"text":"all of it","default_filename":"conversation-y.txt"}`)
	if b, _ := os.ReadFile(filepath.Join(c.sess.Info.Cwd, "notes.txt")); string(b) != "all of it" {
		t.Fatalf("saved by name: %q", b)
	}

	// /subtask goes to Claude as a message asking for a background subagent.
	c.sending = nil
	run("/subtask check the flaky test")
	if len(c.sending) != 1 || !strings.Contains(c.sending[0].text, "run_in_background") || !strings.Contains(c.sending[0].text, "check the flaky test") {
		t.Fatalf("subtask sent: %+v", c.sending)
	}
}

func TestContextAgainstAutoCompactWindow(t *testing.T) {
	m, c := infoModel(t)
	c.sess.Context = 520_000
	text := func(tab int) string {
		m.openInfo(c, tab)
		return ansi.Strip(strings.Join(m.sheet.(*infoSheet).body(m, 120, 50), "\n"))
	}
	if got := text(infoStatus); !strings.Contains(got, "52% · 520k of 1M") || strings.Contains(got, "auto-compact") {
		t.Fatalf("no window of its own:\n%s", got)
	}
	m.snap.Agents[0].Compaction = agent.Compaction{Window: 400_000, Headroom: 33_000}
	want := "130% · 520k of 400k · auto-compacts at 367k · model 1M"
	for _, tab := range []int{infoStatus, infoContext} {
		if got := text(tab); !strings.Contains(got, want) {
			t.Errorf("tab %d missing %q:\n%s", tab, want, got)
		}
	}
	var ov []string
	for _, l := range c.sess.Overview(convo.Options{Width: 100, Now: time.Now(), Compaction: m.snap.Agents[0].Compaction}) {
		ov = append(ov, ansi.Strip(l.Text))
	}
	if o := strings.Join(ov, "\n"); !strings.Contains(o, "130%") || !strings.Contains(o, "auto-compact") {
		t.Fatalf("overview:\n%s", o)
	}
}
