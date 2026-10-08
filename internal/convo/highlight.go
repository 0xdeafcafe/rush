package convo

import (
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/hl"
)

// A small syntax highlighter: one pass over a line's bytes, no regular
// expressions, a word's class looked up without allocating, and colour
// written only where it changes, never reset mid-line, so a background laid
// under the line (a diff's, or its changed words') holds across it.

// Syntax colours, muted to sit under rush's text; SetColours makes them.
var (
	hlKw, hlStr, hlNum string // keywords, strings, numbers and constants
	hlComment          string
	// hlOutComment is a comment in what a tool printed, which is already
	// as quiet as hlComment: it goes a step further under.
	hlOutComment string
	hlFn         string // a call, a key, a variable
	hlType       string
	hlSpace      string // · and → marking spaces and tabs
)

// showSpace marks spaces and tabs in diffs, as · and →, the way an editor
// can; SetShowWhitespace turns it on.
var showSpace bool

// SetShowWhitespace turns marking spaces and tabs in diffs on or off.
func SetShowWhitespace(on bool) {
	if on != showSpace {
		showSpace = on
		palette++ // what's drawn already has them, or hasn't
	}
}

// The lexer is photon/hl's: rush keeps its names for it, and its colours.
type (
	lang    = hl.Lang
	hlState = hl.State
)

var (
	langGo, langJS, langPy, langRust, langSh = hl.For("go"), hl.For("js"), hl.For("py"), hl.For("rs"), hl.For("sh")
	langJSON, langYAML, langTOML, langCSS    = hl.For("json"), hl.For("yaml"), hl.For("toml"), hl.For("css")
	langSQL, langRuby, langC, langLua        = hl.For("sql"), hl.For("rb"), hl.For("c"), hl.For("lua")
	langPHP, langMD                          = hl.For("php"), hl.For("md")
)

// langFor is the language of a fence tag, a file name or an interpreter;
// nil when there's none to highlight.
func langFor(name string) *lang { return hl.For(name) }

// emph is a span of a line (byte offsets) drawn on its own background, as
// the changed words of a diff's line, and the background to return to.
type emph struct {
	from, to int
	on, off  string
}

// highlight draws one line of code in language l over colour base, going
// on from st and leaving st for the next line. Plain text and a nil l come
// back in base.
func highlight(l *lang, st *hlState, s, base string, em *emph) string {
	return paintCode(l, st, s, base, em, false)
}

// paintCode is highlight, drawing each space as · and each tab as → and
// three spaces when marked, in a colour of their own; s keeps its tabs.
func paintCode(l *lang, st *hlState, s, base string, em *emph, marked bool) string {
	var b strings.Builder
	b.Grow(len(s) + 64)
	cur := ""
	inEm := false
	// out writes s[i:j] in colour c, switching the background where the
	// emphasis starts and ends inside it.
	out := func(i, j int, c string) {
		if c == hlComment && base == cOut {
			c = hlOutComment
		}
		for i < j {
			k := j
			if em != nil {
				if on := i >= em.from && i < em.to; on != inEm {
					if on {
						b.WriteString(em.on)
					} else {
						b.WriteString(em.off)
					}
					inEm = on
				}
				if i < em.from && em.from < k {
					k = em.from
				}
				if i < em.to && em.to < k {
					k = em.to
				}
			}
			if marked {
				if ws := strings.IndexAny(s[i:k], " \t"); ws == 0 {
					if cur != hlSpace {
						b.WriteString(hlSpace)
						cur = hlSpace
					}
					if s[i] == '\t' {
						b.WriteString("→   ")
					} else {
						b.WriteString("·")
					}
					i++
					continue
				} else if ws > 0 {
					k = i + ws
				}
			}
			if c != cur {
				b.WriteString(c)
				cur = c
			}
			b.WriteString(s[i:k])
			i = k
		}
	}
	l.Line(st, s, func(i, j int, c hl.Class) { out(i, j, hlColour(c, base)) })
	return end(&b, inEm, em)
}

// hlColour is what rush draws a class of code in, over base.
func hlColour(c hl.Class, base string) string {
	switch c {
	case hl.Keyword:
		return hlKw
	case hl.String:
		return hlStr
	case hl.Number:
		return hlNum
	case hl.Func:
		return hlFn
	case hl.Type:
		return hlType
	case hl.Comment:
		return hlComment
	}
	return base
}

