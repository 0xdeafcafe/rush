package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
)

func humanJSON(t *testing.T, id string) []host.HumanMessage {
	t.Helper()
	out, code := run(t, "", "human", id, "--json")
	if code != 0 {
		t.Fatalf("human: exit %d: %s", code, out)
	}
	var msgs []host.HumanMessage
	if err := jsonx.Unmarshal([]byte(out), &msgs); err != nil {
		t.Fatalf("human printed %q: %v", out, err)
	}
	return msgs
}

// A message is a person's only when its sender says so: --human on start
// and on send. What a script sends without it is delivered the same and
// isn't in the record.
func TestHumanFlagRecordsWhatWasTyped(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin, "--human", "--prompt-file", writeFile(t, dir, "p.txt", "first message\n"))
	sent := filepath.Join(dir, "sent.log")
	waitFor(t, "the first message", func() bool { b, _ := os.ReadFile(sent); return strings.Contains(string(b), "first message") })

	if out, code := run(t, "from a script\n", "send", "11111111"); code != 0 {
		t.Fatalf("send: exit %d: %s", code, out)
	}
	if out, code := run(t, "typed by hand\nin two lines\n", "send", "11111111", "--human"); code != 0 || !strings.Contains(out, "sent to") {
		t.Fatalf("send --human: exit %d: %s", code, out)
	}
	waitFor(t, "both messages", func() bool {
		b, _ := os.ReadFile(sent)
		return strings.Contains(string(b), "from a script") && strings.Contains(string(b), "typed by hand")
	})
	msgs := humanJSON(t, "11111111")
	if len(msgs) != 2 || msgs[0].Text != "first message" || msgs[1].Text != "typed by hand\nin two lines" {
		t.Fatalf("the record: %+v", msgs)
	}
	if msgs[0].At.IsZero() || msgs[1].At.Before(msgs[0].At) || msgs[0].Guessed || msgs[1].UUID != "" {
		t.Fatalf("the record: %+v", msgs)
	}
	// It is in the session's folder, so it's there after the host is gone.
	run(t, "", "stop", "11111111")
	if again := humanJSON(t, "11111111"); len(again) != 2 {
		t.Fatalf("after the host stopped: %+v", again)
	}
	// In words: a line each.
	if out, code := run(t, "", "human", "11111111"); code != 0 || strings.Count(out, "\n") != 2 || !strings.Contains(out, "typed by hand") {
		t.Fatalf("human: exit %d: %q", code, out)
	}
	if _, code := run(t, "", "human", "deadbeef", "--json"); code == 0 {
		t.Fatal("no session should fail")
	}
}

// Once a typed message is in the agent's transcript it's printed with its
// id there. A session with no record at all prints what its transcript
// doesn't mark as delivered by someone else, and says it guessed.
func TestHumanReadsTheTranscript(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	t.Setenv("HOME", dir)
	startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin)
	if msgs := humanJSON(t, "11111111"); len(msgs) != 0 {
		t.Fatalf("nothing typed, no transcript: %+v", msgs)
	}
	cfg, err := host.ReadConfig("11111111")
	if err != nil {
		t.Fatal(err)
	}
	path := agent.TranscriptPath(agent.LegacyKind, cfg.Account, cfg.Cwd, sid)
	if path == "" {
		t.Fatal("no transcript path for a Claude Code session")
	}
	lines := []string{
		`{"type":"user","uuid":"u1","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"fix the login test"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:02Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Fixed."}]}}`,
		`{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:00:03Z"}`,
		`{"type":"user","uuid":"u2","timestamp":"2026-09-23T20:30:00Z","message":{"role":"user","content":"<task-notification><status>completed</status><summary>CI finished</summary></task-notification>"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:30:02Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Green."}]}}`,
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	msgs := humanJSON(t, "11111111")
	if len(msgs) != 1 || msgs[0].Text != "fix the login test" || msgs[0].UUID != "u1" || !msgs[0].Guessed {
		t.Fatalf("read off the transcript: %+v", msgs)
	}
	// With a record, only what it holds, found in the transcript by its words.
	if err := host.RecordHumanAt("11111111", "fix the login test", msgs[0].At); err != nil {
		t.Fatal(err)
	}
	msgs = humanJSON(t, "11111111")
	if len(msgs) != 1 || msgs[0].UUID != "u1" || msgs[0].Guessed {
		t.Fatalf("recorded, with its id: %+v", msgs)
	}
}
