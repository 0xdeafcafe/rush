package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/host"
)

// #away leaves the open session to carry on while you're gone: its host
// tells it to keep going at each check-in until the time's up (away.go
// in host), and saves what it asks for when you're back. #loop is the
// same with you there. #back, or #away off, says what happened.

// awayCommand runs #away, #loop or #back (name) with arg on session c.
func (m *Model) awayCommand(c *hostConn, name, arg string) tea.Cmd {
	if c == nil {
		m.flash("#"+name+" works in an open rush session", true)
		return nil
	}
	if c.client == nil && c.sleeping {
		return m.wakeHostThen(c, func(m *Model, next *hostConn) tea.Cmd { return m.awayCommand(next, name, arg) })
	}
	if c.client == nil {
		m.flash("#"+name+" works in rush-mode sessions · /rush moves this one over", true)
		return nil
	}
	if c.sess.Info.Proto < 11 {
		m.flash("this session's host is older than this rush · /restart it for #"+name, true)
		return nil
	}
	was, loop := c.sess.Info.Away, name == "loop"
	arg = strings.TrimSpace(arg)
	if name == "back" || arg == "off" || arg == "" && was != nil && was.Loop == loop {
		switch {
		case was == nil:
			m.flash("not away or looping", false)
		case was.Loop && name == "back":
			m.flash("looping, not away · #loop off ends it", false)
		case !was.Loop:
			m.openBack(c)
		default:
			return cmdErr(fmt.Sprintf("loop off · %d check-in%s sent", was.Nudges, plural(was.Nudges)), func() error { return c.client.SetAway(nil) })
		}
		return nil
	}
	a, err := parseAway(arg, loop, time.Now())
	if err != nil {
		m.flash(err.Error(), true)
		return nil
	}
	if was != nil {
		a.Held = was.Held // what it saved for you stays till you've seen it
	}
	text := fmt.Sprintf("away until %s · told to keep going every %s, what it asks saved for you · rush can close; leave the lid open",
		a.Until.Local().Format("15:04"), dur(a.Every))
	if loop {
		text = "looping · told to keep going every " + dur(a.Every) + " it's idle, till it's all done"
	}
	return cmdErr(text, func() error { return c.client.SetAway(a) })
}

// parseAway reads "3h", "3h every 15m" or "3h 15m"; a loop needs no time:
// "", "every 15m".
func parseAway(arg string, loop bool, now time.Time) (*host.Away, error) {
	long, every, cut := strings.Cut(arg, "every")
	f := strings.Fields(long)
	if !cut && len(f) == 2 {
		f, every = f[:1], f[1]
	}
	every = strings.TrimSpace(every)
	if len(f) > 1 || len(f) == 0 && !loop || cut && every == "" {
		if loop {
			return nil, fmt.Errorf("#loop, #loop every 15m, #loop 3h every 15m, #loop off")
		}
		return nil, fmt.Errorf("#away how long: #away 3h, #away 3h every 15m, #away off")
	}
	a := &host.Away{From: now, Every: host.DefaultAwayEvery, Loop: loop}
	if len(f) == 1 {
		d, err := time.ParseDuration(f[0])
		if err != nil || d <= 0 {
			return nil, fmt.Errorf("%s: a time like 3h or 90m", f[0])
		}
		a.Until = now.Add(d)
	}
	if every != "" {
		var err error
		if a.Every, err = time.ParseDuration(every); err != nil || a.Every < time.Minute {
			return nil, fmt.Errorf("every %s: a minute or more, like 15m", every)
		}
	}
	return a, nil
}

// backSheet is what happened while you were away: for how long, the
// check-ins, the agent's last words and what it saved for you. Enter ends
// away, l goes on in a loop, esc leaves it as it is.
type backSheet struct{ of string } // the session's key

// openBack opens the back sheet on c, once a connection.
func (m *Model) openBack(c *hostConn) {
	c.backShown = true
	m.sheet = &backSheet{of: c.key}
}

// backFromAway opens the back sheet on an away that ended while you were
// gone, the first time you're in its session since.
func (m *Model) backFromAway(c *hostConn) {
	if a := c.sess.Info.Away; a != nil && !a.On() && !c.backShown && m.sheet == nil && m.grid.conns[c.key] != c {
		m.openBack(c)
	}
}

