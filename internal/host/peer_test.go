package host

import (
	"net"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func TestPeerPIDNamesTheClientProcess(t *testing.T) {
	if runtime.GOOS != "darwin" && runtime.GOOS != "linux" {
		t.Skip("peer credentials are read on darwin and linux")
	}
	dir, err := os.MkdirTemp("/tmp", "peer")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(dir)
	ln, err := net.Listen("unix", filepath.Join(dir, "s"))
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		if c, err := net.Dial("unix", filepath.Join(dir, "s")); err == nil {
			defer c.Close()
			_, _ = c.Read(make([]byte, 1))
		}
	}()
	c, err := ln.Accept()
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	if got := peerPID(c); got != os.Getpid() {
		t.Fatalf("peerPID = %d, want %d", got, os.Getpid())
	}
	a, b := net.Pipe()
	defer a.Close()
	defer b.Close()
	if got := peerPID(a); got != 0 {
		t.Fatalf("peerPID of a pipe = %d, want 0", got)
	}
}

func TestStopsTurn(t *testing.T) {
	for _, c := range []struct {
		o    op
		want bool
	}{
		{op{Op: "interrupt"}, true},
		{op{Op: "stop_task"}, true},
		{op{Op: "send", Now: true}, true},
		{op{Op: "send"}, false},
		{op{Op: "send", Now: true, Guide: true}, false},
		{op{Op: "queue_send"}, true},
		{op{Op: "queue_send", Guide: true}, false},
		{op{Op: "queue_edit"}, false},
	} {
		if got := stopsTurn(c.o); got != c.want {
			t.Errorf("stopsTurn(%+v) = %v, want %v", c.o, got, c.want)
		}
	}
	if d := describePID(os.Getpid()); d == "an unknown client" {
		t.Fatalf("describePID named no process: %q", d)
	}
}
