package ui

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/netwatch"
	"github.com/0xdeafcafe/rush/internal/sysinfo"
	"github.com/charmbracelet/x/ansi"
)

// quietPlan keeps the default provider fixed. Detailed meters for all
// accounts, reset times, and switch forecasts remain in the usage segment.
// Where a provider has more than one account, it is that provider's
// accounts in the order rush moves sessions through them, fitted to w.
func (m *Model) quietPlan(w int) string {
	if queues, single := m.acctQueues(); len(queues) > 0 {
		return planRows(queues, single, m.snap.At, []int{w})[0]
	}
	name, q := m.startPlan()
	k := agent.Kind(m.startKind())
	l := lookOf(k)
	who := paint(l.colour(), l.glyph)
	if _, account, ok := strings.Cut(name, " · "); ok {
		who += dim(" " + account)
	}
	return planSummary(who, q, m.snap.At) + m.otherDrains(k)
}

// acctQueue is one provider's accounts in the order rush moves sessions
// through them when the one in use runs low: the one in use first.
type acctQueue struct {
	kind  agent.Kind
	accts []inQueue
}

// inQueue is an account in a queue.
type inQueue struct {
	name string
	cur  bool
	q    usage.Quota
}

// acctQueues are the providers with more than one account to move
// between, the one new sessions start on first, then the default
// profile's order; and every other provider's account in use, or its one
// sign-in, that has limits to show.
func (m *Model) acctQueues() (queues []acctQueue, single []acctRow) {
	start := agent.Kind(m.startKind())
	rows := m.accountRows()
	for i, r := range rows {
		if !r.head {
			continue
		}
		qu := acctQueue{kind: r.kind}
		switch {
		case r.kind == loginsKind:
			// Claude Code's logins, in the order autoSwitch moves through them.
			for _, l := range fleet.Chain(m.snap.Logins, m.snap.At) {
				qu.accts = append(qu.accts, inQueue{name: l.Name, cur: l.Current, q: l.Quota})
			}
		case switches(r.kind):
			for _, a := range accountChain(accountsOf(rows[i+1:], r.kind)) {
				qu.accts = append(qu.accts, inQueue{name: a.name(), cur: a.current, q: a.q})
			}
		}
		if len(qu.accts) < 2 {
			continue
		}
		if r.kind == start {
			queues = append([]acctQueue{qu}, queues...)
		} else {
			queues = append(queues, qu)
		}
	}
	for _, r := range m.inUseRows() {
		if len(r.q.Windows) > 0 && !slices.ContainsFunc(queues, func(qu acctQueue) bool { return qu.kind == r.kind }) {
			single = append(single, r)
		}
	}
	return queues, single
}

// planRows are the queues a line each, then the providers with one
// account on a line together, each line fitted to its width in rooms.
// With fewer lines than that, the last holds its queue and only the
// glyph and tightest window of each provider after it.
func planRows(queues []acctQueue, single []acctRow, now time.Time, rooms []int) []string {
	out := make([]string, 0, len(rooms))
	for i, qu := range queues {
		at := len(out)
		if at == len(rooms)-1 && (i < len(queues)-1 || len(single) > 0) {
			// The last line, and more to say than fits on it.
			var rest []acctRow
			for _, q := range queues[i+1:] {
				for _, a := range q.accts {
					if a.cur {
						rest = append(rest, acctRow{kind: q.kind, q: a.q})
					}
				}
			}
			tail := drains(append(rest, single...), now)
			w := rooms[at] - cellw.String(tail)
			if w < queueMin(qu, now) {
				tail, w = "", rooms[at]
			}
			return append(out, queueLine(qu, now, w)+tail)
		}
		out = append(out, queueLine(qu, now, rooms[at]))
	}
	if len(single) > 0 {
		out = append(out, singleLine(single, now, rooms[len(out)]))
	}
	return out
}

