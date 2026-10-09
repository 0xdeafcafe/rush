package host

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	claudead "github.com/0xdeafcafe/rush/internal/adapters/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// When Claude Code moves into a worktree, the host follows: the list shows
// the worktree, and a restart resumes there.
func TestFollowCwd(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	cfgDir, start, wt := t.TempDir(), t.TempDir(), t.TempDir()
	s := &server{cfg: Config{ID: "cwd", Kind: string(claudead.Kind), Cwd: start, Account: agent.Profile{Dir: cfgDir}}, clients: map[*conn]struct{}{}}
	s.info = Info{SessionID: "s1", Cwd: start, ClaudePID: 4242}
	s.startCwd = start
	if err := exec.Command("git", "init", "-q", wt).Run(); err != nil { // a checkout of its own
		t.Fatal(err)
	}
	for _, d := range []string{filepath.Join(cfgDir, "sessions"), dir(s.cfg.ID)} {
		if err := os.MkdirAll(d, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	write := func(sid, cwd string) {
		b := []byte(`{"pid":4242,"sessionId":"` + sid + `","cwd":"` + cwd + `"}`)
		if err := os.WriteFile(filepath.Join(cfgDir, "sessions", "4242.json"), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	// follow looks, then gives the look (git, off the lock) time to land:
	// at least 50ms, and up to 5s for it to reach want.
	follow := func(want string) string {
		s.mu.Lock()
		s.followCwd(true)
		s.mu.Unlock()
		for end, least := time.Now().Add(5*time.Second), time.Now().Add(50*time.Millisecond); time.Now().Before(end); time.Sleep(10 * time.Millisecond) {
			s.mu.Lock()
			at := s.cfg.Cwd
			s.mu.Unlock()
			if at == want && time.Now().After(least) {
				break
			}
		}
		s.mu.Lock()
		defer s.mu.Unlock()
		if s.info.Cwd != s.cfg.Cwd {
			t.Fatalf("info %q, config %q", s.info.Cwd, s.cfg.Cwd)
		}
		return s.cfg.Cwd
	}
	write("other", wt)
	if got := follow(start); got != start {
		t.Fatalf("another session's file moved it to %q", got)
	}
	write("s1", filepath.Join(wt, "gone"))
	if got := follow(start); got != start {
		t.Fatalf("a folder that doesn't exist moved it to %q", got)
	}
	write("s1", wt)
	if got := follow(wt); got != wt {
		t.Fatalf("cwd %q, want the worktree %q", got, wt)
	}
	if b, err := os.ReadFile(filepath.Join(dir(s.cfg.ID), "config.json")); err != nil || !strings.Contains(string(b), wt) {
		t.Fatalf("config.json should resume in the worktree: %s %v", b, err)
	}
	// Its shell going back to where the process started, between commands,
	// is no move: the session stays where it went.
	write("s1", start)
	if got := follow(wt); got != wt {
		t.Fatalf("the shell's reset pulled it back to %q", got)
	}
}
