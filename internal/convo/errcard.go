package convo

import (
	"strings"

	"github.com/0xdeafcafe/rush/internal/host"
)

// errCard is the error the session's latest turn stopped on, as a card:
// what kind of trouble it is, in words, what the agent said, and what you
// can do about it. A turn gone past keeps the one red row.
func (d *drawer) errCard(err string) {
	room := min(d.cw, capRow) - gutter - 5
	if room < 24 {
		d.add("", "", d.spine()+blanks(gutter-1)+paint(cRed, "✗ "+err), dim("your next message picks it up"))
		return
	}
	title, todo := errAdvice(err)
	rows := wrap(paint(cSub, err), min(room, 72))
	d.gap()
	d.box("", gutter, paint(cRed, "✗ ")+paint(cText+bold, title), "", rows, dim(todo), min(room, 72), cLostQ)
}

// errAdvice names an error the agent stopped on and what gets past it.
func errAdvice(err string) (title, todo string) {
	t := strings.ToLower(err)
	switch {
	case strings.Contains(t, "usage limit") || strings.Contains(t, "limit reached") || strings.Contains(t, "hit your") && strings.Contains(t, "limit"):
		return "Usage limit reached", "wait for it to reset, or move the session to another account, then send a message"
	case strings.Contains(t, "401") || strings.Contains(t, "authentication") || strings.Contains(t, "log in") || strings.Contains(t, "logged in") || strings.Contains(t, "/login") || strings.Contains(t, "oauth") || strings.Contains(t, "expired"):
		return "Signed out", "log in again, then continue from the card below: the conversation is kept"
	case host.IsOffline(t):
		return "Can't reach the API", "check your connection; once it's back, send a message to carry on"
	case host.IsRetryable(t):
		return "The API failed", "it's usually brief: send a message to try again"
	case strings.TrimSpace(t) == "during execution" || strings.Contains(t, "error_during_execution"):
		return "Claude Code couldn't run the turn", "it said no more than that; after a login switch it's often the conversation not where it looked · send a message to try again"
	case strings.Contains(t, "exited mid-turn"):
		return "The agent stopped mid-turn", "your next message starts it again where it left off"
	}
	return "The turn stopped on an error", "your next message picks it up"
}
