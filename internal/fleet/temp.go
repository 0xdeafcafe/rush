package fleet

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"golang.org/x/sys/unix"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/0xdeafcafe/rush/internal/uithread"
)

// TempDir is a folder of an agent's scratch work.
type TempDir = agent.TempDir

// TempDirs are where an agent's temp work is: what it cloned, built or
// downloaded as scratch, which its agent leaves behind when it finishes.
// A rush session has a folder rush gives it; the rest are its agent's
// own (a background job's tmp folder, each session's scratch).
func (a *Agent) TempDirs() []TempDir {
	var out []TempDir
	var job string
	switch {
	case a.Rush:
		out = append(out, TempDir{Path: host.TempDir(a.ID), Keep: true})
	case !a.Interactive && !a.Past && a.ID != "":
		job = a.ID
	}
	if s, ok := agent.As[agent.Scratcher](a.Acct.Kind); ok {
		out = append(out, s.Scratch(a.Acct, job, a.SessionID, a.Cwd)...)
	}
	return out
}

// scratchRoot is the folder agent k's sessions' scratch folders are two
// below, if it has one.
func scratchRoot(k agent.Kind) string {
	if s, ok := agent.As[agent.Scratcher](k); ok {
		return s.ScratchRoot()
	}
	return ""
}

// DiskUsage is how much disk the folders take, counted as du does
// (allocated blocks), without following links.
func DiskUsage(dirs []TempDir) int64 { return diskUsage(dirs, nil) }

func diskUsage(dirs []TempDir, pace *diskPacer) int64 {
	var n int64
	for _, d := range dirs {
		pace.yield()
		var st unix.Stat_t
		if unix.Lstat(d.Path, &st) != nil {
			continue
		}
		n += st.Blocks * 512
		if st.Mode&unix.S_IFMT == unix.S_IFDIR {
			n += dirUsagePaced(d.Path, pace)
		}
	}
	return n
}

// CleanTemp deletes an agent's temp work. It refuses while the agent has a
// process, since whatever it's running may be using it.
func CleanTemp(a *Agent) error {
	if a.PID != 0 {
		return fmt.Errorf("%s is still running; stop it first", a.DisplayName)
	}
	for _, d := range a.TempDirs() {
		// Only ever a tmp folder of rush's or the agent's, never
		// something a bad id could point elsewhere.
		if root := scratchRoot(a.Acct.Kind); !host.IsTempDir(d.Path) && (root == "" || filepath.Dir(filepath.Dir(d.Path)) != root) {
			return fmt.Errorf("won't delete %s", d.Path)
		}
		if !d.Keep {
			if err := os.RemoveAll(d.Path); err != nil {
				return err
			}
			continue
		}
		ents, err := os.ReadDir(d.Path)
		if err != nil {
			continue
		}
		for _, e := range ents {
			if err := os.RemoveAll(filepath.Join(d.Path, e.Name())); err != nil {
				return err
			}
		}
	}
	return nil
}

// RemoveTempEntry deletes one thing in one of a's temp folders, the rest
// kept. It refuses while a runs, and anything not directly in one of them.
func RemoveTempEntry(a *Agent, path string) error {
	if a.PID != 0 {
		return fmt.Errorf("%s is still running; stop it first", a.DisplayName)
	}
	path = filepath.Clean(path)
	for _, d := range a.TempDirs() {
		if root := scratchRoot(a.Acct.Kind); !host.IsTempDir(d.Path) && (root == "" || filepath.Dir(filepath.Dir(d.Path)) != root) {
			continue
		}
		if filepath.Dir(path) == filepath.Clean(d.Path) {
			return os.RemoveAll(path)
		}
	}
	return fmt.Errorf("won't delete %s: it isn't in %s's temp work", path, a.DisplayName)
}

// CleanStaleTemp empties the tmp folder rush gave a session whose host is
// gone, once nothing at its top has changed for idle: something a stopped
// session left running may still use what changed since. Only that folder,
// never its agent's scratch elsewhere. It says whether it emptied it.
func CleanStaleTemp(a *Agent, idle time.Duration, now time.Time) (bool, error) {
	if !a.Rush || a.PID != 0 || a.ID == "" {
		return false, nil
	}
	dir := host.TempDir(a.ID)
	if !host.IsTempDir(dir) {
		return false, fmt.Errorf("won't delete %s", dir)
	}
	ents, err := os.ReadDir(dir)
	if err != nil || len(ents) == 0 {
		return false, nil
	}
	for _, e := range ents {
		if fi, err := e.Info(); err != nil || now.Sub(fi.ModTime()) < idle {
			return false, nil
		}
	}
	for _, e := range ents {
		if err := os.RemoveAll(filepath.Join(dir, e.Name())); err != nil {
			return true, err
		}
	}
	return true, nil
}

