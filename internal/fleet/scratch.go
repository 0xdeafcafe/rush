package fleet

import (
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/events"
)

// ScratchIdle is how long something in /tmp goes untouched before it's
// taken as left behind.
const ScratchIdle = 24 * time.Hour

// Scratch is what's yours at the top of /tmp: logs, caches and checkouts
// agents wrote there and never took away. Stale is what nothing has
// touched, anywhere inside, for ScratchIdle.
type Scratch struct {
	Items, StaleItems int
	Size, Stale       int64
	Checked           time.Time
}

// ScratchItem is one thing of yours at the top of /tmp.
type ScratchItem struct {
	Path string
	Size int64
}

// StaleScratch is what ClearScratch would remove now, biggest first. It
// walks each, like FindScratch: off the UI.
func StaleScratch() []ScratchItem {
	var out []ScratchItem
	for _, p := range scratchEntries() {
		if size, newest := scratchWalk(p); time.Since(newest) >= ScratchIdle {
			out = append(out, ScratchItem{Path: p, Size: size})
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Size > out[j].Size })
	return out
}

// scratchDir is where agents leave things.
var scratchDir = "/tmp"

// FindScratch walks the user's entries at the top of /tmp. It takes seconds
// on a big one, so it runs off the UI.
func FindScratch() Scratch {
	s := Scratch{Checked: time.Now()}
	for _, p := range scratchEntries() {
		size, newest := scratchWalk(p)
		s.Items++
		s.Size += size
		if time.Since(newest) >= ScratchIdle {
			s.StaleItems++
			s.Stale += size
		}
	}
	return s
}

// ClearScratch removes what's stale, looking again at each first: anything
// touched since it was counted stays.
func ClearScratch() (freed int64, n int, err error) {
	for _, p := range scratchEntries() {
		size, newest := scratchWalk(p)
		if time.Since(newest) < ScratchIdle {
			continue
		}
		if e := os.RemoveAll(p); e != nil {
			if err == nil {
				err = e
			}
			continue
		}
		freed += size
		n++
	}
	f := map[string]any{"dir": scratchDir, "removed": n, "bytes": freed}
	if err != nil {
		f["err"] = err.Error()
	}
	events.Log("scratch-cleared", f)
	return freed, n, err
}

// scratchEntries are the files and folders at the top of /tmp the user
// owns, but for Claude Code's own scratch (an agent's temp work, cleaned
// with it) and anything that isn't a plain file or folder (sockets, links).
func scratchEntries() []string {
	ents, err := os.ReadDir(scratchDir)
	if err != nil {
		return nil
	}
	uid := uint32(os.Getuid())
	// Agents' own scratch roots are cleaned with their sessions.
	own := map[string]bool{}
	for _, a := range agent.All() {
		if s, ok := a.(agent.Scratcher); ok && s.ScratchRoot() != "" {
			own[filepath.Base(s.ScratchRoot())] = true
		}
	}
	var out []string
	for _, e := range ents {
		if own[e.Name()] || strings.HasPrefix(e.Name(), "tmux-") {
			continue
		}
		if t := e.Type(); t&^fs.ModeDir != 0 {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		if st, ok := info.Sys().(*syscall.Stat_t); !ok || st.Uid != uid {
			continue
		}
		out = append(out, filepath.Join(scratchDir, e.Name()))
	}
	return out
}

// scratchWalk is how much disk p takes and when anything in it last
// changed.
// ponytail: a walk with a stat per file; fine for thousands, slow for
// millions (a Go build cache), measure there before optimising.
func scratchWalk(p string) (size int64, newest time.Time) {
	_ = filepath.WalkDir(p, func(_ string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		info, err := d.Info()
		if err != nil {
			return nil
		}
		if t := info.ModTime(); t.After(newest) {
			newest = t
		}
		if st, ok := info.Sys().(*syscall.Stat_t); ok {
			size += st.Blocks * 512
		}
		return nil
	})
	return size, newest
}