// end closes a highlighted line: the emphasis off, then every style.
func end(b *strings.Builder, inEm bool, em *emph) string {
	if inEm {
		b.WriteString(em.off)
	}
	b.WriteString(reset)
	return b.String()
}

// changed is the span of a that differs from b, by their common start and
// end, on UTF-8 boundaries; ok is false when nearly all of it changed, as
// then there's nothing worth pointing out.
func changed(a, b string) (from, to int, ok bool) {
	p := 0
	for p < len(a) && p < len(b) && a[p] == b[p] {
		p++
	}
	for p > 0 && p < len(a) && !utf8.RuneStart(a[p]) {
		p--
	}
	q := 0
	for q < len(a)-p && q < len(b)-p && a[len(a)-1-q] == b[len(b)-1-q] {
		q++
	}
	for q > 0 && len(a)-q < len(a) && !utf8.RuneStart(a[len(a)-q]) {
		q--
	}
	from, to = p, len(a)-q
	if to <= from || p+q == 0 || (p+q)*4 < len(a) {
		return 0, 0, false
	}
	return from, to, true
}

// heredocLang is the language of the heredoc a command line starts: its
// interpreter's (python3 - <<EOF), else the file it writes (cat > x.go).
func heredocLang(line string) *lang {
	toks := fieldsOf(line)
	if len(toks) == 0 {
		return nil
	}
	switch p := filepath.Base(toks[0]); p {
	case "node", "bun", "deno", "tsx", "ts-node":
		return langJS
	case "cat", "tee":
	default:
		if l := langFor(p); l != nil {
			return l
		}
	}
	all := shellTokens(line)
	for i, t := range all {
		t = strings.TrimSpace(t)
		switch {
		case (t == ">" || t == ">>") && i+2 < len(all):
			return langFor(unquote(strings.TrimSpace(all[i+2])))
		case strings.HasPrefix(t, ">") && len(t) > 1 && t != ">>":
			return langFor(unquote(strings.TrimLeft(t, ">")))
		}
	}
	if toks[0] == "tee" && len(toks) > 1 {
		return langFor(unquote(toks[len(toks)-1]))
	}
	return nil
}

// codePrefix is how long the prefix is that a read or a search puts before
// a line of code: "  12→" or "12\t" from a read, "a.go:12:" or "a.go-12-"
// from grep, and the path in it, if any.
func codePrefix(l string) (n int, path string) {
	i := 0
	for i < len(l) && l[i] == ' ' {
		i++
	}
	// path: or path- before the number, as grep writes it; a path can
	// have a - of its own.
	start := i
	word := l[i:]
	if k := strings.IndexAny(word, " \t"); k >= 0 {
		word = word[:k]
	}
	for k := 1; k < len(word); k++ {
		if word[k] != ':' && word[k] != '-' {
			continue
		}
		j := i + k + 1
		m := j
		for m < len(l) && l[m] >= '0' && l[m] <= '9' {
			m++
		}
		if m > j && m < len(l) && (l[m] == ':' || l[m] == '-') && strings.Contains(l[i:i+k], ".") {
			return m + 1, l[i : i+k]
		}
	}
	j := start
	for j < len(l) && l[j] >= '0' && l[j] <= '9' {
		j++
	}
	if j == start {
		return 0, ""
	}
	switch {
	case strings.HasPrefix(l[j:], "→"):
		return j + len("→"), ""
	case j < len(l) && (l[j] == '\t' || l[j] == ':' || l[j] == '-'):
		// grep -n on one file: 12: for a match, 12- for the lines around.
		return j + 1, ""
	}
	return 0, ""
}

// lineNo is the line number a code prefix gives, 0 for none.
func lineNo(prefix string) int {
	prefix = strings.TrimRightFunc(prefix, func(r rune) bool { return r < '0' || r > '9' })
	k := len(prefix)
	for k > 0 && prefix[k-1] >= '0' && prefix[k-1] <= '9' {
		k--
	}
	n, _ := strconv.Atoi(prefix[k:])
	return n
}
