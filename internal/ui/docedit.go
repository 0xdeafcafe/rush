package ui

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/cellw"
)

// docEditor edits one file in place: markdown and JSON are coloured as you
// type, their problems are found as you go, and each gets the keys that
// suit it (lists carry on in markdown, brackets pair up in JSON). Long lines
// wrap; nothing scrolls sideways.
type docEditor struct {
	path     string
	kind     docKind
	buf      []rune
	pos      int
	anchor   int // where a selection started; -1 for none
	goal     int // the column ↑↓ aim for; -1 until they're used
	top      int // the first row shown
	saved    string
	crlf     bool
	missing  bool
	readOnly string // why it can't be edited here; "" when it can
	mod      time.Time
	checked  time.Time
	stale    bool // it changed on disk while you had changes of your own
	indent   string

	undo, redo []docSnap
	typing     bool // the last change was typing, which undo takes back with the typing before it
	ver        int  // bumps on every change

	lay      docLayout
	tw, th   int // the text's width and height when last drawn
	nlines   int
	nlVer    int
	diagVer  int
	diags    []docDiag
	siblings map[string]bool // the .md files beside it, for [[links]]

	loading bool // the first read is still out: there's nothing to edit yet
	saving  bool // a save is out
	reads   offRead[docRead]
	links   docLinks // whether the files it links to exist
	linkVer int      // links' version when diags were found
}

type docKind int

const (
	docText docKind = iota
	docMarkdown
	docJSON
)

func docKindFor(path string) docKind {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".md", ".markdown":
		return docMarkdown
	case ".json":
		return docJSON
	}
	return docText
}

type docSnap struct {
	buf []rune
	pos int
}

// docDiag is a problem found in the file, at a line and column (runes).
type docDiag struct {
	line, col int
	msg       string
	warn      bool // a warning rather than an error
}

// maxDocSize is the most the editor takes in; bigger files open in $EDITOR.
const maxDocSize = 1 << 20

// openDoc opens path in the editor. The file is read in the background:
// until it's in, there's nothing to edit.
func openDoc(path string) *docEditor {
	e := &docEditor{path: path, kind: docKindFor(path), anchor: -1, goal: -1, indent: "  ", diagVer: -1,
		loading: true, readOnly: "still reading it"}
	e.startRead(false)
	e.poll()
	return e
}

// docRead is the file as a read in the background found it.
type docRead struct {
	same     bool // unchanged since it was last read: nothing was read
	missing  bool
	mod      time.Time
	big      int64 // its size, when it's too big to edit here
	err      error
	text     string
	crlf     bool
	siblings map[string]bool // the .md files beside a markdown file, for [[links]]
}

// readDoc reads the file as it is on disk now. check reads it only when
// it changed since mod, and takes a file it can't find as unchanged.
func readDoc(path string, kind docKind, mod time.Time, check bool) docRead {
	st, err := os.Stat(path)
	switch {
	case err != nil && check, err == nil && check && st.ModTime().Equal(mod):
		return docRead{same: true}
	case err != nil:
		return docRead{missing: true}
	}
	r := docRead{mod: st.ModTime()}
	if st.Size() > maxDocSize {
		r.big = st.Size()
		return r
	}
	b, err := os.ReadFile(path)
	if err != nil {
		r.err = err
		return r
	}
	r.text = string(b)
	r.crlf = strings.Contains(r.text, "\r\n")
	if r.crlf {
		r.text = strings.ReplaceAll(r.text, "\r\n", "\n")
	}
	if kind == docMarkdown {
		r.siblings = map[string]bool{}
		files, _ := filepath.Glob(filepath.Join(filepath.Dir(path), "*.md"))
		for _, f := range files {
			r.siblings[strings.TrimSuffix(filepath.Base(f), ".md")] = true
		}
	}
	return r
}

// startRead reads the file in the background, unless a read is out.
func (e *docEditor) startRead(check bool) {
	e.checked = time.Now()
	path, kind, mod := e.path, e.kind, e.mod
	e.reads.start(func() docRead { return readDoc(path, kind, mod, check) })
}

// poll takes in a read that's done.
func (e *docEditor) poll() {
	if r, ok := e.reads.take(); ok {
		e.load(r)
	}
}