// TempSize is an agent's temp work as last measured.
type TempSize struct {
	Bytes int64     `json:"bytes"`
	At    time.Time `json:"at"`
	// Took is how long measuring it took: a folder of millions of files
	// takes a core for most of a minute to walk.
	Took time.Duration `json:"took,omitzero"`
}

// every is how long a running agent's temp work goes before it's
// measured again: ten minutes, or longer for one whose last walk was long,
// so walking never takes more than a sliver of a core.
func (e TempSize) every() time.Duration {
	return min(max(10*time.Minute, e.Took*tempShare), 2*time.Hour)
}

// tempShare is how much longer than a walk took it waits before the next:
// walks take at most 1/tempShare of a core.
const tempShare = 50

// TempSizes remembers what each agent's temp work measured, across
// restarts, so the folders are walked again only when an agent has done
// something since.
type TempSizes struct {
	mu     sync.Mutex
	Sizes  map[string]TempSize `json:"sizes"`
	dirty  bool
	loaded bool // read from disk yet: see NewTempSizes
}

// Bytes is what an agent's temp work measured last.
func (t *TempSizes) Bytes(key string) int64 {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.Sizes[key].Bytes
}

func tempPath() string { return state.CachePath("temp.json") }

// LoadTempSizes reads the last measurements.
func LoadTempSizes() *TempSizes {
	t := NewTempSizes()
	t.Load()
	return t
}

// NewTempSizes is none measured yet, not read from disk: Load reads them,
// off the UI goroutine, keeping any set before it.
func NewTempSizes() *TempSizes { return &TempSizes{Sizes: map[string]TempSize{}} }

// Load reads the last measurements, once, under any set since.
func (t *TempSizes) Load() {
	uithread.Forbid("fleet.TempSizes.Load")
	t.mu.Lock()
	done := t.loaded
	t.mu.Unlock()
	if done {
		return
	}
	var disk TempSizes
	if b, err := os.ReadFile(tempPath()); err == nil {
		_ = jsonx.Unmarshal(b, &disk)
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	for k, v := range disk.Sizes {
		if _, ok := t.Sizes[k]; !ok {
			t.Sizes[k] = v
		}
	}
	t.loaded = true
}

// Due are the agents whose temp work should be measured: never measured,
// busy since, or running and not measured for a while (ten minutes, longer
// for one that's slow to walk). A stopped one isn't walked again until it
// does something: the tidy-up empties what rush gave it.
func (t *TempSizes) Due(agents []*Agent, now time.Time) []*Agent {
	t.mu.Lock()
	defer t.mu.Unlock()
	var out []*Agent
	for _, a := range agents {
		e, ok := t.Sizes[a.Key]
		switch {
		case !ok:
		case a.PID != 0 && now.Sub(e.At) > e.every():
		case a.UpdatedAt.After(e.At) && a.PID == 0:
		default:
			continue
		}
		out = append(out, a)
	}
	return out
}

// Set records measurements.
// They're saved by the next Load, off the UI's goroutine, which hands
// them in.
func (t *TempSizes) Set(m map[string]TempSize) {
	t.mu.Lock()
	defer t.mu.Unlock()
	for k, v := range m {
		t.Sizes[k] = v
	}
	t.dirty = true
}

// Save writes the measurements if they changed.
func (t *TempSizes) Save() {
	t.mu.Lock()
	defer t.mu.Unlock()
	if !t.dirty || !t.loaded {
		return // unread, it would write over what's there
	}
	if b, err := jsonx.Marshal(t); err == nil {
		_ = os.MkdirAll(filepath.Dir(tempPath()), 0o700)
		if os.WriteFile(tempPath()+".tmp", b, 0o600) == nil {
			_ = os.Rename(tempPath()+".tmp", tempPath())
		}
	}
	t.dirty = false
}
