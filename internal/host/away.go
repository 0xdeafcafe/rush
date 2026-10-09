package host

import (
	"cmp"
	"fmt"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/state"
)

// You can leave a session to carry on while you're away (on a flight, in
// a meeting): until Away.Until, each time it has sat idle at a check-in
// it's told to keep going. What it asks or needs approved meanwhile is
// refused at once, with why, and saved for you (Away.Held), so it carries
// on with what it can. The host keeps it, not a view, so it goes on with
// rush closed, and stays up while it does. A loop is the same with you
// there: told to keep going, while what it asks waits for you as ever.
// Either ends once the agent says it's all done.

// Away is a session left to carry on by itself until Until (zero: a loop
// with no end).
type Away struct {
	From   time.Time     `json:"from"`
	Until  time.Time     `json:"until,omitzero"`
	Every  time.Duration `json:"every"`
	Next   time.Time     `json:"next"`             // the next check-in
	Nudges int           `json:"nudges,omitempty"` // check-ins sent so far
	// Loop is you being there: no "I'm away", and nothing held.
	Loop bool `json:"loop,omitzero"`
	// Ended is when an away's time ran out or it said it's all done: kept,
	// Held and all, until you're back to see it.
	Ended time.Time `json:"ended,omitzero"`
	Held  []Held    `json:"held,omitempty"`
}

// Held is something the agent asked, or asked to do, while you were away.
type Held struct {
	Kind string    `json:"kind"` // question, or permission
	Text string    `json:"text"`
	At   time.Time `json:"at"`
}

// On is whether a is still checking in.
func (a *Away) On() bool { return a != nil && a.Ended.IsZero() }

// Gone is whether a's owner is away, not looping: nothing is answered
// till they're back.
func (a *Away) Gone() bool { return a.On() && !a.Loop }

// DefaultAwayEvery is how often an away session is checked on.
const DefaultAwayEvery = 30 * time.Minute

// awayDue is what happens to a at now: over (Until passed), or a check-in
// to send (the session is idle and Next has come). One busy at Next is
// told as soon as it goes idle.
func awayDue(a *Away, idle bool, now time.Time) (over, send bool) {
	if !a.On() {
		return false, false
	}
	if !a.Until.IsZero() && !now.Before(a.Until) {
		return true, false
	}
	return false, idle && !now.Before(a.Next)
}

// awayNote is what a session is told at a check-in.
func awayNote(a *Away, now time.Time) string {
	done := `If it's all done, start your reply with "All done:", say what you did in a line, and stop.`
	if a.Loop {
		return "Rush check-in: keep going with what I asked. " + done
	}
	d := max(a.Until.Sub(now), time.Minute).Round(time.Minute)
	left := fmt.Sprintf("%dm", int(d.Minutes()))
	if d >= time.Hour {
		left = fmt.Sprintf("%dh%02dm", int(d.Hours()), int(d.Minutes())%60)
	}
	return fmt.Sprintf("Rush check-in: I'm away for about another %s. Nothing you ask or need approved is answered till I'm back: "+
		"it's saved for me, so carry on with whatever doesn't depend on it, make reasonable calls yourself and note them. ", left) + done
}

// awayHeld is what a question or permission asked while you're away is
// refused with.
const awayHeld = "The user is away, so nothing is answered or approved until they're back. " +
	"This is saved for them; carry on with whatever doesn't depend on it, and don't ask it again."

// awayDone is whether a turn's last words are the "All done:" awayNote
// asks for once there's nothing left to do.
func awayDone(text string) bool {
	return strings.HasPrefix(strings.ToLower(strings.TrimLeft(text, " \n*_#")), "all done")
}

// askText is a question in full, its options and all, for when you're back.
func askText(q event.Question) string {
	var lines []string
	for _, a := range q.Asks {
		var opts []string
		for _, c := range a.Options {
			opts = append(opts, c.Label)
		}
		if len(opts) > 0 {
			a.Text += " (" + strings.Join(opts, " / ") + ")"
		}
		lines = append(lines, a.Text)
	}
	return cmp.Or(strings.Join(lines, "\n"), q.Title)
}

