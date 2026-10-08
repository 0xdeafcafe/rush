package efficiency

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// cacheVersion is bumped when File changes shape: the transcripts are then
// read again from the start.
const cacheVersion = 2

// Retired is what's kept of transcripts Claude Code has deleted (after 30
// days, by default), so the graphs reach back further than they do.
type Retired struct {
	Hours    map[int64]*Bucket `json:"h"`
	Uses     map[string]*Use   `json:"u"`
	Sessions int               `json:"s"`
}

// Store is every transcript's figures, read incrementally. Scanning happens
// outside the lock, so a view can be worked out while a long first scan
// runs.
type Store struct {
	mu      sync.RWMutex
	files   map[string]*File
	retired map[string]*Retired // by account folder
	saved   time.Time
	dirty   bool

	scan sync.Mutex // one refresh at a time
}

type cacheFile struct {
	Version int              `json:"version"`
	Files   map[string]*File `json:"files"`
}

func cachePath() string   { return state.CachePath("efficiency.json") }
func retiredPath() string { return filepath.Join(Dir(), "retired.json") }

// Dir is where the Efficiency place keeps what can't be worked out again:
// the event log, the retired figures and backups of files it changed.
func Dir() string { return filepath.Join(state.Dir(), "efficiency") }

// Open loads the store from its cache; the first Refresh fills it.
func Open() *Store {
	s := &Store{files: map[string]*File{}, retired: map[string]*Retired{}}
	var c cacheFile
	if b, err := os.ReadFile(cachePath()); err == nil && jsonx.Unmarshal(b, &c) == nil && c.Version == cacheVersion && c.Files != nil {
		s.files = c.Files
	}
	if b, err := os.ReadFile(retiredPath()); err == nil {
		_ = jsonx.Unmarshal(b, &s.retired)
	}
	if s.retired == nil {
		s.retired = map[string]*Retired{}
	}
	return s
}

// Refresh reads what's new in every profile's transcripts. It reports
// whether anything changed.
func (s *Store) Refresh(profiles []agent.Profile) bool {
	src, ok := source()
	if !ok {
		return false
	}
	s.scan.Lock()
	defer s.scan.Unlock()

	type job struct {
		path string
		f    *File
	}
	var jobs []job
	listed := map[string]bool{}
	s.mu.RLock()
	for _, a := range profiles {
		for _, p := range src.Transcripts(a) {
			listed[p] = true
			st, err := os.Stat(p)
			if err != nil {
				continue
			}
			old := s.files[p]
			if old != nil && old.Size == st.Size() && old.Offset == st.Size() {
				continue
			}
			var f *File
			if old != nil {
				f = old.clone()
			} else {
				f = &File{Account: a.Dir, Sub: strings.Contains(p, string(filepath.Separator)+"subagents"+string(filepath.Separator))}
			}
			jobs = append(jobs, job{p, f})
		}
	}
	var gone []string
	for p, f := range s.files {
		if listed[p] {
			continue
		}
		mine := false
		for _, a := range profiles {
			mine = mine || f.Account == a.Dir
		}
		if _, err := os.Stat(p); mine && os.IsNotExist(err) {
			gone = append(gone, p)
		}
	}
	s.mu.RUnlock()

	if len(jobs) > 0 {
		work := make(chan job)
		var wg sync.WaitGroup
		for range 6 {
			wg.Add(1)
			go func() {
				defer wg.Done()
				buf := make([]byte, 0, 64<<10)
				for j := range work {
					buf, _ = Scan(src, j.path, j.f, buf)
					if cap(buf) > 8<<20 {
						buf = make([]byte, 0, 64<<10)
					}
					s.mu.Lock()
					s.files[j.path] = j.f
					s.dirty = true
					s.mu.Unlock()
				}
			}()
		}
		for _, j := range jobs {
			work <- j
		}
		close(work)
		wg.Wait()
	}

	if len(gone) > 0 {
		s.mu.Lock()
		for _, p := range gone {
			s.retire(s.files[p])
			delete(s.files, p)
		}
		s.dirty = true
		s.mu.Unlock()
		s.saveRetired()
	}
	changed := len(jobs) > 0 || len(gone) > 0
	if changed && time.Since(s.saved) > 2*time.Minute {
		s.Save()
	}
	return changed
}

// retire folds a deleted transcript into its account's rollup.
func (s *Store) retire(f *File) {
	if f == nil {
		return
	}
	r := s.retired[f.Account]
	if r == nil {
		r = &Retired{}
		s.retired[f.Account] = r
	}
	if r.Hours == nil {
		r.Hours = map[int64]*Bucket{}
	}
	if r.Uses == nil {
		r.Uses = map[string]*Use{}
	}
	f.EachHour(func(h int64, b *Bucket) {
		if r.Hours[h] == nil {
			r.Hours[h] = &Bucket{}
		}
		r.Hours[h].Add(b)
	})
	for k, u := range f.Uses {
		ru := r.Uses[k]
		if ru == nil {
			ru = &Use{}
			r.Uses[k] = ru
		}
		ru.N += u.N
		if ru.First.IsZero() || u.First.Before(ru.First) {
			ru.First = u.First
		}
		if u.Last.After(ru.Last) {
			ru.Last = u.Last
		}
	}
	if !f.Sub {
		r.Sessions++
	}
}

func (s *Store) saveRetired() {
	s.mu.RLock()
	b, err := jsonx.Marshal(s.retired)
	s.mu.RUnlock()
	if err == nil {
		_ = writeFile(retiredPath(), b)
	}
}

// Save writes the cache when it has changed.
func (s *Store) Save() {
	s.mu.Lock()
	if !s.dirty {
		s.mu.Unlock()
		return
	}
	b, err := jsonx.Marshal(cacheFile{Version: cacheVersion, Files: s.files})
	s.dirty = false
	s.saved = time.Now()
	s.mu.Unlock()
	if err == nil {
		_ = writeFile(cachePath(), b)
	}
}

// writeFile replaces path whole, never leaving half a file.
func writeFile(path string, b []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), filepath.Base(path)+".*.tmp")
	if err != nil {
		return err
	}
	_, err = f.Write(b)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		_ = os.Remove(f.Name())
		return err
	}
	return os.Rename(f.Name(), path)
}
