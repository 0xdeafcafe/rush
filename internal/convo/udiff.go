package convo

import (
	"fmt"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
)

// A diff a command printed (git diff, git show, git log -p, diff -u) is
// drawn as rush draws an edit: a header for each file saying what changed
// in it, line numbers from its hunks, each side highlighted in the file's
// language, and the words that changed in a pair of lines picked out.

// dline is what one line of a command's output is in the diff it prints.
type dline struct {
	// kind is 'f' a file's header, 's' a header line the file's header
	// says for it, 'h' a hunk's start, ' ' '+' '-' a line kept, added or
	// removed, '\\' git's "no newline", and 0 a line that isn't the diff's.
	kind byte
	n    int // the line's number: the old side's for '-', the new's else
	pair int // the line on the other side a '-' or '+' pairs with, or -1
	file int // the index of its file's header line, or -1
	// A file's header: its path, what happened to it, and the lines added
	// and removed in it.
	path, note string
	add, del   int
	ctx        string // a hunk's function, as git says it
}

// udiff is the diff in a command's output, line by line.
type udiff struct {
	ls       []dline
	old, new hlState
	whole    bool // every line is in a diff: it folds by rows, not as output does
}

var hunkRe = lazyRe(`^@@+ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@+ ?(.*)$`)

// diffParse is where parseDiff is: the file it's in and the hunk, and
// the line numbers and lines left on each side.
type diffParse struct {
	u                *udiff
	file             int
	hunk             bool
	oldN, newN       int
	oldLeft, newLeft int
}

// parseDiff reads the diff in lines, those in(i) says are a diff's; nil
// when there's no file in them.
func parseDiff(lines []string, in func(int) bool) *udiff {
	p := &diffParse{u: &udiff{ls: make([]dline, len(lines)), whole: true}, file: -1}
	found := false
	for i, l := range lines {
		p.u.ls[i].pair, p.u.ls[i].file = -1, p.file
		if !in(i) {
			p.u.whole = false
			p.file, p.hunk, p.oldLeft, p.newLeft = -1, false, 0, 0
			p.u.ls[i].file = -1
			continue
		}
		l = cleanOutput(l)
		if p.file >= 0 && p.hunk && p.hunkLine(i, l) {
			continue
		}
		p.hunk = false
		next := ""
		if i+1 < len(lines) {
			next = cleanOutput(lines[i+1])
		}
		found = p.header(i, l, next) || found
	}
	if !found {
		return nil
	}
	p.u.pairUp()
	return p.u
}

// hunkLine reads line i as one of a hunk's; false when it's past the hunk.
// The hunk's counts say how many lines are its; a tool that cut trailing
// spaces leaves a kept blank line empty.
func (p *diffParse) hunkLine(i int, l string) bool {
	x, f := &p.u.ls[i], &p.u.ls[p.file]
	c := byte(' ')
	if l != "" {
		c = l[0]
	}
	if p.oldLeft <= 0 && p.newLeft <= 0 && (c == ' ' || strings.HasPrefix(l, "--- ") || strings.HasPrefix(l, "+++ ")) {
		return false // past its counts: only a stray + or − line is still its
	}
	switch c {
	case ' ':
		x.kind, x.n = ' ', p.newN
		p.oldN, p.newN, p.oldLeft, p.newLeft = p.oldN+1, p.newN+1, p.oldLeft-1, p.newLeft-1
	case '-':
		x.kind, x.n = '-', p.oldN
		p.oldN, p.oldLeft = p.oldN+1, p.oldLeft-1
		f.del++
	case '+':
		x.kind, x.n = '+', p.newN
		p.newN, p.newLeft = p.newN+1, p.newLeft-1
		f.add++
	case '\\':
		x.kind = '\\'
	default:
		return false
	}
	return true
}