func (s *backSheet) body(m *Model, w, h int) []string {
	c := m.host
	if c == nil || c.key != s.of || c.sess.Info.Away == nil {
		return []string{sheetTitle("Back", "not away any more", w), "", keysFit(w, "esc", "close")}
	}
	a, now := c.sess.Info.Away, time.Now()
	end, still := a.Ended, ""
	if a.On() {
		end, still = now, " · still away till "+a.Until.Local().Format("15:04")
	}
	out := []string{sheetTitle("Back", fmt.Sprintf("away %s · %d check-in%s%s", dur(end.Sub(a.From)), a.Nudges, plural(a.Nudges), still), w), ""}
	if d := oneLine(c.sess.Info.Detail); d != "" {
		out = append(out, dim("its last words  ")+fit(paint(cText, d), w-16), "")
	}
	if len(a.Held) == 0 {
		out = append(out, dim("nothing saved for you"))
	} else {
		out = append(out, dim(fmt.Sprintf("saved for you · %d, put in the box as one reply when you end it", len(a.Held))))
		from, to := window(len(a.Held), 0, max(1, h-9))
		for _, it := range a.Held[from:to] {
			mark := paint(cYellow, "? ")
			if it.Kind == "permission" {
				mark = paint(cOrange, "! ")
			}
			out = append(out, "  "+mark+faint(it.At.Local().Format("15:04")+"  ")+fit(paint(cText, oneLine(it.Text)), w-12))
		}
		if to < len(a.Held) {
			out = append(out, faint(fmt.Sprintf("    and %d more", len(a.Held)-to)))
		}
	}
	return append(out, "", keysFit(w, "enter", "end away", "l", "end, then loop", "esc", "keep it"))
}

func (s *backSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	switch k {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "enter", "l":
		m.sheet = nil
		c := m.host
		if c == nil || c.key != s.of || c.sess.Info.Away == nil {
			return nil
		}
		a := c.sess.Info.Away
		if r := heldReply(a.Held); r != "" {
			if len(c.input) > 0 {
				r = "\n\n" + r
			}
			c.input, c.back = append(c.input, []rune(r)...), 0
			m.paneFocus = true
		}
		var next *host.Away
		if k == "l" {
			next = &host.Away{From: time.Now(), Every: a.Every, Loop: true}
		}
		return m.setAway(c, next, a.Nudges)
	}
	return nil
}

// setAway ends an away for good, or turns it into a loop (next), waking
// a host that has gone to sleep since.
func (m *Model) setAway(c *hostConn, next *host.Away, nudges int) tea.Cmd {
	if c.client == nil && c.sleeping {
		return m.wakeHostThen(c, func(m *Model, c *hostConn) tea.Cmd { return m.setAway(c, next, nudges) })
	}
	if c.client == nil {
		return nil
	}
	text := fmt.Sprintf("welcome back · %d check-in%s sent", nudges, plural(nudges))
	if next != nil {
		text += " · looping every " + dur(next.Every)
	}
	cl := c.client
	return cmdErr(text, func() error { return cl.SetAway(next) })
}

// heldReply is one message answering everything the agent saved while
// you were away, for you to fill in.
func heldReply(held []host.Held) string {
	if len(held) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString("On what you saved while I was away:")
	for _, h := range held {
		if h.Kind == "permission" {
			b.WriteString("\n- you asked to " + oneLine(h.Text) + " → ")
			continue
		}
		b.WriteString("\n- " + oneLine(h.Text) + " → ")
	}
	return b.String()
}

// awayChip is the header's say on an away or looping session: time left,
// and when wide a bar of it and when it's next checked on; once an away's
// over, what it saved for you.
func awayChip(a *host.Away, now time.Time, wide bool) string {
	if !a.On() {
		return paint(cBlue, "✈ back") + dim(fmt.Sprintf(" · %d saved for you · #back", len(a.Held)))
	}
	left, chip := a.Until.Sub(now), "✈ away"
	if a.Loop {
		chip = "⟳ loop"
	}
	if !a.Until.IsZero() {
		chip += " " + dur(left)
	}
	chip = paint(cBlue, chip)
	if !wide {
		return chip
	}
	if !a.Until.IsZero() {
		chip += " " + meter(left, a.Until.Sub(a.From), 8)
	}
	next := "next check-in " + dur(a.Next.Sub(now))
	if !now.Before(a.Next) {
		next = "checks in once idle"
	}
	held := ""
	if n := len(a.Held); n > 0 {
		held = fmt.Sprintf(" · %d saved", n)
	}
	return chip + dim(fmt.Sprintf(" · %s · %d sent%s", next, a.Nudges, held))
}

// meter is a bar w cells wide, part of whole filled.
func meter(part, whole time.Duration, w int) string {
	n := 0
	if whole > 0 {
		n = min(w, max(0, int(float64(w)*float64(part)/float64(whole)+0.5)))
	}
	return paint(cBlue, strings.Repeat("━", n)) + faint(strings.Repeat("─", w-n))
}
