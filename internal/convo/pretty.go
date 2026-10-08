package convo

import (
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Pretty is output laid out for reading however it was cut: JSON reindented
// token by token, so a head -c or a truncated answer reads as well as a
// whole one, XML and HTML a tag a line, YAML coloured as it is. Nothing
// here decodes strictly, and what it can't read is left as it came.

// prettyText is s laid out and the language to colour it in; ok is false
// when it isn't JSON, XML, HTML or YAML.
func prettyText(s string) (string, *lang, bool) {
	t := strings.TrimSpace(s)
	switch structure(t) {
	case 'j':
		return reindentJSON(t), langJSON, true
	case 'x':
		return reindentXML(t), nil, true
	case 'y':
		return expandTabs(t), langYAML, true
	}
	return "", nil, false
}

// structure is what t looks like: 'j' JSON, 'x' XML or HTML, 'y' YAML,
// 0 none of them.
func structure(t string) byte {
	if len(t) < 2 {
		return 0
	}
	switch t[0] {
	case '{', '[':
		rest := strings.TrimLeft(t[1:], " \t\r\n")
		if rest == "" || strings.ContainsRune(`"{[]}-0123456789tfn`, rune(rest[0])) {
			return 'j'
		}
	case '<':
		c := t[1]
		if c == '?' || c == '!' || c == '/' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' {
			return 'x'
		}
	}
	if looksYAML(t) {
		return 'y'
	}
	return 0
}

