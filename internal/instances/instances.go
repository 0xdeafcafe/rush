// Package instances is the roster of running rush views, so `rush reload`
// and #reload all can tell the others to reload.
package instances

import (
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"

	"github.com/0xdeafcafe/rush/internal/proc"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Dir holds one file per running view: <pid>, with the arguments it runs.
func Dir() string { return filepath.Join(state.Dir(), "ui") }

// Register lists this view until the returned func is called. Call it once
// the SIGUSR1 handler is set: a view that isn't listed is never signalled.
func Register() func() {
	p := filepath.Join(Dir(), strconv.Itoa(os.Getpid()))
	if os.MkdirAll(Dir(), 0o700) != nil || os.WriteFile(p, []byte(strings.Join(os.Args, "\x00")), 0o600) != nil {
		return func() {}
	}
	return func() { _ = os.Remove(p) }
}

// Reload signals every listed view but skip to reload, and says how many.
// A pid is signalled only while the process there still runs the arguments
// it listed (a reused pid doesn't); otherwise its entry is dropped.
func Reload(skip int) int {
	ents, _ := os.ReadDir(Dir())
	n := 0
	for _, e := range ents {
		pid, err := strconv.Atoi(e.Name())
		if err != nil || pid == skip {
			continue
		}
		p := filepath.Join(Dir(), e.Name())
		b, _ := os.ReadFile(p)
		if !slices.Equal(proc.Args(pid), strings.Split(string(b), "\x00")) || len(b) == 0 {
			_ = os.Remove(p)
			continue
		}
		if syscall.Kill(pid, syscall.SIGUSR1) == nil {
			n++
		}
	}
	return n
}

// held is the lock file of the view that runs: kept so it isn't closed,
// which would let it go.
var held *os.File

// Lock makes this the one rush view on this folder, until it exits: there
// is one, as a view switches logins and restarts sessions' hosts for all
// of them, and two would do it twice. ok is false while another holds it,
// whose pid is holder. An exec (#reload) lets it go, and the new rush
// takes it again.
func Lock() (holder int, ok bool) {
	p := filepath.Join(Dir(), "lock")
	if os.MkdirAll(Dir(), 0o700) != nil {
		return 0, true // nowhere to lock: as before, rather than no rush at all
	}
	f, err := os.OpenFile(p, os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return 0, true
	}
	if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		b, _ := os.ReadFile(p)
		holder, _ = strconv.Atoi(strings.TrimSpace(string(b)))
		f.Close()
		return holder, false
	}
	_ = f.Truncate(0)
	_, _ = f.WriteAt([]byte(strconv.Itoa(os.Getpid())), 0)
	held = f
	return os.Getpid(), true
}
