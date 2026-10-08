package host

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/testhome"
)

// The test binary stands in for rush: Spawn runs `<exe> host run <id>`.
func TestMain(m *testing.M) {
	if len(os.Args) == 4 && os.Args[1] == "host" && os.Args[2] == "run" {
		if err := Run(os.Args[3]); err != nil {
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
		os.Exit(0)
	}
	// Hosts the tests start write shims and sessions: into folders of
	// their own, not the machine's, which sessions' shells run.
	testhome.Main(m)
}

// fakeClaude plays a turn per message: a streamed word, a Bash call that
// needs permission, its result, then the end of the turn. Each launch's
// arguments are appended to args.log.
const fakeClaude = `#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/args.log"
sid=SID; prev=; for a in "$@"; do case "$prev" in --session-id|--resume) sid=$a ;; esac; prev=$a; done
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'","model":"claude-haiku-4-5","permissionMode":"default","tools":["Bash"]}'
while read -r line; do
  case "$line" in
  *'"type":"user"'*)
    echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"On it"}}}'
    echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"On it"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo hi"}}]}}'
    echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hi"},"tool_use_id":"t1"}}'
    ;;
  *'"type":"control_response"'*)
    echo '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"hi"}]}}'
    echo '{"type":"result","subtype":"success","result":"Said hi.","total_cost_usd":0.01}'
    ;;
  esac
done
`

func setup(t *testing.T) (bin string) {
	t.Helper()
	// Short, because unix socket paths are capped near 104 bytes on macOS.
	home, err := os.MkdirTemp("/tmp", "rush-host-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(home) })
	t.Setenv("RUSH_HOME", home)
	// Registered after Setenv, so it runs before RUSH_HOME is restored:
	// the hosts the test started, and any a restart left, must not outlive
	// it, and only this test's home is looked at.
	t.Cleanup(func() {
		if os.Getenv("RUSH_HOME") != home {
			return
		}
		for _, i := range List() {
			if i.HostPID > 0 && i.HostPID != os.Getpid() {
				_ = syscall.Kill(i.HostPID, syscall.SIGKILL)
			}
		}
	})
	bin = filepath.Join(home, "claude")
	if err := os.WriteFile(bin, []byte(fakeClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	return bin
}

// next reads host lines until one decodes to something want accepts.
func next(t *testing.T, c *Client, want func(any) bool) any {
	t.Helper()
	timeout := time.After(5 * time.Second)
	for {
		select {
		case line, ok := <-c.Lines:
			if !ok {
				t.Fatal("connection closed")
			}
			ev, err := Decode(line)
			if err != nil {
				t.Fatalf("decode %s: %v", line, err)
			}
			if want(ev) {
				return ev
			}
		case <-timeout:
			t.Fatal("timed out")
		}
	}
}

func inState(s string) func(any) bool {
	return func(ev any) bool {
		i, ok := ev.(InfoEvent)
		return ok && i.Info.State == s
	}
}

func TestHostLifecycle(t *testing.T) {
	bin := setup(t)
	cfg, err := Spawn(Config{Owner: os.Getpid(), Cwd: filepath.Dir(bin), Prompt: "say hi", Binary: bin, IdleStop: Duration(300 * time.Millisecond)})
	if err != nil {
		t.Fatal(err)
	}
	if len(cfg.ID) != 8 || !strings.HasPrefix(strings.ReplaceAll(cfg.SessionID, "-", ""), cfg.ID) {
		t.Fatalf("ids: %q %q", cfg.ID, cfg.SessionID)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()

	ask := next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok }).(event.Approval)
	if info := next(t, c, inState("blocked")).(InfoEvent).Info; info.Needs != "Bash echo hi" {
		t.Errorf("while asking: %+v", info)
	}
	if err := c.Allow(ask.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { a, ok := ev.(Answered); return ok && a.ID == "r1" })
	res := next(t, c, func(ev any) bool { _, ok := ev.(event.TurnEnd); return ok }).(event.TurnEnd)
	if res.Text != "Said hi." {
		t.Errorf("result: %+v", res)
	}
	idle := next(t, c, inState("idle")).(InfoEvent).Info
	if idle.Detail != "Said hi." || idle.CostUSD != 0.01 || idle.ClaudePID == 0 {
		t.Errorf("idle: %+v", idle)
	}

	// This CLI-owned host stays for its owner after the agent rests.
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.ClaudePID == 0 })

	// A second client is replayed the conversation without stream deltas.
	c2, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	var replay []string
	for line := range c2.Lines {
		replay = append(replay, string(line))
		if strings.Contains(string(line), `"type":"agtop_info"`) {
			break
		}
	}
	c2.Close()
	joined := strings.Join(replay, "\n")
	if strings.Contains(joined, "stream_event") || strings.Contains(joined, `"t":"delta"`) || !strings.Contains(joined, `"agtop_sent":true`) || !strings.Contains(joined, "Said hi.") {
		t.Errorf("replay:\n%s", joined)
	}

	// The next message resumes the same conversation.
	if err := c.Send("again"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok })
	args, _ := os.ReadFile(filepath.Join(filepath.Dir(bin), "args.log"))
	var launches []string // one a line, the prompt's own lines between
	for l := range strings.SplitSeq(strings.TrimSpace(string(args)), "\n") {
		if strings.HasPrefix(l, "-p ") {
			launches = append(launches, l)
		}
	}
	if len(launches) != 2 || !strings.Contains(launches[0], "--session-id "+cfg.SessionID) || !strings.Contains(launches[1], "--resume "+cfg.SessionID) {
		t.Errorf("launches:\n%s", args)
	}

	if err := c.Stop(); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(15 * time.Second)
	for {
		info, _ := ReadInfo(cfg.ID)
		if info.State == "stopped" && !alive(info.HostPID) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("host still up: %+v", info)
		}
		time.Sleep(50 * time.Millisecond)
	}
	if list := List(); len(list) != 1 || list[0].ID != cfg.ID {
		t.Errorf("list: %+v", list)
	}
}

