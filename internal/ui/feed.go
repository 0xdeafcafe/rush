package ui

import (
	"regexp"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// agentEvent is a line in the feed: an agent finished, asked, or failed.
type agentEvent struct {
	at                       time.Time
	key, name, colour, glyph string
	what                     string
	said                     string // its last words then, for telling news from routine
}

// noteEvent adds a line to the feed, keeping the last 30.
func (m *Model) noteEvent(a *fleet.Agent, colour, glyph, what string) {
	if what == "" {
		what = oneLine(a.Detail)
	}
	m.events = append(m.events, agentEvent{at: time.Now(), key: a.Key, name: agentHandle(a), colour: colour, glyph: glyph, what: what, said: oneLine(a.Detail)})
	if n := len(m.events); n > 30 {
		m.events = m.events[n-30:]
	}
}

// newsRe is an agent's last words saying something went badly wrong or
// it found something big: what lifts a finish out of the routine.
// ponytail: words, not judgement; a cheap model reading the turn if this misses too often.
var newsRe = regexp.MustCompile(`(?i)\b(broke|broken|lost|wiped|corrupt\w*|data loss|catastroph\w*|disaster|irrecoverabl\w*|can'?t recover|force[- ]push\w*|wtf|what the|root cause|found it|breakthrough|turns out|figured (it )?out|discover\w*|huge)\b`)

// routine is an event that's only an agent starting and stopping.
func (e agentEvent) routine() bool { return e.glyph == "✓" && !newsRe.MatchString(e.said) }

// feedLines is the feed for the list's empty foot, newest first, framed
// under its title in at most room rows (none when there's no room for the
// frame and a line), with each row's agent for a click.
func (m *Model) feedLines(w, room int) (lines, keys []string) {
	if !agentFeedShown || room < 5 || len(m.events) == 0 || w < 24 {
		return nil, nil
	}
	edge := func(s string) string { return paint(cSub+bold, s) }
	inner := w - 5 // "┃ " … " ┃", and a column clear of the divider
	title := " ◆ from your agents "
	lines = []string{"", edge("┏━") + paint(cText+bold, title) + edge(strings.Repeat("━", max(0, inner+1-cellw.String(title)))+"┓")}
	keys = []string{"", ""}
	now := time.Now()
	for i := len(m.events) - 1; i >= 0 && len(lines) < room-1; i-- {
		e := m.events[i]
		what := e.what
		if e.glyph == "✓" && !e.routine() {
			what = e.said // the news itself
		}
		age := dim(age(now.Sub(e.at)))
		name := cellw.Truncate(e.name, max(8, inner/2), "…")
		left := paint(e.colour, e.glyph) + " " + paint(cText+bold, name) + "  " + paint(cSub, what)
		if e.routine() {
			left = faint(e.glyph + " " + name + "  " + what)
		}
		row := spread(fit(left, inner-cellw.String(age)-1), age, inner)
		lines, keys = append(lines, edge("┃")+" "+row+" "+edge("┃")), append(keys, e.key)
	}
	return append(lines, edge("┗"+strings.Repeat("━", inner+2)+"┛")), append(keys, "")
}

// agentFeedShown draws the from-your-agents box; hidden for now, since
// finished agents leave Active within minutes anyway.
var agentFeedShown = false
