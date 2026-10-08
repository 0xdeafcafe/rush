package ui

import (
	"bufio"
	"errors"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// stubRuns says whether every run is working, and nothing else.
type stubRuns struct {
	agent.SubagentRuns
	live bool
}

func (r *stubRuns) Gone() bool { return false }
func (r *stubRuns) State(string, string) (agent.RunState, string, time.Time) {
	return agent.RunRunning, "", time.Time{}
}
func (r *stubRuns) Going(string, string, time.Time, time.Time) (bool, string) { return r.live, "done" }

func TestSubQueue(t *testing.T) {
	runs := &stubRuns{live: true}
	sa := convo.Subagent{ID: "s1", Type: "lane"}
	c := &hostConn{key: "k", client: &host.Client{}, sess: convo.New(), subs: []convo.Subagent{sa}, subRuns: runs}
	m := &Model{host: c}
	q := m.localQueueOf(subQKey("k", "s1"))
	m.queueSub(c, q, sa, "one")
	m.queueSub(c, q, sa, "two")

	if m.flushSubQueues(); len(q.items) != 1 || q.items[0] != "two" {
		t.Fatalf("the first goes in at once, for the end of the step it's on: %v", q.items)
	}
	if m.flushSubQueues(); len(q.items) != 1 {
		t.Fatal("the next waits for the next step")
	}
	q.at-- // it took one
	if m.flushSubQueues(); len(q.items) != 0 {
		t.Fatalf("one message a step: %v", q.items)
	}
	m.queueSub(c, q, sa, "three")
	if m.flushSubQueues(); len(q.items) != 1 {
		t.Fatal("one sent on this step already: the next waits")
	}
	runs.live = false
	if m.flushSubQueues(); m.localQ[subQKey("k", "s1")] != nil {
		t.Fatal("a finished run's queue goes to the main session")
	}
}

// fakeHost listens where session id's host would, and passes on each op
// a client sends it.
func fakeHost(t *testing.T, id string) (*host.Client, chan string) {
	home, _ := os.MkdirTemp("/tmp", "rush-ui-") // short enough for a socket
	t.Cleanup(func() { os.RemoveAll(home) })
	t.Setenv("RUSH_HOME", home)
	_ = os.MkdirAll(filepath.Dir(host.SockPath(id)), 0o700)
	ln, err := net.Listen("unix", host.SockPath(id))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	ops := make(chan string, 16)
	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		for sc := bufio.NewScanner(conn); sc.Scan(); {
			if !strings.Contains(sc.Text(), `"hello"`) {
				ops <- sc.Text()
			}
		}
	}()
	cl, err := host.Dial(id)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cl.Close() })
	return cl, ops
}

// watching is a session rush runs, watching its running subagent a1, whose
// transcript has lines written to it.
func watching(t *testing.T, inbox bool) (*Model, *hostConn, chan string, string) {
	cl, ops := fakeHost(t, "h1")
	main := filepath.Join(t.TempDir(), "s.jsonl")
	_ = os.WriteFile(main, nil, 0o644)
	path := filepath.Join(filepath.Dir(main), "s", "subagents", "agent-a1.jsonl")
	_ = os.MkdirAll(filepath.Dir(path), 0o755)
	_ = os.WriteFile(strings.TrimSuffix(path, ".jsonl")+".meta.json", []byte(`{"agentType":"Explore"}`), 0o644)
	_ = os.WriteFile(path, []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find the pane"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"previewLines"}}]}}`,
	}, "\n")+"\n"), 0o644)
	sa := convo.Subagent{ID: "a1", Type: "Explore", Path: path, Mod: time.Now().UnixNano()}
	c := &hostConn{kind: "claude", key: "k", client: cl, sess: convo.New(), open: map[string]bool{}, path: main,
		subs: []convo.Subagent{sa}, subTails: map[string]*convo.Tail{}, subRuns: &stubRuns{live: true}}
	c.sess.Info.Inbox = inbox
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}
	for i, v := range m.views(c) {
		if v == "subagents" {
			c.view = i
		}
	}
	m.openSub(c, "a1")
	m.drain(m.refreshSubs())
	return m, c, ops, path
}

