package convo

import (
	"bufio"
	"bytes"
	"compress/flate"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/0xdeafcafe/rush/internal/state"
)

// A conversation's word index is every distinct word, in lower case, of
// what a search looks at in it: what you said, what the agent said, and
// its steps' commands, paths and output; not the folder it ran in, ids or
// pictures, which fill a transcript and match words nobody said. A search
// asks it first: a conversation without the words, as typed or a typo
// away, is passed over without reading the file at all.
//
// It's made from the session whenever one is read whole (a search's read,
// or the indexer's in the background), kept on disk under rush's folder so
// it outlasts rush, and grows with a transcript: the lines added since are
// read raw, their words added, which can only add words, never lose one.

// wordIndex is a conversation's words, each followed by a newline, and the
// file as it was indexed.
type wordIndex struct {
	size  int64
	mod   time.Time
	words []byte
}

// indexVersion is the format on disk; another is read as none.
const indexVersion = "rush-words 1"

var indexes struct {
	mu sync.Mutex
	m  map[string]*wordIndex
}

// indexDir is where the indexes are kept; "" in a test without a rush
// folder of its own, which keeps them in memory, never in the machine's.
func indexDir() string {
	if testing.Testing() && os.Getenv("RUSH_HOME") == "" {
		return ""
	}
	return filepath.Join(state.Dir(), "search")
}

// indexPath is where the index of the transcript at path is kept.
func indexPath(path string) string {
	sum := sha256.Sum256([]byte(path))
	return filepath.Join(indexDir(), hex.EncodeToString(sum[:12])+".words")
}

// indexFor is path's index if it's as the file stands (st), read on to it
// when the file has only grown; nil when there's none to trust.
func indexFor(path string, st os.FileInfo) *wordIndex {
	indexes.mu.Lock()
	ix := indexes.m[path]
	indexes.mu.Unlock()
	if ix == nil {
		ix = loadIndex(path)
		if ix == nil {
			return nil
		}
		keepIndex(path, ix, false)
	}
	switch {
	case st.Size() == ix.size && st.ModTime().Equal(ix.mod):
		return ix
	case st.Size() > ix.size:
		grown, err := ix.readOn(path, st)
		if err != nil {
			return nil
		}
		// Held, not written: the one on disk reads on from where it got
		// to just as well, and a live one grows by the second.
		keepIndex(path, grown, false)
		return grown
	}
	return nil // rewritten: read whole again
}

// mayHold reports whether the conversation can have every word: one of its
// words has it inside, or is a typo away, as wordMatch finds them. A word
// it can't answer for (one a word of text can't hold whole, as "a.b")
// counts as there.
func (ix *wordIndex) mayHold(words []string) bool {
	for _, w := range words {
		w = strings.ToLower(w)
		switch {
		case !indexable(w), bytes.Contains(ix.words, []byte(w)):
		case rawSearchable(w):
			if !newRawWord(w).typoIn(ix.words) {
				return false
			}
		case typos(w) == 0:
			return false
		}
		// A long word past ASCII may be a typo away, letter by letter
		// rather than byte by byte: the session says.
	}
	return true
}

// indexable is a word the index can say yes or no to: all of it is a
// word's, as wordRune has them.
func indexable(w string) bool {
	if w == "" {
		return false
	}
	for _, r := range w {
		if !wordRune(r) {
			return false
		}
	}
	return true
}

// sessionWords are the distinct words of what Search looks at in s, in
// lower case, each followed by a newline.
func sessionWords(s *Session) []byte {
	seen := map[string]struct{}{}
	add := func(text string) { eachWord(text, func(w string) { seen[w] = struct{}{} }) }
	for _, t := range s.Turns {
		add(t.Prompt)
		for _, it := range t.Items {
			switch it.Kind {
			case KText:
				add(it.Text)
			case KStep:
				for _, st := range append([]*Step{it.Step}, it.Step.Children...) {
					x := st.in()
					for _, f := range []string{x.Description, x.Command, x.Path, x.Pattern, x.URL, x.Query, st.Tool, st.Output} {
						add(f)
					}
				}
			}
		}
	}
	return joinWords(seen)
}

func joinWords(seen map[string]struct{}) []byte {
	all := make([]string, 0, len(seen))
	n := 0
	for w := range seen {
		all = append(all, w)
		n += len(w) + 1
	}
	slices.Sort(all)
	out := make([]byte, 0, n)
	for _, w := range all {
		out = append(append(out, w...), '\n')
	}
	return out
}