// TestRealHost runs two turns through a host and the installed claude, with
// Claude Code stopped for idling in between, so the second turn resumes. It
// spends a few cents of Haiku, so it only runs with RUSH_REAL_CLAUDE=1.
func TestRealHost(t *testing.T) {
	if os.Getenv("RUSH_REAL_CLAUDE") == "" {
		t.Skip("set RUSH_REAL_CLAUDE=1 to run against the installed claude")
	}
	bin := setup(t)
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Model: "haiku", Prompt: "The secret word is PELICAN. Do not use any tools. Reply with only: ok",
		IdleStop: Duration(time.Second)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	defer c.Stop()
	wait := func() event.TurnEnd {
		t.Helper()
		timeout := time.After(90 * time.Second)
		for {
			select {
			case line, ok := <-c.Lines:
				if !ok {
					t.Fatal("connection closed")
				}
				switch ev, _ := Decode(line); ev := ev.(type) {
				case event.TurnEnd:
					return ev
				case event.Approval:
					// No tools needed; refuse anything it tries.
					_ = c.Deny(ev.ID, "No tools in this test; just answer.", false)
				}
			case <-timeout:
				log, _ := os.ReadFile(filepath.Join(dir(cfg.ID), "host.log"))
				t.Fatalf("timed out; host.log:\n%s", log)
			}
		}
	}
	if r := wait(); r.Err != "" {
		t.Fatalf("first turn: %+v", r)
	}
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.ClaudePID == 0 })
	if err := c.Send("What is the secret word? Reply with only the word."); err != nil {
		t.Fatal(err)
	}
	r := wait()
	if !strings.Contains(strings.ToUpper(r.Text), "PELICAN") {
		t.Fatalf("resumed turn forgot: %+v", r)
	}
	info, _ := ReadInfo(cfg.ID)
	t.Logf("session %s (config %s), cost $%.4f", info.SessionID, cfg.SessionID, info.CostUSD)
	if info.SessionID != cfg.SessionID {
		t.Errorf("resume changed the session id: %s -> %s", cfg.SessionID, info.SessionID)
	}
}

