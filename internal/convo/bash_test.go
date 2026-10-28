package convo

import (
	"fmt"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

func TestChainLabel(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s}
	for cmd, want := range map[string]string{
		"python3 - <<'EOF'\np='internal/ui/view.go'\ns=open(p).read()\nopen(p,'w').write(s)\nEOF\ngo build ./... && go test ./internal/ui":            "python3 edits internal/ui/view.go, go build ./..., go test ./internal/ui",
		"git status --short; ls -la internal/ui/cleanup.go; grep -n 'tempLoud' internal/ui/*.go; git log --oneline -5":                                "git status, list internal/ui/cleanup.go, search tempLoud in internal/ui/*.go, git log",
		"sed -n 280,300p internal/ui/view.go; sed -n 385,400p internal/ui/view.go; sed -n 455,470p internal/ui/model.go":                              "◧ read internal/ui/view.go:280–300, 385–400, internal/ui/model.go:455–470",
		"cd /work/app && VITEST_MAX_WORKERS=2 pnpm --filter @x/app test:unit src/foo.test.ts 2>&1 | tail -30":                                         "pnpm test:unit src/foo.test.ts in @x/app",
		`echo "=== status ==="; git status --short; echo "=== diff ==="; git diff --stat | tail -3`:                                                   "git status, git diff",
		`for f in a.go b.go; do echo "--- $f"; wc -l $f; done`:                                                                                        "count each of a.go, b.go",
		"sleep 30; gh run view 123 --log-failed | tail -50":                                                                                           "wait 30s, gh run view 123",
		"rm -f /tmp/a /tmp/b && mkdir -p /tmp/c && cp x.go /tmp/c/":                                                                                   "delete /tmp/a, /tmp/b, mkdir /tmp/c, copy x.go → /tmp/c/",
		"go build ./... && go test ./... 2>&1 | grep -v '^ok' | tail -5":                                                                              "go build ./..., go test ./...",
		"git add a.go && git commit -m \"$(cat <<'EOF'\nfix the thing\n\nmore\nEOF\n)\"":                                                              "git add a.go, git commit “fix the thing”",
		"npx tsc --noEmit -p . && uv run python -c 'import x; print(x.y)'":                                                                            "tsc, python print(x.y)",
		"sed -i '' 's/a/b/' x.go y.go; curl -s https://example.com/api/v1/ | jq .":                                                                    "edit x.go, y.go, fetch example.com/api/v1",
		"cargo +nightly fmt --all -- --check && cargo clippy --all-targets -p api -- -D warnings && cargo nextest run -p api --features foo bar::baz": "cargo +nightly fmt, cargo clippy in api, cargo nextest run bar::baz in api",
		"cargo test --package=api --test integration -- --nocapture; cargo build --release --target wasm32-unknown-unknown --bin server":              "cargo test integration in api, cargo build server",
		"rustup target add wasm32-unknown-unknown && rustup component add clippy && cargo add serde --features derive":                                "rustup target add wasm32-unknown-unknown, rustup component add clippy, cargo add serde",
		"RUST_BACKTRACE=1 cargo run --bin server -- --port 8080; cargo t -j 4 tests::x":                                                               "cargo run server, cargo test tests::x",
		// Short and single: the command itself.
		"go test ./internal/ui": "go test ./internal/ui",
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
	if got, want := stripANSI(d.label(&Step{Tool: "Bash", Input: in, Status: OK})), "go vet ./internal/convo, go test ./internal/convo"; got != want {
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

// A long output's fold row is clickable: it carries ShowAllRef.
func TestFoldRowRef(t *testing.T) {
	d := &drawer{s: New(), t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}}, cw: 120}
	var out []string
	for i := range 200 {
		out = append(out, fmt.Sprintf("line %d", i))
	}
	d.output(strings.Join(out, "\n"), 8, false)
	n := 0
	for _, l := range d.lines {
		if strings.Contains(stripANSI(l.Text), "shows all") {
			if l.Ref != ShowAllRef {
				t.Fatalf("fold row ref %q", l.Ref)
			}
			n++
		}
	}
	if n == 0 {
		t.Fatal("no fold row in 200 lines")
	}
}

func TestProgramToolchain(t *testing.T) {
	for cmd, want := range map[string]string{
		"cargo +nightly build --release": "cargo +nightly build",
		"cargo build":                    "cargo build",
		"rustup target add x":            "rustup target",
	} {
		if got := program(cmd); got != want {
			t.Errorf("program(%q) = %q, want %q", cmd, got, want)
		}
	}
}
