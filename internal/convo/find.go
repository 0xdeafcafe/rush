package convo

import (
	"bytes"
	"cmp"
	endian "encoding/binary"
	"errors"
	"io"
	"os"
	"slices"
	"strings"
	"time"
	"unicode/utf8"
)

// SearchFile searches a transcript on disk as Search searches an open
// session: one pass over the raw bytes skips a file that doesn't hold
// every word, and only a file that does is read into a session and
// searched, and held a while for the next search (searchcache.go). It
// keeps up to limit hits and counts them all. A query with no words finds
// nothing: every transcript would have to be read in full.
func SearchFile(path, q string, limit int) (Found, error) {
	return search(path, q, limit, nil)
}

// Found is what a search found in one conversation: its closest hits, the
// closest first, how many there were, and when it started and last moved.
type Found struct {
	Hits        []Hit
	Total       int
	First, Last time.Time
}

// Best is the closest hit's score, 0 with none.
func (f Found) Best() int {
	if len(f.Hits) == 0 {
		return 0
	}
	return f.Hits[0].Score
}

// SearchRead is SearchFile for a file in another agent's format: read
// turns it into a session (nil when it can't), only once the file holds
// every word.
func SearchRead(path, q string, limit int, read func() *Session) (Found, error) {
	return search(path, q, limit, read)
}

// search is SearchFile, or SearchRead when read is set.
func search(path, q string, limit int, read func() *Session) (Found, error) {
	p := parseQuery(q)
	if len(p.words) == 0 {
		return Found{}, nil
	}
	st, err := os.Stat(path)
	if err != nil {
		return Found{}, err
	}
	h := heldFor(path, st)
	if h == nil {
		// Its index says, when it has one; else a pass over the raw bytes.
		ix := indexFor(path, st)
		if ix != nil {
			if !ix.mayHold(p.words) {
				return Found{}, nil
			}
		} else if ok, err := fileHoldsAll(path, p.words); !ok || err != nil {
			return Found{}, err
		}
		h = &heldConvo{size: st.Size(), mod: st.ModTime()}
		if read != nil {
			h.sess = read()
		} else {
			h.tail = NewTail(path)
			if _, err := h.tail.Read(); err != nil {
				return Found{}, err
			}
		}
		s := h.sess
		if h.tail != nil {
			s = h.tail.Sess
		}
		if s != nil && ix == nil {
			indexSession(path, st, s)
		}
		hold(path, h)
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	s := h.sess
	if h.tail != nil {
		if st.Size() > h.size {
			if _, err := h.tail.Read(); err != nil {
				return Found{}, err
			}
			grew(h, st.Size(), st.ModTime())
		}
		s = h.tail.Sess
	}
	if s == nil {
		return Found{}, nil
	}
	all := s.Search(q)
	// The closest first; of those as close, the latest.
	slices.SortStableFunc(all, func(a, b Hit) int {
		return cmp.Or(cmp.Compare(b.Score, a.Score), b.At.Compare(a.At))
	})
	return Found{Hits: all[:min(limit, len(all))], Total: len(all), First: s.First, Last: s.Last}, nil
}

// fileHoldsAll is holdsAll over the file at path.
func fileHoldsAll(path string, words []string) (bool, error) {
	f, err := os.Open(path)
	if err != nil {
		return false, err
	}
	defer f.Close()
	return holdsAll(f, words)
}

// holdsAll reports whether r holds every word, ignoring ASCII case, or a
// word a typo away from it (as wordMatch finds them). It can
// say yes when Search would say no (a word only in a field Search doesn't
// look at), never the other way round: words it can't look for in raw JSON
// (non-ASCII, quotes, backslashes) count as present.
func holdsAll(r io.Reader, words []string) (bool, error) {
	var need []*rawWord
	long := 0
	for _, w := range words {
		if rawSearchable(w) {
			need = append(need, newRawWord(w))
			long = max(long, len(w))
		}
	}
	if len(need) == 0 {
		return true, nil
	}
	bp := readBufs.Get().(*[]byte)
	defer readBufs.Put(bp)
	buf := *bp
	keep := 0 // the end of the last chunk, so a word across two is found
	for {
		n, err := io.ReadFull(r, buf[keep:])
		chunk := buf[:keep+n]
		lowerASCII(chunk[keep:])
		for i := 0; i < len(need); {
			if bytes.Contains(chunk, need[i].w) || need[i].typoIn(chunk) {
				need[i] = need[len(need)-1]
				need = need[:len(need)-1]
				continue
			}
			i++
		}
		if len(need) == 0 {
			return true, nil
		}
		if errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) {
			return false, nil
		}
		if err != nil {
			return false, err
		}
		keep = min(long-1, len(chunk))
		copy(buf, chunk[len(chunk)-keep:])
	}
}

