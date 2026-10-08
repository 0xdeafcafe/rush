package convo

import (
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"unicode/utf8"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agtools"
)

// A step's output can be seen as the text it is, laid out (pretty), as its
// bytes (hex) or as the picture it is. Each step opens in the view that
// suits it; Options.View holds the ones switched to since, by ref.
const (
	ViewText   = "text"
	ViewPretty = "pretty"
	ViewHex    = "hex"
	ViewImage  = "image"
)

// hexCap is the most of a file the hex view reads.
const hexCap = 64 << 10

// NextView is the view after cur among those step ref's output can be
// seen in, or "" when it has no other; cur "" is the one it opened in.
func (s *Session) NextView(ref, cur string) string {
	_, id, _ := strings.Cut(ref, ":s:")
	vs, auto := s.views(s.Step(id))
	if len(vs) < 2 {
		return ""
	}
	if cur == "" {
		cur = auto
	}
	for i, v := range vs {
		if v == cur {
			return vs[(i+1)%len(vs)]
		}
	}
	return vs[0]
}

// views are the ways st's output can be seen, and the one it opens in:
// hex only while the hex plugin is on (Session.Hex).
func (s *Session) views(st *Step) ([]string, string) {
	vs, auto := s.allViews(st)
	if s.Hex {
		return vs, auto
	}
	vs = slices.DeleteFunc(slices.Clone(vs), func(v string) bool { return v == ViewHex })
	if auto == ViewHex {
		auto = ViewText
	}
	return vs, auto
}

func (s *Session) allViews(st *Step) ([]string, string) {
	if st == nil || !viewable(st) {
		return nil, ""
	}
	path := s.readPath(st)
	if len(st.Images) > 0 || thumbable(path) {
		return []string{ViewImage, ViewHex}, ViewImage
	}
	t := viewText(st)
	if binary(t) || st.kind() == tool.Read && st.Status == Failed && fileBinary(path) {
		return []string{ViewText, ViewHex}, ViewHex
	}
	if strings.TrimSpace(t) == "" {
		return nil, ""
	}
	if structure(strings.TrimSpace(t)) == 0 {
		return []string{ViewText, ViewHex}, ViewText
	}
	vs := []string{ViewText, ViewPretty, ViewHex}
	if st.Status != Failed && betterPretty(t) {
		return vs, ViewPretty
	}
	return vs, ViewText
}

// view is the view st is drawn in, and whether it has any other.
func (d *drawer) viewOf(st *Step, ref string) (string, bool) {
	vs, auto := d.s.views(st)
	if v := d.o.View[ref]; v != "" {
		for _, x := range vs {
			if x == v {
				return v, true
			}
		}
	}
	return auto, len(vs) > 1
}

// NoWrapRef is the Options.Open key that keeps each line of step ref's
// output to one row, cut with ›, instead of wrapping it.
func NoWrapRef(ref string) string { return ref + ":nowrap" }

// viewHint says which view a step's output is in, when it's picked or
// isn't in plain text.
func (d *drawer) viewHint(st *Step, ref string) string {
	v, more := d.viewOf(st, ref)
	if !more || d.o.Selected != ref && (v == ViewText || v == ViewImage) {
		return ""
	}
	return faint(v + " · v cycles")
}

// viewable is whether a step's output is its own to switch views of: not
// an edit's diff, a subagent's steps or a message.
func viewable(st *Step) bool {
	switch st.kind() {
	case tool.Edit, tool.Write, tool.Subagent:
		return false
	}
	return !messageTool(st.Tool) && st.Tool != agtools.Show && !stopTool(st.Tool)
}

// viewText is the text a step's views are of: a command's stdout, a read
// file without its line numbers, else what the tool gave back.
func viewText(st *Step) string {
	switch st.kind() {
	case tool.Shell:
		if o := st.out(); o.Stdout != "" {
			return o.Stdout
		}
	case tool.Read:
		return unnumbered(st.Output)
	}
	return st.Output
}

// unnumbered is a read's output without the line numbers before each line.
func unnumbered(s string) string {
	lines := strings.Split(s, "\n")
	for i, l := range lines {
		if n, path := codePrefix(l); n > 0 && path == "" {
			lines[i] = l[n:]
		}
	}
	return strings.Join(lines, "\n")
}

// binary is whether s is bytes rather than text: a NUL, or not UTF-8, or
// more than a few characters that were lost turning it into text.
func binary(s string) bool {
	return strings.IndexByte(s, 0) >= 0 || !utf8.ValidString(s) || strings.Count(s, "�") > max(2, len(s)/100)
}

func (s *Session) readPath(st *Step) string {
	if st.kind() != tool.Read {
		return ""
	}
	return s.abs(st.in().Path)
}

