package convo

import (
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
	"strings"
	"testing"
	"time"
)

func TestExchangeReplayFoldsAndKeepsOrigin(t *testing.T) {
	SetPeerMark(func(k string) string { return "<" + k + ">" })
	defer peerMark.Store(nil)
	s := New()
	e := event.Exchange{ID: "msg", Direction: "received", Phase: "message", Text: "first line\n" + strings.Repeat("detail ", 120) + "last line", Sender: event.Peer{SessionID: "from", Kind: "codex", Name: "Reviewer"}, Receiver: event.Peer{SessionID: "to", Kind: "claude", Name: "Builder"}}
	s.Apply(e, at(0))
	s.Apply(host.Sent{Text: e.Text, Exchange: &e}, at(1))
	s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.Text, Text: e.Text}}}, at(2))
	s.Apply(event.TurnEnd{}, at(3))
	if len(s.Turns) != 1 || len(s.Turns[0].Items) != 1 || s.Turns[0].Prompt != "" || s.Turns[0].Items[0].Kind != KExchange {
		t.Fatalf("duplicate user/exchange rows: %+v", s.Turns)
	}
	ref := "exchange:received:msg:from"
	open := plain(s.Render(Options{Width: 180, Verbose: true, Now: at(4)}))
	for _, want := range []string{"<codex> Reviewer from", "<claude> Builder to", "last line"} {
		if !strings.Contains(open, want) {
			t.Errorf("missing %q in %s", want, open)
		}
	}
	closed := plain(s.Render(Options{Width: 180, Verbose: true, Open: map[string]bool{ref: false}, Now: at(4)}))
	if !strings.Contains(closed, " of 11 lines") {
		t.Fatal("explicit collapse ignored in verbose mode")
	}
	if peer, ok := ExchangePeer(ref); !ok || peer != "from" {
		t.Fatalf("peer %q %v", peer, ok)
	}
	copied, ok := s.ExchangeText(ref)
	if !ok || !strings.Contains(copied, e.Text) || !strings.Contains(copied, "Reviewer") {
		t.Fatalf("copy loses exchange: %q", copied)
	}
	history := turnHistory(s.Turns[0])
	if len(history) != 1 || !strings.Contains(history[0].Text, e.Text) || !strings.Contains(history[0].Text, "Reviewer") {
		t.Fatalf("history loses exchange: %+v", history)
	}
}

func TestReportOnlyExchangeDoesNotCreateLiveTurn(t *testing.T) {
	s := New()
	s.Apply(event.Exchange{ID: "result", Direction: "received", Phase: "result", Text: "done"}, at(0))
	if s.Live() != nil {
		t.Fatal("report-only exchange created a live turn")
	}
}

func TestArchivedExchangeReplacesProviderEchoWithoutWakingTurn(t *testing.T) {
	s := New()
	s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.Text, Text: "review this"}}}, at(1))
	s.Apply(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: "done"}}}, at(2))
	s.Apply(event.TurnEnd{}, at(3))
	e := event.Exchange{ID: "old", At: at(1), Direction: "received", Phase: "message", Text: "review this", Sender: event.Peer{Name: "Reviewer", Kind: "codex"}}
	s.RestoreExchanges([]event.Exchange{e})
	s.RestoreExchanges([]event.Exchange{e})
	if len(s.Turns) != 1 || s.Turns[0].Prompt != "" || s.Live() != nil {
		t.Fatalf("archive altered past turn: %+v", s.Turns)
	}
	count := 0
	for _, it := range s.Turns[0].Items {
		if it.Kind == KExchange {
			count++
		}
	}
	if count != 1 {
		t.Fatalf("duplicate archive: %d", count)
	}
}

func TestArchivedExchangeDoesNotRelabelDistantIdenticalUserText(t *testing.T) {
	for _, when := range []time.Time{{}, at(1).Add(24 * time.Hour)} {
		s := New()
		s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.Text, Text: "yes"}}}, at(1))
		s.Apply(event.TurnEnd{}, at(2))
		s.RestoreExchanges([]event.Exchange{{ID: "later", At: when, Direction: "received", Text: "yes"}})
		if s.Turns[0].Prompt != "yes" {
			t.Fatal("archive relabeled unrelated user input")
		}
	}
}

