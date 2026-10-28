package claude

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"
)

// Writing, tidying and counting the use of auto memory notes, the way
// Claude Code writes them itself: one fact a file, with name, description
// and type in its frontmatter, and a line in MEMORY.md pointing at it.

// NoteTypes are the kinds of memory note Claude Code writes.
var NoteTypes = []string{"user", "feedback", "project", "reference"}

// Note is a memory note to write.
type Note struct {
	Name        string // short-kebab-case, its file's name too
	Title       string // its line in MEMORY.md
	Description string
	Type        string
	Body        string
}

var notSlug = regexp.MustCompile(`[^a-z0-9]+`)

// Slug is s as a note's file name: lower case, words joined by dashes.
func Slug(s string) string {
	s = strings.Trim(notSlug.ReplaceAllString(strings.ToLower(s), "-"), "-")
	if len(s) > 60 {
		s = strings.TrimRight(s[:60], "-")
	}
	return s
}

// WriteNote writes n into the memory folder dir as a new note, and its
// line into MEMORY.md, and says where. A name already taken gets a number
// (testing-2), so no note is ever written over.
func WriteNote(dir string, n Note) (string, error) {
	n.Name = Slug(n.Name)
	if n.Name == "" {
		n.Name = Slug(n.Title)
	}
	if n.Name == "" || strings.TrimSpace(n.Body) == "" {
		return "", errors.New("a memory note needs a name and some text")
	}
	if n.Name == "memory" {
		n.Name = "memory-note" // MEMORY.md, on a disk that ignores case
	}
	if !validType(n.Type) {
		n.Type = "project"
	}
	if n.Title = oneLine(n.Title); n.Title == "" {
		n.Title = n.Name
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	var path string
	err := locked(filepath.Join(dir, ".rush.lock"), func() error {
		for i := 1; ; i++ {
			name := n.Name
			if i > 1 {
				name = fmt.Sprintf("%s-%d", n.Name, i)
			}
			path = filepath.Join(dir, name+".md")
			n.Name = strings.TrimSuffix(filepath.Base(path), ".md")
			f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
			if errors.Is(err, fs.ErrExist) {
				n.Name = strings.TrimSuffix(n.Name, fmt.Sprintf("-%d", i))
				continue
			}
			if err != nil {
				return err
			}
			_, err = f.WriteString(noteText(n))
			if cerr := f.Close(); err == nil {
				err = cerr
			}
			if err != nil {
				os.Remove(path)
				return err
			}
			return indexLine(dir, path, n.Title, n.Description)
		}
	})
	return path, err
}

// locked runs f holding the lock file at lock, so two rushes, or rush and
// an agent's rush memory, don't write over each other's changes.
func locked(lock string, f func() error) error {
	lf, err := os.OpenFile(lock, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer lf.Close()
	if err := syscall.Flock(int(lf.Fd()), syscall.LOCK_EX); err != nil {
		return err
	}
	return f()
}

// readText is the file at path, "" when there's none; any other error is
// one, so a file that can't be read is never written over as if empty.
func readText(path string) (string, error) {
	b, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return "", nil
	}
	return string(b), err
}

// writeText replaces the file at path whole, by a rename, through a link
// to where it points.
func writeText(path, text string) error {
	if real, err := filepath.EvalSymlinks(path); err == nil {
		path = real
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+".*")
	if err != nil {
		return err
	}
	if _, err := tmp.WriteString(text); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	if st, err := os.Stat(path); err == nil {
		_ = os.Chmod(tmp.Name(), st.Mode().Perm())
	} else {
		_ = os.Chmod(tmp.Name(), 0o644)
	}
	return os.Rename(tmp.Name(), path)
}

func validType(t string) bool {
	for _, x := range NoteTypes {
		if x == t {
			return true
		}
	}
	return false
}

func noteText(n Note) string {
	var b strings.Builder
	fmt.Fprintf(&b, "---\nname: %s\ndescription: %s\nmetadata:\n  type: %s\n---\n\n%s\n",
		n.Name, yamlLine(n.Description), n.Type, strings.TrimSpace(n.Body))
	return b.String()
}

// yamlLine is s on one line, quoted, so YAML reads it as the text it is
// whatever it starts with (? - yes 10 ...).
func yamlLine(s string) string {
	b, _ := json.Marshal(oneLine(s))
	return string(b)
}

func oneLine(s string) string { return strings.Join(strings.Fields(s), " ") }

// indexLine gives the note at path a line in its MEMORY.md, unless one
// points at it already. The line keeps its link whole: the hook is what's
// cut to fit. The caller holds the folder's lock.
func indexLine(dir, path, title, hook string) error {
	index := filepath.Join(dir, "MEMORY.md")
	text, err := readText(index)
	if err != nil {
		return err
	}
	base := filepath.Base(path)
	for _, l := range strings.Split(text, "\n") {
		if target(l) == base {
			return nil
		}
	}
	title = strings.NewReplacer("[", "(", "]", ")").Replace(oneLine(title))
	if r := []rune(title); len(r) > 60 {
		title = string(r[:59]) + "…"
	}
	line := "- [" + title + "](" + base + ")"
	if hook = oneLine(hook); hook != "" {
		line += " — " + hook
	}
	if r := []rune(line); len(r) > IndexLineLen {
		line = string(r[:IndexLineLen-1]) + "…"
	}
	text = strings.TrimRight(text, "\n")
	if text != "" {
		text += "\n"
	}
	return writeText(index, text+line+"\n")
}

var indexEntry = regexp.MustCompile(`^\s*[-*]\s*\[[^\]]*\]\(([^)\s]+)\)`)

// target is the note an index line is for: the file its first link,
// at its start, points at, "" for any other line.
func target(line string) string {
	m := indexEntry.FindStringSubmatch(line)
	if m == nil || strings.Contains(m[1], "://") || strings.Contains(strings.TrimPrefix(m[1], "./"), "/") {
		return ""
	}
	return strings.TrimPrefix(m[1], "./")
}

// dropLines rewrites MEMORY.md in dir without the index lines for which
// drop says so, and says how many went. Other lines, and links further
// along a line, are left as they are.
func dropLines(dir string, drop func(note string) bool) (int, error) {
	index := filepath.Join(dir, "MEMORY.md")
	text, err := readText(index)
	if err != nil || text == "" {
		return 0, err
	}
	var keep []string
	n := 0
	for _, l := range strings.Split(text, "\n") {
		if t := target(l); t != "" && drop(t) {
			n++
			continue
		}
		keep = append(keep, l)
	}
	if n == 0 {
		return 0, nil
	}
	return n, writeText(index, strings.Join(keep, "\n"))
}

// ReadNote reads the note at path: its frontmatter, and its text below.
func ReadNote(path string) (Note, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return Note{}, err
	}
	fm := FrontMatterOf(bytes.NewReader(b))
	n := Note{Name: fm["name"], Description: fm["description"], Type: fm["type"], Body: noteBody(string(b))}
	if n.Name == "" {
		n.Name = strings.TrimSuffix(filepath.Base(path), ".md")
	}
	n.Title = n.Name
	return n, nil
}

