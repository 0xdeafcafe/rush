package host

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/prdrive"
)

// following has a follower for session hold its lock, as a running one
// does, and returns its entry file.
func following(t *testing.T, session string, n int) string {
	t.Helper()
	if err := os.MkdirAll(prdrive.Dir(), 0o700); err != nil {
		t.Fatal(err)
	}
	base := filepath.Join(prdrive.Dir(), session+"-o_r-"+string(rune('0'+n)))
	b, _ := jsonx.Marshal(prdrive.Entry{Session: session, Repo: "o/r", Number: n})
	if err := os.WriteFile(base+".json", b, 0o600); err != nil {
		t.Fatal(err)
	}
	f, err := os.OpenFile(base+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { f.Close() })
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		t.Fatal(err)
	}
	return base + ".json"
}

func writeSession(t *testing.T, id string, meta map[string]string) {
	t.Helper()
	if err := os.MkdirAll(dir(id), 0o700); err != nil {
		t.Fatal(err)
	}
	info, _ := jsonx.Marshal(Info{ID: id})
	cfg, _ := jsonx.Marshal(Config{Meta: meta})
	if os.WriteFile(filepath.Join(dir(id), "info.json"), info, 0o600) != nil ||
		os.WriteFile(filepath.Join(dir(id), "config.json"), cfg, 0o600) != nil {
		t.Fatal("couldn't write session")
	}
}

// A session put away, asleep or not, takes its pull request followers
// with it, and those of the sessions it spawned, asleep too: a follower
// left running wakes its session with the next news, and it works on.
func TestStopAllStopsFollowersOfSleepingSessions(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "parent", nil)
	writeSession(t, "child", map[string]string{"spawnedBy": "parent"})
	writeSession(t, "other", nil)
	mine, childs, others := following(t, "parent", 1), following(t, "child", 2), following(t, "other", 3)

	if err := StopAll("parent"); err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{mine, childs} {
		if _, err := os.Stat(f); !os.IsNotExist(err) {
			t.Errorf("%s still followed after StopAll", filepath.Base(f))
		}
	}
	if _, err := os.Stat(others); err != nil {
		t.Errorf("another session's follower was stopped: %v", err)
	}
}