// rawWord is a word looked for in raw JSON, as typed or a typo away.
type rawWord struct {
	w []byte
	k int // how many letters off it may be (typos)
	// class is each byte's: 0 not a word's, 1 a word's that w has, 2 a
	// word's that w hasn't, a letter off.
	class [256]uint8
}

func newRawWord(w string) *rawWord {
	rw := &rawWord{w: []byte(w), k: typos(w)}
	for c := range 256 {
		if wordBytes[c] {
			rw.class[c] = 2
		}
	}
	for i := 0; i < len(w); i++ {
		if wordBytes[w[i]] {
			rw.class[w[i]] = 1
		}
	}
	return rw
}

// wordBytes are the bytes of a word as wordRune has them; anything past
// ASCII counts, so a word with an accent isn't cut in two.
var wordBytes = func() (t [256]bool) {
	for c := range 256 {
		t[c] = 'a' <= c && c <= 'z' || '0' <= c && c <= '9' || c == '_' || c >= 0x80
	}
	return t
}()

// typoIn is whether chunk (lower case) has a word a typo away from rw. Two
// cheap checks rule out nearly every word before distance runs: a byte rw
// hasn't is a letter off, and one letter off keeps one of its first two
// in one of the word's first two places.
func (rw *rawWord) typoIn(chunk []byte) bool {
	k, w := rw.k, rw.w
	if k == 0 {
		return false
	}
	lo, hi := len(w)-k, len(w)+k
	class := &rw.class
	for i := 0; i < len(chunk); {
		if class[chunk[i]] == 0 {
			i++
			continue
		}
		j, off, wide := i, 0, false
		for j < len(chunk) {
			c := chunk[j]
			if class[c] == 0 {
				break
			}
			off += int(class[c] >> 1)
			wide = wide || c >= utf8.RuneSelf
			j++
		}
		if wide {
			// Letters past ASCII take bytes apiece: count letters.
			if distance([]rune(string(w)), []rune(string(chunk[i:j])), k) <= k {
				return true
			}
		} else if n := j - i; n >= lo && n <= hi && off <= k && (k > 1 || near(w, chunk[i:j])) && distance(w, chunk[i:j], k) <= k {
			return true
		}
		i = j
	}
	return false
}

// near is whether t could be one letter off w: changed, added, dropped or
// swapped, one of w's first two letters is in t's first two places.
func near(w, t []byte) bool {
	a, b := t[0], t[min(1, len(t)-1)]
	return a == w[0] || a == w[1] || b == w[0] || b == w[1]
}

// rawSearchable is a word that reads the same inside a JSON string.
func rawSearchable(w string) bool {
	for i := 0; i < len(w); i++ {
		if c := w[i]; c >= 0x80 || c < 0x20 || c == '"' || c == '\\' {
			return false
		}
	}
	return w != ""
}

// lowerASCII lowers b's ASCII capitals in place, eight bytes at a time: a
// byte's top bit says whether it's past 'A' and past 'Z', and a capital
// gets 0x20 added. Bytes past ASCII are left as they are.
func lowerASCII(b []byte) {
	const (
		low  = 0x7f7f7f7f7f7f7f7f
		high = 0x8080808080808080
		ones = 0x0101010101010101
	)
	i := 0
	for ; i+8 <= len(b); i += 8 {
		x := endian.LittleEndian.Uint64(b[i:])
		y := x & low
		upper := (y + ones*(0x80-'A')) &^ (y + ones*(0x80-'Z'-1)) &^ x & high
		if upper != 0 {
			endian.LittleEndian.PutUint64(b[i:], x|upper>>2)
		}
	}
	for ; i < len(b); i++ {
		if c := b[i]; 'A' <= c && c <= 'Z' {
			b[i] = c + 'a' - 'A'
		}
	}
}

// Words are the plain words of a query, without its filters.
func Words(q string) []string { return parseQuery(q).words }

// Filtered reports whether q narrows by kind, file or turn.
func Filtered(q string) bool {
	p := parseQuery(q)
	return len(p.kinds) > 0 || p.file != "" || p.from > 0
}

// FindFold finds w in s ignoring case and returns the byte span in s, or
// -1, -1.
func FindFold(s, w string) (start, end int) {
	if strings.TrimSpace(w) == "" {
		return -1, -1
	}
	return findFold(s, w)
}
