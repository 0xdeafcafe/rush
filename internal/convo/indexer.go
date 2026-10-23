package convo

import (
	"os"
	"sync"
	"testing"
	"time"
)

// The indexer makes the word indexes of the conversations it's given, one
// at a time on a goroutine of its own, so the first search finds them made:
// it never runs on the UI's, and it rests between conversations so it
// takes little from it.

// Source is a conversation to index: its file and, for one not in Claude
// Code's format, how to read it.
type Source struct {
	Path string
	Read func() *Session // nil for Claude Code's transcript
}

// indexRest is how long the indexer rests after each conversation it reads.
const indexRest = 20 * time.Millisecond

// indexerOff keeps a test's goroutine from writing into its folder after
// it's gone; a test of the indexer turns it on.
var indexerOff = testing.Testing()

var indexer struct {
	mu      sync.Mutex
	want    []Source
	running bool
}

// IndexInBackground has the conversations of srcs indexed, the first
// first, those already indexed passed over. It replaces what an earlier
// call asked for and returns at once.
func IndexInBackground(srcs []Source) {
	if indexDir() == "" || indexerOff {
		return // a test's: nowhere to keep them, or none it asked for
	}
	indexer.mu.Lock()
	defer indexer.mu.Unlock()
	indexer.want = srcs
	if !indexer.running {
		indexer.running = true
		go runIndexer()
	}
}

func runIndexer() {
	for {
		indexer.mu.Lock()
		if len(indexer.want) == 0 {
			indexer.running = false
			indexer.mu.Unlock()
			return
		}
		src := indexer.want[0]
		indexer.want = indexer.want[1:]
		indexer.mu.Unlock()
		if indexOne(src) {
			time.Sleep(indexRest)
		}
	}
}

// indexOne indexes src unless it's indexed as it stands; whether it read it.
func indexOne(src Source) bool {
	st, err := os.Stat(src.Path)
	if err != nil || st.IsDir() || indexedAsIs(src.Path, st) {
		return false
	}
	var s *Session
	if src.Read != nil {
		s = src.Read()
	} else {
		t := NewTail(src.Path)
		if _, err := t.Read(); err == nil {
			s = t.Sess
		}
	}
	if s != nil {
		indexSession(src.Path, st, s)
	}
	return true
}

// indexedAsIs is whether path's pieces are indexed as it stands; or it's
// still growing, its words read on as it does, to be indexed again once it
// settles.
func indexedAsIs(path string, st os.FileInfo) bool {
	if piecesStand(path, st) {
		return true
	}
	return indexFor(path, st) != nil && time.Since(st.ModTime()) < settled
}
