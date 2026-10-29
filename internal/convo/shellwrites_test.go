package convo

import (
	"slices"
	"testing"
)

func TestShellWrites(t *testing.T) {
	for _, c := range []struct {
		cmd  string
		want []string
	}{
		{"echo hi > out.txt 2>/dev/null; cat a | tee -a log/b.log", []string{"/w/out.txt", "/w/log/b.log"}},
		{"cd internal/host && sed -i '' 's/a b/c d/g' host.go inbox.go", []string{"/w/internal/host/host.go", "/w/internal/host/inbox.go"}},
		{"python3 -I - internal/ui/panelsubs.go <<'EOF'\nimport sys\nopen(sys.argv[1],'w').write('x > y')\ncat > fake.txt\nEOF\ngo build ./...", []string{"/w/internal/ui/panelsubs.go"}},
		{"gofmt -w a.go && go vet ./... && cp a.go /tmp/b.go && cp a.go /opt/b.go", []string{"/w/a.go", "/opt/b.go"}},
		{"git status --short; rg -n 'x > y' internal | head; go test ./... 2>&1 | tail -3", nil},
		{"perl -pi -e 's/x/y/' internal/a.go; sed -n 1,20p b.go", []string{"/w/internal/a.go"}},
		{"echo $x > \"$f\"; mv a.go b.go # > c.go", []string{"/w/b.go"}},
	} {
		var got []string
		for _, w := range ShellWrites(c.cmd, "/w") {
			got = append(got, w.Path)
		}
		if !slices.Equal(got, c.want) {
			t.Errorf("%q: %q, want %q", c.cmd, got, c.want)
		}
	}
	w := ShellWrites("cat > notes/plan.md <<'EOF'\n# Plan\nstep one\nEOF", "/w")
	if len(w) != 1 || !w[0].Whole || w[0].Content != "# Plan\nstep one\n" {
		t.Fatalf("a heredoc into a file: %+v", w)
	}
}
