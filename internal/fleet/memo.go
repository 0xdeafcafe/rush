package fleet

import (
	"os"
	"time"
)

// fileMemo is what was read from a file, kept until the file's size or
// time changes: most of what the list reads every second hasn't.
type fileMemo struct {
	mod  time.Time
	size int64
	v    any
	at   uint64 // the load that looked at it (see fresh)
}

// memo returns read's result for path, calling read only when the file has
// changed (or appeared, or gone) since it last did.
func (l *Loader) memo(path string, read func() any) any {
	var mod time.Time
	size := int64(-1)
	if st, err := os.Stat(path); err == nil {
		mod, size = st.ModTime(), st.Size()
	}
	if e, ok := l.files[path]; ok && e.size == size && e.mod.Equal(mod) {
		e.at = l.checked()
		l.files[path] = e
		return e.v
	}
	v := read()
	l.files[path] = fileMemo{mod: mod, size: size, v: v, at: l.checked()}
	return v
}
