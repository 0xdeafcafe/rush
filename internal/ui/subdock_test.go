package ui

import (
	"encoding/json/jsontext"
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
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A running subagent shows in the dock with what it's doing and what it
// just did; ↑ from the box picks it, space watches it and ← comes back to
// its row.
func TestSubagentDock(t *testing.T) {
	path := filepath.Join(t.TempDir(), "agent-a1.jsonl")
	os.WriteFile(path, []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find the pane"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"previewLines"}}]}}`,
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"view.go"}]}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:03Z","message":{"id":"m2","role":"assistant","content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/w/view.go"}}]}}`,
	}, "\n")+"\n"), 0o644)
	st := convo.SubagentStats(path)
	st.Read()
	sa := convo.Subagent{ID: "a1", Type: "Explore", Description: "find the pane", Path: path, Mod: time.Now().UnixNano()}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{},
		subs: []convo.Subagent{sa}, subTails: map[string]*convo.Tail{"a1": st}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}

	out := ansi.Strip(strings.Join(m.runningPreview(c, c.runningSubs(), 100), "\n"))
	for _, w := range []string{"subagents ·1", "Explore", "find the pane", "› reading view.go", "‹ searching for", "↑ pick one to watch"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if strings.Contains(out, "alt") {
		t.Errorf("no alt keys:\n%s", out)
	}
	m.moveSel(c, -1)
	if c.sel != "run:a1" {
		t.Fatalf("↑ picked %q", c.sel)
	}
	m.paneKey(tea.KeyPressMsg{}, "space")
	m.drain(m.refreshSubs()) // the run opened is read in the background
	if c.subOpen != "a1" || m.viewName(c) != "subagents" {
		t.Fatalf("space: open %q in %s", c.subOpen, m.viewName(c))
	}
	// Watching, it's plain whose conversation this is, where typing goes,
	// and how to get back.
	banner := ansi.Strip(m.subBanner(c, 120))
	for _, w := range []string{"main session › Explore", "running", "2 steps", "▸✻ Explore", "shift+↑↓ siblings", "esc up"} {
		if !strings.Contains(banner, w) {
			t.Errorf("banner missing %q: %s", w, banner)
		}
	}
	m.paneKey(tea.KeyPressMsg{}, "left")
	if c.subOpen != "" || m.viewName(c) != "conversation" || c.sel != "run:a1" {
		t.Fatalf("←: open %q in %s on %q", c.subOpen, m.viewName(c), c.sel)
	}
	if m.subBanner(c, 120) != "" {
		t.Fatal("banner stays after leaving")
	}
	// esc leaves it too.
	m.paneKey(tea.KeyPressMsg{}, "space")
	c.sel = ""
	m.paneKey(tea.KeyPressMsg{}, "esc")
	if c.subOpen != "" || m.viewName(c) != "conversation" {
		t.Fatalf("esc: open %q in %s", c.subOpen, m.viewName(c))
	}

	// A card waiting sits just above the box, below the subagents: ↑ from
	// the box goes straight onto it, ↑ again to the subagent above, ↓ back
	// onto the card, and ↓ again to the box. It never opens as a modal.
	c.sess.Apply(host.Sent{Text: "go"}, time.Now())
	c.sess.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "Bash", Input: jsontext.Value(`{"command":"ls"}`)}}}, time.Now())
	c.sess.Apply(headless.PermissionRequest{ID: "r1", Tool: "Bash", ToolUseID: "b1"}, time.Now())
	key := func(s string) { m.paneKey(tea.KeyPressMsg{}, s) }
	c.sel = ""
	if m.cardModal(c) {
		t.Fatal("an agent's card shouldn't open as a modal")
	}
	key("up")
	if !c.cardFocus || c.sel != "" {
		t.Fatalf("↑ from the box: on %q, card %v", c.sel, c.cardFocus)
	}
	key("up")
	if c.cardFocus || c.sel != "run:a1" {
		t.Fatalf("↑ off the card: on %q, card %v", c.sel, c.cardFocus)
	}
	key("down")
	if !c.cardFocus || c.sel != "" {
		t.Fatalf("↓ onto the card: on %q, card %v", c.sel, c.cardFocus)
	}
	key("down")
	if c.cardFocus || c.sel != "" {
		t.Fatalf("↓ off the card: on %q, card %v", c.sel, c.cardFocus)
	}
}