// looksYAML is whether t starts as a YAML document does: --- or a key.
func looksYAML(t string) bool {
	first, _, _ := strings.Cut(t, "\n")
	if strings.TrimSpace(first) == "---" {
		return true
	}
	k, _, ok := strings.Cut(first, ": ")
	if !ok {
		k, ok = strings.CutSuffix(first, ":")
	}
	return ok && k != "" && strings.IndexFunc(k, func(r rune) bool {
		return !(r == '_' || r == '-' || r == '.' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9')
	}) < 0 && strings.Count(t, ":") >= 2
}

// betterPretty is whether t reads better laid out than as it came: JSON
// that's whole, or on a line or two, and XML squeezed onto long lines.
func betterPretty(t string) bool {
	t = strings.TrimSpace(t)
	lines := strings.Count(t, "\n") + 1
	switch structure(t) {
	case 'j':
		return lines <= 2 || len(t) <= 4<<20 && jsonx.Valid([]byte(t))
	case 'x':
		return lines <= 2 || len(t)/lines > 200
	}
	return false
}

// reindentJSON lays s out two spaces a level from its tokens alone: a
// string left open, a bracket never closed or a stray one are all drawn
// as they come. Values at the top level (JSON Lines) go a line each.
func reindentJSON(s string) string {
	var b strings.Builder
	depth := 0
	nl := func() {
		b.WriteByte('\n')
		b.WriteString(strings.Repeat("  ", depth))
	}
	top := func() { // a value starting at the top level, after another
		if depth == 0 && b.Len() > 0 {
			b.WriteByte('\n')
		}
	}
	for i := 0; i < len(s); i++ {
		switch c := s[i]; c {
		case ' ', '\t', '\r', '\n':
		case '{', '[':
			top()
			b.WriteByte(c)
			j := skipWS(s, i+1)
			if j < len(s) && (s[j] == '}' || s[j] == ']') {
				b.WriteByte(s[j]) // {} and [] stay whole
				i = j
				continue
			}
			depth++
			if j < len(s) {
				nl()
			}
		case '}', ']':
			depth = max(0, depth-1)
			nl()
			b.WriteByte(c)
		case ',':
			b.WriteByte(c)
			nl()
		case ':':
			b.WriteString(": ")
		case '"':
			top()
			j := i + 1
			for j < len(s) && s[j] != '"' {
				if s[j] == '\\' {
					j++
				}
				j++
			}
			j = min(j+1, len(s))
			b.WriteString(s[i:j])
			i = j - 1
		default:
			top()
			j := i
			for j < len(s) && !strings.ContainsRune(" \t\r\n{}[],:\"", rune(s[j])) {
				j++
			}
			b.WriteString(s[i:j])
			i = j - 1
		}
	}
	return b.String()
}

func skipWS(s string, i int) int {
	for i < len(s) && strings.ContainsRune(" \t\r\n", rune(s[i])) {
		i++
	}
	return i
}

// voidTags are HTML's elements that never close.
var voidTags = map[string]bool{"area": true, "base": true, "br": true, "col": true, "embed": true, "hr": true, "img": true, "input": true, "link": true, "meta": true, "source": true, "track": true, "wbr": true}

// reindentXML puts each tag on a line of its own, two spaces a level, and
// keeps an element holding only text on one line. A tag cut off at the
// end is drawn as it is; a script's or style's body is text.
func reindentXML(s string) string {
	var b strings.Builder
	depth, last := 0, byte(0) // last: 'o' an open tag, 't' text after one, else 0
	put := func(t string) {
		if b.Len() > 0 {
			b.WriteByte('\n')
		}
		b.WriteString(strings.Repeat("  ", depth) + t)
	}
	for i := 0; i < len(s); {
		if s[i] != '<' {
			j := strings.IndexByte(s[i:], '<')
			if j < 0 {
				j = len(s) - i
			}
			if t := strings.Join(strings.Fields(s[i:i+j]), " "); t != "" {
				if last == 'o' {
					b.WriteString(t)
					last = 't'
				} else {
					put(t)
					last = 0
				}
			}
			i += j
			continue
		}
		j := tagEnd(s, i)
		tag := s[i:j]
		name := tagName(tag)
		switch {
		case strings.HasPrefix(tag, "</"):
			depth = max(0, depth-1)
			if last != 0 {
				b.WriteString(tag)
			} else {
				put(tag)
			}
			last = 0
		case strings.HasPrefix(tag, "<!") || strings.HasPrefix(tag, "<?") || strings.HasSuffix(tag, "/>") || voidTags[strings.ToLower(name)] || !strings.HasSuffix(tag, ">"):
			put(tag)
			last = 0
		default:
			put(tag)
			depth++
			last = 'o'
			if n := strings.ToLower(name); n == "script" || n == "style" {
				k := strings.Index(strings.ToLower(s[j:]), "</"+n)
				if k < 0 {
					k = len(s) - j
				}
				if body := strings.TrimSpace(s[j : j+k]); body != "" {
					depth++
					for _, l := range strings.Split(body, "\n") {
						put(strings.TrimSpace(l))
					}
					depth--
					last = 0
				}
				j += k
			}
		}
		i = j
	}
	return b.String()
}

// tagEnd is where the tag at i ends, past its >: quotes, comments and
// CDATA may hold a > of their own. The end of s when it's cut off.
func tagEnd(s string, i int) int {
	for _, p := range [][2]string{{"<!--", "-->"}, {"<![CDATA[", "]]>"}} {
		if strings.HasPrefix(s[i:], p[0]) {
			if k := strings.Index(s[i+len(p[0]):], p[1]); k >= 0 {
				return i + len(p[0]) + k + len(p[1])
			}
			return len(s)
		}
	}
	quote := byte(0)
	for j := i + 1; j < len(s); j++ {
		switch c := s[j]; {
		case quote != 0:
			if c == quote {
				quote = 0
			}
		case c == '"' || c == '\'':
			quote = c
		case c == '>':
			return j + 1
		}
	}
	return len(s)
}

// tagName is the element's name in <name ...> or </name>.
func tagName(tag string) string {
	t := strings.TrimLeft(tag, "</")
	if k := strings.IndexAny(t, " \t\r\n/>"); k >= 0 {
		return t[:k]
	}
	return t
}