// drains are each row's glyph and how much of its tightest window is used.
func drains(rows []acctRow, now time.Time) string {
	var out string
	for _, r := range rows {
		p := min(100, max(0, r.q.Since(now).Used("")))
		l := lookOf(r.kind)
		out += "  " + paint(l.colour(), l.glyph) + " " + paint(usageColor(p), pct(p))
	}
	return out
}

// queueLine is a provider's accounts in the order rush moves through
// them: ▸ before the one in use, → before each after it, with how much
// of the short and long windows each has used and, where it matters,
// when it resets. Short of room it leaves out, in turn, when the others
// reset, their long windows, then the last of them, counting them as +N;
// never the one in use.
func queueLine(qu acctQueue, now time.Time, w int) string {
	return fitDrawn(queueDraw(qu, now), len(qu.accts), w)
}

// queueMin is the least of a queue that's drawn: the account in use.
func queueMin(qu acctQueue, now time.Time) int {
	return cellw.String(queueDraw(qu, now)(2, 1))
}

// queueDraw draws a queue at a level of detail, 0 to 2, with its first
// keep accounts.
func queueDraw(qu acctQueue, now time.Time) func(level, keep int) string {
	l := lookOf(qu.kind)
	lead := paint(l.colour(), l.glyph)
	type part struct{ head, full, short, back string }
	parts := make([]part, len(qu.accts))
	for i, a := range qu.accts {
		p := &parts[i]
		name := ansi.Truncate(a.name, 12, "…")
		switch {
		case a.cur:
			p.head = " " + paint(cBright, "▸ "+name)
		case i == 0:
			p.head = " " + dim(name)
		default:
			p.head = "  " + faint("→ ") + dim(name)
		}
		p.full, p.short = windowsUsed(a.q, now)
		p.back = backIn(a.q, now, a.cur) + quotaNote(a.q, now)
	}
	return func(level, keep int) string {
		out := lead
		for i, p := range parts[:keep] {
			out += p.head
			if !qu.accts[i].cur && level >= 2 {
				out += " " + p.short
			} else {
				out += " " + p.full
			}
			if qu.accts[i].cur || level < 1 {
				out += p.back
			}
		}
		if keep < len(parts) {
			out += dim(fmt.Sprintf("  +%d", len(parts)-keep))
		}
		return out
	}
}

// windowsUsed is how much of its short and long windows an account has
// used, "2%/4%", and of its short one alone; or "out", red, when one is
// full.
func windowsUsed(q usage.Quota, now time.Time) (full, short string) {
	if q.Since(now).Used("") >= 100 {
		out := paint(cRed, "out")
		return out, out
	}
	chosen, n := firstWindows(q)
	if n == 0 {
		return dim("–"), dim("–")
	}
	used := func(win usage.Window) string {
		p := min(100, max(0, win.Percent))
		return paint(usageColor(p), pct(p))
	}
	short = used(chosen[0])
	if n == 1 {
		return short, short
	}
	return short + dim("/") + used(chosen[1]), short
}

// backIn is when an account has room again: for one that's out, when it
// comes back, its day and time when that's more than a day off; for one
// in use, its tightest window's reset once that is half used; for any
// other, its tightest window's reset.
func backIn(q usage.Quota, now time.Time, cur bool) string {
	var back time.Time
	for _, win := range q.Since(now).Windows {
		if win.Percent >= 100 && win.ResetsAt.After(back) {
			back = win.ResetsAt
		}
	}
	if !back.IsZero() {
		if back.Sub(now) > 24*time.Hour {
			return dim(" until " + back.Local().Format("Mon 15:04"))
		}
		return resetIn(back, now, false)
	}
	win, ok := q.Tightest("")
	if !ok || cur && win.Percent < 50 {
		return ""
	}
	return resetIn(win.ResetsAt, now, false)
}

