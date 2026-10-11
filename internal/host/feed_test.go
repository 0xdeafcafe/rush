package host

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/community"
)

func TestAnAgentsLongMessageGoesOnTheFeedSummedUp(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	feedSummer = func(_, text string) string { return "Asks for the tests to be run" }
	t.Cleanup(func() { feedSummer = nil })
	long := "Run the tests.\n" + strings.Repeat("All of them, carefully. ", 10)
	postExchange(event.Exchange{ID: "e1", Text: long, Sender: event.Peer{SessionID: "p", Name: "Parent"}, Receiver: event.Peer{SessionID: "c", Name: "Child"}}, "claude")
	rows, err := community.List()
	if err != nil || len(rows) != 1 {
		t.Fatalf("%v %v", rows, err)
	}
	m := rows[0].Messages[0]
	if rows[0].To == nil || rows[0].To.SessionID != "c" || m.Author.SessionID != "p" || m.Text != "Asks for the tests to be run" || m.Whole() != strings.TrimSpace(long) {
		t.Fatalf("posted %+v", rows[0])
	}
}
