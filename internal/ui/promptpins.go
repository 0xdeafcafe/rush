package ui

import (
	"strings"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// The first thing you asked and the latest stay pinned atop a conversation
// while they're scrolled off its top, stacked, one row each: what the
// session is for and what it's on now. Clicking one goes to it.

const pinRefPrefix = "pin:"

// promptPins are the prompts pinned, kept by turn so a long one's pastes
// are folded once, not every frame.
type promptPins struct {
	first, latest         *convo.Turn
	firstText, latestText string
}

// yourPrompt is whether turn t began with something you said.
func yourPrompt(t *convo.Turn) bool { return t.From == "" && strings.TrimSpace(t.Prompt) != "" }

// pinText is a prompt as its pinned row shows it: one line, pastes folded.
func pinText(t *convo.Turn) string { return oneLine(convo.FoldPastes(t.Prompt)) }

// pinPrompts lays the pinned prompts over the top rows of out, where the
// body begins at row top and shows transcript rows [start, end): only
// those not already on screen, and only when there's a body to spare.
func (m *Model) pinPrompts(c *hostConn, s *convo.Session, out []string, top, start, end, w int) {
	first, latest := -1, -1
	for i, t := range s.Turns {
		if yourPrompt(t) {
			first = i
			break
		}
	}
	for i := len(s.Turns) - 1; i > first; i-- {
		if yourPrompt(s.Turns[i]) {
			latest = i
			break
		}
	}
	p := &c.pins
	// Pinned only once scrolled off the top: one still below is reached by
	// going on down, and a pin for it would sit over what's at the top.
	off := func(i int) bool { return i >= 0 && s.TurnRow(i) < start }
	var rows []string
	var refs []string
	add := func(i int, label, col string, turn **convo.Turn, text *string) {
		if !off(i) {
			return
		}
		tr := s.TurnRef(i)
		if *turn != s.Turns[i] {
			*turn, *text = s.Turns[i], pinText(s.Turns[i])
		}
		row := paint(col, "▌") + " " + faint(label) + " " + paint(cText, *text)
		rows = append(rows, bgChrome+strings.ReplaceAll(fit(row, w), reset, reset+bgChrome)+reset)
		refs = append(refs, pinRefPrefix+tr)
	}
	add(first, "first ", cBlue, &p.first, &p.firstText)
	add(latest, "latest", cOrange, &p.latest, &p.latestText)
	if len(rows) == 0 || end-start < len(rows)+6 {
		return // a short pane keeps its rows for the conversation
	}
	for k, r := range rows {
		out[top+k] = r
		c.rowRefs[top+k] = refs[k]
	}
}
