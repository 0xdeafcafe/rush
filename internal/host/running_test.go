package host

import (
	"os"
	"os/exec"
	"testing"
	"time"
)

// A saved host pid names the host only if that process started before the
// host wrote its info: after a restart the pid can be anyone's.
func TestRunningTellsAReusedPid(t *testing.T) {
	cmd := exec.Command("sleep", "30")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { _ = cmd.Process.Kill(); _, _ = cmd.Process.Wait() }()
	pid := cmd.Process.Pid
	if !Running(Info{HostPID: pid, StartedAt: time.Now().Add(time.Minute)}) {
		t.Fatal("a process that started before its info was taken for gone")
	}
	if Running(Info{HostPID: pid, StartedAt: time.Now().Add(-time.Hour)}) {
		t.Fatal("a process that started an hour after the host was taken for it")
	}
	if Running(Info{HostPID: os.Getpid()}) {
		t.Fatal("the process asking was taken for the host")
	}
	if !Running(Info{HostPID: pid}) {
		t.Fatal("info from before hosts said when they started: the pid alone decides")
	}
}
