package host

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// Claude Code killed mid-turn (the OOM killer's SIGKILL, say) ends the
// turn with why, and what was queued behind it resumes the conversation.
func TestKilledClaudeEndsTheTurnAndSendsTheQueue(t *testing.T) {
	bin := setup(t)
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Prompt: "say hi", Binary: bin})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	defer c.Stop()

	pid := next(t, c, inState("blocked")).(InfoEvent).Info.ClaudePID
	if pid == 0 {
		t.Fatal("no claude pid while blocked")
	}
	if err := c.Send("then this"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && len(i.Info.Queue) == 1 })
	if err := syscall.Kill(pid, syscall.SIGKILL); err != nil {
		t.Fatal(err)
	}
	end := next(t, c, func(ev any) bool { e, ok := ev.(event.TurnEnd); return ok && e.Err != "" }).(event.TurnEnd)
	if !strings.Contains(end.Err, "exited mid-turn") {
		t.Errorf("turn end: %+v", end)
	}
	next(t, c, func(ev any) bool { s, ok := ev.(Sent); return ok && s.Text == "then this" })
	info := next(t, c, inState("blocked")).(InfoEvent).Info
	if info.ClaudePID == 0 || info.ClaudePID == pid || len(info.Queue) != 0 {
		t.Errorf("after the queue went: %+v", info)
	}
	args, _ := os.ReadFile(filepath.Join(filepath.Dir(bin), "args.log"))
	if !strings.Contains(string(args), "--resume "+cfg.SessionID) {
		t.Errorf("the queue didn't resume the conversation:\n%s", args)
	}
}

// An agent being rested that picks up work of its own (a background
// task's notification) as it goes says so, but doesn't set the session
// working: nothing would ever end that turn. Once it has gone the turn it
// began is ended, saying why.
func TestStoppedAgentStartingATurnDoesNotStickWorking(t *testing.T) {
	setup(t)
	s := &server{cfg: Config{ID: "cut"}, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	if err := os.MkdirAll(dir("cut"), 0o700); err != nil {
		t.Fatal(err)
	}
	old := &fakeConn{events: make(chan event.Event, 4)}
	s.mu.Lock()
	s.conn = old
	s.info.State = "idle"
	s.detach() // the idle stop or a move to another account
	s.mu.Unlock()

	old.events <- event.Message{Role: "assistant", ID: "m9", Parts: []event.Part{{Kind: event.Text, Text: "Monitor expired, checking CI"}}}
	_ = old.Close()
	s.watchAgent(old)

	s.mu.Lock()
	defer s.mu.Unlock()
	if s.info.State != "idle" || !strings.Contains(s.info.Error, "picked up work of its own") {
		t.Fatalf("after the stopped agent went: state %q, error %q", s.info.State, s.info.Error)
	}
	var ended bool
	for _, l := range s.ring {
		ended = ended || isEventLine(l, "turn_end")
	}
	if !ended {
		t.Error("the conversation has no end for the turn it began")
	}
}

// deadClaude dies on its first turn, leaving a child that holds its output
// open, so its host never reads to the end; resumed, it answers.
const deadClaude = `#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/args.log"
resumed=; for a in "$@"; do [ "$a" = --resume ] && resumed=1; done
echo '{"type":"system","subtype":"init","session_id":"SID","model":"claude-haiku-4-5"}'
while read -r line; do
  case "$line" in
  *'"type":"user"'*)
    if [ -z "$resumed" ]; then
      echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"working"}]}}'
      sleep 60 &
      kill -9 $$
    fi
    echo '{"type":"result","subtype":"success","result":"Resumed."}' ;;
  esac
done
`

// A turn whose agent is gone while its output never ends is ended by the
// watchdog, and the queue resumes the conversation.
func TestWatchdogEndsATurnWithNoAgent(t *testing.T) {
	bin := setup(t)
	if err := os.WriteFile(bin, []byte(deadClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	was, wasCheck := deadAfter, turnCheck
	deadAfter, turnCheck = 100*time.Millisecond, 20*time.Millisecond
	if err := os.MkdirAll(dir("wd"), 0o700); err != nil {
		t.Fatal(err)
	}
	s := &server{cfg: Config{ID: "wd", Kind: "claude", Cwd: filepath.Dir(bin), Binary: bin, SessionID: "SID", IdleStop: Duration(time.Hour)},
		clients: map[*conn]struct{}{}, pending: map[string]asked{}, quit: make(chan struct{})}
	watching := make(chan struct{})
	go func() { s.watchTurn(); close(watching) }()
	defer func() {
		_ = s.do(op{Op: "stop"})
		<-watching
		deadAfter, turnCheck = was, wasCheck
	}()
	if err := s.send("go", nil, false); err != nil {
		t.Fatal(err)
	}
	if err := s.send("and then this", nil, false); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(10 * time.Second)
	for {
		s.mu.Lock()
		state, detail, queue := s.info.State, s.info.Detail, len(s.info.Queue)
		s.mu.Unlock()
		if state == "idle" && detail == "Resumed." && queue == 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("still %q (%q), %d queued", state, detail, queue)
		}
		time.Sleep(20 * time.Millisecond)
	}
	args, _ := os.ReadFile(filepath.Join(filepath.Dir(bin), "args.log"))
	if n := strings.Count(string(args), "--resume SID"); n != 1 {
		t.Errorf("resumed %d times:\n%s", n, args)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	var why string
	for _, l := range s.ring {
		if isEventLine(l, "turn_end") && strings.Contains(string(l), "without ending it") {
			why = string(l)
		}
	}
	if why == "" {
		t.Error("the conversation doesn't say the turn was ended for want of an agent")
	}
}

// The end of a background task wakes the agent to report it, so the idle
// stop waits for that turn rather than resting it as it begins.
func TestEndedTaskKeepsTheAgentUp(t *testing.T) {
	setup(t)
	if err := os.MkdirAll(dir("bg"), 0o700); err != nil {
		t.Fatal(err)
	}
	c := &fakeConn{events: make(chan event.Event, 1)}
	s := &server{cfg: Config{ID: "bg"}, conn: c, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.info.State = "idle"
	s.onAgentEvent(c, event.Background{Tasks: []event.BackgroundTask{{ID: "b1", Type: "monitor", Label: "PR watch"}}})
	if !s.stillWorking() {
		t.Fatal("a running task should keep it up")
	}
	s.onAgentEvent(c, event.Background{})
	if s.idle != nil {
		s.idle.Stop()
	}
	if !s.stillWorking() {
		t.Fatal("the task's end should keep it up until the agent takes it up")
	}
	s.onAgentEvent(c, event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: "The monitor expired."}}})
	if s.stillWorking() || s.info.State != "working" {
		t.Fatalf("once it answers, the turn is the agent's: state %q", s.info.State)
	}
}
