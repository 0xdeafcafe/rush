package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// signedOut is whether a rush session sits stopped because its sign-in
// ran out, and no continue has gone yet.
func signedOut(c *hostConn) bool {
	i := c.sess.Info
	return c.client != nil && strings.HasPrefix(i.Error, host.AuthStopped) && i.Error != c.signinSent &&
		i.State != "working" && i.State != "blocked" && i.State != "starting"
}

// signinChoice is an account a signed-out session can carry on as: the
// login it was on (once you've logged in again), another of its
// harness's, or another harness's, which takes the conversation over.
type signinChoice struct {
	r    acctRow
	o    startOver
	here bool // the account it's on now
}

// signinChoices are every account the session could carry on as, the one
// it's on first.
func (m *Model) signinChoices(c *hostConn) []signinChoice {
	from := m.sessionStart(c)
	rows := m.accountRows()
	var here, rest []signinChoice
	for i, r := range rows {
		if r.head && i+1 < len(rows) && rows[i+1].kind == r.kind && !rows[i+1].head {
			continue // its accounts are listed: the agent's own line says nothing more
		}
		same := string(r.kind) == from.kind
		if !same && !agent.Supports(r.kind, agent.FeatureHandoffIn) {
			continue
		}
		o := startOver{kind: string(r.kind)}
		if same {
			o = from
		}
		o.account = ""
		if !r.head {
			o.account = r.name()
		}
		ch := signinChoice{r: r, o: o, here: same && (r.head || r.current)}
		if ch.here {
			here = append(here, ch)
		} else {
			rest = append(rest, ch)
		}
	}
	return append(here, rest...)
}

// signinUsage is how full an account's tightest window is, coloured as it
// fills; "" with no reading.
func signinUsage(r acctRow, now time.Time) string {
	w, ok := r.q.Since(now).Tightest("")
	if !ok {
		return ""
	}
	s := fmt.Sprintf("%.0f%% · %s", w.Percent, quotaWindowName(w))
	switch {
	case w.Percent >= 95:
		return paint(cRed, s)
	case w.Percent >= 80:
		return paint(cYellow, s)
	}
	return paint(cSub, s)
}

// signedOutRows is the card a signed-out session waits on: the accounts
// it could carry on as, each with its provider and how much room it has,
// and the one picked carries it on.
func (m *Model) signedOutRows(a *fleet.Agent, c *hostConn, w int) []string {
	card, bar := qCard, paint(cRed, "▍")
	if c.inModal {
		card, bar = "", " "
	}
	var out []string
	cl := func(txt string) {
		if c.inModal {
			out = append(out, fit(txt, w))
			return
		}
		out = append(out, onBg(card, txt, w))
	}
	who := ""
	if a != nil && a.Acct.Name != "" {
		who = paint(cSub, a.Acct.Name) + "  "
	}
	cl(spread(bar+" "+paint(cRed, "✗ ")+paint(cText+bold, "Signed out"), who, w))
	for _, row := range wrap(paint(cSub, strings.TrimPrefix(c.sess.Info.Error, host.AuthStopped)), max(8, w-6)) {
		cl(bar + "     " + row)
	}
	chs := m.signinChoices(c)
	c.signinPick = min(max(c.signinPick, 0), max(len(chs)-1, 0))
	if len(chs) > 0 {
		cl(bar + "     " + dim("Carry on as:"))
	}
	// Six at a time, keeping the picked one in sight.
	lo := max(0, min(c.signinPick-2, len(chs)-6))
	now, kind := time.Now(), m.sessionStart(c).kind
	for i := lo; i < min(len(chs), lo+6); i++ {
		ch := chs[i]
		mark := "  "
		if i == c.signinPick && c.cardFocus {
			mark = paint(cOrange, "▸ ")
		}
		l := lookOf(ch.r.kind)
		name := paint(cText, ch.r.label())
		if e := ch.r.email(); e != "" && e != ch.r.name() {
			name += dim("  " + e)
		}
		var notes []string
		if u := signinUsage(ch.r, now); u != "" {
			notes = append(notes, u)
		}
		switch {
		case ch.here:
			notes = append(notes, paint(cGreen, "in use, logged in again"))
		case ch.o.kind != kind:
			notes = append(notes, dim("takes the conversation over"))
		}
		cl(spread(bar+"   "+mark+paint(l.colour(), l.glyph)+" "+name, strings.Join(notes, dim(" · "))+"  ", w))
	}
	edge := bar
	if c.cardFocus && !c.inModal {
		edge = paint(cOrange, "▍")
	}
	k := func(key, label string) string { return paint(cText+bold, key) + " " + paint(cSub, label) }
	keys := k("y", "continue")
	if len(chs) > 1 {
		keys = k("↑↓", "pick") + "   " + k("enter", "carry on as it")
	}
	cl(edge + "   " + cardHint(c, keys))
	return out
}

// signinKey answers the signed-out card: ↑↓ through its accounts, off its
// ends as any card, and enter (or y) carries on as the one picked.
func (m *Model) signinKey(c *hostConn, s string, direct bool) (tea.Cmd, bool) {
	chs := m.signinChoices(c)
	switch {
	case c.cardFocus && (s == "up" || s == "k") && c.signinPick > 0:
		c.signinPick--
		return nil, true
	case c.cardFocus && (s == "down" || s == "j") && c.signinPick < len(chs)-1:
		c.signinPick++
		return nil, true
	case s == "alt+y" || direct && s == "y" || c.cardFocus && s == "enter":
		if c.signinPick >= len(chs) || chs[c.signinPick].here {
			return m.continueSignedIn(c), true
		}
		ch := chs[c.signinPick]
		if ch.o.kind == m.sessionStart(c).kind {
			// Its own harness's: the login switches, then it carries on.
			return tea.Sequence(m.accountSwitch(ch.r.kind, ch.o.account), m.continueSignedIn(c)), true
		}
		c.signinSent = c.sess.Info.Error
		return m.switchSessionMessage(c, ch.o, "continue"), true
	}
	return nil, false
}

// continueSignedIn carries a signed-out session on: its Claude Code, which
// holds the old sign-in, stops, and a continue starts a fresh one signed in
// as whoever is now.
func (m *Model) continueSignedIn(c *hostConn) tea.Cmd {
	info, cl := c.sess.Info, c.client
	c.signinSent = info.Error
	return func() tea.Msg {
		if err := restartClaude(cl, info, "continue"); err != nil {
			return doneMsg{err: err}
		}
		return doneMsg{text: "carrying on as the login in use"}
	}
}
