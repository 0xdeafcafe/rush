package host

import (
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"testing"
	"time"
)

// A listing with nothing changed is the last one again; a changed info,
// a host that died, a new session and a removed one each make a new one.
func TestListerReusesAnUnchangedListing(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	write := func(id, state string, pid int) {
		t.Helper()
		d := filepath.Join(Root(), id)
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
		tmp := filepath.Join(d, "info.json.tmp")
		_ = os.WriteFile(tmp, []byte(`{"id":"`+id+`","state":"`+state+`","hostPid":`+strconv.Itoa(pid)+`,"updatedAt":"`+time.Now().Format(time.RFC3339Nano)+`"}`), 0o644)
		if err := os.Rename(tmp, filepath.Join(d, "info.json")); err != nil {
			t.Fatal(err)
		}
	}
	host := exec.Command("sleep", "30")
	if err := host.Start(); err != nil {
		t.Fatal(err)
	}
	defer host.Process.Kill() //nolint:errcheck
	write("a", "working", host.Process.Pid)
	write("b", "stopped", 0)
	var l Lister
	stateOf := func(list []Info, id string) string {
		for _, in := range list {
			if in.ID == id {
				return in.State
			}
		}
		return ""
	}
	first := l.List()
	if len(first) != 2 || stateOf(first, "a") != "working" {
		t.Fatalf("first listing: %+v", first)
	}
	if again := l.List(); &again[0] != &first[0] {
		t.Fatal("an unchanged listing was made again")
	}
	_ = host.Process.Kill()
	_ = host.Wait()
	if got := stateOf(l.List(), "a"); got != "stopped" {
		t.Fatalf("its host died: %q", got)
	}
	write("b", "working", os.Getpid())
	if got := stateOf(l.List(), "b"); got != "working" {
		t.Fatalf("its info changed: %q", got)
	}
	write("c", "stopped", 0)
	if got := l.List(); len(got) != 3 {
		t.Fatalf("a new session: %d listed", len(got))
	}
	if err := os.RemoveAll(filepath.Join(Root(), "a")); err != nil {
		t.Fatal(err)
	}
	if got := l.List(); len(got) != 2 || stateOf(got, "a") != "" {
		t.Fatalf("a removed session: %+v", got)
	}
}
