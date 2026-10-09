package host

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

func TestAwayDue(t *testing.T) {
	now := time.Now()
	a := &Away{Until: now.Add(time.Hour), Every: 30 * time.Minute, Next: now}
	if over, send := awayDue(a, true, now); over || !send {
		t.Fatal("idle at a check-in is told to keep going")
	}
	if _, send := awayDue(a, false, now); send {
		t.Fatal("busy is left alone")
	}
	if _, send := awayDue(&Away{Until: now.Add(time.Hour), Next: now.Add(time.Minute)}, true, now); send {
		t.Fatal("not yet due")
	}
	if over, _ := awayDue(a, true, now.Add(time.Hour)); !over {
		t.Fatal("the time's up")
	}
	if over, send := awayDue(nil, true, now); over || send {
		t.Fatal("not away")
	}
	if over, send := awayDue(&Away{Loop: true, Next: now}, true, now.Add(1000*time.Hour)); over || !send {
		t.Fatal("a loop with no time goes on")
	}
	if _, send := awayDue(&Away{Until: now.Add(time.Hour), Next: now, Ended: now}, true, now); send {
		t.Fatal("ended, it's still checked on")
	}
}

func TestAwayDone(t *testing.T) {
	for text, want := range map[string]bool{"All done: tests pass": true, "**All done.** Shipped": true, "all done": true, "Not all done yet": false, "": false} {
		if awayDone(text) != want {
			t.Errorf("awayDone(%q) = %v", text, !want)
		}
	}
	now := time.Now()
	if n := awayNote(&Away{Until: now.Add(2*time.Hour + 14*time.Minute)}, now); !strings.Contains(n, "another 2h14m.") || !strings.Contains(n, "saved for me") {
		t.Fatal(n)
	}
	if n := awayNote(&Away{Until: now.Add(3 * time.Hour)}, now); !strings.Contains(n, "another 3h00m.") {
		t.Fatal(n)
	}
	if n := awayNote(&Away{Loop: true}, now); strings.Contains(n, "away") || !strings.Contains(n, "All done:") {
		t.Fatal(n)
	}
}

// At a check-in an idle session is told to keep going; the time's up
// ends an away but keeps it for you, and ends a loop outright.
func TestCheckAway(t *testing.T) {
	setup(t)
	now := time.Now()
	ic := &inputConn{fakeConn: fakeConn{events: make(chan event.Event, 8)}}
	s := &server{cfg: Config{ID: "aw", Kind: "claude"}, conn: ic, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	s.info.State = "idle"
	s.info.Away = &Away{Until: now.Add(time.Hour), Every: 30 * time.Minute, Next: now, Held: []Held{{Kind: "question", Text: "which?"}}}
	s.checkAway(now)
	if len(ic.got) != 1 || !strings.Contains(ic.got[0].Text, "Rush check-in") || s.info.Away.Nudges != 1 || !s.info.Away.Next.Equal(now.Add(30*time.Minute)) {
		t.Fatalf("idle at its check-in: sent %+v, away %+v", ic.got, s.info.Away)
	}
	s.info.State = "idle"
	s.checkAway(now.Add(time.Hour))
	if a := s.info.Away; a == nil || a.On() || len(a.Held) != 1 {
		t.Fatalf("the time's up: %+v", a)
	}
	if !s.info.Quiet() {
		t.Fatal("what an away left keeps its host up")
	}
	s.info.Away = &Away{Until: now.Add(time.Hour), Loop: true, Next: now.Add(time.Minute)}
	s.checkAway(now.Add(time.Hour))
	if s.info.Away != nil {
		t.Fatal("a loop's time is up and it's still on")
	}
}

// Away, what the agent asks is saved for you and refused at once, so it
// carries on; looping, it waits for you as ever.
func TestAwayHolds(t *testing.T) {
	setup(t)
	ic := &inputConn{fakeConn: fakeConn{events: make(chan event.Event, 8)}}
	s := &server{cfg: Config{ID: "ah", Kind: "claude"}, conn: ic, clients: map[*conn]struct{}{}, pending: map[string]asked{}, options: map[string][]event.Option{}}
	s.info.State = "working"
	s.info.Away = &Away{Until: time.Now().Add(time.Hour)}
	s.mu.Lock()
	s.onAgentEvent(ic, event.Question{ID: "q", Asks: []event.Ask{{Text: "Which DB?", Options: []event.Choice{{Label: "pg"}, {Label: "sqlite"}}}}})
	s.onAgentEvent(ic, event.Approval{ID: "p", Call: tool.Call{Name: "Bash", Input: tool.Input{Command: "rm -rf build"}}})
	blocked, held := s.info.State, s.info.Away.Held
	s.mu.Unlock()
	if blocked == "blocked" || len(held) != 2 || held[0].Text != "Which DB? (pg / sqlite)" || held[1].Kind != "permission" {
		t.Fatalf("away: state %s, held %+v", blocked, held)
	}
	for deadline := time.Now().Add(2 * time.Second); ; time.Sleep(10 * time.Millisecond) {
		s.mu.Lock()
		n := len(s.pending)
		s.mu.Unlock()
		if n == 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("what it asked away wasn't answered")
		}
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	s.info.Away.Loop = true
	s.onAgentEvent(ic, event.Question{ID: "q2", Asks: []event.Ask{{Text: "And?"}}})
	if s.info.State != "blocked" || len(s.info.Away.Held) != 2 {
		t.Fatalf("looping, a question was held: %s %+v", s.info.State, s.info.Away.Held)
	}
}

// Away under a profile that moves on at a limit, a limit doesn't ask:
// nobody's there to say, so it carries on at the reset.
func TestAwayLimitCarriesOn(t *testing.T) {
	setup(t)
	for _, away := range []*Away{nil, {Until: time.Now().Add(time.Hour)}} {
		s := &server{cfg: Config{ID: "al"}, clients: map[*conn]struct{}{}, limited: &event.Limited{ResetsAt: time.Now().Add(time.Hour)}}
		s.info.Away = away
		s.mu.Lock()
		s.stalled(event.TurnEnd{Reason: "error", Err: "usage limit reached"})
		if s.wake != nil {
			s.wake.Stop()
		}
		l := s.info.Limit
		s.mu.Unlock()
		if l.Ask == (away != nil) || l.Continue != (away != nil) {
			t.Fatalf("away %v: limit %+v", away != nil, l)
		}
	}
}
