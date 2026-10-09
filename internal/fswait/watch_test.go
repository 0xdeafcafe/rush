package fswait

import (
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
)

func TestWatcher(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	dir := t.TempDir()
	f := filepath.Join(dir, "t.jsonl")
	_ = os.WriteFile(f, []byte("a\n"), 0o644)
	w := NewWatcher()
	defer w.Close()
	w.Watch([]string{dir, f})
	if !w.Changed() {
		t.Fatal("the first ask should say to look")
	}
	if w.Changed() {
		t.Fatal("nothing happened, but it said something changed")
	}
	fh, _ := os.OpenFile(f, os.O_APPEND|os.O_WRONLY, 0)
	_, _ = fh.WriteString("b\n")
	fh.Close()
	if !w.Changed() {
		t.Fatal("an append wasn't noticed")
	}
	if w.Changed() {
		t.Fatal("one append counted twice")
	}
	// A file replaced by rename, as info.json is: the folder says so.
	tmp := filepath.Join(dir, "info.json.tmp")
	_ = os.WriteFile(tmp, []byte("{}"), 0o644)
	_ = os.Rename(tmp, filepath.Join(dir, "info.json"))
	if !w.Changed() {
		t.Fatal("a rename into the folder wasn't noticed")
	}
	// A watched file removed and written again is watched afresh.
	_ = os.Remove(f)
	if !w.Changed() {
		t.Fatal("removal wasn't noticed")
	}
	_ = os.WriteFile(f, []byte("c\n"), 0o644)
	w.Watch([]string{dir, f})
	w.Changed()
	fh, _ = os.OpenFile(f, os.O_APPEND|os.O_WRONLY, 0)
	_, _ = fh.WriteString("d\n")
	fh.Close()
	if !w.Changed() {
		t.Fatal("the new file's append wasn't noticed")
	}
}

// Changes names what changed: each of many files written, and only those.
func TestWatcherChanges(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	dir := t.TempDir()
	var files []string
	for i := range 40 { // more than one drain's 32 events
		f := filepath.Join(dir, strconv.Itoa(i)+".jsonl")
		_ = os.WriteFile(f, []byte("a\n"), 0o644)
		files = append(files, f)
	}
	quiet := filepath.Join(dir, "quiet")
	_ = os.WriteFile(quiet, nil, 0o644)
	w := NewWatcher()
	defer w.Close()
	w.Watch(append([]string{quiet}, files...))
	if got, all := w.Changes(); !all || !got[quiet] {
		t.Fatalf("first ask: %v all %v, want all with every newly watched path", len(got), all)
	}
	for _, f := range files {
		fh, _ := os.OpenFile(f, os.O_APPEND|os.O_WRONLY, 0)
		_, _ = fh.WriteString("b\n")
		fh.Close()
	}
	got, all := w.Changes()
	if all || len(got) != len(files) || got[quiet] {
		t.Fatalf("after 40 appends: %d paths, all %v, quiet %v", len(got), all, got[quiet])
	}
	// Removed: told, and no longer watched.
	_ = os.Remove(files[0])
	if got, _ := w.Changes(); !got[files[0]] || w.Watching(files[0]) {
		t.Fatalf("removal: told %v, still watching %v", got[files[0]], w.Watching(files[0]))
	}
	// Watched again, it's told once as changed: it may have been written
	// between being read and being watched.
	_ = os.WriteFile(files[0], []byte("c\n"), 0o644)
	w.Watch(append([]string{quiet}, files...))
	if got, _ := w.Changes(); !got[files[0]] || len(got) != 1 {
		t.Fatalf("rewatched: %v", got)
	}
	if !w.Watching(quiet) || w.Watching(filepath.Join(dir, "missing")) {
		t.Fatal("Watching wrong")
	}
}

// A path that wasn't there is tried again only after retryMissing.
func TestWatcherRetriesMissing(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	p := filepath.Join(t.TempDir(), "later")
	w := NewWatcher()
	defer w.Close()
	w.Watch([]string{p})
	_ = os.Mkdir(p, 0o755)
	w.Watch([]string{p})
	if w.Watching(p) {
		t.Fatal("tried again at once")
	}
	w.missed[p] = w.missed[p].Add(-retryMissing)
	w.Watch([]string{p})
	if !w.Watching(p) {
		t.Fatal("not tried again after retryMissing")
	}
}
