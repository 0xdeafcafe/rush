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

// stubMemory makes the machine have avail of 64 GiB, and no reserve
// matters past what the test asks.
func stubMemory(t *testing.T, avail uint64) {
	old := Memory
	Memory = func() (uint64, uint64) { return avail, 64 * gib }
	oldLevel := MemoryLevel
	MemoryLevel = func() int { return -1 }
	t.Cleanup(func() { Memory, MemoryLevel = old, oldLevel })
}

// A run waits while there's no memory for it beside another, and gets in
// once that one's gone; one always may when none run.
func TestMemoryHoldsTheSecond(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	stubMemory(t, 4*gib)
	big := Ask{Name: "tsc", Rule: Rule{Memory: 8 * gib}, Session: "a"}
	a, err := Acquire(ctx(t), big, nil)
	if err != nil {
		t.Fatal(err)
	}
	got := make(chan *Run)
	waits := make(chan Wait, 8)
	go func() {
		b, _ := Acquire(ctx(t), Ask{Name: "go", Rule: Rule{Memory: 2 * gib}, Session: "b"}, func(w Wait) { waits <- w })
		got <- b
	}()
	if w := <-waits; w.Running != 1 || w.Need != 2*gib {
		t.Fatalf("waiting as %+v, want held for memory beside one running", w)
	}
	select {
	case <-got:
		t.Fatal("got in with no memory for it")
	case <-time.After(time.Second):
	}
	if run, wait := Status(0); len(run) != 1 || len(wait) != 1 || !strings.HasPrefix(wait[0].Why, "for memory") {
		t.Fatalf("status %+v %+v", run, wait)
	}
	a.Release()
	select {
	case b := <-got:
		b.Release()
	case <-time.After(5 * time.Second):
		t.Fatal("the second didn't get in after the first left")
	}
}

// A holder killed outright frees its place.
func TestDeadHolderFreesItsPlace(t *testing.T) {
	if os.Getenv("GATE_HOLD") != "" {
		Memory = func() (uint64, uint64) { return 0, 64 * gib }
		if _, err := Acquire(context.Background(), Ask{Name: "held", Rule: Rule{Memory: gib}}, nil); err != nil {
			os.Exit(1)
		}
		os.Stdout.WriteString("held\n")
		time.Sleep(time.Minute)
		return
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	stubMemory(t, 0)
	cmd := exec.Command(os.Args[0], "-test.run=^TestDeadHolderFreesItsPlace$")
	cmd.Env = append(os.Environ(), "GATE_HOLD=1")
	out, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	b := make([]byte, 5)
	if _, err := out.Read(b); err != nil || !strings.HasPrefix(string(b), "held") {
		t.Fatalf("helper said %q, %v", b, err)
	}
	c, cancel := context.WithTimeout(ctx(t), 1200*time.Millisecond)
	if _, err := Acquire(c, Ask{Name: "me", Rule: Rule{Memory: gib}}, nil); err == nil {
		t.Fatal("got in with no memory while the helper ran")
	}
	cancel()
	_ = cmd.Process.Kill()
	_ = cmd.Wait()
	run, err := Acquire(ctx(t), Ask{Name: "me", Rule: Rule{Memory: gib}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	run.Release()
}

func TestParseRules(t *testing.T) {
	got := ParseRules(map[string]string{
		"names": "tsc, go", "limit": "20m", "scope": "worktree", "parallel": "3", "overrides": "tsc=system/1/10s",
		"limits": "tsc=6G/45m, cargo=512M, go=1h, bad",
	})
	want := map[string]Rule{
		"tsc":   {Memory: 6 * gib, Limit: 45 * time.Minute},
		"go":    {Memory: 2 * gib, Limit: time.Hour},
		"cargo": {Memory: 512 << 20, Limit: 20 * time.Minute},
		"bad":   {Memory: otherEstimate, Limit: 20 * time.Minute},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v\nwant %+v", got, want)
	}
	d := ParseRules(nil)
	if d["vitest"] != (Rule{Memory: 3 * gib, Limit: 30 * time.Minute}) || d["oxlint"].Memory != gib || d["tsgo"].Memory != 4*gib {
		t.Fatalf("defaults %+v", d)
	}
	for _, n := range []string{"golangci-lint", "oxlint", "playwright"} {
		if _, ok := d[n]; !ok {
			t.Errorf("%s isn't gated by default", n)
		}
	}
	for in, want := range map[string]float64{"15%": 0.15, "0.2": 0.2, "25": 0.25, "nonsense": 0.15} {
		if got := ParseReserve(map[string]string{"reserve": in}); got != want {
			t.Errorf("ParseReserve(%q) = %v, want %v", in, got, want)
		}
	}
	for in, want := range map[string]uint64{"6G": 6 * gib, "1.5GiB": 3 * gib / 2, "512mb": 512 << 20, "4": 0, "10s": 0} {
		if got, _ := ParseSize(in); got != want {
			t.Errorf("ParseSize(%q) = %d, want %d", in, got, want)
		}
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
		{"cat > b.md <<'EOF'\nruns one server (tsc where it can\nEOF\ngh pr create", ""},
		{"cat <<-END | wc\n\t(tsc\n\tEND\ngo test ./...", "go"},
		{"cat <<A <<\"B\"\n(tsc\nA\n(vitest\nB\nls", ""},
		{"cat <<< tsc", ""},
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
	doc := "cat > m.txt <<'EOF'\nfix\nEOF\ngo test ./..."
	if got := Unwrap(Wrap("/r", "go", "/x", doc)); got != doc {
		t.Fatalf("Unwrap of a here-document's = %q", got)
	}
}

func TestHereDoc(t *testing.T) {
	for cmd, want := range map[string]bool{
		"cat <<EOF\nx\nEOF":           true,
		"python3 - <<'EOF'\nx\nEOF":   true,
		"cat <<-\"END\"\n\tx\nEND":    true,
		`grep x <<< "$s"`:             false,
		"echo 'a << b'; ls":           true, // quoted, but taking it as one only costs the wrapping
		"for i in 1 2; do echo; done": false,
	} {
		if got := HereDoc(cmd); got != want {
			t.Errorf("HereDoc(%q) = %v", cmd, got)
		}
	}
}

// A process whose child holds a running ticket is gated already: node
// replaced in place by a shim must not wait on its own holder's ticket.
func TestHeldForTheHoldersParent(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	stubMemory(t, 64*gib)
	if HeldFor(os.Getppid()) {
		t.Fatal("held before any ticket")
	}
	run, err := Acquire(ctx(t), Ask{Name: "vitest", Rule: Rule{Memory: gib}, Session: "a"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer run.Release()
	if !HeldFor(os.Getppid()) {
		t.Fatal("the holder's parent isn't held")
	}
	if HeldFor(os.Getpid()) {
		t.Fatal("the holder itself counts as held")
	}
}
