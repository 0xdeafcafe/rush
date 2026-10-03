package host

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// What a person typed is kept in the session's own folder, a line each,
// and read back oldest first; a session from before the record says it
// has none, which is not the same as nothing typed.
func TestHumanRecord(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := RecordHuman("nosuch", "hello"); err == nil {
		t.Fatal("a record was made for a session that isn't there")
	}
	if _, err := os.Stat(dir("nosuch")); err == nil {
		t.Fatal("recording made a session folder")
	}
	if err := os.MkdirAll(dir("abc12345"), 0o700); err != nil {
		t.Fatal(err)
	}
	if msgs, recorded, err := HumanMessages("abc12345"); err != nil || recorded || len(msgs) != 0 {
		t.Fatalf("before the record: %v %v %v", msgs, recorded, err)
	}
	at := time.Date(2026, 9, 23, 20, 0, 0, 0, time.UTC)
	if err := RecordHumanAt("abc12345", "first\nof two lines", at); err != nil {
		t.Fatal(err)
	}
	if err := RecordHumanAt("abc12345", "second", at.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	// A line cut short by a crash doesn't lose the rest.
	f, _ := os.OpenFile(HumanPath("abc12345"), os.O_APPEND|os.O_WRONLY, 0)
	f.WriteString(`{"at":"2026-09-23T20:0` + "\n")
	f.Close()
	if err := RecordHumanAt("abc12345", "third", at.Add(2*time.Minute)); err != nil {
		t.Fatal(err)
	}
	msgs, recorded, err := HumanMessages("abc12345")
	if err != nil || !recorded || len(msgs) != 3 {
		t.Fatalf("read back: %+v %v %v", msgs, recorded, err)
	}
	if msgs[0].Text != "first\nof two lines" || !msgs[0].At.Equal(at) || msgs[2].Text != "third" {
		t.Fatalf("read back: %+v", msgs)
	}
	if filepath.Dir(HumanPath("abc12345")) != dir("abc12345") {
		t.Fatal("the record isn't in the session's folder")
	}
}