// load takes in the file as read: at once when it's opened, and after
// that unless you have changes of your own, which it marks stale instead.
func (e *docEditor) load(r docRead) {
	first := e.loading
	e.loading = false
	if r.same {
		return
	}
	if !first && e.dirty() {
		e.stale = true
		return
	}
	e.readOnly = ""
	if r.missing {
		e.missing, e.buf, e.saved, e.mod = true, nil, "", time.Time{}
		e.pos = 0
		e.bump()
		return
	}
	e.missing, e.mod = false, r.mod
	if r.big > 0 {
		e.readOnly = fmt.Sprintf("%s is too big to edit here · ctrl+g opens it in $EDITOR", fileSize(r.big))
		return
	}
	if r.err != nil {
		e.readOnly = r.err.Error()
		return
	}
	e.crlf = r.crlf
	e.saved, e.buf = r.text, []rune(r.text)
	e.pos = min(e.pos, len(e.buf))
	e.anchor = -1
	e.indent = detectIndent(r.text)
	if r.siblings != nil {
		e.siblings = r.siblings
	}
	e.bump()
}

// detectIndent is the file's indent step: the first indented line's
// leading tab, or its spaces (two when they're odd or none are found).
func detectIndent(text string) string {
	for _, l := range strings.Split(text, "\n") {
		switch {
		case strings.HasPrefix(l, "\t"):
			return "\t"
		case strings.HasPrefix(l, "    ") && !strings.HasPrefix(strings.TrimLeft(l, " "), "-"):
			if n := len(l) - len(strings.TrimLeft(l, " ")); n == 4 {
				return "    "
			}
			return "  "
		case strings.HasPrefix(l, "  "):
			return "  "
		}
	}
	return "  "
}

func (e *docEditor) bump() { e.ver++ }

func (e *docEditor) dirty() bool { return e.readOnly == "" && string(e.buf) != e.saved }

// refresh picks up a change made on disk (by Claude, or $EDITOR): looked
// at in the background at most every 700ms, it's read again unless you
// have changes of your own, which it marks stale instead.
func (e *docEditor) refresh() {
	e.poll()
	if e.loading || e.saving || time.Since(e.checked) < 700*time.Millisecond {
		return
	}
	e.startRead(true)
}

// docWrite is what a save writes: text as edited, out as it goes to disk.
type docWrite struct{ path, text, out string }

// docSaved is how a save went, and the file's time after it.
type docSaved struct {
	mod time.Time
	err error
}

// toWrite is what saving would write now, or why it can't.
func (e *docEditor) toWrite() (docWrite, error) {
	if e.readOnly != "" {
		return docWrite{}, fmt.Errorf("%s", e.readOnly)
	}
	text := string(e.buf)
	out := text
	if e.crlf {
		out = strings.ReplaceAll(out, "\n", "\r\n")
	}
	return docWrite{path: e.path, text: text, out: out}, nil
}

// writeDoc writes the file, through a symlink to where it points, keeping
// its permissions; a new file's folder is made.
func writeDoc(w docWrite) docSaved {
	path := w.path
	if p, err := filepath.EvalSymlinks(path); err == nil {
		path = p
	}
	mode := os.FileMode(0o644)
	if st, err := os.Stat(path); err == nil {
		mode = st.Mode().Perm()
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return docSaved{err: err}
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+".rush-*")
	if err != nil {
		return docSaved{err: err}
	}
	_, werr := tmp.WriteString(w.out)
	cerr := tmp.Close()
	if werr == nil {
		werr = cerr
	}
	if werr == nil {
		werr = os.Chmod(tmp.Name(), mode)
	}
	if werr == nil {
		werr = os.Rename(tmp.Name(), path)
	}
	if werr != nil {
		_ = os.Remove(tmp.Name())
		return docSaved{err: werr}
	}
	var s docSaved
	if st, err := os.Stat(path); err == nil {
		s.mod = st.ModTime()
	}
	return s
}

// wrote takes in a save that went through.
func (e *docEditor) wrote(w docWrite, s docSaved) {
	e.saved, e.missing, e.stale = w.text, false, false
	if !s.mod.IsZero() {
		e.mod = s.mod
	}
}

// save writes the file there and then, off the UI goroutine only: the
// editor on screen saves with saveCmd.
func (e *docEditor) save() error {
	w, err := e.toWrite()
	if err != nil {
		return err
	}
	s := writeDoc(w)
	if s.err == nil {
		e.wrote(w, s)
	}
	return s.err
}

