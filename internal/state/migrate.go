package state

import (
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// Migrate moves the folders rush had as agtop (~/.config/agtop and its
// cache folder) to rush's, leaving a link at each old path so older
// builds, and whatever else still holds those paths, keep working. It
// runs only when asked for (the view starting, or rush migrate), never
// from a lookup, and moves nothing while a session's host still runs from
// the old folder. What it did, or why it didn't, goes to w.
func Migrate(w io.Writer) error {
	if testing.Testing() {
		return errors.New("rush migrate does not run from tests")
	}
	if os.Getenv("RUSH_HOME") != "" {
		fmt.Fprintln(w, "rush: RUSH_HOME is set, nothing to move")
		return nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	cache, _ := os.UserCacheDir()
	if os.Getenv("RUSH_CACHE") != "" {
		cache = ""
	}
	err = migrate(w, filepath.Join(home, ".config"), cache)
	dirs.Delete(home)
	return err
}

// migrate is Migrate for the folders under config and cache (cache may
// be empty: no cache folder to move).
func migrate(w io.Writer, config, cache string) error {
	oldConfig := filepath.Join(config, "agtop")
	if n := liveHosts(filepath.Join(oldConfig, "sessions")); n > 0 && realDir(oldConfig) {
		fmt.Fprintf(w, "rush: %d session host(s) still run from %s; nothing moved. Run rush migrate once they stop.\n", n, oldConfig)
		return nil
	}
	var errs []error
	for _, root := range []string{config, cache} {
		if root == "" {
			continue
		}
		from, to := filepath.Join(root, "agtop"), filepath.Join(root, "rush")
		switch {
		case !realDir(from):
			// Not there, or already a link.
		case exists(to):
			fmt.Fprintf(w, "rush: both %s and %s are there; nothing moved\n", from, to)
		default:
			if err := moveLeavingLink(from, to); err != nil {
				errs = append(errs, fmt.Errorf("moving %s: %w", from, err))
				continue
			}
			fmt.Fprintf(w, "rush: moved %s to %s, a link left in its place\n", from, to)
		}
	}
	return errors.Join(errs...)
}

func realDir(p string) bool {
	fi, err := os.Lstat(p)
	return err == nil && fi.IsDir()
}

func exists(p string) bool {
	_, err := os.Lstat(p)
	return !errors.Is(err, fs.ErrNotExist)
}

// liveHosts counts the sessions under root whose host still runs: its
// process is there, or its socket answers.
func liveHosts(root string) int {
	entries, err := os.ReadDir(root)
	if err != nil {
		return 0
	}
	n := 0
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		d := filepath.Join(root, e.Name())
		if pidAlive(hostPID(filepath.Join(d, "info.json"))) || sockAnswers(filepath.Join(d, "host.sock")) {
			n++
		}
	}
	return n
}

func hostPID(info string) int {
	var v struct {
		HostPID int `json:"hostPid"`
	}
	b, err := os.ReadFile(info)
	if err != nil || jsonx.Unmarshal(b, &v) != nil {
		return 0
	}
	return v.HostPID
}

func pidAlive(pid int) bool {
	if pid <= 0 || pid == os.Getpid() {
		return false
	}
	err := syscall.Kill(pid, 0)
	if err != nil && !errors.Is(err, syscall.EPERM) {
		return false
	}
	// A pid the host had can belong to another program by now: only a
	// rush (or agtop) counts.
	name, err := os.Readlink("/proc/" + strconv.Itoa(pid) + "/exe")
	if err != nil {
		return true
	}
	// A binary installed over since it started reads "rush (deleted)".
	base := strings.TrimSuffix(filepath.Base(name), " (deleted)")
	return base == "rush" || base == "agtop" || filepath.Ext(base) == ".test"
}

func sockAnswers(p string) bool {
	if _, err := os.Stat(p); err != nil {
		return false
	}
	c, err := net.DialTimeout("unix", p, 300*time.Millisecond)
	if err != nil {
		return false
	}
	c.Close()
	return true
}

// moveLeavingLink renames from to to and puts a link to to at from. The
// link is made first under a name of its own, so from is missing only
// between two renames; of two rushes moving it at once, one rename fails
// and that one leaves things as the other made them.
func moveLeavingLink(from, to string) error {
	if err := os.MkdirAll(filepath.Dir(to), 0o700); err != nil {
		return err
	}
	link := fmt.Sprintf("%s.rush-link-%d", from, os.Getpid())
	_ = os.Remove(link)
	if err := os.Symlink(to, link); err != nil {
		return err
	}
	if err := os.Rename(from, to); err != nil {
		_ = os.Remove(link)
		return err
	}
	return os.Rename(link, from)
}