// In the subagents view the run under the pointer lights up, the pointer
// says it can be clicked, and resting on one in the wide view shows its
// conversation beside the list in place of the picked one's.
func TestSubagentHover(t *testing.T) {
	dir := t.TempDir()
	write := func(id, said string) convo.Subagent {
		path := filepath.Join(dir, "agent-"+id+".jsonl")
		os.WriteFile(path, []byte(strings.Join([]string{
			`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"go"}}`,
			`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m` + id + `","role":"assistant","content":[{"type":"text","text":"` + said + `"}]}}`,
		}, "\n")+"\n"), 0o644)
		return convo.Subagent{ID: id, Type: "Explore", Description: "run " + id, Path: path, Mod: time.Now().UnixNano()}
	}
	subs := []convo.Subagent{write("a1", "first run speaking"), write("a2", "second run speaking")}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}, subs: subs,
		subTails: map[string]*convo.Tail{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}
	for i, v := range m.views(c) {
		if v == "subagents" {
			c.view = i
		}
	}
	c.sel = "sub:a2"
	c.rowRefs = []string{"", "", "", "sub:a2", "sub:a2", "sub:a1", "sub:a1"}

	if r := m.subHoverAt(10, 5); r != "sub:a1" {
		t.Fatalf("hover at a1's row: %q", r)
	}
	if r := m.subHoverAt(10, 1); r != "" {
		t.Fatalf("hover on the heading: %q", r)
	}
	if _, cmd := m.subMouseMove(10, 6); c.subHover != "sub:a1" || cmd == nil {
		t.Fatalf("moved onto a1: %q, rest tick %v", c.subHover, cmd != nil)
	}
	if changed, _ := m.subMouseMove(10, 5); changed {
		t.Fatal("another row of the same run is no change")
	}
	_, cmd := m.Update(tea.MouseMotionMsg{X: 10, Y: 5})
	if m.pointer != "pointer" || cmd == nil {
		t.Fatalf("pointer shape %q", m.pointer)
	}

	o := convo.Options{Width: 100, Now: time.Now(), Selected: c.sel, Focused: true}
	lines := m.subagentList(c, o)
	for _, l := range lines {
		if hovered := strings.Contains(l.Text, hoverBG); hovered != (l.Ref == "sub:a1") {
			t.Errorf("row of %q hovered=%v: %q", l.Ref, hovered, ansi.Strip(l.Text))
		}
	}
	// A run in a worktree of its own says which; the others say nothing.
	c.subWT = map[string]string{"a1": "worktree-agent-1"}
	said := false
	for _, l := range m.subagentList(c, o) {
		if has := strings.Contains(ansi.Strip(l.Text), "⎇ worktree-agent-1"); has && l.Ref != "sub:a1" {
			t.Errorf("row of %q says a1's worktree: %q", l.Ref, ansi.Strip(l.Text))
		} else if has {
			said = true
		}
	}
	if !said {
		t.Error("a1's worktree missing")
	}
	c.subWT = nil

	c.paneW = 160
	if r := m.subHoverAt(100, 5); r != "" {
		t.Fatalf("the conversation beside the list is no run's: %q", r)
	}
	o.Width = 160
	side := func() string {
		m.subPreviewLines(c, o, 30)
		// A run picked is read in the background, then drawn. The Update
		// above let go of c (the snapshot has no agent "k"), so it's put back.
		m.host, c.paneReading = c, false
		m.drain(m.refreshSubs())
		var b strings.Builder
		for _, l := range m.subPreviewLines(c, o, 30) {
			b.WriteString(ansi.Strip(l) + "\n")
		}
		return b.String()
	}
	if out := side(); !strings.Contains(out, "second run speaking") || strings.Contains(out, "first run speaking") {
		t.Fatalf("before resting, the picked run shows beside:\n%s", out)
	}
	c.subHoverAt = time.Now().Add(-time.Second)
	if out := side(); !strings.Contains(out, "first run speaking") {
		t.Fatalf("rested on a1, its conversation shows beside:\n%s", out)
	}

	m.host = c // Update let go of it, having no agent in the snapshot
	m.key(tea.KeyPressMsg{Code: tea.KeyDown})
	if c.subHover != "" {
		t.Fatal("the keyboard takes over from the pointer")
	}
}

