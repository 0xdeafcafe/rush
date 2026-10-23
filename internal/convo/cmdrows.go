package convo

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/charmbracelet/x/ansi"
)

// cwdAt is the folder a command is in at some point: the session's, taken
// as where it starts, or one a cd before it went to, which is sure. ""
// when it can't be told.
type cwdAt struct {
	dir  string
	sure bool
}

// shortPaths is a command with its absolute paths as short as they still
// mean the same: relative to the folder it's in at that point when they're
// inside it, else with home as ~. A cd at the top level of a chain moves
// cur, and one anywhere else (a subshell, a loop) leaves it unknown. Where
// a cd goes may also go up from a folder an earlier cd went to, when
// that's shorter; it's never "." or up from the session's folder, which
// is only taken as where the shell starts.
func shortPaths(text, home string, cur *cwdAt, top bool) string {
	if !strings.Contains(text, "/") && !strings.Contains(text, "cd") {
		return text
	}
	// Inside a subshell or a $(…) a cd moves a shell of its own: where
	// the words after it are can't be told.
	if strings.Contains(text, "(") && strings.Contains(text, "cd") {
		*cur, top = cwdAt{}, false
	}
	toks := shellTokens(text)
	head, cd := true, false
	for i, t := range toks {
		if strings.TrimSpace(t) == "" {
			continue
		}
		if shOps[t] {
			head = true
			continue
		}
		q, p := "", t
		if len(p) >= 2 && (p[0] == '"' || p[0] == '\'') && p[len(p)-1] == p[0] {
			q, p = p[:1], p[1:len(p)-1]
		}
		switch {
		case cd:
			cd = false
			to := p
			switch {
			case to == "-" || to == "~" || strings.HasPrefix(to, "~") && !strings.HasPrefix(to, "~/"):
				to = ""
			case strings.HasPrefix(to, "~/") && home != "":
				to = home + to[1:]
			case !strings.HasPrefix(to, "/") && cur.dir != "":
				to = filepath.Join(cur.dir, to)
			case !strings.HasPrefix(to, "/"):
				to = ""
			}
			if to != "" && !strings.ContainsAny(to, "$`*") {
				toks[i] = q + cdPath(filepath.Clean(to), home, *cur) + q
			}
			*cur = cwdAt{dir: to, sure: true}
			if !top || to == "" {
				*cur = cwdAt{}
			}
		case head && p == "cd":
			cd = true
		case strings.HasPrefix(p, "/") && !strings.ContainsAny(p, "$`"):
			toks[i] = q + shortPath(p, home, cur.dir) + q
		}
		head = false
	}
	return strings.Join(toks, "")
}

// shortPath is p inside cur relative to it, else with home as ~.
func shortPath(p, home, cur string) string {
	switch {
	case cur != "" && p == cur:
		return "."
	case cur != "" && strings.HasPrefix(p, cur+"/"):
		return p[len(cur)+1:]
	case home != "" && strings.HasPrefix(p, home+"/"):
		return "~" + p[len(home):]
	}
	return p
}

// cdPath is where a cd from cur goes, as shortPath has it, or up from cur,
// two levels at most, when cur is sure and that's shorter.
func cdPath(p, home string, cur cwdAt) string {
	if !cur.sure {
		cur.dir = "" // the session's folder is a guess at where it starts
	}
	s := shortPath(p, home, cur.dir)
	if cur.dir == "" {
		return s
	}
	if r, err := filepath.Rel(cur.dir, p); err == nil && strings.Count(r, "..") <= 2 && len(r) < len(s) {
		return r
	}
	return s
}

// userHome is the home folder, for shortPaths; "" when there's none.
func userHome() string {
	h, _ := os.UserHomeDir()
	return strings.TrimSuffix(h, "/")
}

// tokenRows packs a command's tinted words (quietTokens) into rows, the
// first w cells wide and the rest rest wide, breaking only between words,
// never inside a path; a word wider than a row is cut across rows.
func tokenRows(toks []string, w, rest int) []string {
	w, rest = max(w, 4), max(rest, 4)
	var rows []string
	var b strings.Builder
	n, gap := 0, ""
	flush := func() {
		rows = append(rows, b.String())
		b.Reset()
		n, w = 0, rest
	}
	for _, t := range toks {
		if strings.TrimSpace(t) == "" {
			if n > 0 {
				gap += t
			}
			continue
		}
		tw := cellw.String(t)
		if n > 0 && n+len(gap)+tw > w {
			flush()
		}
		if n > 0 {
			b.WriteString(gap)
			n += len(gap)
		}
		gap = ""
		for n+tw > w {
			k := cutAt(stripANSI(t), w-n)
			b.WriteString(ansi.Truncate(t, k, ""))
			t = ansi.TruncateLeft(t, k, "")
			flush()
			tw = cellw.String(t)
		}
		b.WriteString(t)
		n += tw
	}
	if n > 0 || len(rows) == 0 {
		flush()
	}
	return rows
}

// cutAt is where to cut a word too long for a row of w cells: after the
// last / - . or _ in its second half, so a path breaks between its parts,
// else at w.
func cutAt(word string, w int) int {
	r := []rune(word)
	for i := min(w, len(r)) - 1; i >= w/2; i-- {
		if strings.ContainsRune("/-._", r[i]) {
			return cellw.String(string(r[:i+1]))
		}
	}
	return w
}
