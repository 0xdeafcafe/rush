package gate

import (
	"context"
	"os"
	"os/exec"
	"reflect"
	"strings"
	"testing"
	"time"
)

func ctx(t *testing.T) context.Context {
	c, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	t.Cleanup(cancel)
	return c
}

func TestParallelHoldsTheThird(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	r := Rule{Parallel: 2}
	a, err := Acquire(ctx(t), r, "k", "k", "a", nil)
	if err != nil {
		t.Fatal(err)
	}
	b, err := Acquire(ctx(t), r, "k", "k", "b", nil)
	if err != nil {
		t.Fatal(err)
	}
	got := make(chan func())
	waits := make(chan Wait, 8)
	go func() {
		c, _ := Acquire(ctx(t), r, "k", "k", "c", func(w Wait) { waits <- w })
		got <- c
	}()
	if w := <-waits; w.Running != 2 || w.Ahead != 0 {
		t.Fatalf("waiting as %+v, want 2 running, none ahead", w)
	}
	select {
	case <-got:
		t.Fatal("a third got in with two running")
	case <-time.After(time.Second):
	}
	if s := Status(); len(s) != 1 || len(s[0].Running) != 2 || len(s[0].Waiting) != 1 {
		t.Fatalf("status %+v", s)
	}
	a()
	select {
	case c := <-got:
		c()
	case <-time.After(5 * time.Second):
		t.Fatal("the third didn't get in after one left")
	}
	b()
}

func TestStaggerSpacesStarts(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	r := Rule{Parallel: 2, Stagger: 600 * time.Millisecond}
	start := time.Now()
	a, _ := Acquire(ctx(t), r, "k", "k", "a", nil)
	defer a()
	b, err := Acquire(ctx(t), r, "k", "k", "b", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer b()
	if d := time.Since(start); d < 550*time.Millisecond {
		t.Fatalf("second start after %v, want the stagger", d)
	}
}

// A holder killed outright frees its slot.
func TestDeadHolderFreesItsSlot(t *testing.T) {
	if os.Getenv("GATE_HOLD") != "" {
		release, err := Acquire(context.Background(), Rule{Parallel: 1}, "k", "k", "held", nil)
		if err != nil {
			os.Exit(1)
		}
		_ = release
		os.Stdout.WriteString("held\n")
		time.Sleep(time.Minute)
		return
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	cmd := exec.Command(os.Args[0], "-test.run=^TestDeadHolderFreesItsSlot$")
	cmd.Env = append(os.Environ(), "GATE_HOLD=1")
	out, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	b := make([]byte, 5)
	if _, err := out.Read(b); err != nil || !strings.HasPrefix(string(b), "held") {
		t.Fatalf("helper said %q, %v", b, err)
	}
	c, cancel := context.WithTimeout(ctx(t), 700*time.Millisecond)
	if _, err := Acquire(c, Rule{Parallel: 1}, "k", "k", "me", nil); err == nil {
		t.Fatal("got in while the helper held the only slot")
	}
	cancel()
	_ = cmd.Process.Kill()
	_ = cmd.Wait()
	release, err := Acquire(ctx(t), Rule{Parallel: 1}, "k", "k", "me", nil)
	if err != nil {
		t.Fatal(err)
	}
	release()
}

func TestParseRules(t *testing.T) {
	got := ParseRules(map[string]string{
		"names": "tsc, go", "scope": "worktree", "parallel": "3", "stagger": "0s",
		"overrides": "tsc=system/1/10s, cargo=4, go=repo, bad", "busy": "1.5",
	})
	want := map[string]Rule{
		"tsc":   {Scope: "system", Parallel: 1, Stagger: 10 * time.Second, Busy: 1.5},
		"go":    {Scope: "repo", Parallel: 3, Busy: 1.5},
		"cargo": {Scope: "worktree", Parallel: 4, Busy: 1.5},
		"bad":   {Scope: "worktree", Parallel: 3, Busy: 1.5},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v\nwant %+v", got, want)
	}
	d := ParseRules(nil)
	if d["vitest"] != (Rule{Scope: "system", Parallel: 3, Stagger: 3 * time.Second, Busy: 1}) {
		t.Fatalf("defaults %+v", d)
	}
	for _, n := range []string{"golangci-lint", "oxlint", "playwright"} {
		if _, ok := d[n]; !ok {
			t.Errorf("%s isn't gated by default", n)
		}
	}
}

func TestKey(t *testing.T) {
	dir := t.TempDir()
	if k, _ := Key("repo", dir); !strings.HasPrefix(k, "dir:") {
		t.Fatalf("outside a repo: %s", k)
	}
	if exec.Command("git", "-C", dir, "init", "-q").Run() != nil {
		t.Skip("no git")
	}
	_ = os.Mkdir(dir+"/sub", 0o700)
	w, _ := Key("worktree", dir+"/sub")
	r, _ := Key("repo", dir+"/sub")
	if !strings.HasPrefix(w, "worktree:") || !strings.HasSuffix(r, ".git") {
		t.Fatalf("worktree %s, repo %s", w, r)
	}
}

func TestGated(t *testing.T) {
	rules := ParseRules(map[string]string{"names": "tsc, go, vitest"})
	for _, c := range []struct{ cmd, want string }{
		{"tsc --noEmit", "tsc"},
		{"cd apps/ui && FOO=1 pnpm exec tsc -b", "tsc"},
		{"pnpm --filter ui exec vitest run", "vitest"},
		{"npx -y tsc -p .", "tsc"},
		{"bunx vitest", "vitest"},
		{"git status; time go test ./... | tail", "go"},
		{"(cd x; ./node_modules/.bin/tsc)", "tsc"},
		{"echo $(go env GOPATH)", ""},
		{"echo $(go vet ./...)", "go"},
		{"tsc -w -p .", ""},
		{"pnpm exec vitest --watch", ""},
		{"go run ./cmd/server", ""},
		{"vitest run src/a.test.ts", "vitest"},
		{"nice -n 5 go build", "go"},
		{`git commit -m "go faster" && echo tsc`, ""},
		{"ls -la", ""},
		{"pnpm typecheck", ""},
		{"echo 'unclosed", ""},
		{Wrap("/bin/rush", "tsc", "/x", "tsc"), ""},
	} {
		if got := Gated(c.cmd, rules); got != c.want {
			t.Errorf("Gated(%q) = %q, want %q", c.cmd, got, c.want)
		}
	}
	if got := Gated("tsc", nil); got != "" {
		t.Errorf("gated with no rules: %q", got)
	}
}

func TestUnwrap(t *testing.T) {
	cmd := `cd 'a b' && echo "it's" | tsc`
	w := Wrap("/opt/my rush", "tsc", "/x/it's", cmd)
	if got := Unwrap(w); got != cmd {
		t.Fatalf("Unwrap(%q) = %q", w, got)
	}
	if got := Unwrap("ls"); got != "ls" {
		t.Fatalf("Unwrap(ls) = %q", got)
	}
}