func TestQueueEdits(t *testing.T) {
	setup(t)
	s := &server{cfg: Config{ID: "q"}, clients: map[*conn]struct{}{}}
	s.info.Queue = []string{"a", "b", "c", "d"}
	steps := []struct {
		o    op
		want string
	}{
		{op{Op: "queue_edit", Index: 1, Text: "B"}, "a|B|c|d"},
		{op{Op: "queue_move", Index: 3, To: 0}, "d|a|B|c"},
		{op{Op: "queue_merge", Index: 1}, "d|a\n\nB|c"},
		{op{Op: "queue_remove", Index: 0}, "a\n\nB|c"},
		{op{Op: "queue_move", Index: 0, To: 9}, "c|a\n\nB"},
	}
	for _, st := range steps {
		if err := s.editQueue(st.o); err != nil {
			t.Fatalf("%s: %v", st.o.Op, err)
		}
		if got := strings.Join(s.info.Queue, "|"); got != st.want {
			t.Fatalf("%s: got %q want %q", st.o.Op, got, st.want)
		}
	}
	if err := s.editQueue(op{Op: "queue_merge", Index: 1}); err == nil {
		t.Error("merging the last item should fail")
	}
	if err := s.editQueue(op{Op: "queue_remove", Index: 5}); err == nil {
		t.Error("removing past the end should fail")
	}
}

// inputConn keeps what it's sent.
type inputConn struct {
	fakeConn
	got []agent.Input
}

func (c *inputConn) Send(in agent.Input) error { c.got = append(c.got, in); return nil }

func TestQueuedImages(t *testing.T) {
	setup(t)
	pic := filepath.Join(t.TempDir(), "shot.jpg")
	if err := os.WriteFile(pic, []byte("jpeg"), 0o600); err != nil {
		t.Fatal(err)
	}
	ic := &inputConn{}
	s := &server{cfg: Config{ID: "qi"}, conn: ic, clients: map[*conn]struct{}{}}
	s.info.State = "working"
	if err := s.send("first", nil, false); err != nil {
		t.Fatal(err)
	}
	if err := s.send("look at this", []string{pic}, false); err != nil {
		t.Fatal(err)
	}
	if len(ic.got) != 0 || len(s.info.Queue) != 2 {
		t.Fatalf("sent %v while busy; queue %q", ic.got, s.info.Queue)
	}
	s.mu.Lock()
	s.sendQueue()
	s.mu.Unlock()
	if len(ic.got) != 1 || !slices.Equal(ic.got[0].Images, []string{pic}) || !strings.Contains(ic.got[0].Text, "look at this") {
		t.Fatalf("the queue went as %+v", ic.got)
	}
	if s.info.Queue != nil || s.info.QueueImages != nil {
		t.Fatalf("left queued: %q %q", s.info.Queue, s.info.QueueImages)
	}
}

// stallClaude fails its first turn with an API error or a usage limit, as
// asked, and succeeds on "continue".
const stallClaude = `#!/bin/sh
sid=SID; prev=; for a in "$@"; do case "$prev" in --session-id|--resume) sid=$a ;; esac; prev=$a; done
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'","model":"claude-haiku-4-5"}'
while read -r line; do
  case "$line" in
  *overload*)
    echo '{"type":"assistant","message":{"id":"m1","role":"assistant","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"text","text":"trying"}]}}'
    echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"API Error: 529 Overloaded"}' ;;
  *limit*)
    echo '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":'$(( $(date +%s) + 3600 ))',"rateLimitType":"five_hour"}}'
    echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude usage limit reached"}' ;;
  *continue*)
    echo '{"type":"result","subtype":"success","result":"Recovered."}' ;;
  esac
done
`

