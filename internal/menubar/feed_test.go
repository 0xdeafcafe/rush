package menubar

import (
	"bufio"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

const (
	lineQuestion = `{"type":"control_request","request_id":"req-q","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database?","header":"Storage","multiSelect":false,"options":[{"label":"Postgres (Recommended)","description":"d"},{"label":"SQLite","description":"d","preview":"file.db"}]}]},"tool_use_id":"toolu_q"}}`
	lineOld      = `{"type":"control_request","request_id":"req-old","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"toolu_o"}}`
	lineAnswered = `{"type":"agtop_answered","request_id":"req-old"}`
	lineInfo     = `{"type":"agtop_info","info":{"id":"h1","state":"blocked","needs":"asks: Which database?"}}`
)

// fakeHost serves a replay like a session's host does, and hands back the
// ops a client writes.
func fakeHost(t *testing.T, id string, replay ...string) <-chan map[string]any {
	t.Helper()
	home, err := os.MkdirTemp("/tmp", "rush-mb") // unix socket paths are short
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(home) })
	t.Setenv("RUSH_HOME", home)
	_ = os.MkdirAll(filepath.Dir(host.SockPath(id)), 0o700)
	ln, err := net.Listen("unix", host.SockPath(id))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	ops := make(chan map[string]any, 8)
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				defer c.Close()
				for _, l := range replay {
					c.Write([]byte(l + "\n"))
				}
				sc := bufio.NewScanner(c)
				for sc.Scan() {
					var o map[string]any
					_ = jsonx.Unmarshal(sc.Bytes(), &o)
					if o["op"] == "hello" {
						continue // what every client says first
					}
					ops <- o
				}
			}()
		}
	}()
	return ops
}

func TestQuestionAnsweredFromOutside(t *testing.T) {
	ops := fakeHost(t, "h1", lineOld, lineQuestion, lineAnswered, lineInfo)
	a := &fleet.Agent{ID: "h1", State: "blocked", Needs: "asks: Which database?", Key: "acct/a:h1", Rush: true, PID: 1, DisplayName: "db"}
	asked := map[string]*pending{}
	s := build(&fleet.Snapshot{Agents: []*fleet.Agent{a}}, asked)
	if len(s.Waiting) != 1 {
		t.Fatalf("waiting = %+v", s.Waiting)
	}
	w := s.Waiting[0]
	if w.Kind != "question" || w.Req != "req-q" || w.Text != "Which database?" || w.Header != "Storage" || len(w.Options) != 2 {
		t.Fatalf("wait = %+v", w)
	}
	if err := do(Op{Op: "answer", Key: w.Key, Req: w.Req, Answer: "SQLite"}, asked[w.Key]); err != nil {
		t.Fatal(err)
	}
	select {
	case o := <-ops:
		in, _ := o["input"].(map[string]any)
		ans, _ := in["answers"].(map[string]any)
		notes, _ := in["annotations"].(map[string]any)
		if o["op"] != "allow" || o["id"] != "req-q" || ans["Which database?"] != "SQLite" || notes["Which database?"] == nil {
			t.Fatalf("op = %v", o)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no op reached the host")
	}
	// A stale answer, for a request already settled, goes nowhere.
	if err := do(Op{Op: "answer", Key: w.Key, Req: "req-old", Answer: "x"}, asked[w.Key]); err != errGone {
		t.Fatalf("stale answer: %v", err)
	}
	// Once it isn't waiting, it's forgotten.
	a.State = "working"
	build(&fleet.Snapshot{Agents: []*fleet.Agent{a}}, asked)
	if len(asked) != 0 {
		t.Fatalf("asked kept %v", asked)
	}
}

func TestPermissionAndLimit(t *testing.T) {
	fakeHost(t, "h2", lineOld, `{"type":"agtop_info","info":{"id":"h2","state":"blocked","limit":{"ask":true}}}`)
	a := &fleet.Agent{ID: "h2", State: "blocked", Needs: "continue?", Key: "acct/a:h2", Rush: true, PID: 1}
	s := build(&fleet.Snapshot{Agents: []*fleet.Agent{a}}, map[string]*pending{})
	if w := s.Waiting[0]; w.Kind != "limit" {
		t.Fatalf("wait = %+v", w)
	}
	// A Claude Code agent's question is shown but can't be answered here.
	b := &fleet.Agent{Job: agent.Job{ID: "j", State: "blocked", Needs: "pick one"}, Key: "acct/j", PID: 1}
	s = build(&fleet.Snapshot{Agents: []*fleet.Agent{b}}, map[string]*pending{})
	if w := s.Waiting[0]; w.Kind != "" || w.Needs != "pick one" {
		t.Fatalf("wait = %+v", w)
	}
}

func TestWindowsAreAQuotasFirstTwo(t *testing.T) {
	q := usage.Quota{Windows: []usage.Window{
		{ID: "primary", Label: "7d", Name: "weekly", Percent: 40},
		{ID: "gpt-reserve:primary", Label: "gpt 7d", Percent: 90, Scope: usage.Scope{Models: []string{"gpt"}}},
	}}
	first, second := windows(q)
	if !first.Present || first.Label != "7d" || first.Percent != 40 {
		t.Errorf("first = %+v", first)
	}
	if second.Present {
		t.Errorf("a single model's limit took the second place: %+v", second)
	}
}
