package community

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/state"
)

func TestBoardPersistenceValidationAndLimits(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	author := Author{Name: "Reviewer", Kind: "codex", SessionID: "session-a"}
	question, err := Ask(author, "Need help", "Question\nwith details")
	if err != nil {
		t.Fatal(err)
	}
	answer, err := Reply(question.ID, Author{Name: "Builder", Kind: "claude"}, "Try the parser\nThen verify.")
	if err != nil {
		t.Fatal(err)
	}
	if answer.Messages[0].Author != author || len(answer.Messages) != 2 || answer.Messages[1].Text != "Try the parser\nThen verify." {
		t.Fatalf("lost content or origin: %+v", answer)
	}
	if _, err := Resolve(question.ID, true); err != nil {
		t.Fatal(err)
	}
	rows, err := List()
	if err != nil || len(rows) != 1 || !rows[0].Resolved {
		t.Fatalf("persisted board %v %v", rows, err)
	}
	if _, err := Resolve(question.ID, false); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"../outside", "", strings.Repeat("g", 32)} {
		if _, err := Reply(id, author, "x"); err == nil {
			t.Fatalf("accepted ID %q", id)
		}
	}
	if _, err := Ask(author, "title", strings.Repeat("x", MaxBody+1)); err == nil {
		t.Fatal("accepted oversized body")
	}
	if _, err := Ask(author, "title", strings.Repeat("界", MaxBody)); err != nil {
		t.Fatalf("rejected a %d-character Unicode post: %v", MaxBody, err)
	}
	for _, link := range []string{"see https://x.dev", "www.example.com", "ftp://host/f"} {
		if _, err := Ask(author, "title", link); err == nil {
			t.Fatalf("accepted link %q", link)
		}
		if _, err := Ask(author, link, "x"); err == nil {
			t.Fatalf("accepted link title %q", link)
		}
	}
	for name, want := range map[string]string{"Claude Code": "@claude-code", "  ": "@agent", "Kimi · K3!": "@kimi-k3", "Can you fix the flaky login test?": "@fix-flaky-login", "You": "@you"} {
		if got := (Author{Name: name}).Username(); got != want {
			t.Fatalf("Username(%q) = %q, want %q", name, got, want)
		}
	}
	if got := (Author{Name: "Claude Code", Handle: "fix-flaky-login"}).Username(); got != "@fix-flaky-login" {
		t.Fatalf("Username with handle = %q", got)
	}
	for text, want := range map[string]bool{"thanks @claude-code!": true, "@Claude-Code": true, "@claude-codex": false, "@claude": false} {
		if got := Mentions(text, "@claude-code"); got != want {
			t.Fatalf("Mentions(%q) = %v, want %v", text, got, want)
		}
	}
	if rows, err := Mentioning("@reviewer"); err != nil || len(rows) != 0 {
		t.Fatalf("mentioning before any mention %v %v", rows, err)
	}
	flagged, err := Ask(author, "flag", "@Reviewer this one is yours")
	if err != nil {
		t.Fatal(err)
	}
	if rows, err := Mentioning("@reviewer"); err != nil || len(rows) != 1 || rows[0].ID != flagged.ID {
		t.Fatalf("mentioning %v %v", rows, err)
	}
	if _, err := Ask(author, strings.Repeat("界", MaxTitle+1), "x"); err == nil {
		t.Fatal("accepted oversized Unicode title")
	}
	p := filepath.Join(state.Dir(), "community", "board.json")
	info, err := os.Stat(p)
	if err != nil || info.Mode().Perm() != 0600 {
		t.Fatalf("board permissions %v %v", info, err)
	}
	_, err = transaction(func(b *board) error {
		for len(b.Threads[0].Messages) < MaxMessages {
			b.Threads[0].Messages = append(b.Threads[0].Messages, Message{Author: author, Text: "kept"})
		}
		return nil
	}, true)
	if err != nil {
		t.Fatal(err)
	}
	before, _ := os.ReadFile(p)
	if _, err := Reply(question.ID, author, "over capacity"); err == nil {
		t.Fatal("evicted message at capacity")
	}
	after, _ := os.ReadFile(p)
	if !bytes.Equal(before, after) {
		t.Fatal("failed mutation changed board")
	}
	if err := os.WriteFile(p, []byte("broken json"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := Ask(author, "new", "body"); err == nil {
		t.Fatal("overwrote unreadable board")
	}
	after, _ = os.ReadFile(p)
	if string(after) != "broken json" {
		t.Fatal("destroyed unreadable board")
	}
}

func TestCommunityReplyProcess(t *testing.T) {
	id := os.Getenv("RUSH_COMMUNITY_TEST_ID")
	if id == "" {
		return
	}
	for i := 0; i < 8; i++ {
		if _, err := Reply(id, Author{Name: fmt.Sprint(os.Getpid()), Kind: "test"}, fmt.Sprintf("reply-%d", i)); err != nil {
			t.Fatal(err)
		}
	}
}
func TestConcurrentProcessesDoNotLoseReplies(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	q, err := Ask(Author{Name: "You"}, "Concurrency", "Keep every reply")
	if err != nil {
		t.Fatal(err)
	}
	var cmds []*exec.Cmd
	var outputs []*bytes.Buffer
	for i := 0; i < 6; i++ {
		c := exec.Command(os.Args[0], "-test.run=^TestCommunityReplyProcess$")
		c.Env = append(os.Environ(), "RUSH_COMMUNITY_TEST_ID="+q.ID)
		buf := new(bytes.Buffer)
		c.Stdout = buf
		c.Stderr = buf
		if err := c.Start(); err != nil {
			t.Fatal(err)
		}
		cmds = append(cmds, c)
		outputs = append(outputs, buf)
	}
	for i, c := range cmds {
		if err := c.Wait(); err != nil {
			t.Fatalf("worker: %v %s", err, outputs[i])
		}
	}
	rows, err := List()
	if err != nil || len(rows) != 1 || len(rows[0].Messages) != 49 {
		t.Fatalf("lost concurrent replies: threads=%d err=%v", len(rows), err)
	}
	seen := map[string]bool{}
	for _, m := range rows[0].Messages[1:] {
		key := m.Author.Name + ":" + m.Text
		if seen[key] {
			t.Fatal("duplicate reply")
		}
		seen[key] = true
	}
}

func TestVersionChangesAfterAtomicUpdate(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	v, err := Version()
	if err != nil || v != "" {
		t.Fatalf("missing version %q %v", v, err)
	}
	q, err := Ask(Author{Name: "You"}, "Question", "body")
	if err != nil {
		t.Fatal(err)
	}
	first, _ := Version()
	if first == "" {
		t.Fatal("missing initial version")
	}
	if _, err := Reply(q.ID, Author{Name: "You"}, "reply"); err != nil {
		t.Fatal(err)
	}
	second, _ := Version()
	if second == first {
		t.Fatal("version unchanged after write")
	}
}

func TestBoardCapacityNeverEvictsThreads(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	q, err := Ask(Author{Name: "You"}, "Preserve me", "original")
	if err != nil {
		t.Fatal(err)
	}
	_, err = transaction(func(b *board) error {
		for len(b.Threads) < MaxThreads {
			copy := q
			copy.ID = fmt.Sprintf("%032x", len(b.Threads))
			b.Threads = append(b.Threads, copy)
		}
		return nil
	}, true)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Ask(Author{Name: "You"}, "Too many", "body"); err == nil {
		t.Fatal("accepted thread over limit")
	}
	rows, err := List()
	if err != nil || len(rows) != MaxThreads {
		t.Fatalf("capacity lost threads %d %v", len(rows), err)
	}
	found := false
	for _, r := range rows {
		if r.ID == q.ID && r.Messages[0].Text == "original" {
			found = true
		}
	}
	if !found {
		t.Fatal("evicted original question")
	}
	version, _ := Version()
	_, err = transaction(func(b *board) error {
		for i := range b.Threads {
			b.Threads[i].Messages[0].Text = strings.Repeat("x", 64<<10)
		}
		return nil
	}, true)
	if err == nil {
		t.Fatal("accepted oversized board")
	}
	after, _ := Version()
	if version != after {
		t.Fatal("oversized write replaced existing board")
	}
}

func TestProjectsKeepToThemselvesAndRepeatsAreRefused(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	bean := Author{Name: "Bean", SessionID: "s-bean", Project: "/src/haven"}
	if _, err := Ask(bean, "Tip", "verify sim env"); err != nil {
		t.Fatal(err)
	}
	if _, err := Ask(bean, "Tip", "verify sim env"); err == nil {
		t.Fatal("the same agent chirped the same words twice")
	}
	if !Sees("/src/haven", "/src/haven") || Sees("/src/rush", "/src/haven") || !Sees("", "/src/haven") || !Sees("/src/rush", "") {
		t.Fatal("an agent should see its own project and old chirps; the user everything")
	}
	s := state.Load()
	s.Config.FeedOpen = true
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	if !Sees("/src/rush", "/src/haven") {
		t.Fatal("#feed open should let agents cross projects")
	}
}

func TestAgentsMessagesShareAThreadAndKeepTheWholeMessage(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	a := Author{Name: "Builder", Kind: "claude", SessionID: "sa"}
	b := Author{Name: "Checker", Kind: "codex", SessionID: "sb"}
	long := "Look at the remote module's pairing https://example.com first.\n\n" + strings.Repeat("Then the rest. ", 20)
	first, err := Direct(a, b, "x1", long, long)
	if err != nil {
		t.Fatal(err)
	}
	m := first.Messages[0]
	if first.To == nil || first.To.SessionID != "sb" || m.Full != strings.TrimSpace(long) || strings.Contains(m.Text, "\n") || len([]rune(m.Text)) > MaxBody {
		t.Fatalf("first message %+v", first)
	}
	if _, err := Direct(b, a, "x2", "Done.", "Done."); err != nil {
		t.Fatal(err)
	}
	again, err := Direct(a, b, "x1", long, long) // a retried delivery
	if err != nil || len(again.Messages) != 2 || again.Messages[1].Full != "" || again.Messages[1].Whole() != "Done." {
		t.Fatalf("one thread both ways, posted once: %+v %v", again, err)
	}
	if err := Resay("x1", "Asks for a look at pairing in the remote module"); err != nil {
		t.Fatal(err)
	}
	rows, _ := List()
	if len(rows) != 1 || rows[0].Title != "Asks for a look at pairing in the remote module" || rows[0].Messages[0].Whole() != strings.TrimSpace(long) {
		t.Fatalf("summary should replace the post and keep the message: %+v", rows)
	}
}

func TestAFullBoardDropsAgentsMessagesNeverChirps(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	chirper := Author{Name: "Chirper", SessionID: "c"}
	for i := range MaxThreads - 1 {
		if _, err := Ask(chirper, fmt.Sprintf("chirp %d", i), fmt.Sprintf("chirp %d", i)); err != nil {
			t.Fatal(err)
		}
	}
	old, err := Direct(Author{Name: "A", SessionID: "a"}, Author{Name: "B", SessionID: "b"}, "e1", "hi", "hi")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Direct(Author{Name: "C", SessionID: "c2"}, Author{Name: "D", SessionID: "d"}, "e2", "yo", "yo"); err != nil {
		t.Fatal(err)
	}
	rows, _ := List()
	if len(rows) != MaxThreads || slices.ContainsFunc(rows, func(t Thread) bool { return t.ID == old.ID }) {
		t.Fatalf("the oldest agents' thread should make room: %d threads", len(rows))
	}
	if _, err := Ask(chirper, "one more", "one more"); err == nil {
		t.Fatal("a chirp still never evicts")
	}
}
