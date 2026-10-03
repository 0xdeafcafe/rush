//go:build linux

package proc

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"

	"golang.org/x/sys/unix"
)

// Subreap makes this process the one its orphaned descendants are handed
// to, rather than init: a program a session's agent started with setsid
// and nohup still has the session's host among its ancestors once the
// shell that ran it exits, which is how whatever asks (a vault checking
// which session a command runs under) still finds the session.
//
// Adopted processes are reaped here as they exit. A child of the host's
// own is left to whoever waits on it: Go holds a pidfd for every child it
// starts, and only a zombie no pidfd here names is reaped. Without pidfds
// the two can't be told apart, and Subreap does nothing.
func Subreap() error {
	if !pidfdsNameChildren() {
		return errors.New("this kernel gives children no pidfd: adopted processes can't be told from the host's own")
	}
	if err := unix.Prctl(unix.PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0); err != nil {
		return err
	}
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGCHLD)
	go func() {
		// SIGCHLDs arriving together are one signal: the tick catches any
		// a burst left behind.
		tick := time.NewTicker(time.Minute)
		for {
			select {
			case <-sig:
			case <-tick.C:
			}
			reapAdopted()
		}
	}()
	return nil
}

// pidfdsNameChildren is whether a child started here shows up among this
// process's pidfds, which is what tells it from one adopted.
func pidfdsNameChildren() bool {
	cmd := exec.Command("/bin/sh", "-c", "exit 0")
	if cmd.Start() != nil {
		return false
	}
	defer cmd.Wait()
	return ownPidfds()[cmd.Process.Pid]
}

// reapAdopted waits on every exited child of this process that it did not
// start, and reports how many it reaped.
func reapAdopted() int {
	var info unix.Siginfo
	// A peek: is any child waiting to be reaped at all? Nothing is taken.
	if err := unix.Waitid(unix.P_ALL, 0, &info, unix.WEXITED|unix.WNOHANG|unix.WNOWAIT, nil); err != nil || info.Signo != int32(syscall.SIGCHLD) {
		return 0
	}
	zombies := exitedChildren()
	if len(zombies) == 0 {
		return 0
	}
	own := ownPidfds()
	n := 0
	for _, pid := range zombies {
		if own[pid] {
			continue // its exec.Cmd waits on it
		}
		var ws unix.WaitStatus
		if got, err := unix.Wait4(pid, &ws, unix.WNOHANG, nil); err == nil && got == pid {
			n++
		}
	}
	return n
}

// exitedChildren are the zombies whose parent is this process.
func exitedChildren() []int {
	me := os.Getpid()
	entries, err := os.ReadDir("/proc")
	if err != nil {
		return nil
	}
	var out []int
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil || pid <= 0 {
			continue
		}
		b, err := os.ReadFile("/proc/" + e.Name() + "/stat")
		if err != nil {
			continue
		}
		end := bytes.LastIndexByte(b, ')')
		if end < 0 || end+2 >= len(b) || b[end+2] != 'Z' {
			continue
		}
		if s, ok := parseStat(b); ok && s.ppid == me {
			out = append(out, pid)
		}
	}
	return out
}

// ownPidfds are the pids this process holds a pidfd for: the children Go
// started and has not yet waited on.
func ownPidfds() map[int]bool {
	out := map[int]bool{}
	fds, err := os.ReadDir("/proc/self/fd")
	if err != nil {
		return out
	}
	for _, fd := range fds {
		// "pidfd:[inode]" on kernels with pidfs, "anon_inode:[pidfd]" before.
		if l, err := os.Readlink("/proc/self/fd/" + fd.Name()); err != nil || (l != "anon_inode:[pidfd]" && !strings.HasPrefix(l, "pidfd:")) {
			continue
		}
		b, err := os.ReadFile("/proc/self/fdinfo/" + fd.Name())
		if err != nil {
			continue
		}
		for _, line := range strings.Split(string(b), "\n") {
			if v, ok := strings.CutPrefix(line, "Pid:"); ok {
				if pid, err := strconv.Atoi(strings.TrimSpace(v)); err == nil && pid > 0 {
					out[pid] = true
				}
			}
		}
	}
	return out
}