// runAll runs cmd and what it batches, and hands back their messages.
func runAll(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	if b, ok := cmd().(tea.BatchMsg); ok {
		var out []tea.Msg
		for _, c := range b {
			out = append(out, runAll(c)...)
		}
		return out
	}
	return []tea.Msg{cmd()}
}

func next(t *testing.T, ops chan string) string {
	select {
	case op := <-ops:
		return op
	case <-time.After(2 * time.Second):
		t.Fatal("the host got nothing")
		return ""
	}
}

// What you send a subagent you're watching stays in its view: in its
// queue, then as sent, then as yours in its conversation once it has read
// it. It goes into its inbox at once, not only after its next step.
func TestSubagentMessageStaysInSight(t *testing.T) {
	m, c, ops, path := watching(t, true)
	c.input = []rune("look in view.go")
	m.drain(m.sendPane(c, false))
	if q := m.queueOf(c); len(q.items) != 1 {
		t.Fatalf("not queued: %+v", q)
	}
	for _, msg := range runAll(m.flushSubQueues()) {
		m.onSubSent(msg.(subSentMsg))
	}
	if op := next(t, ops); !strings.Contains(op, `"op":"tell"`) || !strings.Contains(op, `"id":"a1"`) {
		t.Fatalf("host got %s", op)
	}
	if l := ansi.Strip(strings.Join(m.subNoteLines(c, 120), "\n")); !strings.Contains(l, "look in view.go · in its inbox") {
		t.Fatalf("sent, not shown: %q", l)
	}
	note, _ := jsonx.Marshal(host.TellNote + "look in view.go")
	f, _ := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	_, _ = f.WriteString(`{"type":"attachment","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","attachment":{"type":"hook_additional_context","content":[` + string(note) + `],"hookEvent":"PostToolUse"}}` + "\n")
	_ = f.Close()
	m.drain(m.refreshSubs())
	if l := m.subNoteLines(c, 120); len(l) != 0 || !said(c.subTail.Sess, "look in view.go") {
		t.Fatalf("read, but its conversation doesn't say so: notes %q", l)
	}
}

// With a host from before the inbox, the main session passes it on, into
// its turn rather than its queue, and the subagent's view says so.
func TestSubagentMessageRelayed(t *testing.T) {
	m, c, ops, _ := watching(t, false)
	c.input = []rune("look in view.go")
	for _, msg := range runAll(m.sendPane(c, false)) {
		m.onSubSent(msg.(subSentMsg))
	}
	if op := next(t, ops); !strings.Contains(op, `"guide":true`) || !strings.Contains(op, "Pass this message on to your running subagent a1") {
		t.Fatalf("host got %s", op)
	}
	if l := ansi.Strip(strings.Join(m.subNoteLines(c, 120), "\n")); !strings.Contains(l, "look in view.go · the main session passes it on") {
		t.Fatalf("relayed, not shown: %q", l)
	}
}

// A message that couldn't go is kept: a told one back in its queue, any
// other back in the box.
func TestSubagentMessageKept(t *testing.T) {
	c := &hostConn{key: "k", sess: convo.New()}
	m := &Model{host: c}
	m.onSubSent(subSentMsg{key: "k", sub: "a1", text: "one", told: true, err: errors.New("gone")})
	if q := m.localQ[subQKey("k", "a1")]; q == nil || len(q.items) != 1 {
		t.Fatal("a told one isn't back in its queue")
	}
	m.onSubSent(subSentMsg{key: "k", sub: "a1", text: "two", err: errors.New("gone")})
	if string(c.input) != "two" || !m.statusErr {
		t.Fatalf("box %q", string(c.input))
	}
}