// singleLine is each provider with one account: its glyph, name, how
// much of its tightest window is used and when that resets. Short of
// room it leaves out the resets, then the names, then the last of them,
// counting them as +N.
func singleLine(rows []acctRow, now time.Time, w int) string {
	type part struct{ glyph, name, used, back string }
	parts := make([]part, len(rows))
	for i, r := range rows {
		l := lookOf(r.kind)
		_, short := windowsUsed(r.q, now)
		parts[i] = part{paint(l.colour(), l.glyph), dim(" " + string(r.kind)), " " + short, backIn(r.q, now, false) + quotaNote(r.q, now)}
	}
	return fitDrawn(func(level, keep int) string {
		var out string
		for i, p := range parts[:keep] {
			if i > 0 {
				out += "   "
			}
			out += p.glyph
			if level < 2 {
				out += p.name
			}
			out += p.used
			if level < 1 {
				out += p.back
			}
		}
		if keep < len(parts) {
			out += dim(fmt.Sprintf("  +%d", len(parts)-keep))
		}
		return out
	}, len(parts), w)
}

// fitDrawn is the most of what draw draws that fits in w: every detail
// (level 0) before any of the n things drawn goes, then each level less;
// then the last things, one at a time, with the least detail.
func fitDrawn(draw func(level, keep int) string, n, w int) string {
	for keep := n; keep >= 1; keep-- {
		for level := 0; level <= 2; level++ {
			if keep < n && level < 2 {
				continue // later ones go only once the rest are short
			}
			if s := draw(level, keep); cellw.String(s) <= w {
				return s
			}
		}
	}
	return cellw.Truncate(draw(2, 1), max(0, w), "…")
}

// otherDrains is every other provider in use after k: its glyph and
// how much of its tightest window is used.
func (m *Model) otherDrains(k agent.Kind) string {
	var rows []acctRow
	for _, r := range m.inUseRows() {
		if r.kind != k && len(r.q.Windows) > 0 {
			rows = append(rows, r)
		}
	}
	return drains(rows, m.snap.At)
}

// startPlan is the provider and account new agents start on, and its limits.
func (m *Model) startPlan() (string, usage.Quota) {
	q, found := m.startQuota()
	account, k := "", m.startKind() // once, not once a row: it works out the start folder
	for _, r := range m.accountRows() {
		if string(r.kind) == k && r.current && !r.head {
			account = r.name()
			break
		}
	}
	if !found {
		for _, a := range m.snap.Accounts {
			if a.Current {
				q = a.Quota
				account = a.Name
				break
			}
		}
		for _, a := range m.snap.Logins {
			if a.Current {
				q = a.Quota
				account = a.Name
				break
			}
		}
	}
	name := providerName(agent.ProviderOf(agent.Kind(k)))
	if account != "" {
		name += " · " + ansi.Truncate(account, 12, "…")
	}
	return name, q
}

// headerGauges is the header's right side while the top bar is the
// default: spend and the account, a bar for each limit, then the machine,
// a row each. Nil when they don't fit in w.
func (m *Model) headerGauges(today float64, w int) []string {
	name, q := m.startPlan()
	now := m.snap.At
	rows := []string{paint(cText, money(today)) + dim(" today · "+name)}
	wins := slices.Clone(q.Windows)
	slices.SortStableFunc(wins, func(a, b usage.Window) int {
		switch {
		case a.Span == b.Span:
			return strings.Compare(a.Label, b.Label)
		case a.Span == 0:
			return 1
		case b.Span == 0:
			return -1
		}
		return int(a.Span - b.Span)
	})
	for _, win := range wins[:min(2, len(wins))] {
		rows = append(rows, compactWindow(win, now))
	}
	if len(wins) == 0 && q.Problem != "" {
		rows = append(rows, dim("limits ")+paint(cYellow, "unavailable"))
	}
	mc := dim(fmt.Sprintf("%.1fG RAM · %d agents", float64(m.snap.Machine.TotalMem)/(1<<30), len(m.snap.Agents)))
	if s := quietSystem(); s != "" {
		mc += dim(" · ") + s
	}
	rows = append(rows, mc)
	for _, r := range rows {
		if cellw.String(r) > w {
			return nil
		}
	}
	return rows
}

