package remote

import (
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/host"
)

// TestBrowserFixture is not a test: with RUSH_FIXTURE=<dir> it serves the
// web app on 127.0.0.1:<RUSH_FIXTURE_PORT> over fake hosts in an isolated
// home, with the ops they receive logged to <dir>/ops.log, until killed. A
// browser driver then exercises the controls with no real agent behind them.
// Token: "fixture" + 57 x. Sessions: one with an approval and a question
// pending after a short conversation, one that has nothing, one working, and
// 9e9e9e9e: stopped, in a git repository with a.txt changed and new.txt
// untracked. An SSH key is paired besides the browser that signs in.
func TestBrowserFixture(t *testing.T) {
	dir := os.Getenv("RUSH_FIXTURE")
	if dir == "" {
		t.Skip("a fixture for browser runs: RUSH_FIXTURE=dir")
	}
	shortHome(t)
	if err := UpdateConfig(func(c *Config) { c.Web = true }); err != nil {
		t.Fatal(err)
	}
	talk := []event.Event{
		event.Message{Role: "user", Parts: []event.Part{{Kind: event.Text, Text: "The build is failing on main, can you look?"}}},
		event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: "Let me run the build and see what breaks."},
			{Kind: event.ToolCall, Call: &tool.Call{ID: "t1", Name: "Bash", Input: tool.Input{Command: "go build ./...", Description: "Build everything"}}}}},
		event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: "t1", Text: "./main.go:12:2: undefined: run"}}}},
		event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: "`run` was renamed to `start` in the last commit. I'll fix the call, then clear the stale build folder."},
			{Kind: event.ToolCall, Call: &tool.Call{ID: "t2", Name: "Edit", Input: tool.Input{Path: "/work/a/main.go", Edits: []tool.Replace{{Old: "\trun()", New: "\tstart()"}}}}}}},
		event.Plan{Todos: []tool.TodoItem{{Label: "Find the failure", Status: "completed"}, {Label: "Fix the call", Status: "in_progress"}, {Label: "Clear build/", Status: "pending"}}},
	}
	var replay [][]byte
	for _, ev := range talk {
		replay = append(replay, evLine(t, ev))
	}
	a := startFakeHost(t, host.Root(), "a1a1a1a1", "needs you", "/Users/fixture/src/rush", append(replay, approvalLine(t, "ap1"), questionLine(t, "q1"))...)
	b := startFakeHost(t, host.Root(), "b2b2b2b2", "quiet", "/work/b")
	c := startFakeHost(t, host.Root(), "c3c3c3c3", "tidy the router", "/Users/fixture/src/site/web")
	c.info.State, c.info.Kind, c.info.Model, c.info.CostUSD = "working", "codex", "gpt-6", 0.42
	c.info.UpdatedAt = time.Now().Add(-3 * time.Minute)
	c.writeInfo(t, filepath.Join(host.Root(), "c3c3c3c3"))
	gitRepo(t) // session 9e9e9e9e, stopped, in a repository with changes
	log, _ := os.Create(dir + "/ops.log")
	for _, f := range []*fakeHost{a, b, c} {
		go func() {
			for l := range f.ops {
				log.WriteString(f.id + " " + string(l) + "\n")
			}
		}()
	}
	// a's info says it's blocked, as a host waiting on a decision would
	a.info.State, a.info.Needs, a.info.Model, a.info.CostUSD = "blocked", "approval", "claude-opus-5-5", 1.87
	a.writeInfo(t, filepath.Join(host.Root(), "a1a1a1a1"))
	tok := "fixture" + strings.Repeat("x", 57)
	s := &Server{Config: Config{Name: "fixture"}, Token: tok, Start: func(r StartRequest) (host.Info, error) {
		raw, _ := jsonx.Marshal(r)
		log.WriteString("start " + string(raw) + "\n")
		return b.info, nil
	}}
	s.devs.add(Device{Kind: "ssh", Name: "laptop · ed25519", Fingerprint: "SHA256:fixture"})
	ln, err := net.Listen("tcp", "127.0.0.1:"+os.Getenv("RUSH_FIXTURE_PORT"))
	if err != nil {
		t.Fatal(err)
	}
	go http.Serve(ln, s.Handler())
	os.WriteFile(dir+"/ready", nil, 0o600)
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGTERM, os.Interrupt)
	<-sig
}
