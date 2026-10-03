package convo

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
)

// saidTranscript is a session you asked one thing of, which then took a
// background task's report and a message a script sent in your role.
func saidTranscript(t *testing.T) *Session {
	t.Helper()
	lines := []string{
		`{"type":"user","uuid":"u1","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"fix the login test"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:02Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Looking at the test."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./..."}}]}}`,
		`{"type":"user","timestamp":"2026-09-23T20:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"toolUseResult":{"stdout":"ok","stderr":""}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed. The pull request is open."}]}}`,
		`{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:00:13Z"}`,
		`{"type":"user","uuid":"u2","timestamp":"2026-09-23T20:30:00Z","message":{"role":"user","content":"<task-notification><status>completed</status><summary>CI finished</summary></task-notification>"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:30:02Z","message":{"id":"m3","role":"assistant","content":[{"type":"text","text":"CI is green."}]}}`,
		`{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:30:03Z"}`,
		`{"type":"user","uuid":"u3","timestamp":"2026-09-23T21:00:00Z","message":{"role":"user","content":"check the review comments"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T21:00:02Z","message":{"id":"m4","role":"assistant","content":[{"type":"text","text":"Two comments, both answered."}]}}`,
		`{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T21:00:03Z"}`,
	}
	path := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	tl := NewTail(path)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	return tl.Sess
}

// The index of a conversation lists what was said, by whom and when, and
// each message of yours with its id in the transcript.
func TestSaid(t *testing.T) {
	said := saidTranscript(t).Said()
	var got []string
	for _, m := range said {
		got = append(got, m.Who+": "+m.Text)
	}
	want := []string{
		"me: fix the login test",
		"assistant: Looking at the test.",
		"assistant: Fixed. The pull request is open.",
		"background task · completed: CI finished",
		"assistant: CI is green.",
		"me: check the review comments",
		"assistant: Two comments, both answered.",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("said:\n%s", strings.Join(got, "\n"))
	}
	if said[0].ID != "u1" || !said[0].Yours || said[3].Yours || said[5].ID != "u3" {
		t.Fatalf("yours and ids: %+v", said)
	}
	if !said[2].Answer || said[1].Answer || said[2].At.IsZero() || said[2].Turn != "t1" || said[2].Ref == said[1].Ref {
		t.Fatalf("the agent's words: %+v %+v", said[1], said[2])
	}
}

// What a person typed is what its sender wrote down, not the last message
// in the user's role: a script's comes in that role too. A session with no
// record falls back to the transcript.
func TestLastTyped(t *testing.T) {
	said := saidTranscript(t).Said()
	at := time.Date(2026, 9, 23, 20, 0, 0, 0, time.UTC)
	typed := []Typed{{At: at, Text: "fix the login test"}}
	if got := LastTyped(said, typed, true); got != 0 {
		t.Fatalf("recorded: message %d, want the one typed", got)
	}
	if got := TypedIn(said, typed); len(got) != 1 || got[0] != 0 {
		t.Fatalf("typed in: %v", got)
	}
	// With no record, the last message nothing marks as delivered.
	if got := LastTyped(said, nil, false); got != 5 {
		t.Fatalf("no record: message %d, want the last in your role", got)
	}
	// Typed, but not sent yet (queued): the one before it stands.
	typed = append(typed, Typed{At: at.Add(2 * time.Hour), Text: "still waiting in the queue"})
	if got := LastTyped(said, typed, true); got != 0 {
		t.Fatalf("one not delivered yet: message %d", got)
	}
	// The same words sent by a script before you typed them aren't yours.
	typed = []Typed{{At: at.Add(90 * time.Minute), Text: "check the review comments"}}
	if got := TypedIn(said, typed); got[0] != -1 {
		t.Fatalf("a message from before it was typed matched: %v", got)
	}
}

// A message that went with others queued behind a turn is found in the
// one message they were sent as.
func TestTypedInAQueue(t *testing.T) {
	joined := host.JoinQueue([]string{"first of them", "and the second\n\nwith two paragraphs"})
	for _, typed := range []string{"first of them", "and the second\n\nwith two paragraphs"} {
		if !carries(joined, typed) {
			t.Errorf("%q not found in the queue as sent", typed)
		}
	}
	if carries(joined, "second") || carries("please check the review", "check") {
		t.Error("part of a message isn't the message")
	}
	if !carries("<system-reminder>\nnote\n</system-reminder>\n\nhello there", "hello there") {
		t.Error("what rush tells the agent ahead of a message isn't part of it")
	}
}

// The agent's words carry their anchor on every row they're drawn on, so
// a link to them finds them; a message of yours is found by its ref.
func TestSaidRows(t *testing.T) {
	s := saidTranscript(t)
	said := s.Said()
	lines := s.Render(Options{Width: 100, Now: time.Date(2026, 9, 23, 22, 0, 0, 0, time.UTC), History: HistoryOpen})
	find := func(m Said) string {
		var rows []string
		for _, l := range lines {
			if l.Said == m.Ref || l.Ref == m.Ref {
				rows = append(rows, strings.TrimSpace(stripANSI(l.Text)))
			}
		}
		return strings.Join(rows, "\n")
	}
	for i, want := range map[int]string{0: "fix the login test", 2: "Fixed. The pull request is open.", 4: "CI is green.", 5: "check the review comments"} {
		if got := find(said[i]); !strings.Contains(got, want) {
			t.Errorf("message %d (%s): rows %q, want %q", i, said[i].Ref, got, want)
		}
	}
	if got := find(said[2]); strings.Contains(got, "Looking at the test") {
		t.Errorf("one message's rows hold another's: %q", got)
	}
}
