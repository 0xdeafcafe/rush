//go:build darwin

package fswait

import (
	"sync"
	"time"

	"golang.org/x/sys/unix"
)

// Watcher notices writes to a set of files and folders: a file written to,
// grown, replaced or removed, or a folder gaining, losing or renaming an
// entry. The kernel queues what happened, so asking costs one system call
// and nothing is missed between asks.
type Watcher struct {
	mu    sync.Mutex
	kq    int
	fds   map[string]int
	paths map[int]string  // fds' paths, to name what an event was on
	fresh map[string]bool // watched since the last ask: told as changed, once
	dirty bool            // it can't tell what changed since the last ask
	// missed is when each path that couldn't be opened was last tried:
	// most are folders not made yet, tried again only after retryMissing.
	missed map[string]time.Time
}

// retryMissing is how long a path that wasn't there goes untried: unwatched,
// it's only looked at as it would have been anyway.
const retryMissing = 5 * time.Second

// NewWatcher is a watcher of nothing yet; one that can't be made reports
// every ask as a change.
func NewWatcher() *Watcher {
	w := &Watcher{kq: -1, fds: map[string]int{}, paths: map[int]string{}, fresh: map[string]bool{}, missed: map[string]time.Time{}, dirty: true}
	if kq, err := unix.Kqueue(); err == nil {
		unix.CloseOnExec(kq)
		w.kq = kq
	}
	return w
}

// Watch makes paths the set watched, keeping those already watched. One
// that isn't there yet is skipped: watch its folder to see it arrive. One
// newly watched is told as changed at the next ask: it may have changed
// between being read and being watched.
func (w *Watcher) Watch(paths []string) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.kq < 0 {
		return
	}
	want := make(map[string]bool, len(paths))
	for _, p := range paths {
		want[p] = true
	}
	for p, fd := range w.fds {
		if !want[p] {
			w.drop(p, fd)
		}
	}
	for p := range w.missed {
		if !want[p] {
			delete(w.missed, p)
		}
	}
	now := time.Now()
	for p := range want {
		if _, ok := w.fds[p]; ok {
			continue
		}
		if at, ok := w.missed[p]; ok && now.Sub(at) < retryMissing {
			continue
		}
		fd, err := unix.Open(p, unix.O_EVTONLY|unix.O_CLOEXEC, 0)
		if err != nil {
			w.missed[p] = now
			continue
		}
		delete(w.missed, p)
		var ev unix.Kevent_t
		unix.SetKevent(&ev, fd, unix.EVFILT_VNODE, unix.EV_ADD|unix.EV_CLEAR)
		ev.Fflags = unix.NOTE_WRITE | unix.NOTE_EXTEND | unix.NOTE_DELETE | unix.NOTE_RENAME | unix.NOTE_ATTRIB
		if _, err := unix.Kevent(w.kq, []unix.Kevent_t{ev}, nil, nil); err != nil {
			unix.Close(fd) // not watched: never Watching
			continue
		}
		w.fds[p], w.paths[fd], w.fresh[p] = fd, p, true
	}
}

// drop stops watching p; closing its fd drops its registration.
func (w *Watcher) drop(p string, fd int) {
	unix.Close(fd)
	delete(w.fds, p)
	delete(w.paths, fd)
}

// Watching is whether path is watched: a change to it is told at an ask.
func (w *Watcher) Watching(path string) bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	_, ok := w.fds[path]
	return ok
}

// Changed reports whether anything watched changed since the last ask.
func (w *Watcher) Changed() bool {
	paths, all := w.Changes()
	return all || len(paths) > 0
}

// Changes are the watched paths that changed since the last ask; all when
// it can't tell which (the first ask, or the kernel failed it). A path
// removed or replaced is told, and no longer watched: watch it again to
// follow what's there now.
func (w *Watcher) Changes() (paths map[string]bool, all bool) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.kq < 0 {
		return nil, true
	}
	all, w.dirty = w.dirty, false
	paths, w.fresh = w.fresh, map[string]bool{}
	events := make([]unix.Kevent_t, 32)
	var zero unix.Timespec
	for {
		n, err := unix.Kevent(w.kq, nil, events, &zero)
		if err != nil && err != unix.EINTR {
			return paths, true
		}
		for i := range max(n, 0) {
			fd := int(events[i].Ident)
			p, ok := w.paths[fd]
			if !ok {
				continue
			}
			paths[p] = true
			if events[i].Fflags&(unix.NOTE_DELETE|unix.NOTE_RENAME) != 0 {
				w.drop(p, fd) // gone or replaced: watch whatever is there now
			}
		}
		if n < len(events) {
			return paths, all
		}
	}
}

// Close stops watching.
func (w *Watcher) Close() {
	w.mu.Lock()
	defer w.mu.Unlock()
	for p, fd := range w.fds {
		w.drop(p, fd)
	}
	if w.kq >= 0 {
		unix.Close(w.kq)
		w.kq = -1
	}
}