// hold saves what the agent asks while you're away and refuses it now,
// saying why, so it carries on. Never an allow. It reports whether it
// held it. Called with mu held.
func (s *server) hold(id, kind, text string) bool {
	a := s.info.Away
	if !a.Gone() {
		return false
	}
	if !slices.ContainsFunc(a.Held, func(h Held) bool { return h.Kind == kind && h.Text == text }) {
		a.Held = append(a.Held, Held{Kind: kind, Text: text, At: time.Now()})
	}
	go func() { _ = s.do(op{Op: "deny", ID: id, Message: awayHeld}) }()
	return true
}

// awaySwitches is whether this session's profile moves on at a usage
// limit rather than waiting for the reset.
func (s *server) awaySwitches() bool {
	return state.Load().Config.ProfileFor(s.cfg.Cwd, s.cfg.Profile).Limit() != state.LimitWait
}

// watchAway sends an away session its check-ins, looking every 30s.
func (s *server) watchAway() {
	t := time.NewTicker(30 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-s.quit:
			return
		case now := <-t.C:
			s.checkAway(now)
		}
	}
}

// checkAway is a look at an away session at now: over, or due a check-in.
func (s *server) checkAway(now time.Time) {
	s.mu.Lock()
	defer s.mu.Unlock()
	// Not while a limit or a retry carries on by itself: one given up on,
	// the check-in is the retry.
	idle := s.info.State == "idle" && len(s.pending) == 0 && len(s.info.Queue) == 0 &&
		s.info.Limit == nil && (s.info.Retry == nil || s.info.Retry.GaveUp) && !s.info.Sleeping
	over, send := awayDue(s.info.Away, idle, now)
	switch {
	case over:
		s.endAway()
		s.publish()
	case send:
		a := s.info.Away
		a.Next, a.Nudges = now.Add(a.Every), a.Nudges+1
		_ = s.sendLocked(awayNote(a, now))
	}
}

// setAway starts (a non-nil a) or ends being away, keeping the machine
// from sleeping while you're gone. What it already waits on is held, as
// if asked now. Called with mu held; the caller publishes.
func (s *server) setAway(a *Away) {
	if s.awake != nil {
		_ = s.awake.Process.Kill()
		s.awake = nil
	}
	s.info.Away = a
	if !a.On() {
		return
	}
	if a.Every <= 0 {
		a.Every = DefaultAwayEvery
	}
	if a.Next.IsZero() {
		a.Next = time.Now() // idle now, it's told straight away
	}
	if a.Gone() && time.Now().Before(a.Until) {
		s.awake = keepAwake(a.Until)
	}
	for id, q := range s.pending {
		kind := "permission"
		if q.question {
			kind = "question"
		}
		s.hold(id, kind, s.info.Needs)
	}
}

// endAway ends the check-ins: a loop goes, while an away is kept for you
// to see what happened once you're back. Called with mu held.
func (s *server) endAway() {
	a := s.info.Away
	if a.Gone() {
		a.Ended = time.Now()
		s.setAway(a)
		return
	}
	s.setAway(nil)
}

// keepAwake holds off the Mac's idle sleep until until, or the host ends:
// asleep, nothing checks in. A closed lid sleeps all the same.
// shortcut: macOS only (caffeinate), systemd-inhibit would do on Linux.
func keepAwake(until time.Time) *exec.Cmd {
	cmd := exec.Command("caffeinate", "-i", "-w", strconv.Itoa(os.Getpid()), "-t", strconv.Itoa(int(time.Until(until).Seconds())+1))
	if cmd.Start() != nil {
		return nil
	}
	go func() { _ = cmd.Wait() }()
	return cmd
}
