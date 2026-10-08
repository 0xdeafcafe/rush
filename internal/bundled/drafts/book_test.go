package drafts

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

var t0 = time.Date(2026, 9, 29, 12, 0, 0, 0, time.UTC)

func box(s string) plugin.Box { return plugin.Box{Text: s, Cursor: len([]rune(s))} }

// A box is kept until it's sent, and comes back into an empty box.
func TestBoxKeptUntilSent(t *testing.T) {
	b := NewBook(Store{}, 0)
	chip := plugin.Box{Text: "fix [Pasted text #1 +4 lines]", Cursor: 3, Pastes: map[int]string{1: "a\nb\nc\nd"}}
	b.Changed("s1", chip)
	set, ok := b.Opened("s1")
	if !ok || set.If != "" || set.Box.Cursor != 3 || set.Box.Pastes[1] == "" {
		t.Fatalf("opened: %+v %v", set, ok)
	}
	b.Sent("s1", "one", "fix a b c d", t0)
	if _, ok := b.Opened("s1"); ok {
		t.Fatal("a sent box shouldn't come back")
	}
	if len(b.S.History) != 1 || b.S.History[0].Kind != Sent || b.S.History[0].Box.Pastes[1] == "" {
		t.Fatalf("sent should be kept with its chips: %+v", b.S.History)
	}
	b.Changed("s1", box("  "))
	if _, ok := b.Opened("s1"); ok {
		t.Fatal("an emptied box shouldn't come back")
	}
}

// The stash key sets a message aside, brings it back, or swaps; sending
// brings it back by itself.
func TestStash(t *testing.T) {
	b := NewBook(Store{}, 0)
	if r := b.Stash("s1", "one", box(""), t0); r.Set != nil {
		t.Fatal("nothing to stash")
	}
	r := b.Stash("s1", "one", box("first"), t0)
	if r.Set == nil || r.Set.Box.Text != "" || r.Set.If != "first" || b.Note("s1") == "" {
		t.Fatalf("stash: %+v", r)
	}
	r = b.Stash("s1", "one", box("second"), t0)
	if r.Set == nil || r.Set.Box.Text != "first" || r.Set.If != "second" || b.S.Stashes["s1"].Box.Text != "second" {
		t.Fatalf("swap: %+v", r)
	}
	b.Changed("s1", box("third"))
	set, ok := b.Sent("s1", "one", "third", t0)
	if !ok || set.Box.Text != "second" || set.If != "" {
		t.Fatalf("after sending: %+v %v", set, ok)
	}
	// It's kept until the box is seen holding it: rush may not have.
	if b.Note("s1") == "" || !b.Changed("s1", box("second")) || b.Note("s1") != "" {
		t.Fatalf("the stash should go once it's back: %+v", b.S.Stashes)
	}
	b.Stash("s1", "one", box("fourth"), t0)
	r = b.Stash("s1", "one", box(""), t0)
	if r.Set == nil || r.Set.Box.Text != "fourth" || !b.Changed("s1", r.Set.Box) || len(b.S.Stashes) != 0 {
		t.Fatalf("back on the key: %+v", r)
	}
}

// Putting one back keeps what the box held; forgetting drops it.
func TestPutBackAndForget(t *testing.T) {
	b := NewBook(Store{}, 0)
	b.Changed("s1", box("wiped"))
	b.Cleared("s1", "one", "wiped", t0)
	b.Stash("s2", "two", box("aside"), t0)
	p := b.Pick("s1", t0.Add(10*time.Second))
	if p.Tab != 2 || len(p.Items) != 2 || p.Items[0].ID != "stash:s2" || p.Items[1].Tab != 2 {
		t.Fatalf("pick: %+v", p)
	}
	if _, err := plugin.CleanPick(p); err != nil {
		t.Fatalf("the pick should be one rush takes: %v", err)
	}
	cur := box("half typed")
	set, said, ok := b.PutBack(p.Items[1].ID, "s1", "one", &cur, t0)
	if !ok || set.Box.Text != "wiped" || set.If != "half typed" || b.S.History[0].Kind != Replaced || !strings.Contains(said, "Replaced") {
		t.Fatalf("put back: %+v, %+v, %q", set, b.S.History, said)
	}
	set, _, ok = b.PutBack("stash:s2", "s1", "one", nil, t0)
	if !ok || set.Box.Text != "aside" || set.If != "" || len(b.S.Stashes) != 0 {
		t.Fatalf("a stash from another box: %+v", set)
	}
	b.Forget(b.S.History[0].ID)
	if len(b.S.History) != 1 {
		t.Fatalf("forget: %+v", b.S.History)
	}
}

