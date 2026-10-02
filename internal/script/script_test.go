package script

import (
	"os/exec"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestLong(t *testing.T) {
	if Long("go build ./... && go test ./...") || !Long("cd x && go build && go vet && go test") {
		t.Fatal("more than three commands is long")
	}
}

// A script stops at its breakpoint, says where, and finishes once let go;
// its trace has each command's line by the script's own count.
func TestBreakpoint(t *testing.T) {
	if Bash() == "" {
		t.Skip("no bash 4 or later")
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	cmd, err := Wrap("k1", "echo one\nf() { echo in f; }\nf\necho two && echo three\nfor i in 1 2; do echo loop $i; done")
	if err != nil || cmd == "" {
		t.Fatal(cmd, err)
	}
	if on, err := Toggle("k1", 4); !on || err != nil {
		t.Fatal(on, err)
	}
	c := exec.Command("sh", "-c", cmd)
	var out strings.Builder
	c.Stdout, c.Stderr = &out, &out
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	var r Run
	for range 200 {
		if r, _ = Read("k1"); r.HeldAt != 0 {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if r.HeldAt != 4 || r.PID == 0 || strings.Contains(out.String(), "two") {
		t.Fatalf("held at %d, pid %d, out %q", r.HeldAt, r.PID, out.String())
	}
	_ = syscall.Kill(r.PID, syscall.SIGCONT)
	if err := c.Wait(); err != nil {
		t.Fatal(err)
	}
	r, _ = Read("k1")
	var lines []int
	for _, h := range r.Hits {
		lines = append(lines, h.Line)
	}
	if r.HeldAt != 0 || out.String() != "one\nin f\ntwo\nthree\nloop 1\nloop 2\n" || lines[0] != 1 || lines[len(lines)-1] != 5 {
		t.Fatalf("held %d, out %q, lines %v", r.HeldAt, out.String(), lines)
	}
	if on, _ := Toggle("k1", 4); on || len(Breaks("k1")) != 0 {
		t.Fatal("toggled again, the breakpoint goes")
	}
}
