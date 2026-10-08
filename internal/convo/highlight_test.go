package convo

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Highlighting only colours: the text that comes back is the text that went
// in, whatever the language, the state carried in, or the emphasis.
func TestHighlightKeepsText(t *testing.T) {
	files, _ := filepath.Glob("*.go")
	extra := []string{
		"const x = `a ${b}` // c", "def f(x):\n    '''doc\n    more'''\n    return x", `{"a": [1, 2.5, true, null], "b\"c": "d"}`,
		"key: value # c\nlist:\n  - 'x'", "SELECT id FROM t WHERE a = 'x' -- c", "echo \"$HOME\" ${X} $1 # c", "/* open\nstill\nclosed */ x",
		"héllo wörld → ✓ \"ünïcode\"", "", "'", "\"unterminated", "`", "0x1F 1e10 3.14", "$", "#",
	}
	var inputs []string
	for _, f := range files {
		b, _ := os.ReadFile(f)
		inputs = append(inputs, string(b))
	}
	inputs = append(inputs, extra...)
	for _, lg := range []*lang{nil, langGo, langJS, langPy, langRust, langSh, langJSON, langYAML, langTOML, langCSS, langSQL, langRuby, langC, langLua, langPHP, langMD} {
		for _, src := range inputs {
			var st hlState
			for _, l := range strings.Split(src, "\n") {
				if got := stripANSI(highlight(lg, &st, l, cText, nil)); got != l {
					t.Fatalf("highlight changed %q to %q", l, got)
				}
				for _, em := range []emph{{0, len(l), bgAddHi, bgAdd}, {len(l) / 3, len(l) / 2, bgAddHi, bgAdd}} {
					st2 := st
					if got := stripANSI(highlight(lg, &st2, l, cText, &em)); got != l {
						t.Fatalf("highlight with emphasis changed %q to %q", l, got)
					}
				}
			}
		}
	}
}

func TestHighlightClasses(t *testing.T) {
	var st hlState
	got := highlight(langGo, &st, `func main() { return "x" // done`, cText, nil)
	for _, want := range []string{hlKw + "func", hlFn + "main", hlStr + `"x"`, hlComment + "// done"} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q in %q", want, got)
		}
	}
	// A block comment carries over lines.
	st = hlState{}
	highlight(langGo, &st, "x /* start", cText, nil)
	if got := highlight(langGo, &st, "middle", cText, nil); !strings.HasPrefix(got, hlComment) || st == (hlState{}) {
		t.Errorf("a block comment should go on: %q", got)
	}
	// JSON keys and values differ.
	st = hlState{}
	got = highlight(langJSON, &st, `"name": "rush"`, cText, nil)
	if !strings.Contains(got, hlFn+`"name"`) || !strings.Contains(got, hlStr+`"rush"`) {
		t.Errorf("json key/value: %q", got)
	}
}

// Markdown's parts each have their colour; a fence carries over lines.
func TestHighlightMarkdown(t *testing.T) {
	var st hlState
	for line, want := range map[string][]string{
		"## 5. Work completed":                 {hlKw + "## 5. Work completed"},
		"- **process-stores:** two `new` ones": {hlNum + "- ", hlType + "**process-stores:**", hlStr + "`new`"},
		"| a.ts | rewritten |":                 {hlComment + "|", cText + " a.ts "},
		"|---|---|":                            {hlComment + "|---|---|"},
		"see [the spec](https://x.y) now":      {hlFn + "the spec", hlComment + "](https://x.y)"},
		"12. twelfth":                          {hlNum + "12. "},
	} {
		got := highlight(langMD, &st, line, cText, nil)
		for _, w := range want {
			if !strings.Contains(got, w) {
				t.Errorf("%q: missing %q in %q", line, w, got)
			}
		}
	}
	st = hlState{}
	highlight(langMD, &st, "```go", cText, nil)
	if got := highlight(langMD, &st, "# not a heading", cText, nil); strings.Contains(got, hlKw) {
		t.Errorf("a fence's inside isn't Markdown: %q", got)
	}
	if got := highlight(langMD, &st, "return nil", cText, nil); !strings.Contains(got, hlKw+"return") {
		t.Errorf("a go fence's inside should be Go: %q", got)
	}
	highlight(langMD, &st, "```", cText, nil)
	if st != (hlState{}) {
		t.Error("the fence should close")
	}
	highlight(langMD, &st, "```", cText, nil)
	if got := highlight(langMD, &st, "# plain", cText, nil); !strings.HasPrefix(got, hlStr) {
		t.Errorf("a fence with no language should be plain code: %q", got)
	}
}