// The Session's header has no SESSION label; filling the screen, it says how
// to get back to the list, and ends where the conversation does.
func TestPaneHeaderAlone(t *testing.T) {
	m, _ := benchModel(150, 30)
	m.full = true
	m.View()
	a := m.focused()
	head := m.paneHeader(a, m.host, 150)
	row1, row3 := ansi.Strip(head[paneTitleRow]), ansi.Strip(head[paneTabsRow])
	if strings.Contains(row1, "SESSION") {
		t.Errorf("label shows alone: %q", row1)
	}
	if !strings.Contains(row3, "esc back to the list") {
		t.Errorf("no way back: %q", row3)
	}
	if w := ansi.StringWidth(strings.TrimRight(row1, " ")); w > maxPane-3 {
		t.Errorf("header runs to %d, past the conversation", w)
	}
	m.full = false
	m.View()
	if m.listW == 0 {
		t.Fatal("no split at 150")
	}
	if head := ansi.Strip(strings.Join(m.paneHeader(a, m.host, 100), "\n")); strings.Contains(head, "SESSION") || strings.Contains(head, "esc back") {
		t.Errorf("beside the list:\n%s", head)
	}
}

// x on a running subagent's row, or ctrl+x while watching it, stops that
// one alone; ctrl+x elsewhere still stops the turn.
func TestStopOneSubagent(t *testing.T) {
	sa := convo.Subagent{ID: "a1", Type: "Explore", Description: "look", Mod: time.Now().UnixNano()}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}, subs: []convo.Subagent{sa}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}
	c.sel = "run:a1"
	if _, live, ok := m.pickedSub(c); !ok || !live {
		t.Fatalf("picked: ok %v live %v", ok, live)
	}
	if cmd := m.paneKey(tea.KeyPressMsg{Text: "x"}, "x"); cmd == nil || !strings.Contains(m.status, "stopping Explore") {
		t.Fatalf("x on the row: cmd %v, status %q", cmd != nil, m.status)
	}
	c.sess.TaskStatus["a1"] = "stopped"
	if _, live, _ := m.pickedSub(c); live {
		t.Fatal("stopped, it still counts as running")
	}
}

