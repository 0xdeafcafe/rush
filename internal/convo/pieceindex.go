package convo

import (
	"bufio"
	"compress/flate"
	endian "encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A conversation's piece index is what a search scores in it, kept apart
// from the transcript: each thing you said, each thing the agent said, and
// each step's label, command, path and output, with its turn and when it
// was, and for each word the pieces it's in. A search with words alone
// (no is:, file: or turn:) scores only the pieces that have every word,
// as Search scores them, without reading the transcript at all.
//
// It stands for the transcript as it was when it was made: one that has
// grown since is searched by reading it, until the indexer makes it again.

// Kinds of piece.
const (
	pieceYou = iota
	pieceAgent
	pieceStep
)

// piece is one thing Search scores.
type piece struct {
	kind  uint8
	turn  int
	at    time.Time
	ref   string
	who   string // a step's tool
	label string // a step's label
	text  string // what was said; a step's label, command, path and output
	outAt int    // where a step's output starts in text
}

// pieceIndex is a conversation's pieces, and the pieces each word is in.
type pieceIndex struct {
	size        int64
	mod         time.Time
	first, last time.Time
	pieces      []piece
	vocab       []string   // sorted
	posts       [][]uint32 // the pieces each word of vocab is in, in order
	bytes       int64      // about what it takes in memory
}

// sessionPieces are the pieces of s Search scores for words alone, in the
// order it scores them.
func sessionPieces(s *Session) []piece {
	var out []piece
	for _, t := range s.Turns {
		ref := fmt.Sprintf("t%d", t.N)
		if t.Prompt != "" {
			out = append(out, piece{kind: pieceYou, turn: t.N, at: t.Start, ref: ref, text: t.Prompt})
		}
		for _, it := range t.Items {
			switch it.Kind {
			case KText:
				if it.Text != "" {
					out = append(out, piece{kind: pieceAgent, turn: t.N, at: textAt(it, t), ref: ref, text: it.Text})
				}
			case KStep:
				for _, st := range append([]*Step{it.Step}, it.Step.Children...) {
					if hidden(st) {
						continue
					}
					x := st.in()
					file := x.Path
					if k := st.kind(); k == tool.Search || k == tool.Glob {
						file = ""
					}
					label := firstNonEmpty(x.Description, x.Command, file, x.Pattern, x.URL, x.Query, st.Tool)
					head := label + "\n" + x.Command + "\n" + file + "\n"
					out = append(out, piece{kind: pieceStep, turn: t.N, at: firstTime(st.Start, t.Start),
						ref: ref + ":s:" + st.ID, who: st.Tool, label: label, text: head + st.Output, outAt: len(head)})
				}
			}
		}
	}
	return out
}

// hit is pc as Search finds it for p, if it does.
func (p query) hit(pc *piece) (Hit, bool) {
	sc, ok := p.score(pc.text)
	if !ok {
		return Hit{}, false
	}
	h := Hit{Ref: pc.ref, Turn: pc.turn, At: pc.at}
	switch pc.kind {
	case pieceYou:
		h.Who, h.Snippet, h.Score = "you", snippet(pc.text, p.words), sc+20
	case pieceAgent:
		h.Who, h.Snippet, h.Score = WhoAgent, snippet(pc.text, p.words), sc+10
	default:
		h.Who, h.Score, h.Snippet = pc.who, sc, oneLine(pc.label)
		if !p.matches(pc.label) {
			h.Snippet += " · " + snippet(pc.text[pc.outAt:], p.words)
		}
	}
	return h, true
}

// buildPieces indexes s, read whole when its file stood as st.
func buildPieces(s *Session, st os.FileInfo) *pieceIndex {
	ix := &pieceIndex{size: st.Size(), mod: st.ModTime(), first: s.First, last: s.Last, pieces: sessionPieces(s)}
	ids := map[string]int{}
	var posts [][]uint32
	for i := range ix.pieces {
		pc := &ix.pieces[i]
		ix.bytes += int64(len(pc.text) + len(pc.ref) + len(pc.label) + 64)
		eachWord(pc.text, func(w string) {
			id, ok := ids[w]
			if !ok {
				id = len(ix.vocab)
				ids[w] = id
				ix.vocab = append(ix.vocab, w)
				posts = append(posts, nil)
			}
			if p := posts[id]; len(p) == 0 || p[len(p)-1] != uint32(i) {
				posts[id] = append(p, uint32(i))
			}
		})
	}
	// The words sorted, their pieces with them.
	order := make([]int, len(ix.vocab))
	for i := range order {
		order[i] = i
	}
	sort.Slice(order, func(a, b int) bool { return ix.vocab[order[a]] < ix.vocab[order[b]] })
	vocab := make([]string, len(order))
	ix.posts = make([][]uint32, len(order))
	for i, o := range order {
		vocab[i], ix.posts[i] = ix.vocab[o], posts[o]
		ix.bytes += int64(len(vocab[i]) + 4*len(posts[o]) + 40)
	}
	ix.vocab = vocab
	return ix
}

// wordList is the vocabulary as a word index has it.
func (ix *pieceIndex) wordList() []byte {
	n := 0
	for _, w := range ix.vocab {
		n += len(w) + 1
	}
	out := make([]byte, 0, n)
	for _, w := range ix.vocab {
		out = append(append(out, w...), '\n')
	}
	return out
}

// search scores the pieces that can match p's words, as Search would,
// keeping the closest limit.
func (ix *pieceIndex) search(p query, limit int) Found {
	cand := ix.candidates(p.words)
	var all []Hit
	for i := range ix.pieces {
		if cand != nil && !cand[i] {
			continue
		}
		if h, ok := p.hit(&ix.pieces[i]); ok {
			all = append(all, h)
		}
	}
	return found(all, limit, ix.first, ix.last)
}

// candidates are the pieces with every word, as typed or a typo away;
// nil when every piece is (a word the index can't answer for).
func (ix *pieceIndex) candidates(words []string) []bool {
	var cand []bool
	for _, w := range words {
		w = strings.ToLower(w)
		if !indexable(w) || !rawSearchable(w) && typos(w) > 0 {
			continue // the pieces' text says
		}
		in := make([]bool, len(ix.pieces))
		rw := newRawWord(w)
		for i, v := range ix.vocab {
			if !strings.Contains(v, w) && !rw.typoIn([]byte(v)) {
				continue
			}
			for _, pc := range ix.posts[i] {
				in[pc] = true
			}
		}
		if cand == nil {
			cand = in
			continue
		}
		for i := range cand {
			cand[i] = cand[i] && in[i]
		}
	}
	return cand
}

// found is a search's hits, the closest first, then the latest.
func found(all []Hit, limit int, first, last time.Time) Found {
	slices.SortStableFunc(all, func(a, b Hit) int {
		if a.Score != b.Score {
			return b.Score - a.Score
		}
		return b.At.Compare(a.At)
	})
	return Found{Hits: all[:min(limit, len(all))], Total: len(all), First: first, Last: last}
}

// --- on disk ---

const pieceVersion = "rush-pieces 1"

func piecePath(path string) string { return strings.TrimSuffix(indexPath(path), ".words") + ".pieces" }

func savePieces(path string, ix *pieceIndex) error {
	if indexDir() == "" {
		return nil
	}
	if err := os.MkdirAll(indexDir(), 0o700); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(indexDir(), ".pieces-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	bw := bufio.NewWriter(tmp)
	fmt.Fprintf(bw, "%s\n%s\n%d %d %d %d\n", pieceVersion, path, ix.size, ix.mod.UnixNano(), ix.first.UnixNano(), ix.last.UnixNano())
	z, _ := flate.NewWriter(bw, flate.BestSpeed)
	w := bufio.NewWriter(z)
	var buf [endian.MaxVarintLen64]byte
	num := func(n uint64) { w.Write(buf[:endian.PutUvarint(buf[:], n)]) }
	str := func(s string) { num(uint64(len(s))); w.WriteString(s) }
	num(uint64(len(ix.pieces)))
	for _, pc := range ix.pieces {
		w.WriteByte(pc.kind)
		num(uint64(pc.turn))
		num(uint64(pc.at.UnixNano()))
		str(pc.ref)
		str(pc.who)
		str(pc.label)
		str(pc.text)
		num(uint64(pc.outAt))
	}
	num(uint64(len(ix.vocab)))
	for i, v := range ix.vocab {
		str(v)
		num(uint64(len(ix.posts[i])))
		prev := uint32(0)
		for _, pc := range ix.posts[i] {
			num(uint64(pc - prev))
			prev = pc
		}
	}
	if err := w.Flush(); err != nil {
		tmp.Close()
		return err
	}
	if err := z.Close(); err != nil {
		tmp.Close()
		return err
	}
	if err := bw.Flush(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), piecePath(path))
}

var errBadPieces = errors.New("not a piece index")

// loadPieces is path's piece index from disk, if there's one for the file
// as it stands (st).
func loadPieces(path string, st os.FileInfo) (*pieceIndex, error) {
	if indexDir() == "" {
		return nil, errBadPieces
	}
	f, err := os.Open(piecePath(path))
	if err != nil {
		return nil, err
	}
	defer f.Close()
	br := bufio.NewReader(f)
	head := func() string { l, _ := br.ReadString('\n'); return strings.TrimSuffix(l, "\n") }
	if head() != pieceVersion || head() != path {
		return nil, errBadPieces
	}
	var size, mod, first, last int64
	if _, err := fmt.Sscan(head(), &size, &mod, &first, &last); err != nil {
		return nil, errBadPieces
	}
	if size != st.Size() || mod != st.ModTime().UnixNano() {
		return nil, errBadPieces
	}
	ix := &pieceIndex{size: size, mod: time.Unix(0, mod), first: unixOrZero(first), last: unixOrZero(last)}
	r := bufio.NewReader(flate.NewReader(br))
	var rerr error
	num := func() uint64 {
		n, err := endian.ReadUvarint(r)
		if err != nil && rerr == nil {
			rerr = err
		}
		return n
	}
	str := func() string {
		n := num()
		if rerr != nil || n > 1<<30 {
			rerr = cmpErr(rerr, errBadPieces)
			return ""
		}
		b := make([]byte, n)
		if _, err := io.ReadFull(r, b); err != nil && rerr == nil {
			rerr = err
		}
		return string(b)
	}
	n := num()
	if n > 1<<26 {
		return nil, errBadPieces
	}
	ix.pieces = make([]piece, 0, n)
	for range n {
		kind, err := r.ReadByte()
		if err != nil {
			return nil, err
		}
		pc := piece{kind: kind, turn: int(num()), at: unixOrZero(int64(num())), ref: str(), who: str(), label: str(), text: str(), outAt: int(num())}
		if rerr != nil {
			return nil, rerr
		}
		pc.outAt = min(pc.outAt, len(pc.text))
		ix.bytes += int64(len(pc.text) + len(pc.ref) + len(pc.label) + 64)
		ix.pieces = append(ix.pieces, pc)
	}
	nv := num()
	if nv > 1<<26 {
		return nil, errBadPieces
	}
	ix.vocab, ix.posts = make([]string, nv), make([][]uint32, nv)
	for i := range nv {
		ix.vocab[i] = str()
		np := num()
		if rerr != nil || np > n {
			return nil, cmpErr(rerr, errBadPieces)
		}
		ps := make([]uint32, np)
		prev := uint32(0)
		for j := range ps {
			prev += uint32(num())
			if prev >= uint32(n) {
				return nil, errBadPieces
			}
			ps[j] = prev
		}
		ix.posts[i] = ps
		ix.bytes += int64(len(ix.vocab[i]) + 4*len(ps) + 40)
	}
	if rerr != nil {
		return nil, rerr
	}
	return ix, nil
}

// piecesStand reports whether path's pieces on disk are of the file as it
// stands (st), reading only their head.
func piecesStand(path string, st os.FileInfo) bool {
	if indexDir() == "" {
		return false
	}
	f, err := os.Open(piecePath(path))
	if err != nil {
		return false
	}
	defer f.Close()
	br := bufio.NewReader(io.LimitReader(f, 64<<10))
	head := func() string { l, _ := br.ReadString('\n'); return strings.TrimSuffix(l, "\n") }
	if head() != pieceVersion || head() != path {
		return false
	}
	var size, mod int64
	_, err = fmt.Sscan(head(), &size, &mod)
	return err == nil && size == st.Size() && mod == st.ModTime().UnixNano()
}

func unixOrZero(n int64) time.Time {
	if n <= 0 {
		return time.Time{}
	}
	return time.Unix(0, n).UTC()
}

func cmpErr(err, or error) error {
	if err != nil {
		return err
	}
	return or
}
