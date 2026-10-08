package host

import (
	"bufio"
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"golang.org/x/sys/unix"

	"github.com/0xdeafcafe/rush/internal/state"
)

// A running session's tmp folder only grows: every build, test run and
// clone it makes as scratch stays until the session is cleaned, which
// rush won't do while it runs. Two long sessions reached 40 GB and 27 GB
// in two days. When the setting is on and the disk runs low, the host
// clears out its own session's old tmp: what's been untouched for
// trimAge and is open in no process.
const (
	trimEvery = 10 * time.Minute
	trimAge   = 3 * time.Hour
	// lowDisk is when the disk is low: less than this free, or less than
	// lowShare of it.
	lowDisk  = 20 << 30
	lowShare = 10
)

// trimLoop checks now and then whether to trim, until the host stops.
func (s *server) trimLoop() {
	t := time.NewTicker(trimEvery)
	defer t.Stop()
	for {
		select {
		case <-s.quit:
			return
		case <-t.C:
			if !state.Load().Config.TrimRunningTmp {
				continue
			}
			tmp := TempDir(s.cfg.ID)
			if !diskLow(tmp) {
				continue
			}
			held, ok := heldUnder(tmp)
			if !ok {
				continue // without knowing what's open, nothing is safe to remove
			}
			trimTmp(tmp, time.Now(), held)
		}
	}
}

// diskLow is whether the disk dir is on is running out.
func diskLow(dir string) bool {
	var st unix.Statfs_t
	if unix.Statfs(dir, &st) != nil {
		return false
	}
	free, total := st.Bavail*uint64(st.Bsize), st.Blocks*uint64(st.Bsize)
	return free < lowDisk || free*lowShare < total
}

// heldUnder are the entries of dir that a process has open, or works in:
// their names, as they are in dir. ok is false when that can't be told.
func heldUnder(dir string) (map[string]bool, bool) {
	out, err := exec.Command("lsof", "-n", "-Fn", "-u", strconv.Itoa(os.Getuid())).Output()
	if err != nil && len(out) == 0 {
		return nil, false
	}
	// lsof names files by their real path.
	real, err := filepath.EvalSymlinks(dir)
	if err != nil {
		return nil, false
	}
	prefix := real + string(filepath.Separator)
	held := map[string]bool{}
	sc := bufio.NewScanner(bytes.NewReader(out))
	sc.Buffer(make([]byte, 64<<10), 1<<20)
	for sc.Scan() {
		line := sc.Text()
		if !strings.HasPrefix(line, "n"+prefix) {
			continue
		}
		name, _, _ := strings.Cut(strings.TrimPrefix(line, "n"+prefix), string(filepath.Separator))
		held[name] = true
	}
	return held, true
}

// trimTmp removes the entries of dir that are older than trimAge and
// not held. Claude Code's own folders (claude-<uid>: its task output and
// diffs) stay, as the session reads them back.
func trimTmp(dir string, now time.Time, held map[string]bool) (removed int) {
	if !IsTempDir(dir) {
		return 0 // only ever a session's own tmp folder
	}
	ents, err := os.ReadDir(dir)
	if err != nil {
		return 0
	}
	for _, e := range ents {
		name := e.Name()
		if held[name] || strings.HasPrefix(name, "claude-") {
			continue
		}
		fi, err := e.Info()
		if err != nil || now.Sub(fi.ModTime()) < trimAge {
			continue
		}
		if os.RemoveAll(filepath.Join(dir, name)) == nil {
			removed++
		}
	}
	return removed
}