// The dock's running subagents are in the order they started, latest
// first, however they write; a few show at a time, scrolling to keep the
// pick in sight; and when the picked one finishes, the pick stays in its
// place, on the run now there.
func TestSubagentDockOrder(t *testing.T) {
	now := time.Now()
	var subs []convo.Subagent
	for i := range 7 {
		// Written to oldest-born last, so by writes they'd be backwards.
		subs = append(subs, convo.Subagent{ID: fmt.Sprintf("a%d", i), Type: "Explore", Description: fmt.Sprintf("run %d", i),
			Born: now.Add(time.Duration(i) * time.Second).UnixNano(), Mod: now.Add(-time.Duration(i) * time.Second).UnixNano()})
	}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}, subs: subs, subTails: map[string]*convo.Tail{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}

	var ids []string
	for _, sa := range c.runningSubs() {
		ids = append(ids, sa.ID)
	}
	if got := strings.Join(ids, " "); got != "a6 a5 a4 a3 a2 a1 a0" {
		t.Fatalf("order %s", got)
	}
	out := ansi.Strip(strings.Join(m.runningPreview(c, c.runningSubs(), 100), "\n"))
	if !strings.Contains(out, "run 2") || strings.Contains(out, "run 1") || !strings.Contains(out, "+ 2 older") {
		t.Fatalf("first five:\n%s", out)
	}
	// ↑ from the box lands on the oldest; the window follows it down.
	m.moveSel(c, -1)
	if c.sel != "run:a0" {
		t.Fatalf("↑ picked %q", c.sel)
	}
	out = ansi.Strip(strings.Join(m.runningPreview(c, c.runningSubs(), 100), "\n"))
	if !strings.Contains(out, "run 0") || strings.Contains(out, "run 5") || !strings.Contains(out, "… 2 newer") {
		t.Fatalf("scrolled:\n%s", out)
	}
	m.moveSel(c, -1)
	m.moveSel(c, -1)
	if c.sel != "run:a2" {
		t.Fatalf("↑↑ picked %q", c.sel)
	}
	// It finishes: the pick takes the run in its place.
	c.sess.TaskStatus["a2"] = "completed"
	c.runMemo.ok = false // as the notice's line, read in, would
	m.runningPreview(c, c.runningSubs(), 100)
	if c.sel != "run:a1" {
		t.Fatalf("after it finished, picked %q", c.sel)
	}
	// Another writing (and a new one starting) doesn't move the pick.
	c.subs[4].Mod = time.Now().UnixNano()
	c.subs = append(c.subs, convo.Subagent{ID: "a7", Type: "Explore", Born: now.Add(time.Minute).UnixNano(), Mod: now.UnixNano()})
	m.runningPreview(c, c.runningSubs(), 100)
	if c.sel != "run:a1" {
		t.Fatalf("after a reshuffle, picked %q", c.sel)
	}
}

// A subagent launched in the background is a task to Claude Code, but it
// belongs in the subagents view, not the background one; and while its task
// runs it shows as running there and in the dock, however long it has been
// quiet (a long command, a long think), until Claude Code says it's done.
func TestBackgroundSubagentIsASubagent(t *testing.T) {
	path := filepath.Join(t.TempDir(), "agent-a1.jsonl")
	os.WriteFile(path, []byte(`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"go"}}`+"\n"), 0o644)
	now := time.Now()
	s := convo.New()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, State: "idle"}}, now)
	s.Apply(headless.TaskStarted{ID: "a1", ToolUseID: "tA", Type: "local_agent", Description: "look around", SubagentType: "Explore", Backgrounded: true}, now.Add(-10*time.Minute))
	quiet := convo.Subagent{ID: "a1", Type: "Explore", Description: "look around", ToolUseID: "tA", Path: path, Mod: now.Add(-5 * time.Minute).UnixNano()}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{},
		subs: []convo.Subagent{quiet}, subTails: map[string]*convo.Tail{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}

	if slices.Contains(m.views(c), "background") {
		t.Fatalf("a subagent alone makes a background view: %v", m.views(c))
	}
	if run := c.runningSubs(); len(run) != 1 {
		t.Fatalf("a quiet run whose task runs isn't running: %v", run)
	}
	list := func() string {
		var b strings.Builder
		for _, l := range m.subagentList(c, convo.Options{Width: 100, Now: now}) {
			b.WriteString(ansi.Strip(l.Text) + "\n")
		}
		return b.String()
	}
	if out := list(); !strings.Contains(out, "1 running") || !strings.Contains(out, "look around") {
		t.Fatalf("the subagents view doesn't show it running:\n%s", out)
	}

	// A shell alongside: the background view has it, and not the subagent.
	s.Apply(headless.TaskStarted{ID: "b1", ToolUseID: "tS", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now)
	if !slices.Contains(m.views(c), "background") {
		t.Fatal("no background view for a shell")
	}
	var b strings.Builder
	for _, l := range m.jobLines(c, convo.Options{Width: 100, Now: now}) {
		b.WriteString(ansi.Strip(l.Text) + "\n")
	}
	if out := b.String(); !strings.Contains(out, "npm run dev") || strings.Contains(out, "look around") || !strings.Contains(out, "1 tasks") {
		t.Fatalf("background view:\n%s", out)
	}

	// Claude Code says it finished: done, though it wrote just now.
	s.Apply(headless.TaskDone{ID: "a1", ToolUseID: "tA", Status: "completed"}, now)
	c.subs[0].Mod = time.Now().UnixNano()
	if run := c.runningSubs(); len(run) != 0 {
		t.Fatalf("a finished run still running: %v", run)
	}
	if out := list(); strings.Contains(out, "running") || !strings.Contains(out, "completed") {
		t.Fatalf("finished run:\n%s", out)
	}
}