func TestRetryAndLimit(t *testing.T) {
	bin := setup(t)
	if err := os.WriteFile(bin, []byte(stallClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin, Prompt: "please overload", RetryBase: Duration(300 * time.Millisecond)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	// The error schedules a retry; the retry sends "continue" and recovers.
	r := next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.Retry != nil }).(InfoEvent).Info.Retry
	if r.Attempt != 1 || r.GaveUp || !strings.Contains(r.Reason, "529") {
		t.Fatalf("retry: %+v", r)
	}
	next(t, c, func(ev any) bool { s, ok := ev.(Sent); return ok && s.Text == "continue" })
	res := next(t, c, func(ev any) bool { _, ok := ev.(event.TurnEnd); return ok }).(event.TurnEnd)
	if res.Text != "Recovered." {
		t.Fatalf("after retry: %+v", res)
	}
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.Retry == nil && i.Info.State == "idle" })

	// A usage limit asks whether to continue at the reset (opt-in).
	if err := c.Send("hit the limit"); err != nil {
		t.Fatal(err)
	}
	l := next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.Limit != nil }).(InfoEvent).Info.Limit
	if !l.Ask || l.Continue || l.Window != "five_hour" || time.Until(l.ResetsAt) < 50*time.Minute {
		t.Fatalf("limit: %+v", l)
	}
	if err := c.ContinueAtReset(true); err != nil {
		t.Fatal(err)
	}
	l = next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.Limit != nil && !i.Info.Limit.Ask }).(InfoEvent).Info.Limit
	if !l.Continue {
		t.Fatalf("after yes: %+v", l)
	}
	// While waiting for the reset, a new message queues instead of going now.
	if err := c.Send("then do this"); err != nil {
		t.Fatal(err)
	}
	q := next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && len(i.Info.Queue) > 0 }).(InfoEvent).Info.Queue
	if q[0] != "then do this" {
		t.Fatalf("queue: %v", q)
	}
	_ = c.Stop()
}

// fakeProof stands in for netproof: go says whether a session may try
// again, and warm is what the last asked for.
type fakeProof struct {
	looks atomic.Int32
	goes  atomic.Bool
	cold  atomic.Bool // a cold session asked
	fails atomic.Int32
}

func (f *fakeProof) install(t *testing.T) {
	was, wasFail, wasEvery := mayGo, netproofFail, onlineEvery
	t.Cleanup(func() { mayGo, netproofFail, onlineEvery = was, wasFail, wasEvery })
	mayGo = func(_, _ string, warm bool) bool {
		f.looks.Add(1)
		if !warm {
			f.cold.Store(true)
		}
		return f.goes.Load()
	}
	netproofFail = func(string, time.Time) { f.fails.Add(1) }
	onlineEvery = 10 * time.Millisecond
}

// quiet stops what s has scheduled once the test ends, before the
// stand-ins fakeProof installed are put back.
func quiet(t *testing.T, s *server) {
	t.Cleanup(func() {
		s.mu.Lock()
		defer s.mu.Unlock()
		s.gen++
		if s.wake != nil {
			s.wake.Stop()
		}
	})
}

