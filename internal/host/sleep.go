package host

import (
	"errors"
	"net"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// How long a host waits with nothing to do before it sleeps: its agent's
// program stops, its config is saved to resume, and the host exits. A send
// (Ensure) starts it again on the same conversation. A session you started
// waits half an hour; a spawned agent waits as long as its prompt cache
// lasts, as there's no warm cache to keep after that. Variables, so tests
// can shorten them.
var (
	rootIdle    = 30 * time.Minute
	spawnedIdle = 5 * time.Minute
)

// idleFor is how long this host waits idle before it sleeps.
func (s *server) idleFor() time.Duration {
	if s.cfg.Meta["spawnedBy"] != "" {
		return spawnedIdle
	}
	return rootIdle
}

// armIdle starts the idle wait again, from now. Called with mu held.
func (s *server) armIdle() {
	s.disarmIdle()
	generation := s.idleGen
	s.idle = time.AfterFunc(s.idleFor(), func() { s.rest(generation) })
}

// disarmIdle stops the idle wait: work has started again. It also makes a
// rest already running give up. Called with mu held.
func (s *server) disarmIdle() {
	s.idleGen++
	if s.idle != nil {
		s.idle.Stop()
		s.idle = nil
	}
}

// runtimeIdleSafe is whether the agent's program may be stopped: idle, with
// nothing asked of you, no task or side question of its own running, and no
// move to another account under way. A held queue, a retry or a limit
// doesn't need the program, but does need the host (see sleepSafe).
// Called with mu held.
func (s *server) runtimeIdleSafe() bool {
	return s.info.State == "idle" && s.info.Needs == "" && len(s.pending) == 0 && !s.stillWorking() && !s.info.Relogin && !s.quietWait
}

// sleepSafe is whether the host may exit: nothing at all is left for it to
// do. On top of runtimeIdleSafe, nothing queued, no background task, no
// limit or retry waiting, not away (Info.Quiet), and a spawned agent has
// given its answer to the agent waiting on it. Checked again after the
// agent's program has stopped. Called with mu held.
func (s *server) sleepSafe() bool {
	return s.runtimeIdleSafe() && s.info.Quiet() && s.answerGiven()
}

// answerGiven is whether a spawned agent owes no answer: none held for a
// queued turn, none being sent, and nobody asked for one as a message that
// it hasn't sent. Called with mu held.
func (s *server) answerGiven() bool {
	if s.cfg.Meta["spawnedBy"] == "" {
		return true
	}
	if s.owed != "" || s.reporting > 0 {
		return false
	}
	_, err := os.Stat(reportPath(s.cfg.ID))
	return errors.Is(err, os.ErrNotExist)
}

// rest is the idle wait running out: it stops the agent's program once
// nothing needs it, and puts the host to sleep once nothing needs that
// either. Anything left to do waits another idle time. A send since the
// wait began (a newer generation) makes it do nothing.
func (s *server) rest(generation uint64) {
	s.mu.Lock()
	if s.idleGen != generation || s.info.Sleeping || s.info.State != "idle" {
		s.mu.Unlock()
		return
	}
	if !s.runtimeIdleSafe() {
		s.armIdle()
		s.mu.Unlock()
		return
	}
	if s.cfg.Meta["spawnedBy"] != "" && !s.answerGiven() && s.owed == "" && s.reporting == 0 {
		s.reportAnswer() // asked for as a message and not sent yet: send it, then sleep next time
	}
	conn := s.conn
	s.mu.Unlock()
	// Its own shells (a build it left running) are read from the process
	// table, which isn't done holding mu.
	if conn != nil && runsShells(pidOf(conn)) {
		s.mu.Lock()
		if s.idleGen == generation && s.info.State == "idle" {
			s.armIdle()
		}
		s.mu.Unlock()
		return
	}
	s.mu.Lock()
	if s.idleGen != generation || s.conn != conn || !s.runtimeIdleSafe() {
		s.mu.Unlock()
		return
	}
	if conn != nil {
		s.detach()
		s.stopping = make(chan struct{})
		s.publish()
	}
	stopping := s.stopping
	s.mu.Unlock()
	if conn != nil {
		stopRestingAgent(conn)
		s.mu.Lock()
		if s.stopping == stopping {
			s.stopping = nil
		}
		close(stopping)
		s.mu.Unlock()
	} else if stopping != nil {
		<-stopping
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.idleGen != generation || s.conn != nil || s.stopping != nil {
		return
	}
	idle := s.idleFor()
	if !s.sleepSafe() {
		if conn != nil {
			s.event("rest", map[string]any{"why": "idle, but the host still has work waiting", "idleFor": idle.String()})
		}
		s.armIdle()
		return
	}
	// The CLI shim owns its host until it has collected the final reply. Its
	// owner watcher, or an explicit stop, ends it after returning that reply.
	if s.cfg.Owner > 0 {
		return
	}
	s.cfg.Prompt = ""
	s.cfg.Images = nil
	s.cfg.PromptExchange = nil
	s.cfg.Resume = s.cfg.Resume || s.began && s.cfg.SessionID != ""
	s.saveConfig()
	s.event("sleep", map[string]any{"why": "idle with nothing to do", "idleFor": idle.String(), "spawned": s.cfg.Meta["spawnedBy"] != ""})
	s.info.Sleeping = true
	s.publish()
	// Published before the socket closes. The UI also reads the saved info
	// should the socket's end reach it first.
	s.stopOnce.Do(func() {
		if s.quit != nil {
			close(s.quit)
		}
	})
}

// stopRestingAgent stops the agent's program and waits, a moment at most,
// for its process to go: Close can return before it has (an app-server's
// cancelling, notably), and a wake mustn't start a second one on the same
// conversation.
func stopRestingAgent(conn agent.Conn) {
	pid := pidOf(conn)
	stopAgent(conn)
	if pid <= 0 || pid == os.Getpid() {
		return
	}
	deadline := time.Now().Add(time.Second)
	for alive(pid) && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if alive(pid) {
		endGroup(pid)
	}
	deadline = time.Now().Add(time.Second)
	for alive(pid) && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
}

// Ensure wakes a saved host for an explicit action. Viewing/Dial never wakes a
// host. Concurrent sends share a startup lock, so only one process is spawned.
func Ensure(id string) error { return ensure(id, "", "") }

// EnsureBy is Ensure, the host's start caused by line ev (see Config.Cause).
func EnsureBy(id, ev string) error { return ensure(id, "", ev) }

// ensure is Ensure, starting the host with prompt as its first message.
func ensure(id, prompt, cause string) error {
	if id == "" || id == "." || id == ".." || strings.ContainsAny(id, "/\\\x00") {
		return errors.New("invalid session ID")
	}
	if info, err := readInfoFile(id); err == nil && info.IsRemote() {
		if alive(info.HostPID) {
			return nil // the machine it's on wakes it, on a send
		}
		return errors.New(id + " is on " + info.Remote + ", and rush remote attach " + info.Remote + " isn't running")
	}
	lock, err := os.OpenFile(filepath.Join(dir(id), "wake.lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return err
	}
	defer lock.Close()
	deadline := time.Now().Add(15 * time.Second)
	for {
		err = syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			break
		}
		if err != syscall.EWOULDBLOCK && err != syscall.EAGAIN {
			return err
		}
		if time.Now().After(deadline) {
			return errors.New("timed out waiting for session startup")
		}
		time.Sleep(20 * time.Millisecond)
	}
	for {
		info, err := ReadInfo(id)
		if err != nil || !alive(info.HostPID) {
			break
		}
		if !info.Sleeping {
			if c, err := net.DialTimeout("unix", SockPath(id), 250*time.Millisecond); err == nil {
				c.Close()
				return nil
			}
		}
		if time.Now().After(deadline) {
			return errors.New("session host did not become ready or finish exiting")
		}
		time.Sleep(20 * time.Millisecond)
	}
	// Read again under the lock: another start or model change may have updated it.
	cfg, err := ReadConfig(id)
	if err != nil {
		return err
	}
	cfg.Prompt, cfg.Resume = prompt, cfg.Resume || prompt != ""
	cfg.Images = nil
	cfg.PromptExchange = nil
	cfg.Owner = 0
	cfg.Cause = cause
	_, err = Spawn(cfg)
	return err
}

// A clean sleep has no pending work, but its observed choices and cumulative
// counters still belong to this conversation. New/forked sessions start fresh.
func (s *server) restoreSleepingInfo(old Info) {
	if !old.Sleeping || !s.cfg.Resume || old.SessionID != s.cfg.SessionID || old.Kind != s.info.Kind {
		return
	}
	s.info.PermissionModes = old.PermissionModes
	if s.info.PermissionMode == "" {
		s.info.PermissionMode = old.PermissionMode
	}
	s.info.CostUSD = old.CostUSD
	s.info.ContextTokens = old.ContextTokens
	s.info.CacheWarm = old.CacheWarm
	s.info.Home, s.info.HomeAt, s.info.Login = old.Home, old.HomeAt, old.Login
	s.info.Billing = old.Billing
	s.info.QueueHeld = old.QueueHeld
	s.info.QueueSeparate = old.QueueSeparate
	s.info.Detail = old.Detail
	if s.info.Model == "" {
		s.info.Model = old.Model
	}
	if s.info.Effort == "" {
		s.info.Effort = old.Effort
	}
}
