package ui

import (
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// --- recall: a cheap model's notes on the open chat, at the foot of the list ---

// Recall follows the open chat from the foot of the list and notes what
// was decided, what goes against the grain and what's open. Off until
// turned on.
// Notes live in memory only, one set per agent.
type sidekick struct {
	on    bool                  // turned on with session.recall
	notes map[string]*sideNotes // by agent key
	busy  string                // the agent key being read, "" when none

	// Where it was drawn last frame, for clicks and for the tick to read
	// only while it shows: column x, row y, and the item on each row.
	x, y, w int
	items   []string
	shownOn string // the agent key it was drawn for, "" when hidden
}

type sideNotes struct {
	model                string
	at                   time.Time // the last read
	input                string    // what that read was given
	decided, grain, open []string
	err                  string
}

const (
	recallMax = 22 // rows it takes in the list, at most
	sideTurns = 12 // exchanges it reads
	sideKeep  = 5  // items a section keeps
	sideEvery = time.Minute
)

const sideSystem = "You follow along with a developer's conversation with a coding agent and keep short architecture notes. " +
	"Reply only with lines in this format, nothing else:\n" +
	"D: <a decision made in the chat>\n" +
	"G: <something going against the codebase's patterns or earlier decisions>\n" +
	"Q: <an open question>\n" +
	"Write D!:, G!: or Q!: for the few that matter most right now (a decision that changes the design, a real risk, a question blocking work). " +
	"At most 5 of each, each under 70 characters. Keep earlier notes that still hold, drop the ones that don't. Architecture-level only: no chit-chat."

// sideAsker reads for recall: a sonnet where Claude Code has one,
// else what the quick ask uses. A variable so tests don't spend a call.
var sideAsker = func(prompt string) (string, string, error) {
	return askCheap(sideSystem, prompt, "sonnet")
}

// parseSide reads the model's D:/G:/Q: lines; anything else is dropped.
func parseSide(out string) (decided, grain, open []string) {
	for _, l := range strings.Split(out, "\n") {
		l = strings.TrimLeft(strings.TrimSpace(l), "-*• ")
		mark := ""
		if len(l) > 2 && l[1] == '!' {
			mark, l = "!", l[:1]+l[2:] // important: kept as a leading !
		}
		if len(l) < 2 || l[1] != ':' {
			continue
		}
		text := strings.TrimSpace(l[2:])
		if text == "" {
			continue
		}
		add := func(to *[]string) {
			if len(*to) < sideKeep {
				*to = append(*to, mark+text)
			}
		}
		switch l[0] {
		case 'D', 'd':
			add(&decided)
		case 'G', 'g':
			add(&grain)
		case 'Q', 'q':
			add(&open)
		}
	}
	return
}

// sideInput is the latest exchanges of c's chat, as the model reads them.
func sideInput(s *convo.Session) string {
	if s == nil {
		return ""
	}
	turns := s.Turns
	if len(turns) > sideTurns {
		turns = turns[len(turns)-sideTurns:]
	}
	return strings.Join(chatTurns(&convo.Session{Turns: turns}, false), "\n")
}

// sideTick reads the focused chat again when it changed, at most once
// sideEvery, and only while the column shows.
func (m *Model) sideTick() tea.Cmd {
	c, k := m.host, m.side.shownOn
	if c == nil || k == "" || c.key != k || m.side.busy != "" {
		return nil
	}
	n := m.side.notes[k]
	if n != nil && time.Since(n.at) < sideEvery {
		return nil
	}
	in := sideInput(c.sess)
	if strings.TrimSpace(in) == "" || (n != nil && in == n.input) {
		return nil
	}
	prompt := "The latest exchanges:\n\n" + in
	if n != nil {
		var b strings.Builder
		for _, x := range [][2]any{{"D", n.decided}, {"G", n.grain}, {"Q", n.open}} {
			for _, it := range x[1].([]string) {
				k := x[0].(string)
				if t, ok := strings.CutPrefix(it, "!"); ok {
					k, it = k+"!", t
				}
				b.WriteString(k + ": " + it + "\n")
			}
		}
		if b.Len() > 0 {
			prompt = "Your notes so far:\n" + b.String() + "\n" + prompt
		}
	}
	m.side.busy = k
	return sheetDo(func() ([2]string, error) {
		model, out, err := sideAsker(prompt)
		return [2]string{model, out}, err
	}, func(m *Model, r [2]string, err error) tea.Cmd {
		m.side.busy = ""
		if m.side.notes == nil {
			m.side.notes = map[string]*sideNotes{}
		}
		nn := &sideNotes{at: time.Now(), input: in, model: r[0]}
		if old := m.side.notes[k]; old != nil {
			*nn = *old
			nn.at, nn.input, nn.err = time.Now(), in, ""
			if r[0] != "" {
				nn.model = r[0]
			}
		}
		if err != nil {
			nn.err = err.Error()
		} else {
			nn.decided, nn.grain, nn.open = parseSide(r[1])
		}
		m.side.notes[k] = nn
		return nil
	})
}

// toggleRecall is session.recall.
func (m *Model) toggleRecall() tea.Cmd {
	m.side.on = !m.side.on
	m.flash(map[bool]string{true: "recall on: notes on this chat at the foot of the list", false: "recall off"}[m.side.on], false)
	return nil
}

// sideColumn draws recall, w wide and h tall, at screen (x, y).
func (m *Model) sideColumn(w, h, x, y int) []string {
	k := m.host.key
	m.side.x, m.side.y, m.side.w, m.side.shownOn = x, y, w, k
	m.side.items = m.side.items[:0]
	var out []string
	add := func(s, item string) {
		out = append(out, " "+fit(s, w-1))
		m.side.items = append(m.side.items, item)
	}
	n := m.side.notes[k]
	head := paint(cOrange+bold, "◇ recall")
	if n != nil && n.model != "" {
		head += dim(" · " + n.model)
	}
	if n != nil {
		head += dim(" · " + dur(time.Since(n.at)))
	}
	add(head, "")
	switch {
	case m.side.busy == k && (n == nil || len(n.decided)+len(n.grain)+len(n.open) == 0):
		add(dim("reading the chat…"), "")
	case n == nil:
		add(dim("follows the chat once it says something"), "")
	case n.err != "":
		add(paint(cRed, n.err), "")
	}
	if n != nil {
		kinds := []struct {
			glyph, c string
			items    []string
		}{{"✓ ", cGreen, n.decided}, {"⚠ ", cOrange, n.grain}, {"? ", cDim, n.open}}
		for _, important := range []bool{true, false} {
			title := paint(cOrange+bold, "IMPORTANT")
			if !important {
				title = paint(cDim+bold, "RECAP")
			}
			titled := false
			for _, k := range kinds {
				for _, it := range k.items {
					text, mark := strings.CutPrefix(it, "!")
					if mark != important {
						continue
					}
					if !titled {
						add("", "")
						add(title, "")
						titled = true
					}
					if important {
						add(paint(k.c, k.glyph)+paint(cText, text), text)
					} else {
						add(faint(k.glyph)+dim(text), text)
					}
				}
			}
		}
	}
	for len(out) < h {
		add("", "")
	}
	return out[:h]
}

// clickSide puts the clicked item into the box, to say something about it.
func (m *Model) clickSide(x, y int) bool {
	s, c := &m.side, m.host
	if c == nil || s.shownOn == "" || s.shownOn != c.key || x < s.x || x >= s.x+s.w {
		return false
	}
	r := y - s.y
	if r < 0 || r >= len(s.items) || s.items[r] == "" {
		return false
	}
	pos := max(0, len(c.input)-c.back)
	c.undo.save(c.input, c.back, false)
	c.input = insert(c.input, pos, []rune("About: "+s.items[r]+" "))
	m.paneFocus = true
	return true
}
