package claude

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// As a resumed Claude Code wrote it after a restart, a second on from the
// first message, which never reached its queue.
const heardDropped = `{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-23T10:11:57.620Z","content":"<task-notification>\n<task-id>af7034493327da98a</task-id>"}
{"type":"queue-operation","operation":"dequeue","timestamp":"2026-10-23T10:11:57.631Z"}
{"type":"user","timestamp":"2026-10-23T10:11:57.539Z","message":{"role":"user","content":[{"type":"tool_result","content":"[Tool call interrupted: the session ended]"}]}}
{"type":"assistant","timestamp":"2026-10-23T10:11:57.539Z","message":{"content":[{"type":"text","text":"No response requested."}]}}
`

// And later: the next message, with an image, did.
const heardTaken = `{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-23T10:12:03.974Z"}
{"type":"queue-operation","operation":"dequeue","timestamp":"2026-10-23T10:12:03.974Z"}
{"type":"user","timestamp":"2026-10-23T10:12:03.987Z","message":{"role":"user","content":[{"type":"text","text":"[Image #1]"},{"type":"image"},{"type":"text","text":" maybe related"}]}}
{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-23T10:20:00.000Z","content":"say \"hi\" to the \\ team"}
`

func TestHeard(t *testing.T) {
	dropped, p := filepath.Join(t.TempDir(), "a.jsonl"), filepath.Join(t.TempDir(), "b.jsonl")
	if os.WriteFile(dropped, []byte(heardDropped), 0o600) != nil || os.WriteFile(p, []byte(heardDropped+heardTaken), 0o600) != nil {
		t.Fatal("can't write the transcripts")
	}
	at := func(s string) time.Time {
		v, err := time.Parse(time.RFC3339Nano, s)
		if err != nil {
			t.Fatal(err)
		}
		return v
	}
	for _, c := range []struct {
		path, text, since string
		heard, known      bool
	}{
		{dropped, "[Image #1] sometimes sub agent just doen't show", "2026-10-23T10:11:57.115Z", false, true},
		{p, "[Image #1] maybe related", "2026-10-23T10:12:03.970Z", true, true},
		{p, "line one\nsay \"hi\" to the \\ team", "2026-10-23T10:19:59Z", true, true},
		{p, "[Image #1] maybe related", "2026-10-23T10:30:00Z", false, true}, // only what came since
		{p, "[Image #1]", "2026-10-23T10:11:57Z", false, false},              // nothing to look for
	} {
		heard, known := heardIn(c.path, c.text, at(c.since))
		if heard != c.heard || known != c.known {
			t.Errorf("heardIn(%q) = %v, %v; want %v, %v", c.text, heard, known, c.heard, c.known)
		}
	}
	if _, known := heardIn(filepath.Join(t.TempDir(), "none.jsonl"), "hi", time.Now()); known {
		t.Error("a transcript that isn't there can't say")
	}
}