// A fork (or a rewind, or /clear) goes on under another session id than
// the one its pane opened on: the subagents are looked for under the
// conversation Claude Code writes now, so they show as they start.
func TestSubagentsFollowTheSessionID(t *testing.T) {
	cfg := t.TempDir()
	acct := claude.Account{Name: "x", ConfigDir: cfg}
	old := acct.TranscriptPath("/w", "old")
	subs := filepath.Join(strings.TrimSuffix(acct.TranscriptPath("/w", "new"), ".jsonl"), "subagents")
	os.MkdirAll(subs, 0o755)
	os.WriteFile(filepath.Join(subs, "agent-a1.meta.json"), []byte(`{"agentType":"Explore","description":"look","toolUseId":"tA"}`), 0o644)
	os.WriteFile(filepath.Join(subs, "agent-a1.jsonl"), []byte("{}\n"), 0o644)

	s := convo.New()
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}, path: old}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{{Key: "k", Acct: acct.Profile()}}}, host: c}
	m.drain(m.refreshSubs())
	if len(c.subs) != 0 {
		t.Fatalf("found runs under the old id: %v", c.subs)
	}
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, SessionID: "new", Cwd: "/w"}}, time.Now())
	m.drain(m.refreshSubs())
	if len(c.subs) != 1 || c.subs[0].ID != "a1" || c.path != acct.TranscriptPath("/w", "new") {
		t.Fatalf("after the id changed: path %s, runs %v", c.path, c.subs)
	}
}

// A Claude Code session rush doesn't run has no task events: its
// transcript says which runs are working. One quiet for minutes (a long
// command, a long think) whose call has no answer yet is running, in the
// dock and the view, until it answers.
func TestQuietSubagentOfATerminalSession(t *testing.T) {
	dir := t.TempDir()
	main := filepath.Join(dir, "s.jsonl")
	subs := filepath.Join(dir, "s", "subagents")
	os.MkdirAll(subs, 0o755)
	os.WriteFile(filepath.Join(subs, "agent-a1.meta.json"), []byte(`{"agentType":"Explore","description":"dig","toolUseId":"tA"}`), 0o644)
	run := filepath.Join(subs, "agent-a1.jsonl")
	os.WriteFile(run, []byte(`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"dig"}}`+"\n"), 0o644)
	old := time.Now().Add(-6 * time.Minute)
	os.Chtimes(run, old, old)
	os.WriteFile(main, []byte(`{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tA","name":"Agent","input":{"description":"dig"}}]}}`+"\n"), 0o644)

	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}, path: main}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	m.drain(m.refreshSubs())
	if len(c.subs) != 1 || len(c.runningSubs()) != 1 {
		t.Fatalf("runs %v, running %v", c.subs, c.runningSubs())
	}
	// Its process exits with the run unfinished: the run ended with it.
	m.snap.Agents = []*fleet.Agent{{Key: "k", Interactive: true}}
	m.drain(m.refreshSubs())
	if st, live := c.subState(c.subs[0]); live || st != "ended" {
		t.Fatalf("process gone: %q live %v", st, live)
	}
	m.snap.Agents[0].PID = 42
	f, _ := os.OpenFile(main, os.O_APPEND|os.O_WRONLY, 0o644)
	f.WriteString(`{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tA","content":"dug"}]}}` + "\n")
	f.Close()
	m.drain(m.refreshSubs())
	if st, live := c.subState(c.subs[0]); live || st != "completed" {
		t.Fatalf("after its answer: %q live %v", st, live)
	}
}

