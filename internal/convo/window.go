package convo

import (
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/rows"
)

// A conversation is drawn a window at a time: only the turns holding the
// rows asked for are drawn, the rest are counted, as last drawn or at an
// estimate until they are, and a bounded number of turns' drawings is kept.

// keepRows is how many rows of drawn turns are kept for redrawing cheaply:
// several screens either side of any window, never the whole of a long one.
const keepRows = 20000

// rowIndex counts each turn's rows (as last drawn, else estimated) so a
// turn's first row and the turn at a row are found by halving, however long
// the session.
type rowIndex struct {
	rows.Index
	first  *Turn // the session's first turn then: older ones read in renumber all
	layout layoutKey
	hist   HistoryMode
	est    []int // each turn's rows open, estimated at layout
	h      []int // the heights sync counts the index from
	hdr    int   // rows above the first turn: "reading earlier…"
	lo, hi int   // the turns the last window drew
	// drawn are the turns with drawings kept, most recently drawn first.
	drawn *rows.Cache[*Turn, struct{}]
}

// layoutKey is what changes every turn's height at once.
type layoutKey struct {
	width               int
	wide, verb, noActiv bool
	depth               Depth
}

// sync counts the session's turns as they are: a new turn at its estimate,
// every turn again when the layout or the turns before them changed, and
// opened or folded again (from the estimates kept) when the history mode did.
func (ix *rowIndex) sync(s *Session, o Options) {
	lk := layoutKey{o.Width, o.Wide, o.Verbose, o.HideActivity, o.Depth}
	ix.hdr = 0
	if s.Partial {
		ix.hdr = 1
	}
	n, was := len(s.Turns), ix.Len()
	whole := lk != ix.layout || n < was || n > 0 && ix.first != s.Turns[0]
	reopen := whole || o.History != ix.hist
	if !reopen && n == was {
		return
	}
	from := was
	if whole {
		from = 0
	}
	ix.est = append(ix.est[:from], make([]int, n-from)...)
	for i := from; i < n; i++ {
		ix.est[i] = s.estimate(i, o)
	}
	if reopen {
		from = 0
	}
	ix.layout, ix.hist = lk, o.History
	if n > 0 {
		ix.first = s.Turns[0]
	}
	ix.h = ix.h[:0]
	for i := range from {
		ix.h = append(ix.h, ix.Height(i))
	}
	for i := from; i < n; i++ {
		h := 3 // folded: its prompt and a line of what it did
		if s.turnOpen(i, o) {
			h = ix.est[i]
		}
		ix.h = append(ix.h, h)
	}
	if reopen && ix.drawn != nil {
		ix.drawn.Each(func(t *Turn, _ struct{}, drawn int) {
			if k := s.turnIndex(t.N); k >= 0 && k < n && s.cacheFits(t, o, s.turnOpen(k, o)) {
				ix.h[k] = drawn // drawn this way already: its height is known
			}
		})
	}
	ix.Reset(ix.h)
}

// cacheFits is whether t's kept drawing is at o's width and layout, as
// open or folded as it is to be.
func (s *Session) cacheFits(t *Turn, o Options, open bool) bool {
	c, ok := s.cache[t]
	return ok && c.key.width == o.Width && c.key.wide == o.Wide && c.key.verb == o.Verbose && c.key.hideActivity == o.HideActivity && c.key.open == open
}

// estimate is how many rows turn i would take open, before it's drawn: its
// heading, a row a step, its words wrapped, and the gap after it.
func (s *Session) estimate(i int, o Options) int {
	t := s.Turns[i]
	w := max(20, min(o.Width, capProse)-gutter)
	n := 3 + strings.Count(t.Prompt, "\n") + len(t.Prompt)/w
	for _, it := range t.Items {
		switch it.Kind {
		case KText, KInterject:
			n += 2 + strings.Count(it.Text, "\n") + len(it.Text)/w
		case KThinking:
			n++
		default:
			n++
		}
	}
	return n
}

