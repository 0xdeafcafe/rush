package convo

import (
	"reflect"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

func TestShellLines(t *testing.T) {
	for _, c := range []struct {
		cmd  string
		want []shLine
	}{
		{`cd /x; grep -n '"a"\|";"' *.go | grep -v _test | head`, []shLine{
			{text: "cd /x"},
			{text: `grep -n '"a"\|";"' *.go`},
			{text: "| grep -v _test", depth: 1},
			{text: "| head", depth: 1},
		}},
		{`go build ./... && go test ./x 2>&1 | tail -5 || echo "a; b | c"`, []shLine{
			{text: "go build ./..."},
			{text: "&& go test ./x 2>&1"},
			{text: "| tail -5", depth: 1},
			{text: `|| echo "a; b | c"`},
		}},
		{"cat > x <<'EOF'\na; b | c\nEOF\necho $(ls | wc -l) && ok", []shLine{
			{text: "cat > x <<'EOF'"},
			{text: "a; b | c", verbatim: true},
			{text: "EOF", verbatim: true},
			{text: "echo $(ls | wc -l)"},
			{text: "&& ok"},
		}},
	} {
		if got := shellLines(c.cmd); !reflect.DeepEqual(got, c.want) {
			t.Errorf("shellLines(%q)\n got %+v\nwant %+v", c.cmd, got, c.want)
		}
	}
}

func TestShellShape(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s}
	for _, c := range []struct {
		cmd, kind, glyph, what, res string
	}{
		{"sed -n 435,446p internal/ui/question_test.go", "read", "◧", "internal/ui/question_test.go", "lines 435–446"},
		{"sed -n '/^## 12/,$p' /work/design.md | head -60", "read", "◧", "design.md", ""},
		{"cat a.go b.go", "read", "◧", "a.go, b.go", ""},
		{"tail -n 20 /tmp/log.txt", "read", "◧", "/tmp/log.txt", ""},
		{"nl -ba src/a.ts | sed -n 1,40p", "read", "◧", "src/a.ts", ""},
		{`grep -n "m\.scroll" internal/ui/*.go | grep -v _test`, "search", "⌕", `m\.scroll in internal/ui/*.go`, ""},
		{`rg -g '*.go' -e foo`, "search", "⌕", "foo", ""},
		{`grep -n "langFor\|heredocLang" *.go`, "search", "⌕", "langFor|heredocLang in *.go", ""},
		{"cat > internal/ui/zz_test.go <<'EOF'\npackage ui\n\nfunc x() {}\nEOF", "write", "✎", "internal/ui/zz_test.go", "3 lines"},
		{"cat <<'EOF' > out.txt\nhi\nEOF", "write", "✎", "out.txt", "1 line"},
		{"cat <<'EOF' > out.txt && echo done\nhi\nEOF", "", "", "", ""},
		{"cat > a.yaml <<'EOF'\nx: 1\nEOF\nbash build.sh", "", "", "", ""},
		{"python3 - <<'EOF'\nprint(1)\nprint(2)\nEOF", "script", "$", "python3 · print(1)", "2 lines"},
		{"python3 - <<'EOF'\nimport re\np = 'internal/ui/view.go'\ns = open(p).read()\nopen(p, 'w').write(s)\nEOF", "script", "$", "python3 · edits internal/ui/view.go", "4 lines"},
		{`git commit -m "readme: done"`, "commit", "$", "git commit", "“readme: done”"},
		{"cd /elsewhere && cat x.txt", "read", "◧", "x.txt", ""},
		{"sed -n 1,5p a.go; grep x b.go", "", "", "", ""},
		{"grep -rn x internal\ngrep -n y a.go", "", "", "", ""},
		{"grep -n x \\\n  a.go", "search", "⌕", "x in a.go", ""},
		{"go test ./...", "", "", "", ""},
		{"head -5", "", "", "", ""},
	} {
		sh := d.shellShape(c.cmd)
		if sh.kind != c.kind || sh.glyph != c.glyph || sh.what != c.what || sh.res != c.res {
			t.Errorf("shellShape(%q) = %+v, want %s %s %q %q", c.cmd, sh, c.kind, c.glyph, c.what, c.res)
		}
	}
	if sh := d.shellShape("cd /elsewhere && cat x.txt"); sh.in != "/elsewhere" {
		t.Errorf("a cd into another folder should say so, got %q", sh.in)
	}
}

func TestErrorLinePlace(t *testing.T) {
	for out, want := range map[string]string{
		"--- FAIL: TestDoing (0.00s)\n    doing_test.go:22: got x, want y\nFAIL\nFAIL\tgithub.com/x/claude\t0.2s\nFAIL": "doing_test.go:22: got x, want y",
		"# pkg\ninternal/ui/cleanup.go:64:10: m.clean undefined\ninternal/ui/cleanup.go:424:10: too many errors":        "internal/ui/cleanup.go:64:10: m.clean undefined",
	} {
		s := New()
		d := &drawer{s: s, t: &Turn{}, o: Options{Width: 200}, cw: 200}
		d.errorLine(&Step{Tool: "Bash", Status: Failed, Output: out}, 8, "r")
		if len(d.lines) != 1 || !strings.Contains(stripANSI(d.lines[0].Text), want) {
			t.Errorf("errorLine(%q) = %v, want %q", out, d.lines, want)
		}
	}
}