// saveCmd saves the file in the background, and done hears how it went
// once it has. The error is why it can't start.
func (e *docEditor) saveCmd(done func(m *Model, err error) tea.Cmd) (tea.Cmd, error) {
	if e.saving {
		return nil, errors.New("it's still saving the last time")
	}
	w, err := e.toWrite()
	if err != nil {
		return nil, err
	}
	e.saving = true
	return later(func() docSaved { return writeDoc(w) }, func(m *Model, s docSaved) tea.Cmd {
		e.saving = false
		if s.err == nil {
			e.wrote(w, s)
		}
		return done(m, s.err)
	}), nil
}

// --- changes and undo ---

// change replaces the buffer, keeping what was there for undo. Typing
// that follows typing joins its undo step.
func (e *docEditor) change(buf []rune, pos int, typing bool) {
	if e.readOnly != "" {
		return
	}
	if !(typing && e.typing) {
		e.undo = append(e.undo, docSnap{buf: e.buf, pos: e.pos})
		if len(e.undo) > 500 {
			e.undo = e.undo[1:]
		}
	}
	e.typing = typing
	e.redo = nil
	e.buf, e.pos, e.anchor, e.goal = buf, pos, -1, -1
	e.bump()
}

func (e *docEditor) undoStep(from, to *[]docSnap) {
	if len(*from) == 0 {
		return
	}
	s := (*from)[len(*from)-1]
	*from = (*from)[:len(*from)-1]
	*to = append(*to, docSnap{buf: e.buf, pos: e.pos})
	e.buf, e.pos, e.anchor, e.goal, e.typing = s.buf, s.pos, -1, -1, false
	e.bump()
}

// insertText puts text (a paste) at the cursor, over any selection.
func (e *docEditor) insertText(t string) {
	buf, pos := e.buf, e.pos
	if from, to, ok := e.selection(); ok {
		buf, pos = cut(buf, from, to), from
	}
	r := []rune(t)
	e.change(insert(buf, pos, r), pos+len(r), false)
}

func (e *docEditor) selection() (from, to int, ok bool) {
	if e.anchor < 0 || e.anchor == e.pos {
		return 0, 0, false
	}
	return min(e.anchor, e.pos), max(e.anchor, e.pos), true
}

// --- layout ---

// docLayout is the file cut into rows at a width: every line's rows, each
// a span of the buffer.
type docLayout struct {
	ver, w int
	rows   []docRow
	lines  []int // the first row of each line
}

type docRow struct {
	line     int
	from, to int  // buffer offsets; to is past the row's last rune
	last     bool // the line's last row, which owns the position at its end
}

func docRuneW(r rune) int {
	switch {
	case r == '\t':
		return 4
	case r < 0x20 || r == 0x7f:
		return 1 // drawn as ·
	case r < 0x300:
		return 1
	}
	return cellw.String(string(r))
}

func (e *docEditor) layout(w int) *docLayout {
	w = max(4, w)
	if e.lay.ver == e.ver && e.lay.w == w {
		return &e.lay
	}
	l := &e.lay
	l.ver, l.w, l.rows, l.lines = e.ver, w, l.rows[:0], l.lines[:0]
	start, n := 0, 0
	for i := 0; i <= len(e.buf); i++ {
		if i < len(e.buf) && e.buf[i] != '\n' {
			continue
		}
		l.lines = append(l.lines, len(l.rows))
		l.rows = wrapRow(l.rows, e.buf, n, start, i, w)
		start = i + 1
		n++
	}
	return l
}

// wrapRow cuts buf[from:to], line n, into rows of at most w cells,
// breaking after a space where there is one.
func wrapRow(rows []docRow, buf []rune, n, from, to, w int) []docRow {
	for {
		width, brk, i := 0, -1, from
		for ; i < to; i++ {
			cw := docRuneW(buf[i])
			if width+cw > w && i > from {
				break
			}
			width += cw
			if buf[i] == ' ' {
				brk = i + 1
			}
		}
		if i >= to {
			return append(rows, docRow{line: n, from: from, to: to, last: true})
		}
		if brk <= from || brk > i {
			brk = i
		}
		rows = append(rows, docRow{line: n, from: from, to: brk})
		from = brk
	}
}

