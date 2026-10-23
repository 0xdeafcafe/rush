package claude

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// projectIndex is which transcripts each project folder holds, listed
// again only when the folder's time changes (a file added or removed):
// looking for a session that moved stats each folder rather than
// listing every one, some thousands of files long.
type projectIndex struct {
	mu    sync.Mutex
	roots map[string]*projectsRoot
}

type projectsRoot struct {
	mod   time.Time
	names []string // its folders, as of mod
	dirs  map[string]projectDir
}

type projectDir struct {
	mod  time.Time
	sids map[string]bool
}

var findIndex projectIndex

// find is the first project folder under root, by name, holding sid's
// transcript; "" when none does.
func (x *projectIndex) find(root, sid string) string {
	st, err := os.Stat(root)
	if err != nil {
		return ""
	}
	x.mu.Lock()
	defer x.mu.Unlock()
	if x.roots == nil {
		x.roots = map[string]*projectsRoot{}
	}
	r := x.roots[root]
	if r == nil || !r.mod.Equal(st.ModTime()) {
		ents, err := os.ReadDir(root)
		if err != nil {
			return ""
		}
		old := r
		r = &projectsRoot{mod: st.ModTime(), names: make([]string, 0, len(ents)), dirs: map[string]projectDir{}}
		for _, e := range ents {
			r.names = append(r.names, e.Name())
			if old != nil {
				if d, ok := old.dirs[e.Name()]; ok {
					r.dirs[e.Name()] = d
				}
			}
		}
		x.roots[root] = r
	}
	for _, n := range r.names {
		dir := filepath.Join(root, n)
		st, err := os.Stat(dir)
		if err != nil || !st.IsDir() {
			continue
		}
		d, ok := r.dirs[n]
		if !ok || !d.mod.Equal(st.ModTime()) {
			d = projectDir{mod: st.ModTime(), sids: listSessions(dir)}
			r.dirs[n] = d
		}
		if d.sids[sid] {
			return filepath.Join(dir, sid+".jsonl")
		}
	}
	return ""
}

// listSessions is the session ids whose transcripts are in dir.
func listSessions(dir string) map[string]bool {
	f, err := os.Open(dir)
	if err != nil {
		return nil
	}
	defer f.Close()
	names, _ := f.Readdirnames(-1)
	out := make(map[string]bool, len(names))
	for _, n := range names {
		if id, ok := strings.CutSuffix(n, ".jsonl"); ok {
			out[id] = true
		}
	}
	return out
}
