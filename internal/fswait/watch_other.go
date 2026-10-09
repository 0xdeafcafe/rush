//go:build !darwin

package fswait

// Watcher, elsewhere, can't tell: every ask is a change, of everything.
type Watcher struct{}

func NewWatcher() *Watcher                          { return &Watcher{} }
func (w *Watcher) Watch([]string)                   {}
func (w *Watcher) Watching(string) bool             { return false }
func (w *Watcher) Changed() bool                    { return true }
func (w *Watcher) Changes() (map[string]bool, bool) { return nil, true }
func (w *Watcher) Close()                           {}
