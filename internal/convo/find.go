package convo

import (
	"bytes"
	"errors"
	"io"
	"os"
	"strings"
)

// SearchFile searches a transcript on disk as Search searches an open
// session, without keeping anything of it: one pass over the raw bytes
// skips a file that doesn't hold every word, and only a file that does is
// read into a session and searched. It keeps up to limit hits and counts
// them all. A query with no words finds nothing: every transcript would
// have to be read in full.
func SearchFile(path, q string, limit int) (hits []Hit, total int, err error) {
	p := parseQuery(q)
	if len(p.words) == 0 {
		return nil, 0, nil
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, 0, err
	}
	ok, err := holdsAll(f, p.words)
	f.Close()
	if !ok || err != nil {
		return nil, 0, err
	}
	t := NewTail(path)
	if _, err := t.Read(); err != nil {
		return nil, 0, err
	}
	all := t.Sess.Search(q)
	return all[:min(limit, len(all))], len(all), nil
}

// SearchRead is SearchFile for a file in another agent's format: read
// turns it into a session, only once the file holds every word.
func SearchRead(path, q string, limit int, read func() *Session) (hits []Hit, total int, err error) {
	p := parseQuery(q)
	if len(p.words) == 0 {
		return nil, 0, nil
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, 0, err
	}
	ok, err := holdsAll(f, p.words)
	f.Close()
	if !ok || err != nil {
		return nil, 0, err
	}
	all := read().Search(q)
	return all[:min(limit, len(all))], len(all), nil
}

// holdsAll reports whether r holds every word, ignoring ASCII case. It can
// say yes when Search would say no (a word only in a field Search doesn't
// look at), never the other way round: words it can't look for in raw JSON
// (non-ASCII, quotes, backslashes) count as present.
func holdsAll(r io.Reader, words []string) (bool, error) {
	var need [][]byte
	long := 0
	for _, w := range words {
		if rawSearchable(w) {
			need = append(need, []byte(w))
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
			if bytes.Contains(chunk, need[i]) {
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

// rawSearchable is a word that reads the same inside a JSON string.
func rawSearchable(w string) bool {
	for i := 0; i < len(w); i++ {
		if c := w[i]; c >= 0x80 || c < 0x20 || c == '"' || c == '\\' {
			return false
		}
	}
	return w != ""
}

func lowerASCII(b []byte) {
	for i, c := range b {
		if 'A' <= c && c <= 'Z' {
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