// The same text sent twice is kept once; each kind keeps its newest.
func TestKeepsNewest(t *testing.T) {
	b := NewBook(Store{}, 2)
	for i, s := range []string{"a", "b", "a", "c"} {
		b.Changed("s1", box(s))
		b.Sent("s1", "", s, t0.Add(time.Duration(i)*time.Minute))
	}
	if len(b.S.History) != 2 || b.S.History[0].Box.Text != "c" || b.S.History[1].Box.Text != "a" {
		t.Fatalf("history: %+v", b.S.History)
	}
}

// The drafts rush kept itself are taken in once: on the first run, or by a
// store from before that never did; their file is left as it was.
func TestLoadTakesInOldDrafts(t *testing.T) {
	root := t.TempDir()
	data := filepath.Join(root, "plugin-data", "drafts")
	_ = os.MkdirAll(data, 0o700)
	old := `[{"text":"older","at":"2026-09-01T00:00:00Z","kind":"draft"},{"text":"newer","at":"2026-09-02T00:00:00Z","sent":true}]`
	oldPath, path := filepath.Join(root, "drafts.json"), filepath.Join(data, "drafts.json")
	_ = os.WriteFile(oldPath, []byte(old), 0o600)
	s, took := load(path, data)
	if !took || !s.Imported || len(s.History) != 2 || s.History[0].Box.Text != "newer" || s.History[0].Kind != Sent || s.History[1].Kind != Replaced {
		t.Fatalf("history: %+v", s.History)
	}
	// A store from before, which took in one of them, gets the other.
	b, _ := jsonx.Marshal(Store{History: []Entry{{ID: "h1", Kind: Sent, Box: box("newer"), At: t0}}, Seq: 1})
	_ = os.WriteFile(path, b, 0o600)
	s, took = load(path, data)
	if !took || len(s.History) != 2 || s.History[0].Box.Text != "newer" || s.History[1].Box.Text != "older" || s.History[1].ID != "h2" {
		t.Fatalf("taken in again: %+v", s.History)
	}
	b, _ = jsonx.Marshal(s)
	_ = os.WriteFile(path, b, 0o600)
	if s, took = load(path, data); took || len(s.History) != 2 {
		t.Fatalf("taken in twice: %v %+v", took, s.History)
	}
	if b, _ := os.ReadFile(oldPath); string(b) != old {
		t.Fatal("the old file should be left as it was")
	}
}

// Nothing typed is lost: a box cleared, stashed, sent or replaced each
// lands in the history, under its own tab.
func TestEveryWayOutLandsInTheHistory(t *testing.T) {
	b := NewBook(Store{}, 0)
	b.Changed("s1", box("wiped"))
	b.Cleared("s1", "one", "wiped", t0)
	b.Stash("s1", "one", box("aside"), t0)
	b.Changed("s1", box("gone out"))
	b.Sent("s1", "one", "gone out", t0)
	b.Changed("s1", box("aside")) // the stash is back in the box
	cur := box("aside")
	sent := b.S.History[0].ID
	b.PutBack(sent, "s1", "one", &cur, t0)
	b.Stash("s2", "two", box("still aside"), t0)
	tabs := map[string]int{}
	for _, it := range b.Pick("s1", t0).Items {
		tabs[it.Text] = it.Tab
	}
	want := map[string]int{"still aside": 0, "gone out": kindTab[Sent], "wiped": kindTab[Cleared], "aside": kindTab[Replaced]}
	for text, tab := range want {
		if got, ok := tabs[text]; !ok || got != tab {
			t.Errorf("%q: tab %d, found %v; want %d", text, got, ok, tab)
		}
	}
	if p := b.Pick("s1", t0); p.Tabs[0] != "Stashed" || p.Tabs[3] != "Replaced" {
		t.Fatalf("tabs %v", p.Tabs)
	}
}

// What it's called is said everywhere it speaks.
func TestCalledDrafts(t *testing.T) {
	b := NewBook(Store{}, 0)
	b.W = WordsFor("Drafts")
	r := b.Stash("s1", "one", box("x"), t0)
	p := b.Pick("s1", t0)
	if !strings.HasPrefix(r.Said, "kept as a draft") || p.Title != "Drafts" || p.Tabs[0] != "Drafts" || !strings.HasPrefix(b.Note("s1"), "kept as a draft") {
		t.Fatalf("said %q, pick %q %v, note %q", r.Said, p.Title, p.Tabs, b.Note("s1"))
	}
	for _, s := range append([]string{r.Said, p.About, b.Note("s1")}, p.Empty...) {
		if strings.Contains(strings.ToLower(s), "stash") {
			t.Errorf("says stash: %q", s)
		}
	}
}