// rowOf is the row the position sits in.
func (l *docLayout) rowOf(pos int) int {
	lo, hi := 0, len(l.rows)-1
	for lo < hi {
		mid := (lo + hi + 1) / 2
		if l.rows[mid].from <= pos {
			lo = mid
		} else {
			hi = mid - 1
		}
	}
	// A wrapped row's end is the next row's start.
	for lo > 0 && pos < l.rows[lo].from {
		lo--
	}
	return lo
}

func (e *docEditor) colOf(r docRow, pos int) int {
	c := 0
	for i := r.from; i < pos && i < r.to; i++ {
		c += docRuneW(e.buf[i])
	}
	return c
}

// atCol is the position in row r nearest column c.
func (e *docEditor) atCol(r docRow, c int) int {
	end := r.to
	if !r.last && end > r.from {
		end-- // past a wrapped row's end is the next row
	}
	w := 0
	for i := r.from; i < end; i++ {
		cw := docRuneW(e.buf[i])
		if w+cw > c {
			return i
		}
		w += cw
	}
	return end
}

// lineRange is line n's span of the buffer, its newline left out.
func (e *docEditor) lineRange(pos int) (int, int) { return lineStart(e.buf, pos), lineEnd(e.buf, pos) }

// --- keys ---

// key applies one key. act names what the caller must do: "save",
// "format"; copied is text to put on the clipboard.
func (e *docEditor) key(k tea.KeyPressMsg, s string) (act, copied string, used bool) {
	lay, h := e.layout(e.tw), e.th
	moveTo := func(pos int, extend bool) {
		if extend {
			if e.anchor < 0 {
				e.anchor = e.pos
			}
		} else {
			e.anchor = -1
		}
		e.pos = max(0, min(pos, len(e.buf)))
		e.typing = false
	}
	vert := func(d int, extend bool) {
		r := lay.rowOf(e.pos)
		if e.goal < 0 {
			e.goal = e.colOf(lay.rows[r], e.pos)
		}
		t := max(0, min(len(lay.rows)-1, r+d))
		switch {
		case t == r && d < 0:
			moveTo(0, extend)
		case t == r && d > 0:
			moveTo(len(e.buf), extend)
		default:
			g := e.goal
			moveTo(e.atCol(lay.rows[t], g), extend)
			e.goal = g
		}
	}
	switch s {
	case "up", "shift+up":
		vert(-1, s != "up")
		return "", "", true
	case "down", "shift+down":
		vert(1, s != "down")
		return "", "", true
	case "pgup", "shift+pgup":
		vert(-max(1, h-2), s != "pgup")
		return "", "", true
	case "pgdown", "shift+pgdown":
		vert(max(1, h-2), s != "pgdown")
		return "", "", true
	case "ctrl+home", "super+up":
		moveTo(0, false)
		return "", "", true
	case "ctrl+end", "super+down":
		moveTo(len(e.buf), false)
		return "", "", true
	case "ctrl+s", "super+s":
		return "save", "", true
	case "ctrl+z", "super+z", "ctrl+_", "ctrl+/":
		e.undoStep(&e.undo, &e.redo)
		return "", "", true
	case "ctrl+y", "ctrl+shift+z", "super+shift+z":
		e.undoStep(&e.redo, &e.undo)
		return "", "", true
	}
	if e.readOnly != "" {
		// Moving and copying still work; nothing changes it.
		switch s {
		case "left", "right", "home", "end", "ctrl+a", "ctrl+e", "ctrl+left", "ctrl+right", "alt+left", "alt+right",
			"shift+left", "shift+right", "shift+home", "shift+end", "super+a", "ctrl+c", "alt+c":
			_, np, na, cp, _ := editSel(e.buf, e.pos, e.anchor, k, s)
			e.pos, e.anchor = np, na
			return "", cp, true
		}
		return "", "", true
	}
	switch s {
	case "enter":
		e.newline()
		return "", "", true
	case "tab":
		e.indentLines(false)
		return "", "", true
	case "shift+tab":
		e.indentLines(true)
		return "", "", true
	case "alt+up", "alt+down":
		e.moveLine(s == "alt+up")
		return "", "", true
	case "ctrl+t":
		if e.kind == docJSON {
			return "format", "", true
		}
		if e.kind == docMarkdown {
			e.toggleTick()
		}
		return "", "", true
	case "ctrl+b":
		if e.kind == docMarkdown {
			e.wrapSel("**")
		}
		return "", "", true
	case "backspace", "ctrl+h":
		if _, _, sel := e.selection(); !sel && e.softBackspace() {
			return "", "", true
		}
	}
	if e.kind == docJSON && k.Text != "" && k.Mod&^tea.ModShift == 0 {
		if _, _, sel := e.selection(); !sel && e.jsonPair(k.Text) {
			return "", "", true
		}
	}
	buf, pos, anchor, copied, ok := editSel(e.buf, e.pos, e.anchor, k, s)
	if !ok {
		return "", "", s == "ctrl+c" // nothing picked to copy; it never quits from here
	}
	if len(buf) != len(e.buf) || string(buf) != string(e.buf) {
		typed := k.Text != "" && k.Mod&^tea.ModShift == 0 && !strings.ContainsAny(k.Text, " \n")
		e.change(buf, pos, typed)
		return "", copied, true
	}
	e.pos, e.anchor, e.goal, e.typing = pos, anchor, -1, false
	return "", copied, true
}

