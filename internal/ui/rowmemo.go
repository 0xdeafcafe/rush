package ui

// rowMemo keeps rows a frame builds from the same parts as the frame
// before: most of the pane's rows don't change frame to frame. What's
// kept is made only from the key, so a hit is never stale.
type rowMemo[K comparable] struct{ now, old map[K]string }

// turn starts a frame: the last frame's rows stay at hand, older go.
func (r *rowMemo[K]) turn() {
	r.old, r.now = r.now, r.old
	if r.now == nil {
		r.now = map[K]string{}
	}
	clear(r.now)
}

// get is the row k makes, made by mk when neither frame had it.
func (r *rowMemo[K]) get(k K, mk func() string) string {
	if r.now == nil {
		return mk()
	}
	if s, ok := r.now[k]; ok {
		return s
	}
	s, ok := r.old[k]
	if !ok {
		s = mk()
	}
	r.now[k] = s
	return s
}

// mapRowKey is a conversation row beside its minimap row.
type mapRowKey struct {
	row, mini string
	w         int
}

// washKey is a header row's text, washed at a width for a harness.
type washKey struct {
	text    string
	w, row  int
	colours *string // the wash's colours: the palette and the harness
}

var mapRows rowMemo[mapRowKey]
var washRows rowMemo[washKey]
