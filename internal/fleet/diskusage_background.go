package fleet

import (
	"path/filepath"
	"sync"
	"time"

	"golang.org/x/sys/unix"
)

// Automatic disk meters are housekeeping, not foreground work. Share one
// budget across temp and worktree scans so concurrent meters cannot saturate
// a core. Two milliseconds of walking earns eighteen milliseconds of rest.
var backgroundDisk struct {
	sync.Mutex
	pace diskPacer
}

type diskPacer struct {
	last time.Time
	work time.Duration
}

func DiskUsageBackground(dirs []TempDir) int64 {
	backgroundDisk.Lock()
	defer backgroundDisk.Unlock()
	p := &backgroundDisk.pace
	p.last = time.Now() // time between walks does not consume the work budget
	n := diskUsage(dirs, p)
	p.yield()
	return n
}

func (p *diskPacer) yield() {
	if p == nil {
		return
	}
	now := time.Now()
	p.work += now.Sub(p.last)
	if p.work >= 2*time.Millisecond {
		// An unusually slow filesystem call need not impose an unbounded sleep.
		time.Sleep(min(100*time.Millisecond, p.work*9))
		p.work = 0
		now = time.Now()
	}
	p.last = now
}

// Dependency and build folders are huge, rarely change, and each walk of
// one costs thousands of calls. A background walk reuses a folder's last
// total while its own modified time is unchanged and the total is under
// an hour old. Foreground walks (cleaning, checks) always count afresh.
var heavyNames = map[string]bool{"node_modules": true, ".git": true, "target": true, ".venv": true, "venv": true, "Pods": true}

const heavyTTL = time.Hour

type heavyEntry struct {
	mtime int64
	at    time.Time
	total int64
}

var heavyCache struct {
	sync.Mutex
	m map[string]heavyEntry
}

// heavyHit says whether dir is a heavy folder whose last total still
// holds; stamp is its modified time, to store the new total under.
func heavyHit(dir string, pace *diskPacer) (stamp int64, total int64, hit bool) {
	if pace == nil || !heavyNames[filepath.Base(dir)] {
		return 0, 0, false
	}
	var st unix.Stat_t
	if unix.Lstat(dir, &st) != nil {
		return 0, 0, false
	}
	stamp = st.Mtim.Nano()
	heavyCache.Lock()
	defer heavyCache.Unlock()
	if e, ok := heavyCache.m[dir]; ok && e.mtime == stamp && time.Since(e.at) < heavyTTL {
		return stamp, e.total, true
	}
	return stamp, 0, false
}

func heavyStore(dir string, stamp, total int64) {
	if stamp == 0 {
		return
	}
	heavyCache.Lock()
	defer heavyCache.Unlock()
	if heavyCache.m == nil {
		heavyCache.m = map[string]heavyEntry{}
	}
	heavyCache.m[dir] = heavyEntry{mtime: stamp, at: time.Now(), total: total}
}