// newline breaks the line, keeping its indent; in markdown a list goes on
// (and an empty item ends it), and in JSON an opened bracket indents.
func (e *docEditor) newline() {
	buf, pos := e.buf, e.pos
	if from, to, ok := e.selection(); ok {
		buf, pos = cut(buf, from, to), from
	}
	ls := lineStart(buf, pos)
	line := string(buf[ls:pos])
	lead := line[:len(line)-len(strings.TrimLeft(line, " \t"))]
	ins := "\n" + lead
	after := ""
	switch e.kind {
	case docMarkdown:
		if m := mdItem.FindStringSubmatch(line); m != nil {
			if strings.TrimSpace(line[len(m[0]):]) == "" && pos == lineEnd(buf, pos) {
				// An empty item: enter ends the list.
				e.change(cut(buf, ls, pos), ls, false)
				return
			}
			ins += nextBullet(m[2])
			if m[3] != "" {
				ins += "[ ] "
			}
		}
	case docJSON:
		before := strings.TrimRight(line, " ")
		if strings.HasSuffix(before, "{") || strings.HasSuffix(before, "[") {
			ins += e.indent
			if pos < len(buf) && (buf[pos] == '}' || buf[pos] == ']') {
				after = "\n" + lead
			}
		}
	}
	r := []rune(ins + after)
	e.change(insert(buf, pos, r), pos+len([]rune(ins)), false)
}

// nextBullet is the marker for the item after one: the same bullet, or the
// next number.
func nextBullet(b string) string {
	var n int
	var sep string
	if _, err := fmt.Sscanf(b, "%d%s", &n, &sep); err == nil {
		return fmt.Sprintf("%d%s ", n+1, sep)
	}
	return b + " "
}

// indentLines indents (or outdents) the lines of the selection or the
// cursor's line; tab with no selection off a list item types an indent.
func (e *docEditor) indentLines(out bool) {
	from, to, sel := e.selection()
	if !sel {
		from, to = e.pos, e.pos
		ls := lineStart(e.buf, e.pos)
		if !out && !(e.kind == docMarkdown && mdItem.MatchString(string(e.buf[ls:lineEnd(e.buf, e.pos)]))) {
			r := []rune(e.indent)
			e.change(insert(e.buf, e.pos, r), e.pos+len(r), false)
			return
		}
	}
	buf := append([]rune{}, e.buf...)
	pos, anchor := e.pos, e.anchor
	step := []rune(e.indent)
	var starts []int
	for i := lineStart(buf, from); ; {
		starts = append(starts, i)
		j := lineEnd(buf, i)
		if j >= to || j >= len(buf) {
			break
		}
		i = j + 1
	}
	for k := len(starts) - 1; k >= 0; k-- {
		i := starts[k]
		shift := 0
		if out {
			for shift < len(step) && i+shift < len(buf) && (buf[i+shift] == ' ' || buf[i+shift] == '\t') {
				shift++
				if buf[i+shift-1] == '\t' {
					break
				}
			}
			buf = cut(buf, i, i+shift)
			shift = -shift
		} else {
			buf = insert(buf, i, step)
			shift = len(step)
		}
		adj := func(p int) int {
			if p >= i {
				return max(i, p+shift)
			}
			return p
		}
		pos = adj(pos)
		if anchor >= 0 {
			anchor = adj(anchor)
		}
	}
	e.change(buf, pos, false)
	if sel {
		e.anchor = anchor
	}
}

