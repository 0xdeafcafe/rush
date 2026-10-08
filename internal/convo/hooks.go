package convo

import (
	"encoding/json/jsontext"
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Hook is what a hook handed the agent as a session began: its event
// (SessionStart, SubagentStart) and the text it added.
type Hook struct{ Event, Text string }

// hookStarts are the events whose output the conversation shows.
var hookStarts = map[string]bool{"SessionStart": true, "SubagentStart": true}

// hookNotes reads the texts a start hook added from a transcript's
// attachment: what it printed, or the context it returned.
func hookNotes(kind, event string, content jsontext.Value) []Hook {
	if !hookStarts[event] {
		return nil
	}
	var texts []string
	var one string
	switch {
	case jsonx.Unmarshal(content, &one) == nil:
		texts = []string{one}
	case jsonx.Unmarshal(content, &texts) != nil:
		return nil
	}
	var out []Hook
	for _, t := range texts {
		if t = strings.TrimSpace(t); t != "" && !strings.HasPrefix(t, "{") {
			out = append(out, Hook{Event: event, Text: t})
		}
	}
	return out
}

// addHooks keeps hooks the session hasn't got yet, and has the first turn,
// which draws them, drawn again.
func (s *Session) addHooks(hs []Hook) {
	for _, h := range hs {
		if !slices.Contains(s.Hooks, h) {
			s.Hooks = append(s.Hooks, h)
			if len(s.Turns) > 0 {
				s.Turns[0].touch()
			}
		}
	}
}

// hooks draws what start hooks said, at the top of the first turn: one dim
// line each, its event and first line, and under it the whole text when
// it's opened by a click. Never your message, and never the blob unasked.
func (d *drawer) hooks() {
	if len(d.s.Hooks) == 0 || len(d.s.Turns) == 0 || d.s.Turns[0] != d.t {
		return
	}
	for i, h := range d.s.Hooks {
		ref := d.ref + ":hook:" + strconv.Itoa(i)
		head := dim("◇ " + h.Event + " · " + firstLine(h.Text))
		if !d.o.Open[ref] {
			d.add(ref, "", " "+blanks(gutter-3)+faint("▸ ")+head, "")
			continue
		}
		d.add(ref, "", " "+blanks(gutter-3)+faint("▾ ")+head, "")
		for _, l := range strings.Split(h.Text, "\n") {
			for _, r := range wrap(dim(strings.TrimRight(l, " ")), max(10, min(d.cw-gutter-3, capProse))) {
				d.add(ref, "", " "+blanks(gutter+1)+r, "")
			}
		}
	}
}