func TestChainOutputHighlighted(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 200, Verbose: true, Open: map[string]bool{}}, cw: 200}
	cmd := "sed -n 1,3p main.go; echo ---; cat settings.json; git log --format=%s -1"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	res, _ := jsonx.Marshal(map[string]string{"stdout": "package main\n\nfunc main() {}\n---\n{\"a\": true}\nfix: for the thing\n"})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var out string
	for _, l := range d.lines {
		out += l.Text + "\n"
	}
	for _, want := range []string{hlKw + "package", hlKw + "func", hlNum + "true"} {
		if !strings.Contains(out, want) {
			t.Errorf("chain output should highlight %q:\n%q", want, out)
		}
	}
	if strings.Contains(out, hlKw+"---") {
		t.Errorf("the echo's line isn't code:\n%q", out)
	}
}

// A search's hits each go under their file's path, each file's code
// brought left: indentation from different files has nothing to line up.
func TestSearchHitsAligned(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 200, Verbose: true, Open: map[string]bool{}}, cw: 200}
	in, _ := jsonx.Marshal(map[string]string{"command": `grep -rn "x" .`})
	out := "render.go:2267:\t\tlg := x\n../ui/docstyle.go:35:\t\t\tif x {\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		if _, t, ok := strings.Cut(stripANSI(l.Text), "▏"); ok {
			got = append(got, strings.TrimRight(t, " "))
		}
	}
	want := []string{"render.go", "2267  lg := x", "../ui/docstyle.go", "35  if x {"}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("hits = %q, want %q", got, want)
	}
}

// A chain's hits put each file's path on a row of its own and each hit's
// text just after its line number; bare hits keep a gutter as narrow as
// their own numbers.
func TestSearchHitsHeadedByFile(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 100, Verbose: true, Open: map[string]bool{}}, cw: 100}
	cmd := "ls services/langevals/tests | head\ngrep -n \"langevals\" a.md | head -3\ngit grep -n \"langevals\" -- 'dev/docs/adr/*' | head -5"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	p := "dev/docs/adr/004-docker-dev-environment.md"
	out := "conftest.py\n10:langevals runs here\n19:and here\n" + p + ":26:The langevals image\n" + p + ":104:more langevals\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		if _, t, ok := strings.Cut(stripANSI(l.Text), "▏"); ok {
			got = append(got, strings.TrimRight(t, " "))
		}
	}
	want := []string{"conftest.py", "10:langevals runs here", "19:and here", p, " 26  The langevals image", "104  more langevals"}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("hits = %q, want %q", got, want)
	}
}

// Hits whose paths would leave the code a sliver put each file's path
// above its hits, numbered.
func TestSearchHitsLongPaths(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 80, Verbose: true, Open: map[string]bool{}}, cw: 80}
	in, _ := jsonx.Marshal(map[string]string{"command": `grep -rn "x" .`})
	p := "enterprise/modules/governance/process/src/services/cli.service.ts"
	out := p + ":166:  async x(input) {\n" + p + ":170:    return x;\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		if t := strings.TrimSpace(strings.TrimLeft(stripANSI(l.Text), " ▏")); t != "" && !strings.HasPrefix(t, "$") && !strings.HasPrefix(t, "✓") {
			got = append(got, t)
		}
	}
	want := []string{p, "166  async x(input) {", "170    return x;"}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("hits = %q, want %q", got, want)
	}
}

// Short hits aren't lined up with long paths that went above theirs.
func TestSearchHitsMixedPaths(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 110, Verbose: true, Open: map[string]bool{}}, cw: 110}
	cmd := "cd /Users/lw/x\ngit show origin/main:platform/app/src/pages/ops/backoffice/_shell.tsx | grep -n \"Container\\|padding\" | head\n" +
		"grep -rn \"Container\\|padding=\" modules/ops/browser/src/features/backoffice/ui/sections/users-view.tsx modules/ops/browser/src/features/backoffice/ui/sections/*shell* 2>/dev/null | head"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	out := "3:import SettingsLayout from \"~/components/SettingsLayout\";\n10: * Renders inside {@link SettingsLayout} so Backoffice uses the same left\n" +
		"modules/ops/browser/src/features/backoffice/ui/sections/users-view.tsx:504:            paddingX={2}\n" +
		"modules/ops/browser/src/features/backoffice/ui/sections/backoffice-table-shell.tsx:52:      <Box paddingY={10} paddingX={4}>\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var all []string
	for _, l := range d.lines {
		all = append(all, stripANSI(l.Text))
	}
	if j := strings.Join(all, "\n"); !strings.Contains(j, "3:import SettingsLayout") && !strings.Contains(j, "3: import SettingsLayout") {
		t.Errorf("the short hit is pushed along by the long path:\n%s", strings.Join(all, "\n"))
	}
}