// eachWord calls f with each word of text, in lower case.
func eachWord(text string, f func(string)) {
	for i := 0; i < len(text); {
		r, n := utf8.DecodeRuneInString(text[i:])
		if !wordRune(r) {
			i += n
			continue
		}
		j := i
		for j < len(text) {
			r, n := utf8.DecodeRuneInString(text[j:])
			if !wordRune(r) {
				break
			}
			j += n
		}
		f(lowerWord(text[i:j]))
		i = j
	}
}

// lowerWord is w in lower case, without a copy when it already is.
func lowerWord(w string) string {
	for i := 0; i < len(w); i++ {
		if c := w[i]; 'A' <= c && c <= 'Z' || c >= utf8.RuneSelf {
			return strings.Map(unicode.ToLower, w)
		}
	}
	return w
}

// readOn is ix with the words of what the file gained since added: read
// raw, as JSON, its escapes (\n, é) breaking words rather than
// joining them, so no word said is lost; what isn't said (a key, an id)
// can only make a search read a file it needn't.
func (ix *wordIndex) readOn(path string, st os.FileInfo) (*wordIndex, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	if _, err := f.Seek(ix.size, io.SeekStart); err != nil {
		return nil, err
	}
	b, err := io.ReadAll(io.LimitReader(f, st.Size()-ix.size))
	if err != nil {
		return nil, err
	}
	seen := map[string]struct{}{}
	for w := range bytes.SplitSeq(ix.words, []byte{'\n'}) {
		if len(w) > 0 {
			seen[string(w)] = struct{}{}
		}
	}
	rawWords(b, func(w string) { seen[w] = struct{}{} })
	return &wordIndex{size: ix.size + int64(len(b)), mod: st.ModTime(), words: joinWords(seen)}, nil
}

// rawWords calls f with each word of raw JSON b, in lower case: a
// backslash and what it escapes end a word.
func rawWords(b []byte, f func(string)) {
	var cur []byte
	flush := func() {
		if len(cur) > 0 {
			f(lowerWord(string(cur)))
			cur = cur[:0]
		}
	}
	for i := 0; i < len(b); {
		if b[i] == '\\' {
			flush()
			i += 2
			if i-1 < len(b) && b[i-1] == 'u' {
				i += 4
			}
			continue
		}
		r, n := utf8.DecodeRune(b[i:])
		if wordRune(r) {
			cur = append(cur, b[i:i+n]...)
		} else {
			flush()
		}
		i += n
	}
	flush()
}

// indexSession indexes the conversation at path from s, as read whole when
// the file stood as st.
func indexSession(path string, st os.FileInfo, s *Session) {
	keepIndex(path, &wordIndex{size: st.Size(), mod: st.ModTime(), words: sessionWords(s)}, true)
}

// keepIndex holds ix for path and, when save, writes it to disk.
func keepIndex(path string, ix *wordIndex, save bool) {
	indexes.mu.Lock()
	if indexes.m == nil {
		indexes.m = map[string]*wordIndex{}
	}
	indexes.m[path] = ix
	indexes.mu.Unlock()
	if save {
		_ = saveIndex(path, ix)
	}
}

// Indexed reports whether the conversation at path has an index as the
// file stands, or one it can be read on to.
func Indexed(path string) bool {
	st, err := os.Stat(path)
	return err == nil && indexFor(path, st) != nil
}

func saveIndex(path string, ix *wordIndex) error {
	if indexDir() == "" {
		return nil
	}
	if err := os.MkdirAll(indexDir(), 0o700); err != nil {
		return err
	}
	to := indexPath(path)
	tmp, err := os.CreateTemp(indexDir(), ".words-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	w := bufio.NewWriter(tmp)
	fmt.Fprintf(w, "%s\n%s\n%d %d\n", indexVersion, path, ix.size, ix.mod.UnixNano())
	z, _ := flate.NewWriter(w, flate.BestSpeed)
	_, _ = z.Write(ix.words)
	if err := z.Close(); err != nil {
		tmp.Close()
		return err
	}
	if err := w.Flush(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), to)
}

func loadIndex(path string) *wordIndex {
	if indexDir() == "" {
		return nil
	}
	f, err := os.Open(indexPath(path))
	if err != nil {
		return nil
	}
	defer f.Close()
	r := bufio.NewReader(f)
	head := func() string { l, _ := r.ReadString('\n'); return strings.TrimSuffix(l, "\n") }
	if head() != indexVersion || head() != path {
		return nil
	}
	var size, mod int64
	if _, err := fmt.Sscan(head(), &size, &mod); err != nil {
		return nil
	}
	words, err := io.ReadAll(flate.NewReader(r))
	if err != nil {
		return nil
	}
	return &wordIndex{size: size, mod: time.Unix(0, mod), words: words}
}
