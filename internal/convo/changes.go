package convo

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// FileChange is everything this session did to one file.
type FileChange struct {
	Path    string
	Add     int
	Del     int
	New     bool
	Turns   []int
	Patches []tool.Patch
	From    []string // for each patch, the step that made it ("t3:s:toolu_…")
	Content string   // a new file's content
	NewFrom string   // the step that created it
}

// Changes collects the session's edits by file, in the order files were
// first touched.
func (s *Session) Changes() []*FileChange {
	// Worked out again only when a step changed.
	if s.changes != nil && s.changesVer == s.stepVer {
		return s.changes
	}
	s.changes, s.changesVer = s.changesNow(), s.stepVer
	return s.changes
}

func (s *Session) changesNow() []*FileChange {
	byPath := map[string]*FileChange{}
	var order []*FileChange
	for _, t := range s.Turns {
		for _, it := range t.Items {
			if it.Kind != KStep {
				continue
			}
			st := it.Step
			switch {
			case st.kind() == tool.Edit || st.kind() == tool.Write:
			default:
				continue
			}
			if st.Status != OK {
				continue
			}
			path := st.in().Path
			fc := byPath[path]
			if fc == nil {
				fc = &FileChange{Path: path}
				byPath[path] = fc
				order = append(order, fc)
			}
			if n := len(fc.Turns); n == 0 || fc.Turns[n-1] != t.N {
				fc.Turns = append(fc.Turns, t.N)
			}
			o := st.out()
			stepRef := fmt.Sprintf("t%d:s:%s", t.N, st.ID)
			if o.Created {
				content := st.in().Content
				fc.New, fc.Content, fc.NewFrom = true, content, stepRef
				fc.Add += countLines(content)
				continue
			}
			for _, p := range o.Patches {
				fc.Patches = append(fc.Patches, p)
				fc.From = append(fc.From, stepRef)
				for _, l := range p.Lines {
					switch {
					case strings.HasPrefix(l, "+"):
						fc.Add++
					case strings.HasPrefix(l, "-"):
						fc.Del++
					}
				}
			}
		}
	}
	return order
}

// TreeFile is one file git reports as changed in the working tree.
type TreeFile struct {
	Path      string
	Add, Del  int
	Binary    bool
	Untracked bool
	Ours      bool // this session edited it
}

// Tree is what git sees in dir: changed and untracked files against HEAD.
type Tree struct {
	Dir   string
	Head  string
	Root  string
	Files []TreeFile
	Err   string
	at    time.Time
}

// Stat is the lines added and deleted in the working tree.
func (t *Tree) Stat() (add, del int) {
	for _, f := range t.Files {
		add, del = add+f.Add, del+f.Del
	}
	return add, del
}

var trees = struct {
	sync.Mutex
	m       map[string]*Tree
	reading map[string]bool
}{m: map[string]*Tree{}, reading: map[string]bool{}}

// WorkingTree is the last reading of git for dir. It never runs git while
// a frame is drawn: a reading older than 5 seconds is refreshed in the
// background, and the next frame picks it up. The first call returns an
// empty tree marked as reading.
func WorkingTree(dir string) *Tree {
	trees.Lock()
	defer trees.Unlock()
	t := trees.m[dir]
	if (t == nil || time.Since(t.at) > 5*time.Second) && !trees.reading[dir] {
		trees.reading[dir] = true
		go func() {
			nt := readTree(dir)
			trees.Lock()
			trees.m[dir], trees.reading[dir] = nt, false
			trees.Unlock()
		}()
	}
	if t == nil {
		return &Tree{Dir: dir, Err: "reading git…"}
	}
	return t
}

// RemoteGit, when set, runs git for a folder that is another machine's
// (handled says it was one); the rush view sets it to rush remote's.
var RemoteGit func(dir string, args ...string) (out []byte, handled bool, err error)

