package convo

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeTranscript(t *testing.T, lines ...string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(p, []byte(strings.Join(lines, "\n")+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

var findLines = []string{
	`{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"fix the Flaky login test"},"origin":{"kind":"human"}}`,
	`{"type":"assistant","timestamp":"2026-09-23T20:00:02Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./auth"}}]}}`,
	`{"type":"user","timestamp":"2026-09-23T20:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Exit code 1\nFAIL TestLogin","is_error":true}]},"toolUseResult":{"stdout":"FAIL TestLogin","stderr":""}}`,
	`{"type":"assistant","timestamp":"2026-09-23T20:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"The session cookie expired too early."}]}}`,
}

func TestSearchFile(t *testing.T) {
	p := writeTranscript(t, findLines...)
	f, err := SearchFile(p, "LOGIN flaky", 10)
	if err != nil || f.Total != 1 || f.Hits[0].Who != "you" || f.Hits[0].Turn != 1 || f.Hits[0].At.IsZero() {
		t.Fatalf("got %+v %v", f, err)
	}
	if f.First.IsZero() || f.Last.Before(f.First) {
		t.Fatalf("when it started and last moved: %v %v", f.First, f.Last)
	}
	f, _ = SearchFile(p, "cookie", 10)
	if f.Total != 1 || f.Hits[0].Who != WhoAgent {
		t.Fatalf("cookie: %+v", f.Hits)
	}
	if f, _ = SearchFile(p, "testlogin is:failed", 10); f.Total != 1 {
		t.Fatalf("failed step not found: %d", f.Total)
	}
	if f, _ = SearchFile(p, "login nowhere", 10); f.Total != 0 {
		t.Fatalf("a word the file lacks still matched")
	}
	if f, _ = SearchFile(p, "is:failed", 10); f.Hits != nil {
		t.Fatalf("a query without words reads every file: %+v", f.Hits)
	}
	// A typo still finds it; a short word must be as typed.
	if f, _ = SearchFile(p, "cokie", 10); f.Total != 1 {
		t.Fatalf("a typo found nothing: %+v", f)
	}
	if f, _ = SearchFile(p, "fla", 10); f.Total != 1 {
		t.Fatalf("part of a word found nothing: %+v", f)
	}
}

// The closest match scores highest: a whole word, then the start of one,
// then inside one, then a typo; words side by side as typed count extra.
func TestMatchScores(t *testing.T) {
	q := parseQuery("login")
	score := func(text string) int { s, _ := q.score(text); return s }
	whole, start, inside, typo := score("the login page"), score("the loginpage"), score("relogin now"), score("the lgoin page")
	if !(whole > start && start > inside && inside > typo && typo > 0) {
		t.Fatalf("whole %d start %d inside %d typo %d", whole, start, inside, typo)
	}
	if score("nothing like it") != 0 {
		t.Fatal("matched what isn't there")
	}
	two := parseQuery("flaky login")
	side, _ := two.score("a flaky login test")
	apart, _ := two.score("login is flaky")
	if side <= apart {
		t.Fatalf("side by side %d, apart %d", side, apart)
	}
	if i, j := MatchSpan("fix the lgoin bug", "login"); i != 8 || j != 13 {
		t.Fatalf("typo span %d %d", i, j)
	}
}

func TestHoldsAllAcrossChunks(t *testing.T) {
	// The word straddles the 64 KiB read buffer's edge.
	s := strings.Repeat("x", 64<<10-3) + "NeedleY"
	ok, err := holdsAll(strings.NewReader(s), []string{"needley"})
	if !ok || err != nil {
		t.Fatalf("missed a word across chunks: %v %v", ok, err)
	}
	ok, _ = holdsAll(strings.NewReader(s), []string{"needley", "absent"})
	if ok {
		t.Fatal("found a word that isn't there")
	}
	// Words raw JSON can't be searched for are left to the full search.
	ok, _ = holdsAll(strings.NewReader("plain"), []string{`"quoted"`, "über"})
	if !ok {
		t.Fatal("unsearchable words should count as present")
	}
}

// lowerASCII lowers every ASCII capital and nothing else, eight bytes at a
// time or one.
func TestLowerASCII(t *testing.T) {
	var all []byte
	for c := range 256 {
		all = append(all, byte(c))
	}
	for _, in := range []string{string(all), "Hello WORLD @[`{ ÄÖÜ zZ", "AZ", "@[", "QWERTYUIOPASDFGHJKL"} {
		for off := range 8 {
			b := []byte(in[min(off, len(in)):])
			want := []byte(string(b))
			for i, c := range want {
				if 'A' <= c && c <= 'Z' {
					want[i] = c + 32
				}
			}
			lowerASCII(b)
			if !bytes.Equal(b, want) {
				t.Fatalf("lowerASCII(%q) = %q, want %q", in[off:], b, want)
			}
		}
	}
}

// A conversation a search read is held for the next: one that grows is
// read on, one rewritten is read again, and nothing stale is found.
func TestSearchHoldsWhatItRead(t *testing.T) {
	p := writeTranscript(t, findLines[0])
	if f, _ := SearchFile(p, "flaky", 3); f.Total != 1 {
		t.Fatalf("first search: %+v", f)
	}
	st, _ := os.Stat(p)
	if heldFor(p, st) == nil {
		t.Fatal("not held after a search read it")
	}
	more := `{"type":"user","timestamp":"2026-09-23T20:05:00Z","cwd":"/work","message":{"role":"user","content":"the flaky one again"},"origin":{"kind":"human"}}`
	fh, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0)
	fh.WriteString(more + "\n")
	fh.Close()
	if f, _ := SearchFile(p, "flaky", 3); f.Total != 2 {
		t.Fatalf("grown: %d found, want 2", f.Total)
	}
	if err := os.WriteFile(p, []byte(`{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"nothing here"},"origin":{"kind":"human"}}`+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if f, _ := SearchFile(p, "flaky", 3); f.Total != 0 {
		t.Fatalf("rewritten: still found %d", f.Total)
	}
}