// continued waits for the session's continue to go (it fails to start the
// fake claude, which sets Error).
func continued(t *testing.T, s *server) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		s.mu.Lock()
		tried := s.info.Error != ""
		s.mu.Unlock()
		if tried {
			return
		}
		if time.Now().After(deadline) {
			t.Fatal("never continued")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

// While the prompt cache stays warm, a retry just waits its turn; once the
// next try would land after it expires, the session waits for proof the
// connection holds instead, and each failure is shared.
func TestRetryWaitsForProofPastTheCache(t *testing.T) {
	setup(t)
	var f fakeProof
	f.install(t)
	// "q" is the ID that waits no jitter.
	s := &server{cfg: Config{ID: "q", Binary: "/nonexistent/claude", RetryBase: Duration(time.Minute), RetryMax: 3}, clients: map[*conn]struct{}{}}
	quiet(t, s)
	s.info.CacheWarm = time.Now().Add(90 * time.Second)
	s.mu.Lock()
	s.retry("API Error: Connection dropped (ECONNRESET)", false)
	r := s.info.Retry
	if r.Proof || r.Next.IsZero() || r.Attempt != 1 {
		t.Fatalf("the first try fits in the cache: %+v", r)
	}
	s.wake.Stop()
	s.retry("API Error: Connection dropped (ECONNRESET)", false) // waits 2m, past the cache
	if !r.Proof || !r.Next.IsZero() || r.GaveUp {
		t.Fatalf("past the cache it should wait for proof: %+v", r)
	}
	s.mu.Unlock()
	for f.looks.Load() < 3 {
		time.Sleep(5 * time.Millisecond)
	}
	if s.info.Error != "" {
		t.Fatal("went without proof")
	}
	f.goes.Store(true)
	continued(t, s)
	if f.fails.Load() != 2 {
		t.Fatalf("each failure should be shared: %d", f.fails.Load())
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.retry("x", false)
	s.retry("x", false)
	if !r.GaveUp || !strings.Contains(r.Why, "3 retries used") {
		t.Fatalf("should give up once the retries are used: %+v", r)
	}
}

// Cut off by the network, a warm session waits until the API can be
// reached, however long, then continues; one whose cache ran out while it
// waited asks for proof instead.
func TestOfflineWaitsForTheNetwork(t *testing.T) {
	setup(t)
	var f fakeProof
	f.install(t)
	s := &server{cfg: Config{ID: "q", Binary: "/nonexistent/claude"}, clients: map[*conn]struct{}{}}
	quiet(t, s)
	s.info.CacheWarm = time.Now().Add(time.Hour)
	s.mu.Lock()
	if !s.stalled(event.TurnEnd{Reason: "done", Text: "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"}) {
		t.Fatal("an unreachable API should stall the turn")
	}
	r := s.info.Retry
	if r == nil || !r.Offline || r.Proof || r.GaveUp || r.Attempt != 1 {
		t.Fatalf("waiting for the network: %+v", r)
	}
	s.mu.Unlock()
	for f.looks.Load() < 3 {
		time.Sleep(5 * time.Millisecond)
	}
	s.mu.Lock()
	if !r.Offline || s.info.Error != "" || f.cold.Load() {
		t.Fatalf("still offline, warm: %+v %q cold %v", r, s.info.Error, f.cold.Load())
	}
	s.info.CacheWarm = time.Now() // the cache runs out
	s.mu.Unlock()
	for !f.cold.Load() {
		time.Sleep(5 * time.Millisecond)
	}
	s.mu.Lock()
	if !r.Proof {
		t.Fatalf("a cold session waits for proof: %+v", r)
	}
	s.mu.Unlock()
	f.goes.Store(true)
	continued(t, s)
	if r.Offline || r.Proof {
		t.Fatalf("back online: %+v", r)
	}
}

func TestOfflineErrors(t *testing.T) {
	for text, want := range map[string]bool{
		"API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)": true,
		"API Error: Unable to connect to API (ENOTFOUND)":                                true,
		"API Error: No response from API (waited 5m, then 10m on the retry).":            true,
		"API Error: Your computer went to sleep mid-response.":                           true,
		"API Error: 529 Overloaded":                                                      false,
		"API Error: Connection lost mid-response.":                                       false,
	} {
		if got := IsOffline(strings.ToLower(text)); got != want {
			t.Errorf("IsOffline(%q) = %v", text, got)
		}
	}
	if !IsRetryable(strings.ToLower("API Error: Response stalled mid-stream.")) {
		t.Error("a stalled stream should be retried")
	}
}

// A rewind switches the host to the cut conversation and keeps the path it
// leaves as a branch; going back down that branch keeps this one in turn.
func TestRewindKeepsBranches(t *testing.T) {
	setup(t)
	if err := os.MkdirAll(dir("r"), 0o700); err != nil {
		t.Fatal(err)
	}
	s := &server{cfg: Config{ID: "r", SessionID: "old", Resume: true}, began: true, clients: map[*conn]struct{}{}}
	s.ring, s.ringN = [][]byte{[]byte(`{"type":"assistant"}`)}, 20
	s.info.State = "working"
	if err := s.rewind("cut", true, &Branch{From: 3}); err == nil {
		t.Fatal("rewinding mid-turn should be refused")
	}
	s.info.State = "idle"
	if err := s.rewind("cut", true, &Branch{From: 3, Turns: 5, Last: "try the cache"}); err != nil {
		t.Fatal(err)
	}
	if s.cfg.SessionID != "cut" || !s.began || len(s.ring) != 0 || s.info.RewoundAt.IsZero() || s.info.SessionID != "cut" {
		t.Fatalf("after rewind: cfg %+v ring %d info %+v", s.cfg, len(s.ring), s.info)
	}
	if b := s.cfg.Branches; len(b) != 1 || b[0].SessionID != "old" || b[0].From != 3 || b[0].Last != "try the cache" {
		t.Fatalf("branches %+v", b)
	}
	saved, err := ReadConfig("r")
	if err != nil || saved.SessionID != "cut" || len(saved.Branches) != 1 {
		t.Fatalf("saved %+v %v", saved, err)
	}
	// Back down the old path: it stops being a branch, and "cut" becomes one.
	if err := s.rewind("old", true, &Branch{From: 3, Turns: 2}); err != nil {
		t.Fatal(err)
	}
	if b := s.cfg.Branches; s.cfg.SessionID != "old" || len(b) != 1 || b[0].SessionID != "cut" {
		t.Fatalf("after switching back: %s %+v", s.cfg.SessionID, b)
	}
	// Before the first message (/clear): a fresh conversation, nothing to
	// resume, and no cache of its own to call cold.
	s.info.CacheWarm, s.info.ContextTokens = time.Now().Add(-time.Hour), 9000
	if err := s.rewind("fresh", false, &Branch{From: 1, Turns: 5}); err != nil {
		t.Fatal(err)
	}
	if s.began || s.cfg.Resume || len(s.cfg.Branches) != 2 || !s.info.CacheWarm.IsZero() || s.info.ContextTokens != 0 {
		t.Fatalf("fresh: began %v cfg %+v", s.began, s.cfg)
	}
}

// A host from before rewind is restarted on this build, already rewound,
// with the path it left kept as a branch.
func TestRewindByRestart(t *testing.T) {
	bin := setup(t)
	acct := claude.Account{Name: "t", ConfigDir: t.TempDir()}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin, Account: acct.Profile(), Resume: true, SessionID: "old-0000-aaaa"})
	if err != nil {
		t.Fatal(err)
	}
	old, _ := ReadInfo(cfg.ID)
	// The old conversation has a transcript, so it's worth keeping.
	tp := acct.TranscriptPath(cfg.Cwd, cfg.SessionID)
	os.MkdirAll(filepath.Dir(tp), 0o700)
	os.WriteFile(tp, []byte("{}\n"), 0o600)
	if err := RewindByRestart(cfg.ID, "cut-0000-bbbb", true, Branch{From: 2, Turns: 3}); err != nil {
		t.Fatal(err)
	}
	info, _ := ReadInfo(cfg.ID)
	got, _ := ReadConfig(cfg.ID)
	if info.HostPID == old.HostPID || !alive(info.HostPID) || info.Proto != Proto || info.SessionID != "cut-0000-bbbb" {
		t.Fatalf("restarted: %+v (was pid %d)", info, old.HostPID)
	}
	if got.SessionID != "cut-0000-bbbb" || len(got.Branches) != 1 || got.Branches[0].SessionID != "old-0000-aaaa" || got.Branches[0].From != 2 {
		t.Fatalf("config %+v", got)
	}
	if c, err := Dial(cfg.ID); err == nil {
		c.Stop()
		c.Close()
	}
}