// Two greps on their own lines, the second with lines around its match:
// each part is highlighted in its own file's language, and a string one
// match leaves open doesn't colour the next.
func TestSearchChainHighlighted(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 200, Verbose: true, Open: map[string]bool{}}, cw: 200}
	cmd := "grep -rn \"type Agent\" internal\ngrep -n \"func x\" -A2 a-b/c.go | head -30"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	out := "internal/fleet/fleet.go:20:type Agent struct { s := `open\n" +
		"a-b/x.go:9:\treturn nil\n" +
		"823:func x() {\n824-\treturn nil\n825-}\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		got = append(got, l.Text)
	}
	all := strings.Join(got, "\n")
	for _, want := range []string{hlKw + "type", hlKw + "func", hlKw + "return"} {
		if !strings.Contains(all, want) {
			t.Errorf("search chain output should highlight %q:\n%q", want, all)
		}
	}
	if strings.Count(all, hlKw+"return") != 2 {
		t.Errorf("the open string shouldn't reach the next match:\n%q", all)
	}
}

// A script's heredoc, a grep and a git diff in one step: the grep's hits
// are highlighted in their file's language after the script's output, and
// the diff's lines are coloured by what they do.
func TestHeredocChainWithDiff(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 200, Verbose: true, Open: map[string]bool{}}, cw: 200}
	cmd := "python3 - <<'EOF'\nprint('ok')\nEOF\ngrep -n \"confirm\" internal/ui/keys.go | head\ngit diff internal/ui/fleetslash.go | head -20"
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	out := "patched\n40:\t\tif m.confirm != nil {\n" +
		"diff --git a/internal/ui/fleetslash.go b/internal/ui/fleetslash.go\n" +
		"--- a/internal/ui/fleetslash.go\n+++ b/internal/ui/fleetslash.go\n@@ -1,2 +1,3 @@\n" +
		" \tvar x = 1\n+\treturn nil\n-\tbreak\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		got = append(got, l.Text)
	}
	all := strings.Join(got, "\n")
	for _, want := range []string{hlKw + "if", hlKw + "var", hlKw + "return", hlKw + "break", plusSign(), minusSign(), paint(cText+bold, "fleetslash.go")} {
		if !strings.Contains(all, want) {
			t.Errorf("output should have %q:\n%q", want, all)
		}
	}
}

// Hits from different files, as a search across a package gives them,
// each start at the code column, not at their own files' indentation.
func TestSearchHitsEachLeft(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 160, Verbose: true, Open: map[string]bool{}}, cw: 160}
	in, _ := jsonx.Marshal(map[string]string{"command": `grep -rn '"background"' internal/ui/*.go`})
	out := "internal/ui/jobs.go:119:\t\t\thint = keys(\"b\", \"background\")\n" +
		"internal/ui/rushmode.go:44:\t\tv = append(v, \"background\")\n" +
		"internal/ui/rushmode.go:1615:\tcase \"background\":\n"
	res, _ := jsonx.Marshal(map[string]string{"stdout": out})
	d.body(&Step{Tool: "Bash", Input: in, Result: res, Status: OK}, 4)
	var got []string
	for _, l := range d.lines {
		if _, t, ok := strings.Cut(stripANSI(l.Text), "▏"); ok {
			got = append(got, strings.TrimRight(t, " "))
		}
	}
	want := []string{"internal/ui/jobs.go", "119  hint = keys(\"b\", \"background\")", "internal/ui/rushmode.go", "  44  v = append(v, \"background\")", "1615  case \"background\":"}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("hits = %q, want %q", got, want)
	}
}

// A subshell of several commands in a chain is laid out a command a line
// inside its brackets, lined up, and still counts as one of the chain's.
func TestShellSubshellLines(t *testing.T) {
	cmd := `go vet ./tools/visualdiff && (go run ./cmd/visualdiff check > run4.out 2> run4.err; echo "exit=$?" >> run4.out)`
	var got []string
	for _, l := range shellLines(cmd) {
		for _, x := range subshellLines(l) {
			got = append(got, strings.Repeat("  ", x.depth)+x.text)
		}
	}
	want := []string{"go vet ./tools/visualdiff", "&& ( go run ./cmd/visualdiff check > run4.out 2> run4.err", `  echo "exit=$?" >> run4.out)`}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("got\n%s\nwant\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	if n := len(segments(cmd)); n != 2 {
		t.Errorf("a subshell is one command of its chain: %d", n)
	}
	for _, c := range []string{`(a) && (b; c)`, `(cd x && make)`, `echo "(a; b)"`} {
		ls := shellLines(c)
		if c == `(cd x && make)` {
			if n := len(subshellLines(ls[0])); n != 2 {
				t.Errorf("%s: %d lines", c, n)
			}
			continue
		}
		if n := len(subshellLines(ls[0])); n != 1 {
			t.Errorf("%s shouldn't split: %d", c, n)
		}
	}
}
