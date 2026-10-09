package host

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

func waitAsleep(t *testing.T, id string) Info {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		i, err := ReadInfo(id)
		if err == nil && i.Sleeping && !Alive(i.HostPID) {
			return i
		}
		time.Sleep(20 * time.Millisecond)
	}
	i, _ := ReadInfo(id)
	t.Fatalf("host did not sleep: %+v", i)
	return i
}

func TestIdleHostExitsAndExplicitWakeResumes(t *testing.T) {
	bin := setup(t)
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin, Prompt: "first", IdleStop: Duration(150 * time.Millisecond)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok })
	_ = c.Allow("r1", nil, false)
	idle := next(t, c, inState("idle")).(InfoEvent).Info
	slept := waitAsleep(t, cfg.ID)
	c.Close()
	if Alive(idle.ClaudePID) || slept.ClaudePID != 0 || slept.State != "idle" {
		t.Fatalf("runtime survived idle exit: %+v", slept)
	}
	if c, err := Dial(cfg.ID); err == nil {
		c.Close()
		t.Fatal("view-only Dial woke host")
	}
	var wg sync.WaitGroup
	errs := make(chan error, 6)
	for i := 0; i < 6; i++ {
		wg.Add(1)
		go func() { defer wg.Done(); errs <- Ensure(cfg.ID) }()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	awake, _ := ReadInfo(cfg.ID)
	if awake.Sleeping || awake.HostPID == slept.HostPID {
		t.Fatalf("host did not restart: %+v", awake)
	}
	// Wake without sending, let it sleep, then wake again: resume must survive.
	waitAsleep(t, cfg.ID)
	saved, _ := ReadConfig(cfg.ID)
	if !saved.Resume || saved.Prompt != "" {
		t.Fatalf("lost resume state: %+v", saved)
	}
	if err := Ensure(cfg.ID); err != nil {
		t.Fatal(err)
	}
	c, err = Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	defer c.Stop()
	if err := c.Send("second"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok })
	args, _ := os.ReadFile(filepath.Join(filepath.Dir(bin), "args.log"))
	if !bytes.Contains(args, []byte("--resume "+saved.SessionID)) {
		t.Fatal("new process did not resume saved session")
	}
}

func TestIdleSleepPreservesPendingWork(t *testing.T) {
	cases := map[string]func(*server){
		"queue":      func(s *server) { s.info.Queue = []string{"keep"}; s.info.QueueHeld = true },
		"approval":   func(s *server) { s.pending = map[string]asked{"a": {}} },
		"question":   func(s *server) { s.asking = 1 },
		"background": func(s *server) { s.info.Background = []Task{{ID: "running"}} },
		"retry":      func(s *server) { s.info.Retry = &Retry{} },
		"limit":      func(s *server) { s.info.Limit = &Limit{Continue: true} },
		"unanswered": func(s *server) { s.waiting = time.Now() },
	}
	for name, change := range cases {
		t.Run(name, func(t *testing.T) {
			s := &server{cfg: Config{IdleStop: Duration(time.Hour)}, info: Info{State: "idle"}, idleGen: 1, quit: make(chan struct{})}
			change(s)
			s.rest(1)
			if s.idle != nil {
				s.idle.Stop()
			}
			if s.info.Sleeping {
				t.Fatal("slept with pending work")
			}
		})
	}
}

func TestStaleIdleCallbackCannotCloseResumedHost(t *testing.T) {
	s := &server{info: Info{State: "idle"}, idleGen: 2, quit: make(chan struct{})}
	s.rest(1)
	if s.info.Sleeping {
		t.Fatal("stale callback closed host")
	}
}

type earlyCloseProcess struct {
	fakeConn
	pid int
}

func (c *earlyCloseProcess) PID() int     { return c.pid }
func (c *earlyCloseProcess) Close() error { return nil }
func TestIdleRetirementWaitsForActualProcess(t *testing.T) {
	cmd := exec.Command("/bin/sleep", "30")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer cmd.Process.Kill()
	done := make(chan struct{})
	go func() { cmd.Wait(); close(done) }()
	stopRestingAgent(&earlyCloseProcess{pid: cmd.Process.Pid})
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("runtime survived bounded retirement")
	}
	if Alive(cmd.Process.Pid) {
		t.Fatal("runtime PID remains alive")
	}
}