// A subagent run in the background returns its call at once: watching
// it, its turn stays open while its task runs, rather than each line it
// writes starting a turn of its own, and ends with the task.
func TestBackgroundSubagentStaysOneTurn(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{},
		subs: []convo.Subagent{{ID: "a1", ToolUseID: "ag1"}}, subOpen: "a1", subTail: convo.SubagentTail("")}
	now := time.Now()
	for _, l := range []struct{ role, msg string }{
		{"assistant", `{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"ag1","name":"Agent","input":{"prompt":"go","run_in_background":true}}]}`},
		{"user", `{"role":"user","content":[{"type":"tool_result","tool_use_id":"ag1","content":"started"}]}`},
	} {
		ev, err := headless.DecodeMessage(l.role, []byte(l.msg), nil)
		if err != nil {
			t.Fatal(err)
		}
		c.sess.Apply(ev, now)
	}
	c.sess.Apply(event.TaskStarted{ID: "a1", CallID: "ag1", Kind: event.SubagentTask, Background: true}, now)
	c.subTail.Sess.Apply(host.Sent{Text: "go"}, now)
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	m.readSub()
	if c.subTail.Sess.Live() == nil {
		t.Fatal("the turn ended while its task runs")
	}
	c.sess.Apply(event.TaskUpdated{ID: "a1", Status: "completed"}, now)
	m.readSub()
	if c.subTail.Sess.Live() != nil {
		t.Error("the turn outlived its task")
	}
}

// A task a running subagent started hangs under it in the dock, not in the
// tasks card, and ↑ goes from the subagent onto it; one the session started
// says whose it is after its command.
func TestSubagentsTasksUnderIt(t *testing.T) {
	path := filepath.Join(t.TempDir(), "agent-a1.jsonl")
	os.WriteFile(path, []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"merge main"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t9","name":"Bash","input":{"command":"cd /r/.worktrees/merge-0930 && git diff --stat","run_in_background":true}}]}}`,
	}, "\n")+"\n"), 0o644)
	st := convo.SubagentStats(path)
	st.Read()
	sa := convo.Subagent{ID: "a1", Type: "lane-opus", Description: "merge main", Path: path, Mod: time.Now().UnixNano()}
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "b9", ToolUseID: "t9", Type: "local_bash", Description: "git diff --stat", Backgrounded: true}, now.Add(-time.Minute))
	s.Apply(headless.TaskStarted{ID: "b1", ToolUseID: "t1", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now.Add(-time.Minute))
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{},
		subs: []convo.Subagent{sa}, subTails: map[string]*convo.Tail{"a1": st}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}

	if loose := c.looseJobs(); len(loose) != 1 || loose[0].ID != "b1" {
		t.Fatalf("loose: %v", loose)
	}
	out := ansi.Strip(strings.Join(m.runningPreview(c, c.runningSubs(), 110), "\n"))
	if !strings.Contains(out, "git diff --stat") {
		t.Fatalf("its task isn't under it:\n%s", out)
	}
	if got := ansi.Strip(jobWho(c, s.Job("b9"))); got != "  → lane-opus(merge-0930)" {
		t.Fatalf("whose: %q", got)
	}
	m.moveSel(c, -1)
	m.moveSel(c, -1)
	if c.sel != "job:b9" {
		t.Fatalf("↑↑ picked %q", c.sel)
	}
	m.moveSel(c, -1)
	if c.sel != "run:a1" {
		t.Fatalf("↑↑↑ picked %q", c.sel)
	}
}