// planSummary is name, drawn as the caller likes, and its windows. It
// orders windows by identity/duration, never by their usage,
// so changing percentages do not reorder the header on each update.
func planSummary(name string, q usage.Quota, now time.Time) string {
	if len(q.Windows) == 0 {
		if q.Problem != "" {
			return name + dim(" limits ") + paint(cYellow, "unavailable")
		}
		return ""
	}
	chosen, n := firstWindows(q)
	var parts []string
	for _, win := range chosen[:n] {
		parts = append(parts, compactWindow(win, now))
	}
	out := name + " " + strings.Join(parts, "  ")
	if len(q.Windows) > n {
		out += dim(fmt.Sprintf(" +%d limits", len(q.Windows)-n))
	}
	return out + quotaNote(q, now)
}

// quotaNote says when a reading can't be trusted: a window has reset
// since it was read, so it needs reading again, or it's old.
func quotaNote(q usage.Quota, now time.Time) string {
	for _, win := range q.Windows {
		if !win.ResetsAt.IsZero() && !now.Before(win.ResetsAt) {
			return paint(cYellow, " refresh needed")
		}
	}
	if q.Problem != "" || !q.FetchedAt.IsZero() && now.Sub(q.FetchedAt) > 3*usage.Every {
		return paint(cYellow, " stale")
	}
	return ""
}

// firstWindows are q's two shortest windows, the shortest first; one
// that doesn't say how long it runs goes after those that do, and two
// as long by their labels.
func firstWindows(q usage.Quota) (chosen [2]usage.Window, n int) {
	less := func(a, b usage.Window) bool {
		if a.Span != b.Span {
			if a.Span == 0 {
				return false
			}
			if b.Span == 0 {
				return true
			}
			return a.Span < b.Span
		}
		return a.Label < b.Label
	}
	for _, win := range q.Windows {
		if n == 0 {
			chosen[0] = win
			n = 1
			continue
		}
		if less(win, chosen[0]) {
			chosen[1], chosen[0] = chosen[0], win
			n = 2
			continue
		}
		if n == 1 || less(win, chosen[1]) {
			chosen[1] = win
			n = 2
		}
	}
	return chosen, n
}

// compactWindow is a plan window in a few cells: its label, how much of
// it is used, and how long until it resets. One that runs out within the
// hour, before it resets, says when, in red, ahead of the reset.
func compactWindow(win usage.Window, now time.Time) string {
	label := win.Label
	if label == "" {
		label = "limit"
	}
	p := min(100, max(0, win.Percent))
	out := dim(label+" ") + paint(usageColor(p), pct(p))
	if rate := win.Rate(now); rate > 0 && p < 100 {
		left := time.Duration((100 - p) / rate * float64(time.Hour))
		if left < time.Hour && (win.ResetsAt.IsZero() || left < win.ResetsAt.Sub(now)) {
			out += paint(cRed, " out in "+roughly(left))
		}
	}
	return out + resetIn(win.ResetsAt, now, false)
}

// systemAlerts shows only conditions that need attention; healthy battery,
// disk and network readings do not compete with the conversation.
func systemAlerts(b sysinfo.Battery, d sysinfo.Disk, known, up, steady bool) string {
	var out []string
	if known {
		if !up {
			out = append(out, paint(cRed, "network offline"))
		} else if !steady {
			out = append(out, paint(cYellow, "network unstable"))
		}
	}
	if d.Total > 0 && d.Free < 20<<30 {
		color := cYellow
		if d.Free < 5<<30 {
			color = cRed
		}
		out = append(out, paint(color, "disk low"))
	}
	if b.Present && !b.Charging && b.Percent < 30 {
		color := cYellow
		if b.Percent < 15 {
			color = cRed
		}
		out = append(out, paint(color, "battery low"))
	}
	return strings.Join(out, dim(" · "))
}

func quietSystem() string {
	b, d := sysinfo.Now()
	n := netwatch.Now()
	return systemAlerts(b, d, n.Known, n.Up, n.Steady)
}