// moveLine swaps the cursor's line with the one above or below.
func (e *docEditor) moveLine(up bool) {
	ls, le := e.lineRange(e.pos)
	col := e.pos - ls
	if up {
		if ls == 0 {
			return
		}
		ps := lineStart(e.buf, ls-1)
		a, b := string(e.buf[ps:ls-1]), string(e.buf[ls:le])
		buf := append(append(append([]rune{}, e.buf[:ps]...), []rune(b+"\n"+a)...), e.buf[le:]...)
		e.change(buf, ps+col, false)
		return
	}
	if le >= len(e.buf) {
		return
	}
	ne := lineEnd(e.buf, le+1)
	a, b := string(e.buf[ls:le]), string(e.buf[le+1:ne])
	buf := append(append(append([]rune{}, e.buf[:ls]...), []rune(b+"\n"+a)...), e.buf[ne:]...)
	e.change(buf, ls+len([]rune(b))+1+col, false)
}

// toggleTick ticks or unticks a markdown task item: "- [ ]" and "- [x]".
func (e *docEditor) toggleTick() {
	ls, le := e.lineRange(e.pos)
	line := string(e.buf[ls:le])
	m := mdItem.FindStringSubmatchIndex(line)
	if m == nil {
		return
	}
	var repl string
	switch {
	case m[6] >= 0 && strings.ContainsAny(line[m[6]:m[7]], "xX"):
		repl = line[:m[6]] + "[ ] " + line[m[7]:]
	case m[6] >= 0:
		repl = line[:m[6]] + "[x] " + line[m[7]:]
	default:
		repl = line[:m[1]] + "[ ] " + line[m[1]:]
	}
	d := len([]rune(repl)) - len([]rune(line))
	buf := append(append(append([]rune{}, e.buf[:ls]...), []rune(repl)...), e.buf[le:]...)
	e.change(buf, max(ls, e.pos+d), false)
}

// wrapSel puts mark either side of the selection, or of an empty pair
// with the cursor inside.
func (e *docEditor) wrapSel(mark string) {
	r := []rune(mark)
	from, to, ok := e.selection()
	if !ok {
		e.change(insert(e.buf, e.pos, append(append([]rune{}, r...), r...)), e.pos+len(r), false)
		return
	}
	buf := insert(insert(e.buf, to, r), from, r)
	e.change(buf, to+2*len(r), false)
}

// softBackspace takes back an indent step in leading space, or an empty
// pair of JSON brackets or quotes, whole. It says whether it did.
func (e *docEditor) softBackspace() bool {
	p := e.pos
	if p == 0 {
		return false
	}
	if e.kind == docJSON && p < len(e.buf) {
		if o, c := e.buf[p-1], e.buf[p]; o == '{' && c == '}' || o == '[' && c == ']' || o == '"' && c == '"' {
			e.change(cut(e.buf, p-1, p+1), p-1, false)
			return true
		}
	}
	ls := lineStart(e.buf, p)
	if e.indent == "\t" || p-ls < len(e.indent) || strings.TrimLeft(string(e.buf[ls:p]), " ") != "" {
		return false
	}
	n := (p - ls) % len(e.indent)
	if n == 0 {
		n = len(e.indent)
	}
	e.change(cut(e.buf, p-n, p), p-n, false)
	return true
}

// jsonPair closes a bracket or quote as it's opened, and steps over a
// closing one that's already there. It says whether it did either.
func (e *docEditor) jsonPair(t string) bool {
	if len(t) != 1 {
		return false
	}
	c := rune(t[0])
	var next rune
	if e.pos < len(e.buf) {
		next = e.buf[e.pos]
	}
	if (c == '}' || c == ']' || c == '"') && next == c {
		if c != '"' || e.inString() {
			e.pos++
			e.typing = false
			return true
		}
	}
	shut := map[rune]rune{'{': '}', '[': ']', '"': '"'}[c]
	if shut == 0 || c == '"' && e.inString() {
		return false
	}
	if next != 0 && !strings.ContainsRune(" \n\t,}]:", next) {
		return false
	}
	e.change(insert(e.buf, e.pos, []rune{c, shut}), e.pos+1, false)
	return true
}

// inString says whether the cursor is inside a JSON string on its line.
func (e *docEditor) inString() bool {
	in := false
	for i := lineStart(e.buf, e.pos); i < e.pos; i++ {
		switch e.buf[i] {
		case '\\':
			i++
		case '"':
			in = !in
		}
	}
	return in
}