// Marked, a space shows as · and a tab as → and three spaces, in their
// own colour; the rest of the line keeps its own.
func TestPaintCodeMarksSpace(t *testing.T) {
	var st hlState
	got := paintCode(langGo, &st, "\treturn x ", cText, nil, true)
	if s := stripANSI(got); s != "→   return·x·" {
		t.Fatalf("marked = %q", s)
	}
	if !strings.Contains(got, hlSpace+"→") || !strings.Contains(got, hlKw+"return") {
		t.Errorf("colours: %q", got)
	}
	em := emph{from: 1, to: 7, on: bgAddHi, off: bgAdd}
	st = hlState{}
	if s := stripANSI(paintCode(langGo, &st, "a b\tc", cText, &em, true)); s != "a·b→   c" {
		t.Errorf("marked with emphasis = %q", s)
	}
}

func TestChanged(t *testing.T) {
	for _, c := range []struct {
		a, b     string
		from, to int
		ok       bool
	}{
		{"return x + 1", "return x + 2", 11, 12, true},
		{"foo(bar)", "foo(baz, qux)", 6, 7, true},
		{"abc", "xyz", 0, 0, false},
		{"same", "same", 0, 0, false},
		{"héllo wörld", "héllo world", 8, 10, true}, // é is two bytes
	} {
		from, to, ok := changed(c.a, c.b)
		if ok != c.ok || ok && (from != c.from || to != c.to) {
			t.Errorf("changed(%q, %q) = %d, %d, %v; want %d, %d, %v", c.a, c.b, from, to, ok, c.from, c.to, c.ok)
		}
	}
}

func TestLangFor(t *testing.T) {
	for name, want := range map[string]*lang{
		"go": langGo, "internal/ui/view.go": langGo, "App.tsx": langJS, "x.py": langPy, "Makefile": langSh,
		"config.yaml": langYAML, "TypeScript": langJS, "README.md": langMD, "": nil, "noext": nil,
	} {
		if got := langFor(name); got != want {
			t.Errorf("langFor(%q) wrong", name)
		}
	}
	for line, want := range map[string]*lang{
		"python3 - <<'EOF'": langPy, "cat > src/app.ts <<'EOF'": langJS, "cat <<EOF > x.json": langJSON, "node <<'JS'": langJS, "cat > notes.txt <<EOF": nil,
	} {
		if got := heredocLang(line); got != want {
			t.Errorf("heredocLang(%q) wrong", line)
		}
	}
}

func TestCodePrefix(t *testing.T) {
	for l, want := range map[string]string{
		"    12→\tfunc x()": "    12→", "12\tfunc x()": "12\t", "internal/ui/view.go:1749:func x": "internal/ui/view.go:1749:",
		"a.go-12-  ctx": "a.go-12-", "no prefix here": "", "12:30 meeting": "12:",
		"a-b/c.go:12:x": "a-b/c.go:12:", "824-\treturn": "824-",
	} {
		n, _ := codePrefix(l)
		if l[:n] != want {
			t.Errorf("codePrefix(%q) = %q, want %q", l, l[:n], want)
		}
	}
}

func BenchmarkHighlight(b *testing.B) {
	src, _ := os.ReadFile("render.go")
	lines := strings.Split(string(src), "\n")
	b.SetBytes(int64(len(src)))
	b.ReportAllocs()
	for i := 0; i < b.N; i++ {
		var st hlState
		for _, l := range lines {
			highlight(langGo, &st, l, cText, nil)
		}
	}
}

// JSON a tool printed is laid out; JSON lines keep their lines; anything
// else is left alone.
func TestPrettyJSON(t *testing.T) {
	for _, c := range []struct {
		in, want string
		ok       bool
	}{
		{`{"a":1,"b":[true,null]}`, "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}", true},
		{"{\"a\":1}\n{\"a\":2}\n", "{\"a\":1}\n{\"a\":2}", true},
		{"[1, 2", "", false},
		{"{not json}", "", false},
		{"ok 1.2s", "", false},
		{`"just a string"`, "", false},
	} {
		got, ok := prettyJSON(c.in)
		if ok != c.ok || got != c.want {
			t.Errorf("prettyJSON(%q) = %q, %v; want %q, %v", c.in, got, ok, c.want, c.ok)
		}
	}
}

// A comment in what a tool printed stands apart from the code around it,
// which is already drawn as quietly as a comment in Claude's own code.
func TestCommentUnderToolOutput(t *testing.T) {
	var st hlState
	got := highlight(langGo, &st, `m.hover = "" // the keyboard`, cOut, nil)
	if !strings.Contains(got, hlOutComment+"// the keyboard") {
		t.Fatalf("comment in tool output should be under it: %q", got)
	}
	st = hlState{}
	if got := highlight(langGo, &st, "x // y", cText, nil); !strings.Contains(got, hlComment+"// y") {
		t.Fatalf("comment in Claude's code keeps its colour: %q", got)
	}
}
