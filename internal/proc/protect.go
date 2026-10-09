package proc

import (
	"errors"
	"sync"
)

// ErrProtected is what Kill says of a process rush has been told not to
// signal.
var ErrProtected = errors.New("that process stands in for a session on another machine (rush remote attach): stop the session, not it")

var (
	protectMu sync.RWMutex
	protected map[int]bool
)

// Protect says which processes Kill must refuse: the attach processes whose
// pid every remote session's stand-in carries, replacing what was said
// before.
func Protect(pids []int) {
	m := make(map[int]bool, len(pids))
	for _, p := range pids {
		m[p] = true
	}
	protectMu.Lock()
	protected = m
	protectMu.Unlock()
}

func guard(pid int) error {
	protectMu.RLock()
	defer protectMu.RUnlock()
	if protected[pid] {
		return ErrProtected
	}
	return nil
}