// git runs git in dir, giving up after 10 seconds.
func git(dir string, args ...string) ([]byte, error) {
	if RemoteGit != nil {
		if out, ok, err := RemoteGit(dir, args...); ok {
			return out, err
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0") // a look must not block the agent's own git
	return cmd.Output()
}

func readTree(dir string) *Tree {
	t := &Tree{Dir: dir, at: time.Now()}
	top, err := git(dir, "rev-parse", "--show-toplevel")
	if err != nil {
		t.Err = "not a git repository"
		return t
	}
	root := strings.TrimSpace(string(top))
	t.Root = root
	base := "HEAD"
	if head, err := git(dir, "rev-parse", "--short", "HEAD"); err == nil {
		t.Head = strings.TrimSpace(string(head))
	} else {
		t.Head, base = "no commits yet", "--cached" // staged files are all there is
	}
	// -z keeps odd paths unquoted; no renames, so each path is a real file.
	out, _ := git(root, "diff", base, "--numstat", "-z", "--no-renames")
	for _, rec := range strings.Split(string(out), "\x00") {
		f := strings.SplitN(rec, "\t", 3)
		if len(f) != 3 || f[2] == "" {
			continue
		}
		tf := TreeFile{Path: filepath.Join(root, f[2])}
		if f[0] == "-" {
			tf.Binary = true
		} else {
			tf.Add, _ = strconv.Atoi(f[0])
			tf.Del, _ = strconv.Atoi(f[1])
		}
		t.Files = append(t.Files, tf)
	}
	// From the top, so a session in a subfolder still sees the whole repo.
	untracked, _ := git(root, "ls-files", "--others", "--exclude-standard", "-z")
	for _, p := range strings.Split(string(untracked), "\x00") {
		if p != "" {
			t.Files = append(t.Files, TreeFile{Path: filepath.Join(root, p), Untracked: true})
		}
	}
	sort.Slice(t.Files, func(i, j int) bool { return t.Files[i].Path < t.Files[j].Path })
	return t
}

// realPath resolves symlinks (macOS's /tmp is /private/tmp), so the same
// file matches however it was named.
func realPath(p string) string {
	realPaths.Lock()
	defer realPaths.Unlock()
	if r, ok := realPaths.m[p]; ok {
		return r
	}
	r := p
	if x, err := filepath.EvalSymlinks(p); err == nil {
		r = x
	} else if x, err := filepath.EvalSymlinks(filepath.Dir(p)); err == nil {
		r = filepath.Join(x, filepath.Base(p))
	}
	realPaths.m[p] = r
	return r
}

var realPaths = struct {
	sync.Mutex
	m map[string]string
}{m: map[string]string{}}

// ChangesView draws the session's own edits, then the working tree as git
// sees it, with the files this session didn't touch marked as such.
func (s *Session) ChangesView(o Options) []Line {
	if s.Partial {
		return Counting()
	}
	w := min(o.Width, o.rowCap())
	d := drawer{s: s, t: &Turn{}, o: o, cw: w}
	var out []Line
	add := func(ref, left, right string) {
		if ref != "" && ref == o.Selected {
			left = cursor() + strings.TrimPrefix(left, " ")
			out = append(out, Line{Text: row("", left, right, o.Width, w), Ref: ref})
			return
		}
		out = append(out, Line{Text: row("", left, right, o.Width, w), Ref: ref})
	}
	rule := func(title, meta string) {
		if len(out) > 0 {
			add("", "", "")
		}
		head := "  " + faint("▾") + " " + paint(cSub+bold, title)
		if meta != "" {
			head += "  " + dim(meta)
		}
		add("", head+" "+faint(strings.Repeat("─", max(0, w-len([]rune(stripANSI(head)))-3))), "")
	}

	changes := s.Changes()
	ours := map[string]bool{}
	adds, dels := 0, 0
	for _, fc := range changes {
		ours[realPath(fc.Path)] = true
		adds, dels = adds+fc.Add, dels+fc.Del
	}
	meta := "nothing edited yet"
	if len(changes) > 0 {
		meta = fmt.Sprintf("%s · +%d −%d", plural(len(changes), "file"), adds, dels)
		seen := 0
		for _, fc := range changes {
			if o.Marks[fc.Path] {
				seen++
			}
		}
		meta += fmt.Sprintf(KeyWord(" · %d of %d reviewed · alt+r marks one"), seen, len(changes))
	}
	rule("This session", meta)
	for _, fc := range changes {
		ref := "chg:" + fc.Path
		open := o.Open[ref]
		arrow := faint("▸")
		if open {
			arrow = faint("▾")
		}
		counts := paint(cGreen, fmt.Sprintf("+%d", fc.Add)) + " " + paint(cRed, fmt.Sprintf("−%d", fc.Del))
		if fc.New {
			counts = paint(cGreen, fmt.Sprintf("new · %d lines", fc.Add))
		}
		var turns []string
		for _, n := range fc.Turns {
			turns = append(turns, fmt.Sprintf("#%d", n))
		}
		name := text(d.rel(fc.Path))
		mark := "  "
		if o.Marks[fc.Path] {
			mark, name = paint(cGreen, "✓ "), dim(d.rel(fc.Path))
		}
		name = d.fileLink(fc.Path, name)
		add(ref, "  "+arrow+" "+mark+name, counts+"   "+dim(strings.Join(turns, " ")))
		if !open {
			continue
		}
		pad := strings.Repeat(" ", 7)
		bw := w - 16
		// Each hunk under a row naming the turn that made it; enter on it
		// goes to that step in the conversation.
		hunk := func(from, what string) {
			turn, _, _ := strings.Cut(from, ":")
			add("jump:"+from, pad+paint(cBlue, "@ ")+dim(what)+"  "+sub("#"+strings.TrimPrefix(turn, "t")), dim("enter goes to the step"))
		}
		lg := langFor(fc.Path)
		if fc.New {
			hunk(fc.NewFrom, "created")
			var st hlState
			for i, l := range strings.Split(strings.TrimRight(fc.Content, "\n"), "\n") {
				if i >= 40 && !o.Verbose {
					add("", pad+dim("… ctrl+o shows the rest"), "")
					break
				}
				add("", pad+paint(cGreen, "▏")+dim(fmt.Sprintf("%5d ", i+1))+diffText(lg, &st, l, cSub, bw), "")
			}
			// Edits made after it was created follow.
		}
		for pi, p := range fc.Patches {
			from := ""
			if pi < len(fc.From) {
				from = fc.From[pi]
			}
			hunk(from, fmt.Sprintf("line %d", p.NewStart))
			oldN, newN := p.OldStart, p.NewStart
			var oldSt, newSt hlState
			for _, l := range p.Lines {
				if l == "" {
					l = " "
				}
				if l[0] == '\\' {
					continue // "\ No newline at end of file"
				}
				switch l[0] {
				case '+':
					out = append(out, Line{Text: row(bgAdd, pad+dim(fmt.Sprintf("%5d ", newN))+plusSign()+" "+diffText(lg, &newSt, l[1:], cText, bw), "", o.Width, w)})
					newN++
				case '-':
					out = append(out, Line{Text: row(bgDel, pad+dim(fmt.Sprintf("%5d ", oldN))+minusSign()+" "+diffText(lg, &oldSt, l[1:], cText, bw), "", o.Width, w)})
					oldN++
				default:
					out = append(out, Line{Text: row(bgWell, pad+dim(fmt.Sprintf("%5d ", newN))+"  "+diffText(lg, &newSt, l[1:], cSub, bw), "", o.Width, w)})
					oldSt = newSt
					oldN++
					newN++
				}
			}
		}
	}

	dir := firstNonEmpty(s.Info.Cwd, s.Cwd)
	if dir == "" {
		return out
	}
	tree := WorkingTree(dir)
	if tree.Err != "" {
		rule("Working tree", tree.Err)
		return out
	}
	others := 0
	for i := range tree.Files {
		if ours[realPath(tree.Files[i].Path)] {
			tree.Files[i].Ours = true
		} else {
			others++
		}
	}
	meta = fmt.Sprintf("%s changed against %s", plural(len(tree.Files), "file"), tree.Head)
	if others > 0 {
		meta += fmt.Sprintf(" · %d not from this session", others)
	}
	rule("Working tree", meta)
	if len(tree.Files) == 0 {
		add("", "    "+dim("clean"), "")
	}
	for i, f := range tree.Files {
		if i >= 60 && !o.Verbose {
			add("", "    "+dim(fmt.Sprintf("… %d more · ctrl+o shows all", len(tree.Files)-i)), "")
			break
		}
		counts := paint(cGreen, fmt.Sprintf("+%d", f.Add)) + " " + paint(cRed, fmt.Sprintf("−%d", f.Del))
		switch {
		case f.Untracked:
			counts = paint(cGreen, "new")
		case f.Binary:
			counts = dim("binary")
		}
		who := dim("not from this session")
		mark := dim("·")
		if f.Ours {
			who, mark = paint(cOrange, "this session"), paint(cOrange, "●")
		}
		tref := "tree:" + f.Path
		arrow := faint("▸")
		if o.Open[tref] {
			arrow = faint("▾")
		}
		add(tref, "  "+arrow+" "+mark+" "+d.fileLink(f.Path, text(d.rel(f.Path))), counts+"   "+who)
		if o.Open[tref] {
			lg := langFor(f.Path)
			var oldSt, newSt hlState
			for _, l := range treeDiff(tree, f, false) {
				b, sign, st := bgWell, " ", &newSt
				switch {
				case strings.HasPrefix(l, "@@"):
					out = append(out, Line{Text: row("", "       "+paint(cBlue, truncateCells(l, w-10)), "", o.Width, w)})
					oldSt, newSt = hlState{}, hlState{}
					continue
				case strings.HasPrefix(l, "+"):
					b, sign = bgAdd, plusSign()
				case strings.HasPrefix(l, "-"):
					b, sign, st = bgDel, minusSign(), &oldSt
				}
				body := cleanOutput(l)
				if len(body) > 0 && strings.ContainsRune("+- ", rune(body[0])) {
					body = body[1:]
				}
				out = append(out, Line{Text: row(b, "       "+sign+" "+diffText(lg, st, body, cText, w-12), "", o.Width, w)})
				if b == bgWell {
					oldSt = newSt
				}
			}
		}
	}
	return out
}

// FileView draws one changed file in full, git's diff of it laid over it:
// every line, what was added on green, what went on red where it was.
func (s *Session) FileView(path string, o Options) []Line {
	w := min(o.Width, o.rowCap())
	d := drawer{s: s, t: &Turn{}, o: o, cw: w}
	tree := WorkingTree(firstNonEmpty(s.Info.Cwd, s.Cwd, filepath.Dir(path)))
	f := TreeFile{Path: path}
	for _, tf := range tree.Files {
		if realPath(tf.Path) == realPath(path) {
			f = tf
		}
	}
	lg := langFor(path)
	var oldSt, newSt hlState
	var body []Line
	adds, dels, n := 0, 0, 0
	for _, l := range treeDiff(tree, f, true) {
		if strings.HasPrefix(l, "@@") {
			continue // the one hunk is the whole file
		}
		b, sign, st, num := bgWell, " ", &newSt, "      "
		switch {
		case strings.HasPrefix(l, "+"):
			b, sign = bgAdd, plusSign()
			adds++
		case strings.HasPrefix(l, "-"):
			b, sign, st = bgDel, minusSign(), &oldSt
			dels++
		}
		if b != bgDel {
			n++
			num = dim(fmt.Sprintf("%5d ", n))
		}
		l = cleanOutput(l)
		if len(l) > 0 && strings.ContainsRune("+- ", rune(l[0])) {
			l = l[1:]
		}
		body = append(body, Line{Text: row(b, "  "+num+sign+" "+diffText(lg, st, l, cText, w-12), "", o.Width, w)})
		if b == bgWell {
			oldSt = newSt
		}
	}
	head := "  " + paint(cSub+bold, d.rel(path)) + "  " + paint(cGreen, fmt.Sprintf("+%d", adds)) + " " + paint(cRed, fmt.Sprintf("−%d", dels))
	return append([]Line{{Text: row("", head, "", o.Width, w)}, {}}, body...)
}

func firstNonEmpty(xs ...string) string {
	for _, x := range xs {
		if x != "" {
			return x
		}
	}
	return ""
}

// diffs holds each file's latest diff, by root and path: one per file, not
// one per reading of the tree.
var diffs = struct {
	sync.Mutex
	m       map[string]treeDiffs
	reading map[string]bool
}{m: map[string]treeDiffs{}, reading: map[string]bool{}}

type treeDiffs struct {
	at    int64 // the reading of the tree it's of
	lines []string
}

// treeDiff is git's own diff of one working-tree file against HEAD (or the
// file itself when it's untracked), read in the background: the first call
// says so and the next frame has it. It is read again with each new
// reading of the tree, and the last one shows until that's in. Full, it's
// the whole file with the diff laid over it, uncut, and the file as it is
// when git has no diff of it.
func treeDiff(t *Tree, f TreeFile, full bool) []string {
	key, at := t.Root+"\x00"+f.Path+"\x00"+strconv.FormatBool(full), t.at.UnixNano()
	diffs.Lock()
	defer diffs.Unlock()
	d, ok := diffs.m[key]
	if ok && d.at == at {
		return d.lines
	}
	if !diffs.reading[key] {
		diffs.reading[key] = true
		go func() {
			args := []string{"diff"}
			if full {
				args = append(args, "--unified=100000000")
			}
			if f.Untracked {
				args = append(args, "--no-index", "--", "/dev/null", f.Path)
			} else {
				args = append(args, "HEAD", "--", f.Path)
			}
			out, _ := git(firstNonEmpty(t.Root, t.Dir), args...)
			if full && len(out) == 0 {
				b, _ := os.ReadFile(f.Path)
				out = []byte(" " + strings.ReplaceAll(strings.TrimRight(string(b), "\n"), "\n", "\n "))
			}
			var lines []string
			for _, l := range strings.Split(strings.TrimRight(string(out), "\n"), "\n") {
				// The header lines say what the row already says.
				if strings.HasPrefix(l, "diff --git") || strings.HasPrefix(l, "index ") || strings.HasPrefix(l, "--- ") ||
					strings.HasPrefix(l, "+++ ") || strings.HasPrefix(l, "new file") || strings.HasPrefix(l, "\\") {
					continue
				}
				lines = append(lines, l)
			}
			if len(lines) > 400 && !full {
				lines = append(lines[:400], fmt.Sprintf("… %d more lines", len(lines)-400))
			}
			diffs.Lock()
			diffs.m[key] = treeDiffs{at: at, lines: lines}
			delete(diffs.reading, key)
			diffs.Unlock()
		}()
	}
	if ok {
		return d.lines
	}
	return []string{"reading the diff…"}
}