// noteBody is a note's text without its frontmatter.
func noteBody(s string) string {
	if !strings.HasPrefix(s, "---") {
		return strings.TrimSpace(s)
	}
	rest := s[3:]
	if i := strings.Index(rest, "\n---"); i >= 0 {
		rest = rest[i+4:]
		return strings.TrimSpace(rest)
	}
	return strings.TrimSpace(s)
}

// FixMemory tidies the memory folder dir the ways that need no judgement:
// a line in MEMORY.md for each note without one, and none for notes that
// don't exist. It says what it did.
func FixMemory(dir string) ([]string, error) {
	var did []string
	err := locked(filepath.Join(dir, ".rush.lock"), func() error {
		gone, err := dropLines(dir, func(note string) bool {
			_, err := os.Stat(filepath.Join(dir, note))
			return errors.Is(err, fs.ErrNotExist)
		})
		if err != nil {
			return err
		}
		if gone > 0 {
			did = append(did, plural(gone, "line for a note that doesn't exist taken out", "lines for notes that don't exist taken out"))
		}
		linked := map[string]bool{}
		for _, l := range ReadMemIndex(filepath.Join(dir, "MEMORY.md")).Links {
			linked[l.File] = true
		}
		paths, _ := filepath.Glob(filepath.Join(dir, "*.md"))
		sort.Strings(paths)
		added := 0
		for _, p := range paths {
			if filepath.Base(p) == "MEMORY.md" || linked[p] {
				continue
			}
			fm := FrontMatter(p)
			title := fm["name"]
			if title == "" {
				title = strings.TrimSuffix(filepath.Base(p), ".md")
			}
			if err := indexLine(dir, p, title, fm["description"]); err != nil {
				return err
			}
			added++
		}
		if added > 0 {
			did = append(did, plural(added, "note added to MEMORY.md", "notes added to MEMORY.md"))
		}
		return nil
	})
	return did, err
}

