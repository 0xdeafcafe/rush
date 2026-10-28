package claude

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
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

// WriteNote writes n into the memory folder dir, and its line into
// MEMORY.md, and says where. A note of the same name is replaced, and keeps
// its line.
func WriteNote(dir string, n Note) (string, error) {
	n.Name = Slug(n.Name)
	if n.Name == "" {
		n.Name = Slug(n.Title)
	}
	if n.Name == "" || strings.TrimSpace(n.Body) == "" {
		return "", errors.New("a memory note needs a name and some text")
	}
	if !validType(n.Type) {
		n.Type = "project"
	}
	if n.Title == "" {
		n.Title = n.Name
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	path := filepath.Join(dir, n.Name+".md")
	if err := os.WriteFile(path, []byte(noteText(n)), 0o644); err != nil {
		return "", err
	}
	return path, indexLine(dir, path, n.Title, n.Description)
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

// yamlLine is s on one line, quoted when YAML would read it otherwise.
func yamlLine(s string) string {
	s = strings.Join(strings.Fields(s), " ")
	if s == "" || strings.ContainsAny(s, ":#\"'[]{}&*!|>%@`") || strings.HasPrefix(s, "-") {
		b, _ := json.Marshal(s)
		return string(b)
	}
	return s
}

// indexLine gives the note at path a line in its MEMORY.md, unless one
// points at it already.
func indexLine(dir, path, title, hook string) error {
	index := filepath.Join(dir, "MEMORY.md")
	b, _ := os.ReadFile(index)
	if bytes.Contains(b, []byte("("+filepath.Base(path)+")")) {
		return nil
	}
	line := "- [" + strings.ReplaceAll(title, "]", ")") + "](" + filepath.Base(path) + ")"
	if hook = strings.Join(strings.Fields(hook), " "); hook != "" {
		line += " — " + hook
	}
	if r := []rune(line); len(r) > IndexLineLen {
		line = string(r[:IndexLineLen-1]) + "…"
	}
	text := strings.TrimRight(string(b), "\n")
	if text != "" {
		text += "\n"
	}
	return os.WriteFile(index, []byte(text+line+"\n"), 0o644)
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
	index := filepath.Join(dir, "MEMORY.md")
	ix := ReadMemIndex(index)
	var did []string
	if len(ix.Links) > 0 {
		b, err := os.ReadFile(index)
		if err != nil {
			return nil, err
		}
		gone := map[int]bool{}
		for _, l := range ix.Links {
			if _, err := os.Stat(l.File); err != nil {
				gone[l.Line] = true
			}
		}
		if len(gone) > 0 {
			var keep []string
			for i, l := range strings.Split(string(b), "\n") {
				if !gone[i+1] {
					keep = append(keep, l)
				}
			}
			if err := os.WriteFile(index, []byte(strings.Join(keep, "\n")), 0o644); err != nil {
				return nil, err
			}
			did = append(did, plural(len(gone), "line for a note that doesn't exist taken out", "lines for notes that don't exist taken out"))
		}
	}
	linked := map[string]bool{}
	for _, l := range ReadMemIndex(index).Links {
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
			return did, err
		}
		added++
	}
	if added > 0 {
		did = append(did, plural(added, "note added to MEMORY.md", "notes added to MEMORY.md"))
	}
	return did, nil
}

// MergeNotes puts the note at drop into the one at keep, below its text,
// and deletes drop and its line in MEMORY.md.
func MergeNotes(keep, drop string) error {
	k, err := os.ReadFile(keep)
	if err != nil {
		return err
	}
	d, err := ReadNote(drop)
	if err != nil {
		return err
	}
	text := strings.TrimRight(string(k), "\n") + "\n\n" + d.Body + "\n"
	if err := os.WriteFile(keep, []byte(text), 0o644); err != nil {
		return err
	}
	return ForgetNote(drop)
}

// ForgetNote deletes a file, and when it's a memory note, its line in
// MEMORY.md.
func ForgetNote(path string) error {
	if err := os.Remove(path); err != nil {
		return err
	}
	dir := filepath.Dir(path)
	index := filepath.Join(dir, "MEMORY.md")
	if filepath.Base(dir) != "memory" || path == index {
		return nil
	}
	b, err := os.ReadFile(index)
	if err != nil {
		return nil
	}
	link := "(" + filepath.Base(path) + ")"
	var keep []string
	for l := range strings.SplitSeq(string(b), "\n") {
		if !strings.Contains(l, link) {
			keep = append(keep, l)
		}
	}
	return os.WriteFile(index, []byte(strings.Join(keep, "\n")), 0o644)
}

// MakeGlobal moves the note at path into the user's CLAUDE.md in cfg,
// which every session in every project loads, under a heading of its own.
func MakeGlobal(cfg, path string) error {
	n, err := ReadNote(path)
	if err != nil {
		return err
	}
	md := filepath.Join(cfg, "CLAUDE.md")
	b, _ := os.ReadFile(md)
	text := strings.TrimRight(string(b), "\n")
	if text != "" {
		text += "\n\n"
	}
	head := n.Description
	if head == "" {
		head = n.Name
	}
	text += "## " + strings.Join(strings.Fields(head), " ") + "\n\n" + n.Body + "\n"
	if err := os.WriteFile(md, []byte(text), 0o644); err != nil {
		return err
	}
	return ForgetNote(path)
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

type recallScan struct {
	off  int64
	uses map[string]Use // a note's file name → its reads in this transcript
}

// NoteUses counts the reads of the notes in the memory folder dir, across
// the transcripts in projects (Claude Code's projects folder) of the
// project whose folder is slug and its worktrees, modified within since.
// A note is opened by reading it, or by a shell command naming it, so
// those are what's counted,
// through any account's folder (accounts share projects by a link).
func NoteUses(projects, slug, dir string, since time.Duration) map[string]Use {
	out := map[string]Use{}
	dirs, _ := filepath.Glob(filepath.Join(projects, slug+"*"))
	cutoff := time.Now().Add(-since)
	key := []byte("/projects/" + slug + "/memory/")
	for _, d := range dirs {
		if b := filepath.Base(d); b != slug && !strings.HasPrefix(b, slug+"--") {
			continue // another project whose name starts the same
		}
		files, _ := filepath.Glob(filepath.Join(d, "*.jsonl"))
		for _, f := range files {
			st, err := os.Stat(f)
			if err != nil || st.ModTime().Before(cutoff) {
				continue
			}
			uses := scanReads(f, st.Size(), key)
			recallCache.Lock()
			for name, u := range uses {
				p := filepath.Join(dir, name)
				o := out[p]
				o.Reads += u.Reads
				if u.Last.After(o.Last) {
					o.Last = u.Last
				}
				out[p] = o
			}
			recallCache.Unlock()
		}
	}
	return out
}

var (
	noteFile = regexp.MustCompile(`^[A-Za-z0-9._-]+\.md`)
	// writes is a shell command that writes or deletes rather than reads.
	writes = regexp.MustCompile(`(^|[\s;&|])(rm|mv|cp|tee|sed -i)\s|>\s*\S*/memory/`)
)

// scanReads is the reads of notes whose path has key in the transcript f,
// by file name, read from where it was read to last.
func scanReads(f string, size int64, key []byte) map[string]Use {
	recallCache.Lock()
	defer recallCache.Unlock()
	s := recallCache.byFile[f+"\x00"+string(key)]
	if s == nil || size < s.off {
		s = &recallScan{uses: map[string]Use{}}
		recallCache.byFile[f+"\x00"+string(key)] = s
	}
	if size == s.off {
		return s.uses
	}
	fh, err := os.Open(f)
	if err != nil {
		return s.uses
	}
	defer fh.Close()
	if _, err := fh.Seek(s.off, io.SeekStart); err != nil {
		return s.uses
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
				u := s.uses[name]
				u.Reads++
				if rec.Timestamp.After(u.Last) {
					u.Last = rec.Timestamp
				}
				s.uses[name] = u
			}
		}
	}
	s.off = off
	return s.uses
}
