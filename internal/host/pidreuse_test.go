package host

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// A session whose host ended without saying so, and whose pid the system
// then gave to another process, is not running.
func TestHostPIDTakenByALaterProcessIsNotTheHost(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", "")
	id := "reused00"
	d := dir(id)
	if err := os.MkdirAll(d, 0o700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(d, "info.json")
	b, _ := jsonx.Marshal(Info{ID: id, HostPID: os.Getpid(), State: "idle", Sleeping: true})
	if err := os.WriteFile(path, b, 0o600); err != nil {
		t.Fatal(err)
	}

	info, err := ReadInfo(id)
	if err != nil || !alive(info.HostPID) {
		t.Fatalf("a host that wrote its info after it began is running: pid %d, err %v", info.HostPID, err)
	}

	// The info is from before this process began: its host was another one.
	old := time.Now().Add(-24 * time.Hour)
	if err := os.Chtimes(path, old, old); err != nil {
		t.Fatal(err)
	}
	info, err = ReadInfo(id)
	if err != nil || info.HostPID != 0 || alive(info.HostPID) {
		t.Fatalf("a process begun after the info was written is not its host: pid %d, err %v", info.HostPID, err)
	}
}
