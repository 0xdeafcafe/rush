package state

import "sync"

// The view saves its config and overlay from the UI goroutine, where
// nothing may wait on the disk. With WriteBehind on, a save marshals there
// (memory only) and one writer goroutine puts the bytes on disk, in order;
// a file saved again before its last save was written is written once,
// with the newest. Flush waits for everything handed over. Hosts and the
// CLI leave it off and write as they save.

var behind struct {
	sync.Mutex
	on      bool
	pending map[string]func() error // by what it writes: a newer job of the same replaces it
	order   []string
	busy    bool
	idle    *sync.Cond
	lastErr error
}

// WriteBehind has saves written by a goroutine of their own from now on.
func WriteBehind() {
	behind.Lock()
	behind.on = true
	if behind.idle == nil {
		behind.idle = sync.NewCond(&behind.Mutex)
	}
	behind.Unlock()
}

// Flush waits until every save handed over is on disk, and says how the
// last one that failed went.
func Flush() error {
	behind.Lock()
	defer behind.Unlock()
	for behind.busy {
		behind.idle.Wait()
	}
	err := behind.lastErr
	behind.lastErr = nil
	return err
}

// queueWrite hands job to the writer, under key: a job queued under a key
// already waiting takes its place in the order.
func queueWrite(key string, job func() error) {
	behind.Lock()
	defer behind.Unlock()
	if behind.pending == nil {
		behind.pending = map[string]func() error{}
	}
	if _, queued := behind.pending[key]; !queued {
		behind.order = append(behind.order, key)
	}
	behind.pending[key] = job
	if !behind.busy {
		behind.busy = true
		go writeBehind()
	}
}

// behindOn is whether saves are handed to the writer.
func behindOn() bool {
	behind.Lock()
	defer behind.Unlock()
	return behind.on
}

func writeBehind() {
	behind.Lock()
	for len(behind.order) > 0 {
		key := behind.order[0]
		behind.order = behind.order[1:]
		job := behind.pending[key]
		delete(behind.pending, key)
		behind.Unlock()
		err := job()
		behind.Lock()
		if err != nil {
			behind.lastErr = err
		}
	}
	behind.busy = false
	behind.idle.Broadcast()
	behind.Unlock()
}