// turnOpen is whether turn i draws open: the newest always, every one
// unless the reader collapsed older ones, and those that ran, failed or
// wait on you; a fold of its own over all of that.
func (s *Session) turnOpen(i int, o Options) bool {
	t := s.Turns[i]
	open := o.History == HistoryOpen || i == len(s.Turns)-1 || t.Live || t.Err != "" || waiting(t)
	if v, ok := o.Open[s.turnRef(t)]; ok {
		open = v
	}
	return open
}

// TurnOpen is whether the turn of ref draws open with o: whole, rather
// than settled or folded.
func (s *Session) TurnOpen(ref string, o Options) bool {
	i := s.TurnOf(ref)
	return i >= 0 && s.turnOpen(i, o)
}

func (s *Session) turnRef(t *Turn) string {
	if t.ref == "" {
		t.ref = "t" + strconv.Itoa(t.N)
	}
	return t.ref
}

// RenderWindow draws the turns holding rows [from, to) of the session as
// it's counted now, whole and oldest first, into buf's storage, and only
// those: a long session costs what a screen of it does. Rows says where
// they sit; turns drawn are counted as drawn from then on. A live turn
// below them is drawn too, to count it as it grows.
func (s *Session) RenderWindow(o Options, from, to int, buf []Line) []Line {
	s.Fast = false
	o.scriptsKey = scriptsSig(o.Scripts)
	if o.Width < 20 {
		o.Width = 20
	}
	s.memoTurn()
	ix := &s.index
	ix.sync(s, o)
	s.stale, s.drew, s.deadline = false, false, time.Time{}
	if o.Budget > 0 {
		s.deadline = time.Now().Add(o.Budget)
	}
	n := len(s.Turns)
	ix.lo, ix.hi = 0, 0
	out := buf[:0]
	if n == 0 {
		if s.Partial {
			out = append(out, Line{Text: dim("  reading earlier…")})
		}
		return out
	}
	lo := ix.Find(from - ix.hdr)
	hi := ix.Find(max(from, to-1)-ix.hdr) + 1
	folds := foldsByTurn(o.Open, o.View)
	latest, latestIn := s.latestAt()
	if cap(s.parts) < hi-lo {
		s.parts = make([][]Line, hi-lo)
	}
	parts := s.parts[:hi-lo]
	// Newest first: when there's only time for some, the end of what's
	// asked for is drawn and the rest shows as it was.
	if last := n - 1; last >= hi && (s.Turns[last].Live || waiting(s.Turns[last])) {
		s.drawIndexed(last, o, folds, latest, latestIn)
	}
	for i := hi - 1; i >= lo; i-- {
		parts[i-lo] = s.drawIndexed(i, o, folds, latest, latestIn)
	}
	if lo == 0 && s.Partial {
		out = append(out, Line{Text: dim("  reading earlier…")})
	}
	for _, p := range parts {
		out = append(out, p...)
	}
	clear(parts) // don't keep turns' lines alive through this slice
	ix.lo, ix.hi = lo, hi
	s.evict()
	return out
}

// drawIndexed draws turn i and counts it as drawn.
func (s *Session) drawIndexed(i int, o Options, folds map[string]string, latest *Step, latestIn *Turn) []Line {
	t := s.Turns[i]
	var mine *Step
	if t == latestIn {
		mine = latest
	}
	recent := o.History == HistoryOpen || i == len(s.Turns)-1
	before := s.stale
	s.stale = false
	ls := s.turn(t, o, recent, folds, mine)
	stale := s.stale
	s.stale = before || stale
	ix := &s.index
	if !stale { // drawn another way, it isn't counted: it's drawn again soon
		ix.Set(i, len(ls))
	}
	if ix.drawn == nil {
		ix.drawn = rows.NewCache[*Turn, struct{}](keepRows)
		ix.drawn.OnEvict = func(t *Turn, _ struct{}) {
			delete(s.cache, t)
			for _, it := range t.Items {
				it.drawn = nil
			}
		}
	}
	ix.drawn.Put(t, struct{}{}, len(ls))
	return ls
}

