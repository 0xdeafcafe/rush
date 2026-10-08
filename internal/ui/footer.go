package ui

import (
	"cmp"
	"slices"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The side list's footer: what the next agent starts as, where, and the
// setups agents in the fleet ran lately, a click away.

const (
	footNextKey   = "⚙next"   // a click opens the start sheet
	footRecentKey = "⚙recent" // a click takes the setup under it
)

// footHit is a recent setup on the footer's last line, by column.
type footHit struct {
	x0, x1 int
	o      startOver
}

// footLines is the footer, w wide, with each row's key for a click: a
// rule titled like the feed's above it, the setup under it in the
// harness's colour, where it starts, and the recent setups to switch to.
func (m *Model) footLines(w int) (lines, keys []string) {
	dir := m.startDir()
	cur := m.nextStart(dir)
	title := " next agent "
	lines = []string{
		fit(faint("──")+paint(cText+bold, title)+faint(strings.Repeat("─", max(0, w-3-cellw.String(title)))), w-1),
		" " + fit(m.footWho(cur, w-2), w-2),
		"   " + fit(footModelLine(cur), w-4),
		"   " + faint("in ") + paint(cSub, shortPath(tildify(dir), w-7)),
	}
	keys = []string{"", footNextKey, footNextKey, footNextKey}
	m.footHits = m.footHits[:0]
	line, x := faint("   or"), 5
	for _, o := range m.recentSetups(cur) {
		chip, cw := footChip(o, cur)
		if x+2+cw > w-1 {
			break
		}
		m.footHits = append(m.footHits, footHit{x + 2, x + 2 + cw, o})
		line += "  " + chip
		x += 2 + cw
	}
	if len(m.footHits) > 0 {
		lines, keys = append(lines, line), append(keys, footRecentKey)
	}
	return lines, keys
}

// footWho is what runs the next agent and who pays: the harness, or your
// profile, then "on" its provider and the account or API key, the
// provider left out when w is too narrow for both.
func (m *Model) footWho(o startOver, w int) string {
	k := agent.Kind(o.kind)
	l := lookOf(k)
	name := cmp.Or(o.profile, agent.HarnessLabel(k))
	var on []string
	if p := agent.ProviderLabel(agent.ProviderOf(k)); p != name {
		on = append(on, p)
	}
	switch acct := cmp.Or(o.account, m.accountOf(k)); {
	case o.billing == state.BillingKey || agent.KeyOnly(k):
		on = append(on, "API key")
	case acct != "" && agent.HarnessOf(k) == k:
		on = append(on, acct)
	}
	if len(on) == 2 && cellw.String(name+"  on "+strings.Join(on, " · "))+2 > w {
		on = on[1:] // narrow: the account says more than the provider
	}
	out := paint(l.colour(), l.glyph) + " " + paint(cText+bold, name)
	if len(on) > 0 {
		out += faint("  on ") + paint(cSub, strings.Join(on, " · "))
	}
	return out
}

// footModelLine is the next agent's model and effort, each under its label.
func footModelLine(o startOver) string {
	model := "default"
	if o.model != "" {
		model = modelWord(o.kind, o.model)
	}
	out := faint("model ") + paint(cText, model)
	if o.effort != "" {
		out += faint("   effort ") + paint(cText, o.effort)
	}
	return out
}

// footChip is a recent setup, split like the lines above it: its
// harness as its glyph, named too when it isn't cur's or is a profile,
// its model, its effort faint; and its width.
func footChip(o, cur startOver) (string, int) {
	k := agent.Kind(o.kind)
	l := lookOf(k)
	chip, plain := paint(l.colour(), l.glyph), l.glyph
	if o.kind != cur.kind || o.profile != "" {
		name := cmp.Or(o.profile, agent.HarnessLabel(k))
		chip, plain = chip+" "+paint(cSub, name), plain+" "+name
	}
	word := cmp.Or(modelWord(o.kind, o.model), "default")
	chip, plain = chip+" "+paint(cText, word), plain+" "+word
	if o.effort != "" {
		chip, plain = chip+faint(" "+o.effort), plain+" "+o.effort
	}
	return chip, cellw.String(plain)
}

// recentSetups are up to 3 setups the fleet's agents ran, newest first,
// each once, cur left out.
func (m *Model) recentSetups(cur startOver) []startOver {
	if m.snap == nil {
		return nil
	}
	as := slices.Clone(m.snap.Agents)
	slices.SortStableFunc(as, func(a, b *fleet.Agent) int { return b.CreatedAt.Compare(a.CreatedAt) })
	seen := []string{strings.Join(m.startWords(cur), " · ")}
	var out []startOver
	for _, a := range as {
		if a.Advisor || len(out) == 3 {
			continue
		}
		k := cmp.Or(a.Kind, string(a.Acct.Kind))
		if k == "" {
			continue
		}
		o := m.startDefaults(k)
		if a.Spend.Model != "" {
			o.model = a.Spend.Model
		}
		if c := m.host; c != nil && c.key == a.Key && c.sess != nil {
			o = m.sessionStart(c)
		}
		if w := strings.Join(m.startWords(o), " · "); !slices.Contains(seen, w) {
			seen, out = append(seen, w), append(out, o)
		}
	}
	return out
}

// footClick takes the recent setup at column x as the next agent's.
func (m *Model) footClick(x int) {
	for _, h := range m.footHits {
		if x >= h.x0 && x < h.x1 {
			o := h.o
			m.startOver = &o
			m.flash("the next session starts as "+m.startWith(m.startDir(), true), false)
		}
	}
}
