package host

import (
	"strings"
	"testing"
	"time"
)

// Moved to another folder, a session is named afresh with it in mind.
func TestRetitleNamesItForItsNewFolder(t *testing.T) {
	setup(t)
	got := make(chan string, 1)
	titler = func(_, text string) string { got <- text; return "Rush Status Icons" }
	t.Cleanup(func() { titler = nil })
	s := &server{cfg: Config{ID: "r", Kind: "claude", Name: "Yap Usage Icons"}, clients: map[*conn]struct{}{}}
	if err := s.do(op{Op: "retitle", Text: "/src/rush"}); err != nil {
		t.Fatal(err)
	}
	if text := <-got; !strings.Contains(text, "Yap Usage Icons") || !strings.Contains(text, "/src/rush") {
		t.Fatalf("titled from %q", text)
	}
	for range 100 {
		s.mu.Lock()
		name := s.info.Name
		s.mu.Unlock()
		if name == "Rush Status Icons" {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("not renamed")
}