// MergeNotes puts the note at drop into the one at keep, below its text,
// and deletes drop and its line in MEMORY.md.
func MergeNotes(keep, drop string) error {
	if keep == drop {
		return errors.New("a note can't be merged into itself")
	}
	dir := filepath.Dir(drop)
	return locked(filepath.Join(dir, ".rush.lock"), func() error {
		k, err := os.ReadFile(keep)
		if err != nil {
			return err
		}
		d, err := ReadNote(drop)
		if err != nil {
			return err
		}
		if err := writeText(keep, strings.TrimRight(string(k), "\n")+"\n\n"+d.Body+"\n"); err != nil {
			return err
		}
		return forget(drop)
	})
}

// ForgetNote deletes a file, and when it's a memory note, its line in
// MEMORY.md.
func ForgetNote(path string) error {
	dir := filepath.Dir(path)
	if filepath.Base(dir) != "memory" || filepath.Base(path) == "MEMORY.md" {
		return os.Remove(path)
	}
	return locked(filepath.Join(dir, ".rush.lock"), func() error { return forget(path) })
}

// forget is ForgetNote, its folder's lock held.
func forget(path string) error {
	if err := os.Remove(path); err != nil {
		return err
	}
	base := filepath.Base(path)
	_, err := dropLines(filepath.Dir(path), func(note string) bool { return note == base })
	return err
}

// MakeGlobal moves the note at path into the user's CLAUDE.md in cfg,
// which every session in every project loads, under a heading of its own.
func MakeGlobal(cfg, path string) error {
	n, err := ReadNote(path)
	if err != nil {
		return err
	}
	md := filepath.Join(cfg, "CLAUDE.md")
	lock := md + ".rush.lock"
	if real, err := filepath.EvalSymlinks(md); err == nil {
		lock = real + ".rush.lock" // one lock for every account that links to it
	}
	return locked(lock, func() error {
		text, err := readText(md)
		if err != nil {
			return err
		}
		if text = strings.TrimRight(text, "\n"); text != "" {
			text += "\n\n"
		}
		head := n.Description
		if head == "" {
			head = n.Name
		}
		text += "## " + oneLine(head) + "\n\n" + n.Body + "\n"
		if err := writeText(md, text); err != nil {
			return err
		}
		return ForgetNote(path)
	})
}

// Use is how often sessions read a memory note, and when last.
type Use struct {
	Reads int
	Last  time.Time
}

// recallCache keeps what each transcript was read for, by path, so a
// transcript is read only past where it was read last.
var recallCache = struct {
	sync.Mutex
	byFile map[string]*recallScan
}{byFile: map[string]*recallScan{}}

// recallScan is one transcript's reads of notes: each note's file name →
// when it was read. Its own lock is held while it's read, not the cache's.
type recallScan struct {
	sync.Mutex
	off   int64
	reads map[string][]time.Time
}

