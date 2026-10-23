package fleet

import (
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/failure"
)

// A finished turn is one of three things: an answer, a question, or an
// error Claude Code gave up on. Questions are Needs you; these are the
// other two, which otherwise sink into Idle unnoticed.

// Halted is an agent whose last turn ended on an error: the API was out of
// reach, or a usage limit was hit. It will sit there until told to go on.
func (a *Agent) Halted() bool {
	return a.Spend.Halt != nil && !a.Live() && !a.Busy() && !a.Checking && !a.Past && !a.Done && !a.Interactive &&
		a.State != "stopped"
}

// Continues is a halted Claude Code session rush isn't hosting whose
// turn, in the last day, died on an error trying again gets past: the
// network down, a connection dropped or stalled, the API failing. rush
// tells it to continue once the API can be reached, as its own sessions
// do.
func (a *Agent) Continues(now time.Time) bool {
	if !a.Halted() || a.Rush || now.Sub(a.Spend.Halt.At) >= 24*time.Hour {
		return false
	}
	return failure.KindOf(a.Spend.Halt.Text).Retries()
}

// Retryable is a halt trying again gets past that isn't the network: the
// connection dropped or stalled, the API failing, busy or rate limiting.
func (a *Agent) Retryable() bool {
	if a.Spend.Halt == nil {
		return false
	}
	k := failure.KindOf(a.Spend.Halt.Text)
	return k == failure.Transient || k == failure.Capacity
}

// Offline is a halt the network being down caused.
func (a *Agent) Offline() bool {
	return a.Spend.Halt != nil && failure.KindOf(a.Spend.Halt.Text) == failure.Offline
}

// YourTurn is an agent that finished its turn without asking anything and
// hasn't been looked at since: it is waiting for the next thing to do,
// often just "keep going".
func (a *Agent) YourTurn(now time.Time) bool {
	return a.State == "done" && !a.Checking && !a.Busy() && !a.Past && !a.Done && !a.Interactive && !a.Seen &&
		a.Spend.Halt == nil && now.Sub(a.UpdatedAt) < 24*time.Hour
}

// HaltReason is a halt said briefly, for a row.
func (a *Agent) HaltReason() string {
	h := a.Spend.Halt
	if h == nil {
		return ""
	}
	t := strings.TrimPrefix(h.Text, "API Error: ")
	switch {
	case t == "":
		t = strings.ReplaceAll(h.Kind, "_", " ")
	case h.Kind == "rate_limit":
		// "You've hit your session limit · resets 5am (Europe/London)"
		t = strings.TrimPrefix(t, "You've hit your ")
		if i := strings.Index(t, " ("); i > 0 {
			t = t[:i]
		}
	case a.Continues(time.Now()) && a.Offline():
		t = "offline · continues when the network is back"
	case a.Continues(time.Now()):
		t += " · continues by itself"
	}
	return t
}

// ContinueText is what going on means for it: after an error Claude Code
// wants "continue"; after a finished turn, "keep going".
func (a *Agent) ContinueText() string {
	if a.Spend.Halt != nil {
		return "continue"
	}
	return "keep going"
}