// --- drawing ---

var edSelBG, edErrBG string

const edCursor = "\x1b[7m"

// view draws the file in w×h cells: a line-number gutter, then its rows,
// the cursor shown when it has the keys.
func (e *docEditor) view(w, h int, focused bool) []string {
	if h <= 0 {
		return nil
	}
	if e.nlVer != e.ver {
		e.nlines, e.nlVer = strings.Count(string(e.buf), "\n")+1, e.ver
	}
	gut := len(fmt.Sprint(e.nlines)) + 2
	tw := max(4, w-gut-1)
	e.tw, e.th = tw, h
	lay := e.layout(tw)
	cur := lay.rowOf(e.pos)
	if focused {
		switch {
		case cur < e.top:
			e.top = cur
		case cur >= e.top+h:
			e.top = cur - h + 1
		}
	}
	e.top = max(0, min(e.top, len(lay.rows)-h))

	diags := e.problems()
	byLine := map[int]docDiag{}
	for _, d := range diags {
		if old, ok := byLine[d.line]; !ok || old.warn && !d.warn {
			byLine[d.line] = d
		}
	}
	last := min(len(lay.rows), e.top+h)
	firstLine, lastLine := 0, 0
	if e.top < len(lay.rows) {
		firstLine = lay.rows[e.top].line
		lastLine = lay.rows[max(e.top, last-1)].line
	}
	styles := e.styles(firstLine, lastLine)
	lineStarts := make(map[int]int, lastLine-firstLine+1)
	for r := e.top; r < last; r++ {
		if _, ok := lineStarts[lay.rows[r].line]; !ok {
			lineStarts[lay.rows[r].line] = lineStart(e.buf, lay.rows[r].from)
		}
	}
	selFrom, selTo, sel := e.selection()
	out := make([]string, 0, h)
	for r := e.top; r < last; r++ {
		row := lay.rows[r]
		var b strings.Builder
		// The gutter: a mark on a line with a problem, and the number on
		// the line's first row.
		mark := " "
		if d, ok := byLine[row.line]; ok {
			if d.warn {
				mark = paint(cYellow, "▍")
			} else {
				mark = paint(cRed, "▍")
			}
		}
		b.WriteString(mark)
		num := strings.Repeat(" ", gut-2)
		if r == 0 || lay.rows[r-1].line != row.line {
			n := fmt.Sprintf("%*d", gut-2, row.line+1)
			if row.line == lay.rows[cur].line && focused {
				num = paint(cSub, n)
			} else {
				num = faint(n)
			}
		}
		b.WriteString(num + " ")
		st := styles[row.line]
		ls := lineStarts[row.line]
		errCol := -1
		if d, ok := byLine[row.line]; ok && !d.warn {
			errCol = d.col
		}
		prev := "\x00"
		used := 0
		for i := row.from; i < row.to; i++ {
			c := ""
			if k := i - ls; k >= 0 && k < len(st) {
				c = st[k]
			}
			if i-ls == errCol {
				c += edErrBG
			}
			if sel && i >= selFrom && i < selTo {
				c += edSelBG
			}
			if focused && i == e.pos {
				c += edCursor
			}
			if c != prev {
				b.WriteString(reset + c)
				prev = c
			}
			ch := e.buf[i]
			switch {
			case ch == '\t':
				b.WriteString("    ")
			case ch < 0x20 || ch == 0x7f:
				b.WriteString("·")
			default:
				b.WriteRune(ch)
			}
			used += docRuneW(ch)
		}
		b.WriteString(reset)
		if focused && e.pos == row.to && row.last {
			b.WriteString(edCursor + " " + reset)
			used++
		}
		if errCol >= 0 && row.last && errCol >= row.to-ls && used < tw {
			b.WriteString(edErrBG + " " + reset) // the problem is at the line's end
			used++
		}
		b.WriteString(strings.Repeat(" ", max(0, tw+1-used)))
		out = append(out, b.String())
	}
	for len(out) < h {
		out = append(out, faint(" ~"))
	}
	return out
}

// cursorAt is where the cursor is, as a line and column, from one.
func (e *docEditor) cursorAt() (int, int) {
	ls := lineStart(e.buf, e.pos)
	return strings.Count(string(e.buf[:ls]), "\n") + 1, e.pos - ls + 1
}
