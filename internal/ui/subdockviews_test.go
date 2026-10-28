package ui

import (
	"os"
	"path/filepath"
	"testing"
	"time"

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