// header reads line i outside a hunk, next the line after it; true when
// a file starts there.
func (p *diffParse) header(i int, l, next string) bool {
	x := &p.u.ls[i]
	switch {
	case strings.HasPrefix(l, "diff -"):
		p.file, x.kind, x.file = i, 'f', i
		x.path = diffPath(l)
		return true
	case p.file >= 0 && headerLine(l):
		x.kind = 's'
		fileNote(&p.u.ls[p.file], l)
	case strings.HasPrefix(l, "--- ") && strings.HasPrefix(next, "+++ "):
		// diff -u: a file starts at its two paths.
		p.file, x.kind, x.file = i, 'f', i
		x.path = sidePath(next)
		if x.path == "" {
			x.path, x.note = sidePath(l), "deleted"
		} else if sidePath(l) == "" {
			x.note = "new"
		}
		return true
	case p.file >= 0 && strings.HasPrefix(l, "@@"):
		if m := hunkRe().FindStringSubmatch(l); m != nil {
			p.hunk = true
			p.oldN, _ = strconv.Atoi(m[1])
			p.newN, _ = strconv.Atoi(m[3])
			p.oldLeft, p.newLeft = count(m[2]), count(m[4])
			x.kind, x.ctx = 'h', m[5]
		}
	default:
		// A commit's header and message between one diff and the next.
		p.file, x.file = -1, -1
	}
	return false
}

// fileNote reads what header line l says about file f.
func fileNote(f *dline, l string) {
	switch {
	case strings.HasPrefix(l, "new file"):
		f.note = "new"
	case strings.HasPrefix(l, "deleted file"):
		f.note = "deleted"
	case strings.HasPrefix(l, "rename from "):
		f.note = "renamed from " + strings.TrimPrefix(l, "rename from ")
	case strings.HasPrefix(l, "rename to "):
		f.path = strings.TrimPrefix(l, "rename to ")
	case strings.HasPrefix(l, "copy from "):
		f.note = "copied from " + strings.TrimPrefix(l, "copy from ")
	case strings.HasPrefix(l, "Binary files "):
		f.note = "binary"
	case strings.HasPrefix(l, "+++ "):
		if p := sidePath(l); p != "" {
			f.path = p
		}
	}
}

// pairUp pairs a run of removed lines with the added lines after it, in
// order, so each pair can show the words that changed.
func (u *udiff) pairUp() {
	for i := 0; i < len(u.ls); i++ {
		if u.ls[i].kind != '-' {
			continue
		}
		j := i
		for j < len(u.ls) && u.ls[j].kind == '-' {
			j++
		}
		k := j
		for k < len(u.ls) && u.ls[k].kind == '+' {
			k++
		}
		for n := 0; n < j-i && n < k-j; n++ {
			u.ls[i+n].pair, u.ls[j+n].pair = j+n, i+n
		}
		i = k - 1
	}
}

// count is a hunk's line count, which git leaves out when it's 1.
func count(s string) int {
	if s == "" {
		return 1
	}
	n, _ := strconv.Atoi(s)
	return n
}

// headerLine is whether l is one of the lines git puts between a file's
// diff line and its first hunk.
func headerLine(l string) bool {
	for _, p := range []string{"index ", "new file mode", "deleted file mode", "old mode", "new mode",
		"similarity index", "dissimilarity index", "rename from ", "rename to ", "copy from ", "copy to ",
		"Binary files ", "--- ", "+++ "} {
		if strings.HasPrefix(l, p) {
			return true
		}
	}
	return false
}

// diffPath is the file a diff line names: the new side's for git, the
// last path for diff -u.
func diffPath(l string) string {
	if rest, ok := strings.CutPrefix(l, "diff --git "); ok {
		if _, b, ok := strings.CutLast(rest, " b/"); ok {
			return unquote(b)
		}
		return unquote(rest)
	}
	f := strings.Fields(strings.TrimPrefix(strings.TrimPrefix(l, "diff --cc "), "diff "))
	for _, a := range slices.Backward(f) {
		if !strings.HasPrefix(a, "-") {
			return unquote(a)
		}
	}
	return ""
}

