package ui

import (
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// Every provider with limits shows in the top bar beside the one new
// sessions start on, as a mini meter; and a session's header says when
// its account is running low, and where there's room.

// lowAt is how full, in percent, an account's tightest window gets before
// a session on it says where else there's room.
const lowAt = 80

// inUseRows are each provider's account in use, or its one sign-in.
func (m *Model) inUseRows() []acctRow {
	if m.drawing && m.accountFrame.inUseOK {
		return m.accountFrame.inUse
	}
	var out []acctRow
	heads := map[agent.Kind]acctRow{}
	found := map[agent.Kind]bool{}
	var order []agent.Kind
	for _, r := range m.accountRows() {
		switch {
		case r.head:
			heads[r.kind] = r
			order = append(order, r.kind)
		case r.current:
			out, found[r.kind] = append(out, r), true
		}
	}
	for _, k := range order {
		if !found[k] {
			out = append(out, heads[k]) // no account of it kept, or none in use
		}
	}
	if m.drawing {
		m.accountFrame.inUse, m.accountFrame.inUseOK = out, true
	}
	return out
}

// miniBar is a five-cell bar of p percent used.
func miniBar(p float64) string {
	fill := min(5, max(0, int(p/20+0.5)))
	return paint(usageColor(p), strings.Repeat("━", fill)) + faint(strings.Repeat("─", 5-fill))
}

// miniMeter is an account's tightest window at a glance: its provider's
// glyph, the free tier said as such, a short bar and the percentage.
func miniMeter(r acctRow) string {
	l := lookOf(r.kind)
	s := paint(l.colour(), l.glyph) + " "
	if r.q.Plan == "free" {
		s += dim("free ")
	}
	p := r.q.Used("")
	return s + miniBar(p) + " " + paint(usageColor(p), pct(p))
}

// meterRow is a mini meter for each row with limits, the least room first,
// as many as fit in w.
func meterRow(rows []acctRow, w int) string {
	var have []acctRow
	for _, r := range rows {
		if len(r.q.Windows) > 0 {
			have = append(have, r)
		}
	}
	sort.SliceStable(have, func(i, j int) bool { return have[i].q.Used("") > have[j].q.Used("") })
	var out string
	for _, r := range have {
		next := out + "  " + miniMeter(r)
		if cellw.String(next) > w {
			break
		}
		out = next
	}
	return out
}

// otherMeters are the providers other than k, a mini meter each.
func (m *Model) otherMeters(k agent.Kind, w int) string {
	var rows []acctRow
	for _, r := range m.inUseRows() {
		if r.kind != k {
			r.q = r.q.Since(m.snap.At) // a window that has reset since is empty
			rows = append(rows, r)
		}
	}
	return meterRow(rows, w)
}

// label is an account as rush names it: harness:account, or the harness
// alone for one with a single sign-in of its own.
func (r acctRow) label() string {
	if r.head {
		return setupLabel(r.kind, "", "")
	}
	return setupLabel(r.kind, "", r.name())
}

// agentLabel is what session c runs as: its profile when you made one,
// else harness:account (codex:alex), and its model unless the header's
// own model segment shows it.
func (m *Model) agentLabel(a *fleet.Agent, c *hostConn, model string) string {
	k := sessionAgent(c)
	if k == "" {
		k = agent.Kind(firstNonEmpty(a.Kind, string(loginsKind)))
	}
	o := m.sessionStart(c)
	if !m.showProfile(o.profile) {
		o.profile = ""
	}
	l := lookOf(k)
	name := harnessName(string(k))
	if account := firstNonEmpty(o.account, m.accountOf(k)); account != "" && account != "default" {
		name += " · " + account
	}
	if o.profile != "" {
		name += " · " + o.profile
	}
	s := paint(l.colour(), l.glyph) + " " + paint(cText, name)
	if model != "" && !m.barLayout(barAgent).Shown("model") {
		s += dim(" · " + modelWord(string(k), model))
	}
	return s
}

// lowNote is what a session on provider k's account in use says once
// that account is running low: how full its tightest window is, and the
// account, of any provider, with the most room.
func (m *Model) lowNote(k agent.Kind) string {
	if m.drawing {
		for _, n := range m.accountFrame.notes {
			if n.kind == k {
				return n.text
			}
		}
	}
	text := m.lowNoteNow(k)
	if m.drawing {
		m.accountFrame.notes = append(m.accountFrame.notes, accountFrameNote{kind: k, text: text})
	}
	return text
}

func (m *Model) lowNoteNow(k agent.Kind) string {
	var cur acctRow
	ok := false
	for _, r := range m.inUseRows() {
		if r.kind == k {
			cur, ok = r, true
		}
	}
	now := time.Now()
	// Most accounts have no quota reading or enough room. Do not construct
	// every alternative account just to have switchNote return nothing.
	if !ok || switchNote(cur.q, nil, now) == "" {
		return ""
	}
	var others []acctRow
	for _, r := range m.accountRows() {
		// Not the account itself, nor a provider's line over its accounts.
		if r.kind == k && (r.current || r.head) || r.head && switches(r.kind) {
			continue
		}
		others = append(others, r)
	}
	return switchNote(cur.q, others, now)
}

// switchNote is lowNote's words: nothing while q has room or its reading
// is old; else how full it is, and where there's most room, when anywhere
// has room.
func switchNote(q usage.Quota, others []acctRow, now time.Time) string {
	w, ok := q.Tightest("")
	if !ok || w.Percent < lowAt || now.Sub(q.FetchedAt) > fresh {
		return ""
	}
	s := "low: " + pct(w.Percent) + " of " + w.Label + " used"
	var best acctRow
	found := false
	for _, r := range others {
		if len(r.q.Windows) == 0 || now.Sub(r.q.FetchedAt) > time.Hour || r.q.Used("") >= lowAt {
			continue
		}
		if !found || r.q.Used("") < best.q.Used("") {
			best, found = r, true
		}
	}
	if found {
		s += " · " + best.label() + " has " + pct(100-best.q.Used("")) + " left"
	}
	return s
}
