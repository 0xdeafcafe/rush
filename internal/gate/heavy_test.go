package gate

import (
	"context"
	"strings"
	"testing"
	"time"
)

func TestHeavy(t *testing.T) {
	for _, c := range []struct {
		cmd  string
		want bool
	}{
		{"tsc -b", true},
		{"tsc -p tsconfig.json --noEmit", true},
		{"tsc --watch", false},
		{"tsc -w", false},
		{"tsc --version", false},
		{"tsgo --lsp --stdio", false},
		{"vitest", true},
		{"vitest run src", true},
		{"vitest watch", false},
		{"vitest dev", false},
		{"go test ./...", true},
		{"go build -o x .", true},
		{"go run ./cmd/x", false},
		{"go env GOPATH", false},
		{"go list -m all", false},
		{"cargo clippy", true},
		{"cargo run", false},
		{"vite", false},
		{"vite build", true},
		{"next dev", false},
		{"next build", true},
		{"playwright test e2e", true},
		{"playwright install chromium", false},
		{"golangci-lint run ./...", true},
		{"golangci-lint version", false},
		{"oxlint --type-aware src", true},
		{"eslint --help", false},
	} {
		w := strings.Fields(c.cmd)
		if got := Heavy(w[0], w[1:]); got != c.want {
			t.Errorf("Heavy(%q) = %v, want %v", c.cmd, got, c.want)
		}
	}
}

func TestNodeName(t *testing.T) {
	rules := ParseRules(nil)
	for _, c := range []struct{ script, want string }{
		{"/r/node_modules/.pnpm/typescript@7/node_modules/typescript/bin/tsc", "tsc"},
		{"/r/node_modules/.pnpm/vitest@5/node_modules/vitest/vitest.mjs", "vitest"},
		{"/r/node_modules/vitest/dist/workers/forks.js", "vitest"},
		{"/r/node_modules/@playwright/test/cli.js", "playwright"},
		{"/r/node_modules/oxlint/bin/oxlint", "oxlint"},
		{"/r/node_modules/pnpm/bin/pnpm.cjs", ""},
		{"/r/apps/api/src/main.ts", ""},
	} {
		if got := NodeName(c.script, rules); got != c.want {
			t.Errorf("NodeName(%q) = %q, want %q", c.script, got, c.want)
		}
	}
}

func TestWorkerEnv(t *testing.T) {
	t.Setenv("VITEST_MAX_WORKERS", "")
	r := Rule{Parallel: 2}
	env := WorkerEnv(r, "vitest", []string{"vitest", "run"})
	if len(env) != 1 || !strings.HasPrefix(env[0], "VITEST_MAX_WORKERS=") {
		t.Fatalf("env %v", env)
	}
	if env := WorkerEnv(r, "vitest", []string{"vitest", "--maxWorkers=1"}); env != nil {
		t.Fatalf("overrode a count it was given: %v", env)
	}
	if env := WorkerEnv(r, "tsc", []string{"tsc"}); env != nil {
		t.Fatalf("told tsc of workers: %v", env)
	}
}

// A busy machine holds a second run while one runs, never the first.
func TestBusyHoldsTheSecondNotTheFirst(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	r := Rule{Parallel: 4, Busy: 0.0001} // any load at all is too much
	if load1() <= 0 {
		t.Skip("no load average here")
	}
	a, err := Acquire(ctx(t), r, "k", "k", "a", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer a()
	waits := make(chan Wait, 4)
	c, cancel := context.WithTimeout(ctx(t), 700*time.Millisecond)
	defer cancel()
	if _, err := Acquire(c, r, "k", "k", "b", func(w Wait) { waits <- w }); err == nil {
		t.Fatal("a second got in on a busy machine")
	}
	if w := <-waits; w.Load <= 0 || w.Running != 1 {
		t.Fatalf("waited as %+v, want held back by load", w)
	}
}
