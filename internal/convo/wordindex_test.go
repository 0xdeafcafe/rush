package convo

import (
	"os"
	"reflect"
	"testing"
	"time"
)

// The index has the words of what was said and done, not of the folder it
// ran in; it answers for typos and parts of words as search does, and
// grows with the transcript.
func TestWordIndex(t *testing.T) {
	p := writeTranscript(t,
		`{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/src/worktrees/x","message":{"role":"user","content":"rotate the Zebracorn keys at the café"},"origin":{"kind":"human"}}`)
	if f, _ := SearchFile(p, "rotate", 3); f.Total != 1 {
		t.Fatalf("search: %+v", f)
	}
	st, _ := os.Stat(p)
	ix := indexFor(p, st)
	if ix == nil {
		t.Fatal("no index after a search read it")
	}
	for _, c := range []struct {
		words []string
		want  bool
	}{
		{[]string{"zebracorn"}, true},
		{[]string{"ebraco"}, true},     // part of a word
		{[]string{"zebracron"}, true},  // a typo
		{[]string{"cafe"}, true},       // a letter past ASCII, a typo away
		{[]string{"worktrees"}, false}, // only in the folder it ran in
		{[]string{"rotate", "nowhere"}, false},
		{[]string{"keys.json"}, true}, // can't say: the session does
	} {
		if got := ix.mayHold(c.words); got != c.want {
			t.Errorf("mayHold(%v) = %v, want %v", c.words, got, c.want)
		}
	}
	// Grown: the new line's words are taken in, raw, escapes and all.
	fh, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0)
	fh.WriteString(`{"type":"user","timestamp":"2026-09-23T20:01:00Z","message":{"role":"user","content":"then\nunicornify it"}}` + "\n")
	fh.Close()
	st, _ = os.Stat(p)
	if ix = indexFor(p, st); ix == nil || !ix.mayHold([]string{"unicornify"}) || !ix.mayHold([]string{"then"}) {
		t.Fatalf("grown index lacks the new words: %q", ix.words)
	}
	if f, _ := SearchFile(p, "unicornify", 3); f.Total != 1 {
		t.Fatalf("search after growing: %+v", f)
	}
}

// An index is kept on disk and read back by a rush started afresh; one of
// a file since rewritten isn't trusted.
func TestWordIndexOnDisk(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	p := writeTranscript(t, findLines...)
	if f, _ := SearchFile(p, "cookie", 3); f.Total != 1 {
		t.Fatalf("search: %+v", f)
	}
	indexes.mu.Lock()
	delete(indexes.m, p)
	indexes.mu.Unlock()
	st, _ := os.Stat(p)
	ix := indexFor(p, st)
	if ix == nil || !ix.mayHold([]string{"cookie"}) || ix.mayHold([]string{"zebracorn"}) {
		t.Fatalf("not read back from disk: %v", ix)
	}
	if err := os.WriteFile(p, []byte(findLines[0]+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	st, _ = os.Stat(p)
	if indexFor(p, st) != nil {
		t.Fatal("trusted the index of a rewritten file")
	}
}

// The indexer indexes what it's given in the background.
func TestIndexInBackground(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	indexerOff = false
	t.Cleanup(func() { indexerOff = true })
	p := writeTranscript(t, findLines...)
	IndexInBackground([]Source{{Path: p}})
	for range 200 {
		if Indexed(p) {
			return
		}
		sleepBriefly()
	}
	t.Fatal("not indexed")
}

func sleepBriefly() { time.Sleep(10 * time.Millisecond) }

// samePieces fails t unless searching the pieces of the transcript at p
// finds just what reading it whole does, for each query.
func samePieces(t *testing.T, p string, queries []string) {
	t.Helper()
	tl := NewTail(p)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(p)
	pi := buildPieces(tl.Sess, st)
	for _, q := range queries {
		want := found(tl.Sess.Search(q), 1<<30, tl.Sess.First, tl.Sess.Last)
		got := pi.search(parseQuery(q), 1<<30)
		if got.Total != want.Total || !reflect.DeepEqual(got.Hits, want.Hits) || !got.First.Equal(want.First) || !got.Last.Equal(want.Last) {
			t.Errorf("%q: pieces found %d, the session %d\npieces  %+v\nsession %+v", q, got.Total, want.Total, got.Hits, want.Hits)
		}
	}
}

// The pieces find what the session does: what was said, steps by their
// command and output, typos, words side by side.
func TestPiecesFindAsTheSessionDoes(t *testing.T) {
	samePieces(t, writeTranscript(t, findLines...), []string{"login", "flaky login", "LOGIN flaky", "cookie", "cokie",
		"testlogin", "go test", "auth", "fail", "exit code", "nowhere", "the", "fla", "flaky nowhere"})
}

// So do a written one's pieces, read back from disk.
func TestPiecesOnDisk(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	p := writeTranscript(t, findLines...)
	tl := NewTail(p)
	tl.Read()
	st, _ := os.Stat(p)
	pi := buildPieces(tl.Sess, st)
	if err := savePieces(p, pi); err != nil {
		t.Fatal(err)
	}
	back, err := loadPieces(p, st)
	if err != nil {
		t.Fatal(err)
	}
	for _, q := range []string{"login", "cookie", "go test"} {
		if a, b := pi.search(parseQuery(q), 9), back.search(parseQuery(q), 9); !reflect.DeepEqual(a.Hits, b.Hits) {
			t.Errorf("%q: before %+v, read back %+v", q, a.Hits, b.Hits)
		}
	}
	if !piecesStand(p, st) {
		t.Error("pieces don't stand for the file they were made of")
	}
	// A search answered by the pieces alone.
	indexSession(p, st, tl.Sess)
	held.mu.Lock()
	delete(held.m, p)
	held.mu.Unlock()
	if f, _ := SearchFile(p, "cookie", 3); f.Total != 1 {
		t.Fatalf("search by pieces: %+v", f)
	}
	if h := heldFor(p, st); h == nil || h.pieces == nil {
		t.Fatal("the search didn't go by the pieces")
	}
	// is: needs the session; it's read.
	if f, _ := SearchFile(p, "testlogin is:failed", 3); f.Total != 1 {
		t.Fatalf("is:failed: %+v", f)
	}
}