func TestWakeKeepsSameSessionCountersAndPermissionChoices(t *testing.T) {
	old := Info{Sleeping: true, SessionID: "same", Kind: "kimi", PermissionMode: "ask", PermissionModes: []event.PermissionMode{{ID: "ask", Name: "Ask"}}, CostUSD: 1.25, ContextTokens: 900, CacheWarm: time.Now().Add(time.Hour), Billing: "api", QueueHeld: true, QueueSeparate: true, StartedAt: time.Unix(1, 0), ReplayFrom: time.Unix(2, 0)}
	for _, same := range []bool{true, false} {
		s := &server{cfg: Config{Resume: true, SessionID: "same"}, info: Info{Kind: "kimi", SessionID: "same", StartedAt: time.Now()}}
		if !same {
			s.cfg.SessionID = "fork"
		}
		s.restoreSleepingInfo(old)
		if same && (s.info.CostUSD != 1.25 || s.info.PermissionMode != "ask" || s.info.ContextTokens != 900 || len(s.info.PermissionModes) != 1 || !s.info.QueueHeld || !s.info.QueueSeparate || s.info.Billing != "api") {
			t.Fatalf("lost conversation metadata: %+v", s.info)
		}
		if !same && (s.info.CostUSD != 0 || len(s.info.PermissionModes) != 0) {
			t.Fatal("copied old session metadata to fork")
		}
		if s.info.Sleeping || !s.info.ReplayFrom.IsZero() || s.info.StartedAt == old.StartedAt {
			t.Fatal("restored transient sleep/replay state")
		}
	}
}

type blockingRestConn struct {
	fakeConn
	entered, release chan struct{}
}

func (c *blockingRestConn) Close() error { close(c.entered); <-c.release; return nil }
func TestIdleCallbackCannotExitAfterActivityDuringRetirement(t *testing.T) {
	setup(t)
	c := &blockingRestConn{entered: make(chan struct{}), release: make(chan struct{})}
	s := &server{cfg: Config{ID: "retiring"}, conn: c, info: Info{State: "idle"}, idleGen: 1, quit: make(chan struct{}), clients: map[*conn]struct{}{}}
	done := make(chan struct{})
	go func() { s.rest(1); close(done) }()
	<-c.entered
	s.mu.Lock()
	s.idleGen++
	s.info.State = "working"
	s.mu.Unlock()
	close(c.release)
	<-done
	if s.info.Sleeping {
		t.Fatal("stale retirement callback exited resumed host")
	}
	select {
	case <-s.quit:
		t.Fatal("closed resumed host")
	default:
	}
}

type countedCloseConn struct {
	fakeConn
	closed bool
}

func (c *countedCloseConn) Close() error { c.closed = true; return nil }
func TestIdleQueueAndRetryReleaseRuntimeButKeepHost(t *testing.T) {
	setup(t)
	for _, name := range []string{"held queue", "retry", "limit"} {
		t.Run(name, func(t *testing.T) {
			c := &countedCloseConn{}
			s := &server{cfg: Config{ID: "pending", IdleStop: Duration(time.Hour)}, conn: c, info: Info{State: "idle"}, idleGen: 1, quit: make(chan struct{}), clients: map[*conn]struct{}{}}
			switch name {
			case "held queue":
				s.info.Queue = []string{"keep"}
				s.info.QueueHeld = true
			case "retry":
				s.info.Retry = &Retry{}
			case "limit":
				s.info.Limit = &Limit{Continue: true, ResetsAt: time.Now().Add(time.Hour)}
			}
			s.rest(1)
			if s.idle != nil {
				s.idle.Stop()
			}
			if !c.closed || s.conn != nil {
				t.Fatal("kept heavy idle runtime")
			}
			if s.info.Sleeping {
				t.Fatal("lost host-owned pending work")
			}
			select {
			case <-s.quit:
				t.Fatal("exited with pending work")
			default:
			}
		})
	}
}

// A session restarted while idle keeps when it went idle (Serve restores it), and
// detail it publishes while idle doesn't move it: the list shows no finish.
func TestIdleSinceSurvivesWakeAndPublish(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	went := time.Unix(1000, 0)
	s := &server{cfg: Config{ID: "idle", Resume: true, SessionID: "same"}, began: true, clients: map[*conn]struct{}{}, info: Info{Kind: "codex", SessionID: "same", State: "idle"}}
	if err := os.MkdirAll(dir(s.cfg.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	s.info.IdleSince = went // as Serve restores it
	s.publish()
	s.info.Detail = "changed"
	s.publish()
	if !s.info.IdleSince.Equal(went) {
		t.Fatalf("idle since %v, want %v", s.info.IdleSince, went)
	}
	s.info.State = "working"
	s.publish()
	s.info.State = "idle"
	s.publish()
	if !s.info.IdleSince.After(went) {
		t.Fatal("a turn's end didn't move IdleSince")
	}
}