// fileBinary is whether the file at path starts with bytes, not text.
func fileBinary(path string) bool {
	bs, _, err := readHead(path, 4096)
	return err == nil && binary(string(bs))
}

// heads keeps each file's first bytes, read by a goroutine the first time
// they're asked for: a frame never waits on the disk, and is drawn again
// (lookupsGen) once they're in.
// ponytail: never forgets or rereads; key on mtime if files change under it.
var heads sync.Map

type head struct {
	bs   []byte
	size int64
	err  error
}

// errNotYet is a file whose head is still being read.
var errNotYet = errors.New("not read yet")

// readHead is up to n of path's first bytes and how big it is.
func readHead(path string, n int) ([]byte, int64, error) {
	if path == "" {
		return nil, 0, os.ErrNotExist
	}
	k := fmt.Sprint(path, n)
	h, known := heads.LoadOrStore(k, head{err: errNotYet})
	if !known {
		go func() {
			heads.Store(k, readHeadNow(path, n))
			lookupsGen.Add(1)
		}()
	}
	hd := h.(head)
	if hd.err == errNotYet {
		lookupWaits.Add(1)
	}
	return hd.bs, hd.size, hd.err
}

func readHeadNow(path string, n int) head {
	f, err := os.Open(path)
	if err != nil {
		return head{err: err}
	}
	defer f.Close()
	fi, err := f.Stat()
	if err != nil || fi.IsDir() {
		return head{err: os.ErrNotExist}
	}
	bs, err := io.ReadAll(io.LimitReader(f, int64(n)))
	return head{bs: bs, size: fi.Size(), err: err}
}

// realDirs are folders as their links resolve, by folder.
var realDirs sync.Map

// realDir is where dir's links lead, once a goroutine has looked: a render
// never waits on the disk, it draws with dir until then.
func realDir(dir string) (string, bool) {
	r, looked := realDirs.LoadOrStore(dir, "")
	if r != "" {
		return r.(string), true
	}
	lookupWaits.Add(1)
	if !looked {
		go func() {
			r, err := filepath.EvalSymlinks(dir)
			if err != nil {
				r = dir
			}
			realDirs.Store(dir, r)
			lookupsGen.Add(1)
		}()
	}
	return dir, false
}

// hexSource is the bytes st's hex view shows and how many there are in
// all: the file it read or the image it has, else the text it gave back.
func (d *drawer) hexSource(st *Step) ([]byte, int64) {
	paths := []string{d.s.readPath(st)}
	for _, img := range st.Images {
		if len(img.Data) > 0 {
			return img.Data[:min(len(img.Data), hexCap)], int64(len(img.Data))
		}
		paths = append(paths, d.abs(img.Path))
	}
	for _, p := range paths {
		if bs, size, err := readHead(p, hexCap); err == nil {
			return bs, size
		}
	}
	t := viewText(st)
	if st.kind() == tool.Read {
		t = st.Output // no file to read: what the tool gave back, as it came
	}
	return []byte(t[:min(len(t), hexCap)]), int64(len(t))
}

// hexBody draws st's bytes as a hex viewer does: sixteen to a row, or
// eight in a narrow pane; the first rows until ctrl+o shows them all.
func (d *drawer) hexBody(st *Step, indent int) {
	bs, size := d.hexSource(st)
	pad := d.spine() + strings.Repeat(" ", indent-1) + faint("▏")
	per := 16
	if d.cw-indent-2 < 78 {
		per = 8
	}
	rows := (len(bs) + per - 1) / per
	show := rows
	if !d.o.Verbose && rows > 16 {
		show = 12
	}
	for r := range show {
		d.add("", bgWell, pad+hexRow(r*per, bs[r*per:min(len(bs), (r+1)*per)], -1, per), "")
	}
	if show < rows {
		d.add("", bgWell, pad+dim(fmt.Sprintf("… %d more rows ", rows-show))+faint("·")+" "+paint(cOrange+bold, "ctrl+o")+dim(" shows all"), "")
	}
	switch {
	case len(bs) == 0:
		d.add("", bgWell, pad+faint("no bytes"), "")
	case int64(len(bs)) < size:
		d.add("", bgWell, pad+faint(fmt.Sprintf("the first %s of %s", sizeWords(int64(len(bs))), sizeWords(size))), "")
	}
}

// sizeWords is n bytes as a person says it.
func sizeWords(n int64) string {
	switch {
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MiB", float64(n)/(1<<20))
	case n >= 1<<10:
		return fmt.Sprintf("%d KiB", n>>10)
	}
	return fmt.Sprintf("%d bytes", n)
}
