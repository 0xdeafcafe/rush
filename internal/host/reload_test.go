package host

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/events"
)

// installAs makes the rush a restart starts hosts on a script, with the
// test binary kept beside it as the rush before it.
func installAs(t *testing.T, script string) {
	t.Helper()
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	exe := filepath.Join(os.Getenv("RUSH_HOME"), "rush")
	if err := os.WriteFile(exe, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(self, PrevExe(exe)); err != nil {
		t.Fatal(err)
	}
	was := installedExe
	installedExe = func() (string, error) { return exe, nil }
	t.Cleanup(func() { installedExe = was })
}

func spawnIdle(t *testing.T, sid string) (Config, Info) {
	t.Helper()
	bin := setup(t)
	acct := claude.Account{Name: "t", ConfigDir: t.TempDir()}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin, Account: acct.Profile(), SessionID: sid})
	if err != nil {
		t.Fatal(err)
	}
	info, _ := ReadInfo(cfg.ID)
	return cfg, info
}

func sessionEvents(t *testing.T, id string) string {
	t.Helper()
	events.Flush()
	b, _ := os.ReadFile(EventsPath(id))
	return string(b)
}

// An installed rush that says it can't run the session is never started:
// the old host isn't stopped, and the restart says why.
func TestRestartKeepsTheHostWhenPreflightFails(t *testing.T) {
	cfg, old := spawnIdle(t, "pref-0000-aaaa")
	installAs(t, "#!/bin/sh\necho 'rush: unknown agent \"nova\"' >&2\nexit 1\n")
	err := Restart(cfg.ID, func(*Config) {})
	if err == nil || !strings.Contains(err.Error(), "unknown agent") {
		t.Fatalf("restart: %v", err)
	}
	now, _ := ReadInfo(cfg.ID)
	if now.HostPID != old.HostPID || !alive(old.HostPID) {
		t.Fatalf("the old host was stopped: pid %d, was %d", now.HostPID, old.HostPID)
	}
	if ev := sessionEvents(t, cfg.ID); !strings.Contains(ev, "reload-refused") {
		t.Fatalf("events:\n%s", ev)
	}
}

// A new rush that says it can, then whose host doesn't come up, is
// replaced by a host on the rush before it; the restart still fails, and
// what the failed host wrote is in the session's events.
func TestRestartRollsBackWhenTheNewHostDoesNotStart(t *testing.T) {
	cfg, old := spawnIdle(t, "roll-0000-aaaa")
	installAs(t, "#!/bin/sh\ncase \"$2\" in\npreflight) echo '"+`{"ok":true}`+"' ;;\n*) echo 'panic: new build broke' >&2; exit 2 ;;\nesac\n")
	var l Lister
	stale, restarted, failed, _ := l.Sweep(BinStamp{Mod: old.Exe.Mod.Add(1e9)}, true)
	if failed != 1 || restarted != 0 || stale != 1 {
		t.Fatalf("stale %d restarted %d failed %d", stale, restarted, failed)
	}
	now, _ := ReadInfo(cfg.ID)
	if now.HostPID == old.HostPID || !alive(now.HostPID) || now.Gen != old.Gen+1 {
		t.Fatalf("not rolled back: %+v (was pid %d, host %d)", now, old.HostPID, old.Gen)
	}
	ev := sessionEvents(t, cfg.ID)
	if !strings.Contains(ev, "reload-rolled-back") || !strings.Contains(ev, "new build broke") || !strings.Contains(ev, "restart-failed") {
		t.Fatalf("events:\n%s", ev)
	}
}

// Preflight reads the session's config, finds its folder and its agent.
func TestPreflight(t *testing.T) {
	cfg, _ := spawnIdle(t, "pfok-0000-aaaa")
	if err := Preflight(cfg.ID); err != nil {
		t.Fatalf("preflight: %v", err)
	}
	if err := preflight(os.Args[0], cfg.ID); err != nil {
		t.Fatalf("asked of this build: %v", err)
	}
	gone := filepath.Join(t.TempDir(), "removed-worktree")
	b := []byte(`{"id":"` + cfg.ID + `","cwd":"` + gone + `"}`)
	if err := os.WriteFile(filepath.Join(dir(cfg.ID), "config.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := Preflight(cfg.ID); err == nil || !strings.Contains(err.Error(), "folder is gone") {
		t.Fatalf("a session whose worktree was removed: %v", err)
	}
	b = []byte(`{"id":"` + cfg.ID + `","kind":"nova"}`)
	if err := os.WriteFile(filepath.Join(dir(cfg.ID), "config.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := Preflight(cfg.ID); err == nil || !strings.Contains(err.Error(), "nova") {
		t.Fatalf("an agent this rush doesn't know: %v", err)
	}
}

// A session whose folder is gone is never restarted: it's counted gone,
// not stale or failed, and its host carries on.
func TestSweepCountsAGoneFolderApart(t *testing.T) {
	cfg, old := spawnIdle(t, "gone-0000-aaaa")
	b := []byte(`{"id":"` + cfg.ID + `","cwd":"` + filepath.Join(t.TempDir(), "removed-worktree") + `"}`)
	if err := os.WriteFile(filepath.Join(dir(cfg.ID), "config.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	var l Lister
	stale, restarted, failed, gone := l.Sweep(BinStamp{Mod: old.Exe.Mod.Add(1e9)}, true)
	if gone != 1 || stale != 0 || restarted != 0 || failed != 0 {
		t.Fatalf("stale %d restarted %d failed %d gone %d", stale, restarted, failed, gone)
	}
	if now, _ := ReadInfo(cfg.ID); now.HostPID != old.HostPID || !alive(old.HostPID) {
		t.Fatalf("the host was touched: pid %d, was %d", now.HostPID, old.HostPID)
	}
}
