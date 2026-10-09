package claude

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// scanner owns the cost cache; only its goroutine touches it.
type scanner struct {
	mu    sync.Mutex // held for a whole scan; the cache is only touched under it
	cache *state.CostCache[claude.Totals]
	sizes map[string]int64
	seen  map[string]seenTarget
	buf   []byte
	saved time.Time
	day   string
}

// seenTarget is how a target's files stood at its last scan.
type seenTarget struct {
	main   int64     // its transcript's size
	subDir time.Time // its subagents folder's time, which changes as runs start
	subs   []string  // the subagents' transcripts
	at     time.Time // when they were looked at
}

// SpendScanner prices Claude Code's transcripts. It reads nothing: its
// cost cache is read as it first runs, off the UI goroutine.
func (Adapter) SpendScanner() agent.SpendScanner {
	return &scanner{sizes: map[string]int64{}, seen: map[string]seenTarget{}, buf: make([]byte, 0, 64<<10)}
}

// files lists a target's transcripts, and reports false when nothing about
// a target that isn't live has changed since the last scan.
func (s *scanner) files(t agent.SpendTarget) ([]string, bool) {
	if prev, ok := s.seen[t.Path]; ok && t.Past && !t.Live && time.Since(prev.at) < pastEvery {
		return nil, false
	}
	var main int64
	if st, err := os.Stat(t.Path); err == nil {
		main = st.Size()
	}
	var subDir time.Time
	if st, err := os.Stat(filepath.Join(strings.TrimSuffix(t.Path, ".jsonl"), "subagents")); err == nil {
		subDir = st.ModTime()
	}
	prev, ok := s.seen[t.Path]
	if ok && !t.Live && prev.main == main && prev.subDir.Equal(subDir) {
		prev.at = time.Now()
		s.seen[t.Path] = prev
		return nil, false
	}
	subs := prev.subs
	if !ok || !prev.subDir.Equal(subDir) {
		subs = claude.SubagentTranscripts(t.Path)
	}
	s.seen[t.Path] = seenTarget{main: main, subDir: subDir, subs: subs, at: time.Now()}
	return append([]string{t.Path}, subs...), true
}

// Run scans every target whose files grew and returns the new totals.
func (s *scanner) Run(targets []agent.SpendTarget) map[string]agent.Spend { //nolint:gocognit // moved from fleet as it was
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.cache == nil {
		s.cache = state.LoadCostCache[claude.Totals]()
	}
	out := map[string]agent.Spend{}
	today := claude.Day(time.Now())
	if today != s.day {
		// A new day: today's spend starts again, so everything is summed afresh.
		s.day, s.sizes, s.seen = today, map[string]int64{}, map[string]seenTarget{}
	}
	for _, t := range targets {
		files, maybe := s.files(t)
		if !maybe {
			continue
		}
		changed := false
		sizes := make([]int64, len(files))
		for i, f := range files {
			sizes[i] = -1
			st, err := os.Stat(f)
			if err != nil {
				continue
			}
			sizes[i] = st.Size()
			if s.sizes[f] != st.Size() {
				changed = true
			}
		}
		if !changed && len(s.sizes) > 0 {
			if _, ok := s.sizes[t.Path]; ok {
				continue
			}
		}
		var sp agent.Spend
		for i, f := range files {
			tot := s.cache.Get(f)
			// Of a session's many subagent transcripts, most haven't grown
			// since they were last read: their totals stand without opening
			// them again.
			if sizes[i] < 0 || tot.Size != sizes[i] {
				before := tot.Offset
				s.buf, _ = claude.Scan(f, tot, s.buf)
				if tot.Offset != before {
					s.cache.MarkDirty()
				}
			}
			s.sizes[f] = tot.Size
			c, u := tot.Spend()
			sp.Cost += c
			sp.Usage.Add(u)
			sp.Today += tot.DaySpend(today)
			if f == t.Path {
				sp.Model = tot.LastModel
				sp.First, sp.Last = tot.First, tot.Last
				if tot.Halt != nil {
					h := *tot.Halt
					sp.Halt = &h
				}
				sp.Progress, sp.ProgressAt = tot.Progress, tot.ProgressAt
				sp.Context, sp.Compacts = tot.Context(), tot.Compacts
				sp.Dir, sp.DirAt = tot.Dir, tot.DirAt
			}
			if tot.Last.After(sp.Last) {
				sp.Last = tot.Last
			}
			for _, p := range tot.PRs {
				addUnique(&sp.PRs, p)
			}
			for _, d := range tot.Dirs {
				addUnique(&sp.Dirs, d)
			}
		}
		if cap(s.buf) > 8<<20 {
			s.buf = make([]byte, 0, 64<<10)
		}
		sp.Ready = true
		out[t.Key] = sp
	}
	if time.Since(s.saved) > 30*time.Second {
		_ = s.cache.Save()
		s.saved = time.Now()
	}
	return out
}

// Flush saves the cost cache unless a scan is mid-way; the cache also saves
// itself every 30s, so skipping one save loses nothing.
func (s *scanner) Flush() {
	if s.mu.TryLock() {
		if s.cache != nil {
			_ = s.cache.Save()
		}
		s.mu.Unlock()
	}
}

func addUnique(list *[]string, v string) {
	if !slices.Contains(*list, v) {
		*list = append(*list, v)
	}
}

var _ agent.SpendReader = Adapter{}
