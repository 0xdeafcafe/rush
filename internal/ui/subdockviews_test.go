package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The subagents sit right of the conversation's text only when the text
// leaves them room once the session's views take theirs: with the views
// open they'd be drawn over the text, so they go under it instead.
func TestSubagentDockMakesRoomForViews(t *testing.T) {
	path := filepath.Join(t.TempDir(), "agent-a1.jsonl")
	os.WriteFile(path, []byte(`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find the pane"}}`+"\n"), 0o644)
	st := convo.SubagentStats(path)
	st.Read()
	sa := convo.Subagent{ID: "a1", Type: "Explore", Description: "find the pane", Path: path, Mod: time.Now().UnixNano()}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{},
		subs: []convo.Subagent{sa}, subTails: map[string]*convo.Tail{"a1": st}}
	c.sess.Info.ID = "s1"
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, host: c, paneFocus: true, w: 240, h: 60}
	a := &fleet.Agent{Key: "k", Rush: true}
	if len(c.dockSubs()) == 0 {
		t.Fatal("no running subagent to dock")
	}

	m.paneDock(a, c, 200, 60)
	if c.sideW == 0 {
		t.Fatalf("no views, 200 wide: the subagents should sit beside the text")
	}
	m.agentViews = map[string]*sessionViews{"s1": {checked: time.Now(), list: []host.View{{Name: "prs"}}}}
	m.paneDock(a, c, 200, 60)
	if c.sideW != 0 {
		t.Fatalf("with views beside a 200-wide pane, the subagents sat %d wide over the text", c.sideW)
	}
}

// With the session's panel open, its subagents show at the panel's top,
// what each is doing under its name, and leave the dock; ↑ from the box
// still picks one, lit in the panel, and a click on one watches it.
func TestSubagentsInPanel(t *testing.T) {
	path := filepath.Join(t.TempDir(), "agent-a1.jsonl")
	os.WriteFile(path, []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find the pane"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:03Z","message":{"id":"m2","role":"assistant","content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/w/view.go"}}]}}`,
	}, "\n")+"\n"), 0o644)
	st := convo.SubagentStats(path)
	st.Read()
	sa := convo.Subagent{ID: "a1", Type: "Explore", Description: "find the pane", Path: path, Mod: time.Now().UnixNano()}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{},
		subs: []convo.Subagent{sa}, subTails: map[string]*convo.Tail{"a1": st}}
	c.sess.Info.ID = "s1"
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, host: c, paneFocus: true, w: 240, h: 60}
	a := &fleet.Agent{Key: "k", Rush: true}

	// No views: the subagents stay in the dock.
	dock := ansi.Strip(strings.Join(m.paneDock(a, c, 160, 60), "\n"))
	if c.subsInPanel || !strings.Contains(dock, "Subagents") {
		t.Fatalf("no views, yet the subagents left the dock:\n%s", dock)
	}
	m.agentViews = map[string]*sessionViews{"s1": {checked: time.Now(), list: []host.View{{Name: "prs", Title: "Pull requests", Updated: time.Now()}}}}
	dock = ansi.Strip(strings.Join(m.paneDock(a, c, 160, 60), "\n"))
	if !c.subsInPanel || strings.Contains(dock, "Subagents") {
		t.Fatalf("with the panel open the subagents stayed in the dock:\n%s", dock)
	}
	c.views.live = m.panelSubs(c, 44)
	draw := func() string {
		c.views.x, c.views.y = 0, 0
		return ansi.Strip(strings.Join(viewsPanel(m.viewsOf("s1"), nil, 48, 20, time.Now(), &c.views), "\n"))
	}
	out := draw()
	for _, w := range []string{"▾ Subagents  1 working", "find the pane", "› reading view.go", "Pull requests"} {
		if !strings.Contains(out, w) {
			t.Errorf("no %q in the panel:\n%s", w, out)
		}
	}
	m.moveSel(c, -1)
	if c.sel != "run:a1" {
		t.Fatalf("↑ from the box picked %q", c.sel)
	}
	c.sel = ""
	// The run's name is the body's second row, under a blank and the heading.
	m.clickViews(c, m.paneX()+5, 3)
	if c.subOpen != "a1" || m.viewName(c) != "subagents" {
		t.Fatalf("a click on it: open %q in %s", c.subOpen, m.viewName(c))
	}
}

// A shell running in the background goes to the panel's top with the
// subagents while the panel is open, its latest output under it, and
// leaves the dock; ↑ picks it and a click picks it, its keys under it.
func TestShellsInPanel(t *testing.T) {
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{ID: "s1", Proto: 3, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, host: c, paneFocus: true, w: 240, h: 60}
	a := &fleet.Agent{Key: "k", Rush: true}
	if len(c.looseJobs()) != 1 {
		t.Fatal("no running shell")
	}
	dock := ansi.Strip(strings.Join(m.paneDock(a, c, 160, 60), "\n"))
	if c.subsInPanel || !strings.Contains(dock, "npm run dev") {
		t.Fatalf("no views, yet the shell left the dock:\n%s", dock)
	}
	m.agentViews = map[string]*sessionViews{"s1": {checked: now, list: []host.View{{Name: "prs", Title: "Pull requests", Updated: now}}}}
	dock = ansi.Strip(strings.Join(m.paneDock(a, c, 160, 60), "\n"))
	if !c.subsInPanel || strings.Contains(dock, "npm run dev") {
		t.Fatalf("with the panel open the shell stayed in the dock:\n%s", dock)
	}
	c.views.live = m.panelSubs(c, 44)
	draw := func() string {
		c.views.x, c.views.y = 0, 0
		return ansi.Strip(strings.Join(viewsPanel(m.viewsOf("s1"), nil, 48, 20, time.Now(), &c.views), "\n"))
	}
	out := draw()
	for _, w := range []string{"▾ Shells  1 running", "shell npm run dev", "Pull requests"} {
		if !strings.Contains(out, w) {
			t.Errorf("no %q in the panel:\n%s", w, out)
		}
	}
	m.moveSel(c, -1)
	if c.sel != "job:b2" {
		t.Fatalf("↑ from the box picked %q", c.sel)
	}
	c.sel = ""
	m.clickViews(c, m.paneX()+5, 3)
	if c.sel != "job:b2" {
		t.Fatalf("a click on it picked %q", c.sel)
	}
	c.views.live = m.panelSubs(c, 44)
	if out := draw(); !strings.Contains(out, "stop") {
		t.Errorf("picked, its keys don't show:\n%s", out)
	}
}
