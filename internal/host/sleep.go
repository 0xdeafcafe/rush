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

// sleepSafe is checked again after retiring the agent, while holding mu. An
// idle label alone is insufficient: held queues, questions and scheduled work
// all require their host to survive.
func (s *server) sleepSafe() bool {
	return s.runtimeIdleSafe() && s.info.Quiet()
}

// A future retry or held queue needs its lightweight host, not an idle runtime.
func (s *server) runtimeIdleSafe() bool {
	return s.info.State == "idle" && s.info.Needs == "" && len(s.pending) == 0 && !s.stillWorking() && !s.info.Relogin && !s.quietWait
}

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
	conn := s.conn
	s.mu.Unlock()
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
	if !s.sleepSafe() {
		s.armIdle()
		return
	}
	// The CLI shim owns its host until it has collected the final reply. Its
	// existing owner watcher/explicit Stop ends it after returning that reply.
	if s.cfg.Owner > 0 {
		return
	}
	s.cfg.Prompt = ""
	s.cfg.Images = nil
	s.cfg.PromptExchange = nil
	s.cfg.Resume = s.cfg.Resume || s.began && s.cfg.SessionID != ""
	s.saveConfig()
	s.info.Sleeping = true
	s.publish()
	// Publish precedes socket close. UI also checks saved info if EOF wins the
	// final notification race; no active view process is terminated here.
	s.stopOnce.Do(func() {
		if s.quit != nil {
			close(s.quit)
		}
	})
}

// Ensure wakes a saved host for an explicit action. Viewing/Dial never wakes a
// host. Concurrent sends share a startup lock, so only one process is spawned.
func Ensure(id string) error { return ensure(id, "") }

// ensure is Ensure, starting the host with prompt as its first message.
func ensure(id, prompt string) error {
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
	_, err = Spawn(cfg)
	return err
}

// Close can return before its process exits (notably app-server cancellation).
// Capture the owned PID first, and finish retirement before allowing a wake.
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
