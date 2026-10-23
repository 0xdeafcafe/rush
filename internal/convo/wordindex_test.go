package convo

import (
	"os"
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
