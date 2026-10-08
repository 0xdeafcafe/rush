package state

import (
	"bytes"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func agtopTree(t *testing.T) (config, cache string) {
	t.Helper()
	// Short: a unix socket's path is capped near 100 bytes.
	dir, err := os.MkdirTemp("/tmp", "rush-migrate")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	config, cache = filepath.Join(dir, "config"), filepath.Join(dir, "cache")
	os.MkdirAll(filepath.Join(config, "agtop", "sessions", "abc", "tmp"), 0o700)
	os.WriteFile(filepath.Join(config, "agtop", "config.json"), []byte("{}"), 0o600)
	os.MkdirAll(filepath.Join(cache, "agtop"), 0o700)
	os.WriteFile(filepath.Join(cache, "agtop", "costs.json"), []byte("{}"), 0o600)
	return config, cache
}

func isLink(p string) bool {
	fi, err := os.Lstat(p)
	return err == nil && fi.Mode()&os.ModeSymlink != 0
}

// Both agtop folders move to rush's, each old path left as a link that
// still reaches the same files.
func TestMigrateMovesLeavingLinks(t *testing.T) {
	config, cache := agtopTree(t)
	held, err := os.Create(filepath.Join(config, "agtop", "sessions", "abc", "host.log"))
	if err != nil {
		t.Fatal(err)
	}
	defer held.Close()
	var out bytes.Buffer
	if err := migrate(&out, config, cache); err != nil {
		t.Fatal(err)
	}
	for _, root := range []string{config, cache} {
		if !isLink(filepath.Join(root, "agtop")) || !realDir(filepath.Join(root, "rush")) {
			t.Fatalf("%s not moved: %s", root, out.String())
		}
	}
	held.WriteString("still here\n")
	if b, _ := os.ReadFile(filepath.Join(config, "rush", "sessions", "abc", "host.log")); string(b) != "still here\n" {
		t.Fatalf("an open file lost its writes: %q", b)
	}
	os.WriteFile(filepath.Join(config, "agtop", "sessions", "abc", "tmp", "x"), []byte("x"), 0o600)
	if _, err := os.Stat(filepath.Join(config, "rush", "sessions", "abc", "tmp", "x")); err != nil {
		t.Fatal("a write through agtop's path did not land in rush's")
	}
	if got := pick(filepath.Join(config, "rush"), filepath.Join(config, "agtop"), "config.json"); got != filepath.Join(config, "rush") {
		t.Fatalf("pick after: %s", got)
	}
	// Again: already links, nothing to do, nothing left behind.
	out.Reset()
	if err := migrate(&out, config, cache); err != nil || out.Len() != 0 {
		t.Fatalf("again: %v %q", err, out.String())
	}
	if entries, _ := os.ReadDir(config); len(entries) != 2 {
		t.Fatalf("left behind %v", entries)
	}
}

// A host still answering on its socket under agtop's folder keeps both
// folders where they are, and says so.
func TestMigrateSkipsWhileHostRuns(t *testing.T) {
	config, cache := agtopTree(t)
	l, err := net.Listen("unix", filepath.Join(config, "agtop", "sessions", "abc", "host.sock"))
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	var out bytes.Buffer
	if err := migrate(&out, config, cache); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "still run") {
		t.Fatalf("said %q", out.String())
	}
	for _, root := range []string{config, cache} {
		if !realDir(filepath.Join(root, "agtop")) || exists(filepath.Join(root, "rush")) {
			t.Fatalf("%s moved while a host runs", root)
		}
	}
	l.Close()
	out.Reset()
	if err := migrate(&out, config, cache); err != nil || !isLink(filepath.Join(config, "agtop")) {
		t.Fatalf("after the host stopped: %v %q", err, out.String())
	}
}

// A host's pid that is gone, or was never a pid, doesn't hold the move.
func TestMigrateIgnoresDeadHost(t *testing.T) {
	config, cache := agtopTree(t)
	info := `{"hostPid":` + strconv.Itoa(1<<22-3) + `}`
	os.WriteFile(filepath.Join(config, "agtop", "sessions", "abc", "info.json"), []byte(info), 0o600)
	var out bytes.Buffer
	if err := migrate(&out, config, cache); err != nil || !isLink(filepath.Join(config, "agtop")) {
		t.Fatalf("%v %q", err, out.String())
	}
}

// With both folders there, nothing moves.
func TestMigrateLeavesBoth(t *testing.T) {
	config, cache := agtopTree(t)
	os.MkdirAll(filepath.Join(config, "rush"), 0o700)
	var out bytes.Buffer
	if err := migrate(&out, config, cache); err != nil {
		t.Fatal(err)
	}
	if !realDir(filepath.Join(config, "agtop")) || !strings.Contains(out.String(), "both") {
		t.Fatalf("agtop's was replaced: %q", out.String())
	}
	if !isLink(filepath.Join(cache, "agtop")) {
		t.Fatal("the cache, alone there, should still move")
	}
}

// Neither there: nothing is made.
func TestMigrateNeither(t *testing.T) {
	dir := t.TempDir()
	var out bytes.Buffer
	if err := migrate(&out, dir, ""); err != nil || out.Len() != 0 {
		t.Fatalf("%v %q", err, out.String())
	}
	if entries, _ := os.ReadDir(dir); len(entries) != 0 {
		t.Fatalf("made %v", entries)
	}
}

// Tests never move the machine's own folders.
func TestMigrateRefusesInTests(t *testing.T) {
	if err := Migrate(&bytes.Buffer{}); err == nil {
		t.Fatal("Migrate ran from a test")
	}
}