func TestRingTrimsWholeTurns(t *testing.T) {
	setup(t)
	if err := os.MkdirAll(dir("tr"), 0o700); err != nil {
		t.Fatal(err)
	}
	s := &server{cfg: Config{ID: "tr"}, clients: map[*conn]struct{}{}}
	big := []byte(`{"type":"assistant","message":{"content":"` + strings.Repeat("x", 1<<20) + `"}}`)
	for turn := range 5 {
		echo, _ := jsonx.Marshal(map[string]any{"type": "user", "message": map[string]any{"role": "user", "content": fmt.Sprint("turn ", turn)}, "agtop_sent": true})
		s.record(echo)
		for range 3 {
			s.record(append([]byte(nil), big...))
		}
	}
	if s.ringN > ringMax {
		t.Fatalf("ring holds %d bytes, over %d", s.ringN, ringMax)
	}
	from, ok := turnStart(s.ring[0], s.ring[1])
	if !ok {
		t.Fatalf("the ring should start at a turn, starts with %.60s", s.ring[0])
	}
	if !s.info.ReplayFrom.Equal(from) {
		t.Fatalf("ReplayFrom is %v, the ring starts at %v", s.info.ReplayFrom, from)
	}
	if !bytes.Contains(s.ring[1], []byte("turn 3")) {
		t.Fatalf("the ring should keep the last two turns, starts with %.80s", s.ring[1])
	}

	// A turn bigger than the ring keeps its latest part and still says
	// when it began.
	var last time.Time
	for i := range s.ring[:len(s.ring)-1] {
		if t, ok := turnStart(s.ring[i], s.ring[i+1]); ok {
			last = t
		}
	}
	for range 10 {
		s.record(append([]byte(nil), big...))
	}
	if s.ringN > ringMax {
		t.Fatalf("ring holds %d bytes, over %d", s.ringN, ringMax)
	}
	if !s.info.ReplayFrom.Equal(last) {
		t.Fatalf("ReplayFrom is %v, the last turn began at %v", s.info.ReplayFrom, last)
	}
}