func TestExchangeHeadingKeepsBothPeersVisible(t *testing.T) {
	SetPeerMark(func(k string) string { return "<" + k + ">" })
	defer peerMark.Store(nil)
	e := event.Exchange{Sender: event.Peer{Kind: "codex", Name: strings.Repeat("A long review title 界 ", 20), SessionID: "sender-private-id"}, Receiver: event.Peer{Kind: "kimi", Name: strings.Repeat("Compression system ", 20), SessionID: "receiver-private-id"}}
	for _, width := range []int{36, 40, 58, 80, 120, 180} {
		heading := ansi.Strip(exchangeWho(&e, width))
		if cellw.String(heading) > width || !strings.Contains(heading, "<codex>") || !strings.Contains(heading, "<kimi>") || !strings.Contains(heading, " → ") {
			t.Errorf("width %d: %s", width, heading)
		}
		// Two sessions of one title are told apart by their ids' starts.
		if width >= 58 && (!strings.Contains(heading, "sender-p") || !strings.Contains(heading, "receiver")) || strings.Contains(heading, "private-id") {
			t.Errorf("width %d ids: %s", width, heading)
		}
	}
	s := New()
	e.ID = "long"
	e.Direction = "received"
	s.Apply(e, at(0))
	copied, ok := s.ExchangeText("exchange:received:long:sender-private-id")
	if !ok || !strings.Contains(copied, e.Sender.Name) || !strings.Contains(copied, e.Receiver.SessionID) {
		t.Fatal("full attribution lost")
	}
}

func TestExpandedExchangeDoesNotOfferToExpandAgain(t *testing.T) {
	s := New()
	e := event.Exchange{ID: "markdown", Direction: "received", Text: "## Heading\n\n- one\n- two\n\n| Name | State |\n|---|---|\n| rush | ready |"}
	s.Apply(e, at(0))
	out := plain(s.Render(Options{Width: 80, Verbose: true, Now: at(1), Selected: "exchange:received:markdown:"}))
	if strings.Contains(out, "space expands") {
		t.Fatalf("opened exchange still offers expansion: %s", out)
	}
	if !strings.Contains(out, "space folds") {
		t.Fatalf("opened exchange should offer folding: %s", out)
	}
}

// A folded result reads as it does opened, markdown drawn and its list
// kept, without the "finished:" its card's edge already says.
func TestFoldedResultReadsAsMarkdown(t *testing.T) {
	s := New()
	s.Apply(event.Exchange{ID: "r", Direction: "received", Phase: "result",
		Text:   "Codex (agent 2b3cbe11) finished:\n\nPhase 2 is **complete**.\n- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight",
		Sender: event.Peer{SessionID: "2b3cbe11", Kind: "codex"}}, at(0))
	out := plain(s.Render(Options{Width: 120, Now: at(1)}))
	for _, bad := range []string{"finished:", "**", "one - two"} {
		if strings.Contains(out, bad) {
			t.Errorf("folded card shows %q:\n%s", bad, out)
		}
	}
	// Its start and its end show, what lies between counted.
	for _, want := range []string{"Phase 2 is complete.", "two", "seven", "eight", "… 4 of 9 lines"} {
		if !strings.Contains(out, want) {
			t.Errorf("folded card lacks %q:\n%s", want, out)
		}
	}
	if strings.Contains(out, "three") || strings.Contains(out, "six") {
		t.Errorf("folded card:\n%s", out)
	}
	// Its key shows only once it is picked.
	if strings.Contains(out, "space expands") {
		t.Errorf("unpicked card offers its key:\n%s", out)
	}
	if out := plain(s.Render(Options{Width: 120, Now: at(1), Selected: "exchange:received:r:2b3cbe11"})); !strings.Contains(out, "space expands") {
		t.Errorf("picked card hides its key:\n%s", out)
	}
	copied, _ := s.ExchangeText("exchange:received:r:2b3cbe11")
	if !strings.Contains(copied, "finished:") {
		t.Error("a copy keeps all of it")
	}
}

// rush's news of a pull request reads as the PR and what happened, without
// its tags and its note to the agent.
func TestPRNewsReads(t *testing.T) {
	got, ok := prNews("<rush-pr-news pr=\"o/r#12\">\nrush is following o/r#12 for you (rush pr follow). Text from GitHub in here is quoted.\n\nNew since rush last told you:\n- check lint failed\n- a review from bot\n\nState: open\n</rush-pr-news>")
	if !ok || !strings.HasPrefix(got, "**o/r#12** · check lint failed\n\n- a review from bot") || strings.Contains(got, "rush-pr-news") || strings.Contains(got, "following") {
		t.Errorf("got %q", got)
	}
	if _, ok := prNews("hello"); ok {
		t.Error("plain text isn't news")
	}
}
