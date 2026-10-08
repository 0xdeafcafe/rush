package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

func TestChainLabel(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s}
	for cmd, want := range map[string]string{
		"python3 - <<'EOF'\np='internal/ui/view.go'\ns=open(p).read()\nopen(p,'w').write(s)\nEOF\ngo build ./... && go test ./internal/ui": "$ python3 edits internal/ui/view.go › go build ./... › go test ./internal/ui",
		"git status --short; ls -la internal/ui/cleanup.go; grep -n 'tempLoud' internal/ui/*.go; git log --oneline -5":                     "$ git status › list internal/ui/cleanup.go › search tempLoud in internal/ui/*.go › git log",
		"sed -n 280,300p internal/ui/view.go; sed -n 385,400p internal/ui/view.go; sed -n 455,470p internal/ui/model.go":                   "◧ read internal/ui/view.go:280–300, 385–400, internal/ui/model.go:455–470",
		"cd /work/app && VITEST_MAX_WORKERS=2 pnpm --filter @x/app test:unit src/foo.test.ts 2>&1 | tail -30":                              "$ in app · pnpm test:unit src/foo.test.ts in @x/app",
		`echo "=== status ==="; git status --short; echo "=== diff ==="; git diff --stat | tail -3`:                                        "$ git status › git diff",
		`for f in a.go b.go; do echo "--- $f"; wc -l $f; done`:                                                                             "$ count each of a.go, b.go",
		"sleep 30; gh run view 123 --log-failed | tail -50":                                                                                "$ wait 30s › gh run view 123",
		"rm -f /tmp/a /tmp/b && mkdir -p /tmp/c && cp x.go /tmp/c/":                                                                        "$ delete /tmp/a, /tmp/b › mkdir /tmp/c › copy x.go → /tmp/c/",
		"go build ./... && go test ./... 2>&1 | grep -v '^ok' | tail -5":                                                                   "$ go build ./... › go test ./...",
		"git add a.go && git commit -m \"$(cat <<'EOF'\nfix the thing\n\nmore\nEOF\n)\"":                                                   "$ git add a.go › git commit “fix the thing”",
		"npx tsc --noEmit -p . && uv run python -c 'import x; print(x.y)'":                                                                 "$ tsc › python print(x.y)",
		"sed -i '' 's/a/b/' x.go y.go; curl -s https://example.com/api/v1/ | jq .":                                                         "$ edit x.go, y.go › fetch example.com/api/v1",
		// Short and single: the command itself.
		"go test ./internal/ui": "$ go test ./internal/ui",
	} {
		in, _ := jsonx.Marshal(map[string]string{"command": cmd})
		if got := stripANSI(d.label(&Step{Tool: "Bash", Input: in, Status: OK})); got != want {
			t.Errorf("label(%q)\n got %q\nwant %q", cmd, got, want)
		}
	}
}

func TestEchoMarks(t *testing.T) {
	s := New()
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}}, cw: 120}
	cmd := `echo "=== status ==="; git status --short; echo ---; echo "n=$n"; git log -1`
	d.marks = echoMarks(cmd)
	d.output("=== status ===\n M a.go\n---\nabc123 fix\nn=3", 8, false)
	var got []string
	for _, l := range d.lines {
		got = append(got, strings.TrimSpace(strings.TrimLeft(strings.TrimSpace(stripANSI(l.Text)), "▏")))
	}
	want := []string{"── status ──", "M a.go", strings.Repeat("─", 24), "abc123 fix", "n=3"}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Errorf("output with marks\n got %q\nwant %q", got, want)
	}
}

func TestChainFlagValues(t *testing.T) {
	s := New()
	d := &drawer{s: s}
	in, _ := jsonx.Marshal(map[string]string{"command": "go vet ./internal/convo && go test ./internal/convo -run ZZPeek -count=1 -v"})
	if got, want := stripANSI(d.label(&Step{Tool: "Bash", Input: in, Status: OK})), "$ go vet ./internal/convo › go test ./internal/convo"; got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}

// A chain's parts line up their own lines: grep's long path prefix doesn't
// push sed's code or ls's names out to meet it.
func TestChainOutputAlignsEachPart(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}, Verbose: true}, cw: 120}
	d.spans = d.chainSpans(`sed -n 1,3p internal/convo/render.go; grep -rn "Cwd" internal/convo/model.go | head; ls internal`)
	d.output("}\n\nfunc bashOut(st *Step) string {\ninternal/convo/model.go:12:\tCwd string\ninternal/convo/model.go:40:\ts.Cwd = cwd\nsqueeze\nstate", 8, false)
	var got []string
	for _, l := range d.lines {
		got = append(got, strings.TrimRight(strings.SplitN(stripANSI(l.Text), "▏", 2)[1], " "))
	}
	want := []string{"}", "", "func bashOut(st *Step) string {", "internal/convo/model.go", "12  Cwd string", "40  s.Cwd = cwd", "squeeze", "state"}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Errorf("chain output\n got %q\nwant %q", got, want)
	}
}