// interruptConn counts the turns it was asked to stop.
type interruptConn struct {
	fakeConn
	stops int
}

func (c *interruptConn) Interrupt() error { c.stops++; return nil }

// Sent now mid-turn, a message stops the turn and waits in the queue
// after what was there, rather than going to a turn that may be stuck in
// a long tool; a queued one sent now goes first.
func TestSendNowMidTurnCutsIn(t *testing.T) {
	setup(t)
	c := &interruptConn{}
	s := &server{cfg: Config{ID: "q", Kind: "claude"}, conn: c, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Queue, s.info.QueueHeld = "working", []string{"a", "b"}, true
	if err := s.send("now", nil, true); err != nil {
		t.Fatal(err)
	}
	if err := s.do(op{Op: "queue_send", Index: 2, Was: "b"}); err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(s.info.Queue, "|"); got != "b|a|now" || s.info.QueueHeld || c.stops != 2 {
		t.Errorf("queue %q held %v stops %d", got, s.info.QueueHeld, c.stops)
	}
}

// Steering with a queued message hands it to the turn under way: out of
// the queue, into the agent, and nothing stopped.
func TestSteerQueuedDoesNotStop(t *testing.T) {
	setup(t)
	c := &inputConn{}
	s := &server{cfg: Config{ID: "sq", Kind: "claude"}, conn: c, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Queue = "working", []string{"a", "b"}
	if err := s.do(op{Op: "queue_send", Index: 1, Was: "b", Guide: true}); err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(s.info.Queue, "|"); got != "a" || len(c.got) != 1 || c.got[0].Text != "b" {
		t.Errorf("queue %q, the agent got %v", got, c.got)
	}
}

// A message with an image sent now mid-turn cuts in with its image, and a
// queued one sent now keeps its own: each message's images stay its own.
func TestCutInKeepsImages(t *testing.T) {
	setup(t)
	pic := filepath.Join(t.TempDir(), "shot.jpg")
	if err := os.WriteFile(pic, []byte("jpeg"), 0o600); err != nil {
		t.Fatal(err)
	}
	c := &interruptConn{}
	s := &server{cfg: Config{ID: "qc", Kind: "claude"}, conn: c, clients: map[*conn]struct{}{}}
	s.info.State, s.info.QueueHeld = "working", true
	if err := s.send("a", nil, false); err != nil {
		t.Fatal(err)
	}
	if err := s.send("b", []string{pic}, false); err != nil {
		t.Fatal(err)
	}
	if err := s.send("now", []string{pic}, true); err != nil {
		t.Fatal(err)
	}
	if err := s.do(op{Op: "queue_send", Index: 2, Was: "b"}); err != nil {
		t.Fatal(err)
	}
	want := [][]string{{pic}, nil, {pic}}
	if got := strings.Join(s.info.Queue, "|"); got != "b|a|now" || !slices.EqualFunc(s.info.QueueImages, want, slices.Equal) || c.stops != 2 {
		t.Errorf("queue %q images %q stops %d", got, s.info.QueueImages, c.stops)
	}
}

// A message sent while idle, with the queue still waiting and not held,
// goes after what's queued, all of it together, not round it.
func TestIdleSendJoinsTheQueue(t *testing.T) {
	setup(t)
	c := &inputConn{}
	s := &server{cfg: Config{ID: "iq", Kind: "claude"}, conn: c, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Queue = "idle", []string{"a", "b"}
	if err := s.send("new", nil, false); err != nil {
		t.Fatal(err)
	}
	if len(s.info.Queue) != 0 || len(c.got) != 1 || c.got[0].Text != JoinQueue([]string{"a", "b", "new"}) {
		t.Errorf("queue %q, the agent got %+v", s.info.Queue, c.got)
	}
}

// A session started without a message, by /clear, is named from the
// first one it's sent, pastes and images left out; later ones don't
// rename it.
func TestNamedFromFirstMessage(t *testing.T) {
	setup(t)
	os.MkdirAll(dir("nf"), 0o700)
	s := &server{cfg: Config{ID: "nf", Name: "fresh session in app", NameFirst: true}, conn: &inputConn{}, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Name = "working", s.cfg.Name
	s.send("[Image #1] retry the upload <pasted_content id=\"a\">\nlots\n</pasted_content> when it times out please now", nil, false)
	if s.info.Name != "retry the upload when it times" || s.cfg.NameFirst {
		t.Fatalf("named %q, still to name: %v", s.info.Name, s.cfg.NameFirst)
	}
	s.send("something else", nil, false)
	if s.info.Name != "retry the upload when it times" {
		t.Fatalf("renamed by the second message: %q", s.info.Name)
	}
}

// /clear rewinds to a fresh conversation: the one left is kept as a
// branch, and the session is named again by its next message.
func TestRewindFreshRenames(t *testing.T) {
	setup(t)
	os.MkdirAll(dir("cl"), 0o700)
	s := &server{cfg: Config{ID: "cl", SessionID: "old", Resume: true, Cwd: "/work/app", Name: "fix the upload"}, began: true,
		conn: &inputConn{}, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Name = "idle", "fix the upload"
	if err := s.rewind("new", false, &Branch{From: 1, Turns: 4}); err != nil {
		t.Fatal(err)
	}
	if s.info.Name != "fresh session in app" || !s.cfg.NameFirst || len(s.cfg.Branches) != 1 || s.cfg.Branches[0].SessionID != "old" {
		t.Fatalf("after clearing: name %q, to name %v, branches %+v", s.info.Name, s.cfg.NameFirst, s.cfg.Branches)
	}
	s.send("add retries to the upload", nil, false)
	if s.info.Name != "add retries to the upload" {
		t.Fatalf("not named by its next message: %q", s.info.Name)
	}
	// Back down the old path, which isn't fresh: its name stays.
	if err := s.rewind("old", true, &Branch{From: 1}); err != nil || s.cfg.NameFirst || s.info.Name != "add retries to the upload" {
		t.Fatalf("resuming renamed it: %q %v %v", s.info.Name, s.cfg.NameFirst, err)
	}
}

// #compact by another model carries on in a fresh conversation that
// starts with the summary, under the same name, the old one kept.
func TestCompactedKeepsName(t *testing.T) {
	setup(t)
	os.MkdirAll(dir("cp"), 0o700)
	s := &server{cfg: Config{ID: "cp", Kind: "fake", SessionID: "old", Resume: true, Cwd: "/work/app", Name: "fix the upload", Account: agent.Profile{Kind: "fake", Name: "fake", Dir: t.TempDir()}}, began: true,
		conn: &inputConn{fakeConn: fakeConn{events: make(chan event.Event, 1)}}, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Name = "idle", "fix the upload"
	s.info.SessionID, s.info.UpdatedAt = "old", time.Now()
	if err := s.do(op{Op: "compacted_checked", Text: "new", Message: "summary: it was all about uploads", Branch: &Branch{From: 1, Turns: 4}, ExpectedSession: s.info.SessionID, ExpectedUpdatedAt: s.info.UpdatedAt}); err != nil {
		t.Fatal(err)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	// The fake agent started on it may already have named the session its own.
	if s.cfg.SessionID != "new" && s.cfg.SessionID != "fake-session" || s.info.Name != "fix the upload" || s.cfg.NameFirst || len(s.cfg.Branches) != 1 || s.cfg.Branches[0].SessionID != "old" {
		t.Fatalf("after compacting: session %q name %q, to name %v, branches %+v", s.cfg.SessionID, s.info.Name, s.cfg.NameFirst, s.cfg.Branches)
	}
}