// NoteUses counts the reads of the notes in the memory folder dir since
// a time, across the transcripts in projects (Claude Code's projects
// folder) of the project whose folder is slug and its worktrees, their
// subagents' too. A note is opened by reading it, or by a shell command
// naming it, through any account's folder (accounts share projects by a
// link), so those are what's counted.
func NoteUses(projects, slug, dir string, since time.Duration) map[string]Use {
	out := map[string]Use{}
	dirs, _ := filepath.Glob(filepath.Join(projects, slug+"*"))
	cutoff := time.Now().Add(-since)
	key := "/projects/" + slug + "/memory/"
	live := map[string]bool{}
	for _, d := range dirs {
		if b := filepath.Base(d); b != slug && !strings.HasPrefix(b, slug+"--") {
			continue // another project whose name starts the same
		}
		files, _ := filepath.Glob(filepath.Join(d, "*.jsonl"))
		subs, _ := filepath.Glob(filepath.Join(d, "*", "subagents", "*.jsonl"))
		for _, f := range append(files, subs...) {
			st, err := os.Stat(f)
			if err != nil || st.ModTime().Before(cutoff) {
				continue
			}
			live[f+"\x00"+key] = true
			for name, at := range scanReads(f, st.Size(), key) {
				p := filepath.Join(dir, name)
				o := out[p]
				for _, t := range at {
					if t.Before(cutoff) {
						continue
					}
					o.Reads++
					if t.After(o.Last) {
						o.Last = t
					}
				}
				if o.Reads > 0 {
					out[p] = o
				}
			}
		}
	}
	// Transcripts gone or out of the window are let go.
	recallCache.Lock()
	for k := range recallCache.byFile {
		if strings.HasSuffix(k, "\x00"+key) && !live[k] {
			delete(recallCache.byFile, k)
		}
	}
	recallCache.Unlock()
	return out
}

var (
	noteFile = regexp.MustCompile(`^[A-Za-z0-9._-]+\.md`)
	// writes is a shell command that writes or deletes rather than reads.
	writes = regexp.MustCompile(`(^|[\s;&|])(rm|mv|cp|tee|sed -i)\s|>\s*\S*/memory/`)
)

// scanReads is when each note whose path has key was read in the
// transcript f, by file name, read from where it was read to last.
func scanReads(f string, size int64, key string) map[string][]time.Time {
	recallCache.Lock()
	s := recallCache.byFile[f+"\x00"+key]
	if s == nil {
		s = &recallScan{reads: map[string][]time.Time{}}
		recallCache.byFile[f+"\x00"+key] = s
	}
	recallCache.Unlock()
	s.Lock()
	defer s.Unlock()
	if size < s.off { // written over: from the start
		s.off, s.reads = 0, map[string][]time.Time{}
	}
	if size > s.off {
		s.read(f, []byte(key))
	}
	out := make(map[string][]time.Time, len(s.reads))
	for k, v := range s.reads {
		out[k] = v
	}
	return out
}

// read reads f from s.off on, its lock held.
func (s *recallScan) read(f string, key []byte) {
	fh, err := os.Open(f)
	if err != nil {
		return
	}
	defer fh.Close()
	if _, err := fh.Seek(s.off, io.SeekStart); err != nil {
		return
	}
	r := bufio.NewReaderSize(fh, 256<<10)
	off := s.off
	for {
		line, err := r.ReadBytes('\n')
		if err != nil {
			break // a line still being written is read next time
		}
		off += int64(len(line))
		if !bytes.Contains(line, key) || !bytes.Contains(line, []byte(`"Read"`)) && !bytes.Contains(line, []byte(`"Bash"`)) {
			continue
		}
		var rec struct {
			Timestamp time.Time `json:"timestamp"`
			Message   struct {
				Content []struct {
					Type  string `json:"type"`
					Name  string `json:"name"`
					Input struct {
						FilePath string `json:"file_path"`
						Command  string `json:"command"`
					} `json:"input"`
				} `json:"content"`
			} `json:"message"`
		}
		if json.Unmarshal(line, &rec) != nil {
			continue
		}
		for _, c := range rec.Message.Content {
			var names []string
			switch {
			case c.Type != "tool_use":
			case c.Name == "Read":
				if _, name, ok := strings.Cut(c.Input.FilePath, string(key)); ok && !strings.Contains(name, "/") {
					names = append(names, name)
				}
			case c.Name == "Bash" && !writes.MatchString(c.Input.Command):
				for rest := c.Input.Command; ; {
					_, after, ok := strings.Cut(rest, string(key))
					if !ok {
						break
					}
					if m := noteFile.FindString(after); m != "" && m != "MEMORY.md" {
						names = append(names, m)
					}
					rest = after
				}
			}
			for _, name := range names {
				s.reads[name] = append(s.reads[name], rec.Timestamp)
			}
		}
	}
	s.off = off
}