// sidePath is the path after --- or +++, without git's a/ or b/ or the
// time diff -u puts after it; "" for /dev/null.
func sidePath(l string) string {
	p, _, _ := strings.Cut(l[4:], "\t")
	p = unquote(strings.TrimSpace(p))
	if p == "/dev/null" {
		return ""
	}
	if strings.HasPrefix(p, "a/") || strings.HasPrefix(p, "b/") {
		p = p[2:]
	}
	return p
}

// fileHead is a file's header in a diff: its name bright after its
// folder, what happened to it, and its lines added and removed.
func fileHead(x dline) string {
	dir, name := filepath.Split(x.path)
	s := faint(dir) + paint(cText+bold, name)
	switch {
	case x.note == "new":
		s += "  " + paint(cGreen, "new")
	case x.note == "deleted":
		s += "  " + paint(cRed, "deleted")
	case x.note != "":
		s += "  " + dim(x.note)
	}
	if x.add > 0 {
		s += "  " + paint(cGreen, fmt.Sprintf("+%d", x.add))
	}
	if x.del > 0 {
		s += " " + paint(cRed, fmt.Sprintf("−%d", x.del))
	}
	return s
}

// line draws line i of the diff after pad, w cells wide, its code in lg
// when its file doesn't say; false when it isn't one of the diff's.
func (u *udiff) line(d *drawer, pad string, w int, lg *lang, lines []string, i int) bool {
	x := u.ls[i]
	if x.file >= 0 {
		if flg := langFor(u.ls[x.file].path); flg != nil {
			lg = flg
		}
	}
	l := expandTabs(cleanOutput(lines[i]))
	code := func(j int) string {
		if s := cleanOutput(lines[j]); s != "" {
			return s[1:]
		}
		return ""
	}
	switch x.kind {
	case 'f':
		u.old, u.new = hlState{}, hlState{}
		if i > 0 {
			d.add("", bgWell, pad, "")
		}
		d.addRows(bgWell, pad, "", fileHead(x), w, 2)
	case 's':
	case 'h':
		u.old, u.new = hlState{}, hlState{}
		// The first hunk of a file follows its header; the others are a
		// gap, with the function git says they're in.
		if i > 0 && u.ls[i-1].kind != 's' && u.ls[i-1].kind != 'f' || x.ctx != "" {
			var st hlState
			d.addRows(bgWell, pad, faint("    ⋯ "), highlight(lg, &st, expandTabs(x.ctx), cDim, nil), w-6, 1)
		}
	case '+', '-':
		st, pair := &u.new, ""
		if x.kind == '-' {
			st = &u.old
		}
		if x.pair >= 0 {
			pair = code(x.pair)
		}
		d.diffLine(pad, w-8, lg, st, x.kind, x.n, code(i), pair, x.pair >= 0)
	case ' ':
		for j, r := range d.codeRows(highlight(lg, &u.new, expandTabs(code(i)), cSub, nil), w-8, 6) {
			if j == 0 {
				d.add("", bgWell, pad+faint(fmt.Sprintf("%5d ", x.n))+"  "+r, "")
			} else {
				d.add("", bgWell, pad+blanks(8)+r, "")
				d.wrapped()
			}
		}
		u.old = u.new // a line both sides share leaves them alike
	case '\\':
		d.addRows(bgWell, pad, blanks(8), faint(strings.TrimPrefix(l, `\ `)), w-8, 1)
	default:
		return false
	}
	return true
}

// drawn is whether line i draws a row of its own.
func (u *udiff) drawn(i int) bool { return u.ls[i].kind != 's' }

// hasDiff is whether lines hold a diff, whatever printed them: a file's
// start (a diff line, or --- then +++) and a hunk. Cheap, so output can
// ask it of anything.
func hasDiff(lines []string) bool {
	file, hunk := false, false
	for i, l := range lines {
		switch {
		case strings.HasPrefix(l, "diff -"), strings.HasPrefix(l, "--- ") && i+1 < len(lines) && strings.HasPrefix(lines[i+1], "+++ "):
			file = true
		case file && strings.HasPrefix(l, "@@ -"):
			hunk = true
		}
		if hunk {
			return true
		}
	}
	return false
}