// evict lets go of the drawings of the turns drawn longest ago past
// keepRows, but never one in the last window.
func (s *Session) evict() {
	ix := &s.index
	ix.drawn.Evict(func(t *Turn) bool {
		k := s.turnIndex(t.N)
		return k >= ix.lo && k < ix.hi // the rest were drawn since
	})
}

// RenderTurn is turn i alone as RenderWindow would draw it.
func (s *Session) RenderTurn(o Options, i int) []Line {
	if i < 0 || i >= len(s.Turns) {
		return nil
	}
	o.scriptsKey = scriptsSig(o.Scripts)
	if o.Width < 20 {
		o.Width = 20
	}
	s.index.sync(s, o)
	s.deadline = time.Time{} // drawn whole, whatever the last window's budget
	latest, latestIn := s.latestAt()
	return s.drawIndexed(i, o, foldsByTurn(o.Open, o.View), latest, latestIn)
}

// Rows is where the last RenderWindow's rows sit: the row they start at,
// and how many rows the whole session has, as counted then.
func (s *Session) Rows() (base, total int) {
	ix := &s.index
	if ix.lo > 0 {
		base = ix.hdr + ix.Prefix(ix.lo)
	}
	return base, ix.hdr + ix.Total()
}

// WindowTurns are the turns the last RenderWindow drew, [lo, hi).
func (s *Session) WindowTurns() (lo, hi int) { return s.index.lo, s.index.hi }

// TurnRow is the row turn i begins at, as the session is counted now.
func (s *Session) TurnRow(i int) int {
	ix := &s.index
	if i <= 0 {
		return 0
	}
	return ix.hdr + ix.Prefix(i)
}

// TurnAt is the turn at row r, the nearest when r is off either end.
func (s *Session) TurnAt(r int) int { return s.index.Find(r - s.index.hdr) }

// TurnOf is the index of the turn a ref names ("t13", "t13:s:…"), or -1.
func (s *Session) TurnOf(ref string) int {
	num, _, _ := strings.Cut(ref, ":")
	if !strings.HasPrefix(num, "t") {
		return -1
	}
	n, err := strconv.Atoi(num[1:])
	if err != nil {
		return -1
	}
	return s.turnIndex(n)
}

// TurnRef is turn i's ref, "t13".
func (s *Session) TurnRef(i int) string {
	if i < 0 || i >= len(s.Turns) {
		return ""
	}
	return s.turnRef(s.Turns[i])
}

// turnIndex is the index of turn number n, or -1.
func (s *Session) turnIndex(n int) int {
	i := sort.Search(len(s.Turns), func(i int) bool { return s.Turns[i].N >= n })
	if i < len(s.Turns) && s.Turns[i].N == n {
		return i
	}
	for i, t := range s.Turns { // numbered out of order: look at them all
		if t.N == n {
			return i
		}
	}
	return -1
}

// ActiveFrom is the oldest turn whose steps may still be writing: among the
// newest, those live or finished since since, and any with a background job
// still running.
func (s *Session) ActiveFrom(since time.Time) int {
	i := len(s.Turns)
	for ; i > 0; i-- {
		if t := s.Turns[i-1]; !t.Live && !t.End.IsZero() && t.End.Before(since) {
			break
		}
	}
	for _, j := range s.jobs {
		if st := s.byID[j.ToolUseID]; j.Running() && st != nil && st.turn != nil {
			if k := s.turnIndex(st.turn.N); k >= 0 {
				i = min(i, k)
			}
		}
	}
	return i
}

// Count is how many rows the whole session has for o, as it's counted now:
// each turn as last drawn, else at its estimate.
func (s *Session) Count(o Options) int {
	if o.Width < 20 {
		o.Width = 20
	}
	s.index.sync(s, o)
	return s.index.hdr + s.index.Total()
}

// Tail draws the session's last rows rows, as RenderWindow does.
func (s *Session) Tail(o Options, rows int) []Line {
	n := s.Count(o)
	return s.RenderWindow(o, n-rows, n, nil)
}
