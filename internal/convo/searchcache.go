package convo

import (
	"os"
	"sync"
	"time"
)

// Searches hold the conversations they read for a while, so the next one
// (the same words with a letter more, as you type) searches them in memory
// rather than reading and decoding them again. Claude Code's are held as a
// Tail, so one that has grown since reads only what's new.

// heldBytes is how much of the files on disk the held ones may come to, the
// first read first: searched most recent first, the latest stay held. They
// take about a third of that in memory.
const heldBytes = 320 << 20

// holdFor is how long one is held after a search last used it.
const holdFor = 2 * time.Minute

type heldConvo struct {
	mu   sync.Mutex // one search at a time reads or grows it
	tail *Tail      // Claude Code's, which reads on from where it got to
	sess *Session   // another agent's, read whole
	// pieces are what's scored in it, from its index: held in place of
	// the session when that was never read.
	pieces *pieceIndex
	size   int64
	mod    time.Time
	used   time.Time
}

var held struct {
	mu    sync.Mutex
	m     map[string]*heldConvo
	bytes int64
	sweep *time.Timer
}

// session is the conversation as read, nil when only its pieces are held.
func (h *heldConvo) session() *Session {
	if h.tail != nil {
		return h.tail.Sess
	}
	return h.sess
}

// heldFor is the conversation at path as held, if it still stands as st
// says, or can be read on from where it got to.
func heldFor(path string, st os.FileInfo) *heldConvo {
	held.mu.Lock()
	defer held.mu.Unlock()
	h := held.m[path]
	if h == nil {
		return nil
	}
	same := st.Size() == h.size && st.ModTime().Equal(h.mod)
	grown := h.tail != nil && st.Size() > h.size
	if !same && !grown {
		held.bytes -= h.size
		delete(held.m, path)
		return nil
	}
	h.used = time.Now()
	return h
}

// hold keeps h for path, when there's room.
func hold(path string, h *heldConvo) {
	held.mu.Lock()
	defer held.mu.Unlock()
	if held.m == nil {
		held.m = map[string]*heldConvo{}
	}
	if old := held.m[path]; old != nil {
		held.bytes -= old.size
	}
	if held.bytes+h.size > heldBytes {
		delete(held.m, path)
		return
	}
	h.used = time.Now()
	held.m[path] = h
	held.bytes += h.size
	if held.sweep == nil {
		held.sweep = time.AfterFunc(holdFor, sweepHeld)
	}
}

// grew notes that h was read on to size.
func grew(h *heldConvo, size int64, mod time.Time) {
	held.mu.Lock()
	defer held.mu.Unlock()
	held.bytes += size - h.size
	h.size, h.mod = size, mod
}

// sweepHeld lets go of those no search has used for holdFor, and comes
// back while any are left.
func sweepHeld() {
	held.mu.Lock()
	defer held.mu.Unlock()
	for p, h := range held.m {
		if time.Since(h.used) >= holdFor {
			held.bytes -= h.size
			delete(held.m, p)
		}
	}
	held.sweep = nil
	if len(held.m) > 0 {
		held.sweep = time.AfterFunc(holdFor, sweepHeld)
	}
}
