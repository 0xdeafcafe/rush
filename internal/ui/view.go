package ui

import (
	"cmp"
	"fmt"
	"path/filepath"
	"slices"
	"sort"
	"regexp"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/statusline"
	"github.com/0xdeafcafe/rush/internal/theme"
)

func (m *Model) View() tea.View {
	defer uiBusy("frame")()
	frame := m.lastFrame
	if !m.sameFrame || frame == "" {
		m.drawing, m.tickerOn = true, false // the frame says whether a post still scrolls
		frame = m.render()
		m.drawing, m.kindMemo, m.accountFrame = false, kindMemo{}, accountFrame{}
		m.lastFrame = frame
	}
	m.sameFrame = false
	v := tea.NewView(frame)
	v.AltScreen = true
	v.WindowTitle = m.title()
	v.MouseMode = tea.MouseModeAllMotion
	v.ReportFocus = true // to know when a notification is worth sending
	return v
}

// title says where you are (the place, or the agent whose Session is
// open) and counts the agents waiting on you, so a tab or dock shows it.
func (m *Model) title() string {
	where := viewNames[m.view]
	if m.view == placeAgents {
		if _, paneW := m.widths(); paneW > 0 {
			if a := m.focused(); a != nil {
				where = cmp.Or(oneLine(a.DisplayName), agentHandle(a))
			}
		}
		if m.zen {
			where = "Zen · " + where
		}
	}
	t := "rush · " + where
	n := 0
	for _, a := range m.snap.Agents {
		if a.NeedsYou() || a.Halted() && !a.Seen {
			n++
		}
	}
	if n > 0 {
		t = fmt.Sprintf("(%d) %s", n, t)
	}
	return t
}

type tally struct {
	blocked, working, busy, done int
	today                        float64
}

func (m *Model) tally() tally {
	var t tally
	agents := m.snap.Agents
	if m.hosted != "" {
		agents = m.fleetAgents
	}
	for _, a := range agents {
		switch {
		case a.NeedsYou():
			t.blocked++
		case a.Live():
			t.working++
		case a.Busy():
			t.busy++
		default:
			t.done++
		}
	}
	for _, av := range m.snap.Accounts {
		t.today += av.Today
	}
	return t
}

// mood is clanker's reading of the fleet.
func (m *Model) mood(t tally) mood {
	switch {
	case t.blocked > 0:
		return moodNeedsYou
	case t.working > 0:
		return moodWorking
	}
	if h := m.snap.At.Hour(); h >= 23 || h < 7 {
		return moodSleepy
	}
	return moodIdle
}

var (
	mdPlain        = strings.NewReplacer("**", "", "`", "", "__", "")
	mdPlainNoUnder = strings.NewReplacer("**", "", "`", "")
)

// headH is the header's height: clanker's, or three lines when the screen
// is too narrow for him beside the text.
func (m *Model) headH() int {
	if m.w < narrowHead || m.store.Config.HideLogo {
		return 3
	}
	return clkH - 1 // the bottle's foot shares the pages row
}

// topH is the rows above the body: the header and the row under it. Zen
// has none; it's only what needs you.
func (m *Model) topH() int {
	if m.zen {
		return 0
	}
	return m.headH() + 2
}

// underHead is the pages row, level with the tabs above it, and a rule
// under both.
func (m *Model) underHead() []string {
	lead := strings.Repeat(" ", 6+clkW) // where the tabs' names start
	if m.store.Config.HideLogo {
		lead = "   "
	}
	if m.dialog != nil {
		lead = "  "
	}
	if m.w >= narrowHead && !m.store.Config.HideLogo && m.headerIconTail != "" {
		lead = "  " + m.headerIconTail + strings.Repeat(" ", max(1, 6+clkW-2-cellw.String(m.headerIconTail)))
	}
	m.headerPointerLines[1] = fit(lead+strings.TrimLeft(m.pagesWidth(max(0, m.w-cellw.String(lead)+3)), " "), m.w)
	return []string{m.headerPointerLines[1], faint(strings.Repeat("─", m.w))}
}

// narrowHead is the width below which the header drops clanker's body for
// his face and keeps its lines whole.
const narrowHead = 60

func (m *Model) header() (rows []string) {
	defer func() {
		row := 2 // the tabs
		if row < len(rows) {
			m.headerPointerLines[0] = rows[row]
		}
	}()
	t := m.tally()
	md := m.mood(t)
	robot, gap := clanker(m.clkState(md, t)), "   "
	if m.store.Config.HideLogo {
		robot, gap = make([]string, clkH), ""
	}
	var counts []string
	if t.blocked > 0 {
		counts = append(counts, paint(cYellow+bold, fmt.Sprintf("● %d needs you", t.blocked)))
	}
	if t.working > 0 {
		counts = append(counts, paint(cOrange, fmt.Sprintf("✻ %d working", t.working)))
	}
	if t.busy > 0 {
		counts = append(counts, paint(cSub, fmt.Sprintf("◌ %d in background", t.busy)))
	}
	counts = append(counts, dim(fmt.Sprintf("%d finished", t.done)))
	if mc := m.snap.Machine; mc.Orphans > 0 {
		counts = append(counts, paint(cYellow, fmt.Sprintf("%d orphaned · %s", mc.Orphans, mem(mc.OrphanMem)))+dim(" · Projects to end"))
	}
	left1 := paint(cText+bold+italic, "rush") + "   " + strings.Join(counts, "   ")

	// Which provider, account and profile new sessions start on: beside
	// the usage it reads when the top bar shows usage, here when it doesn't.
	acct := m.startTag()
	left2 := acct + dim(" · "+tildify(m.launchDir))
	if m.barLayout(barTop).Shown("usage") {
		left2 = dim(tildify(m.launchDir))
	}
	if m.w < narrowHead {
		// Only the label fits.
		var g clkGrid
		g.sprite(m.clkState(md, t))
		if m.store.Config.HideLogo {
			return m.narrowHeader(robot, counts, acct)
		}
		return m.narrowHeader(g.lines(), counts, acct)
	}

	// The right is the top bar you build in /statusline; what's left of
	// the width after clanker and the counts is its room.
	x := &barCtx{m: m, t: t}
	room := func(r, l string) int { return m.w - cellw.String("  "+r+gap+l) - 4 }
	right1 := m.barLine(barTop, 0, x, room(robot[1], left1))
	right2 := m.barLine(barTop, 1, x, room(robot[2], left2))
	if !m.loaded {
		right2 = dim("costing transcripts…   ") + right2
	}

	right := func(body, rt string) string {
		gap := m.w - cellw.String(body) - cellw.String(rt) - 2
		if gap < 2 {
			return fit(body, m.w)
		}
		return body + strings.Repeat(" ", gap) + rt
	}
	line := func(r, l, rt string) string { return right("  "+r+gap+l, rt) }
	// The top bar's third line sits right of the tabs.
	withRight3 := func(tabs string) string {
		return right(tabs, m.barLine(barTop, 2, x, m.w-cellw.String(tabs)-4))
	}
	// < > (or , .) go between the places; in a Session's box they're
	// text, so it's ctrl+\\ there. The hint goes first when it won't fit.
	places := "< >"
	if m.hosted != "" || m.paneFocus && m.host != nil && m.mode == modeList && m.dialog == nil {
		places = "ctrl+\\"
	}
	if m.store.Config.HideLogo {
		strip := "  " + gap + strings.Join(m.tabs(), " ")
		return []string{line(robot[1], left1, right1), line(robot[2], left2, right2), withRight3(withTabHint(strip, places, "places", "", m.w))}
	}
	// Keep the icon's original screen rows, while the text and everything
	// below it move up. Its last row shares the pages row's unused left side.
	m.headerIconTail = robot[len(robot)-1]
	tabs, tail := withTabHint("  "+robot[2]+gap+strings.Join(m.tabs(), " "), places, "places", "", m.w), "  "+robot[3]
	// The default top bar is a block of gauges down the right, a row each,
	// rather than one line beside the counts.
	if fmt.Sprint(m.barLayout(barTop).Lines) == fmt.Sprint(statusline.DefaultTop().Lines) {
		gw := m.w - max(cellw.String(left1), cellw.String(left2), cellw.String(tabs)-cellw.String("  "+robot[2]+gap)) - clkW - 12
		if g := m.headerGauges(t.today, gw); g != nil {
			col := m.w - 2 - slices.Max(slicesMap(g, cellw.String))
			at := func(row, r string) string {
				if r == "" {
					return row
				}
				f := fit(strings.TrimRight(row, " "), col)
				return f + strings.Repeat(" ", max(0, col-cellw.String(f))) + r
			}
			g = append(g, "", "", "")
			return []string{at(line(robot[0], left1, ""), g[0]), at(line(robot[1], left2, ""), g[1]), at(tabs, g[2]), at(tail, g[3])}
		}
	}
	return []string{line(robot[0], left1, right1), line(robot[1], left2, right2), withRight3(tabs), tail}
}

// slicesMap is f of each of xs.
func slicesMap[T, U any](xs []T, f func(T) U) []U {
	out := make([]U, len(xs))
	for i, x := range xs {
		out[i] = f(x)
	}
	return out
}

// withTabHint is a strip of tabs with a quiet hint after it, the keys
// that change tab and what they go through, then rest. The hint is
// dropped first when the row hasn't the room for it.
func withTabHint(strip, keys, what, rest string, w int) string {
	hint := dim("   "+keys) + faint(" "+what)
	if cellw.String(strip)+cellw.String(hint)+cellw.String(rest) > w {
		return strip + rest
	}
	return strip + hint + rest
}

// pageTabs are the pages of the place you're in, the one showing, and
// what turns to one; none where the place has no pages.
func (m *Model) pageTabs() (names []string, cur int, turn func(int) tea.Cmd) {
	switch {
	case m.dialog != nil:
		lo, hi := placePages(m.view)
		for _, p := range m.settingsPages()[lo:hi] {
			names = append(names, p.name)
		}
		return names, m.dialog.page - lo, func(i int) tea.Cmd { m.setSettingsPage(lo + i); return nil }
	case m.zen || m.hosted != "":
		return nil, 0, nil
	case m.mode == modeWall || m.view == placeAgents:
		return agentsPages, m.work.page, func(i int) tea.Cmd { m.setAgentsPage(i); return m.refreshFolders() }
	}
	return nil, 0, nil
}

// clickTab goes to the place or page whose tab is at x on row y of the
// header, reporting whether there was one.
func (m *Model) clickTab(x, y int) (tea.Cmd, bool) {
	h := m.headerTabAt(x, y)
	if !h.valid {
		return nil, false
	}
	if h.page {
		_, _, turn := m.pageTabs()
		return turn(h.index), true
	}
	m.setView(h.index)
	return tea.Batch(m.loadPreview(), m.projectsOpen()), true
}

// pages are the pages of the place you're in, the one showing bright; [
// and ] go through them. In Agents it says whether Zen is on.
func (m *Model) pages() string { return m.pagesWidth(m.w) }

func (m *Model) pagesWidth(width int) string {
	if m.dialog != nil {
		return m.settingsTabs(max(1, width-3))
	}
	hint := "[ ]"
	switch {
	case m.zen:
		return "   " + paint(cYellow, "zen") + faint(" ctrl+z")
	case m.dialog == nil && m.mode != modeEff && m.mode != modeProjects && m.hosted != "":
		return "" // no zen in hosted
	}
	names, cur, _ := m.pageTabs()
	if names == nil {
		return faint("   ctrl+z zen")
	}
	out := make([]string, len(names))
	for i, n := range names {
		if i == cur {
			out[i] = paint(cText+bold, n)
		} else if m.topHover.valid && m.topHover.page && m.topHover.index == i {
			out[i] = hoverLine(paint(cText, n), cellw.String(n))
		} else {
			out[i] = dim(n)
		}
	}
	full := "   " + strings.Join(out, dim(" · ")) + faint("  "+hint)
	// Where the row hasn't the room for every page, the one showing and
	// where it is among them.
	if cellw.String(ansi.Strip(full)) > width {
		return "   " + faint("‹ ") + paint(cText+bold, names[cur]) + faint(fmt.Sprintf(" %d/%d › ", cur+1, len(names))) + faint(hint)
	}
	return full
}

func (m *Model) tabs() []string {
	var tabs []string
	for i, v := range viewNames {
		if i == m.view {
			tabs = append(tabs, tabOn+" "+v+" "+reset)
		} else if m.topHover.valid && !m.topHover.page && m.topHover.index == i {
			tabs = append(tabs, hoverLine(paint(cText, " "+v+" "), cellw.String(v)+2))
		} else {
			tabs = append(tabs, tabOff+" "+v+" "+reset)
		}
	}
	return tabs
}

// narrowHeader is the header on a narrow screen: clanker's face instead of
// all of him, the folder shortened from the left, and as many tabs as fit
// around the current one, so no line is cut off.
func (m *Model) narrowHeader(robot, counts []string, acct string) []string {
	face := robot[clkFace]
	indent := strings.Repeat(" ", cellw.String(face)+3)
	// Whole counts or none: the last ones (finished, orphans) go first.
	title := paint(cText+bold, "rush")
	for n := len(counts); n >= 0; n-- {
		t := strings.Join(append([]string{paint(cText+bold, "rush")}, counts[:n]...), "  ")
		if cellw.String(t) <= m.w-cellw.String(face)-4 || n == 0 {
			title = t
			break
		}
	}
	out := []string{
		"  " + face + "  " + fit(title, m.w-cellw.String(face)-4),
		"  " + indent + acct + dim(" · "+shortPath(tildify(m.launchDir), m.w-cellw.String(indent)-cellw.String(ansi.Strip(acct))-5)),
	}
	tabs := m.tabs()
	room := m.w - cellw.String(indent) - 4
	width := func(ts []string) int { return cellw.String(strings.Join(ts, " ")) }
	start := 0
	for start < m.view && width(tabs[start:m.view+1]) > room-2 {
		start++
	}
	end := m.view + 1
	for end < len(tabs) && width(tabs[start:end+1]) <= room-2 {
		end++
	}
	line := strings.Join(tabs[start:end], " ")
	if start > 0 {
		line = faint("‹ ") + line
	}
	if end < len(tabs) {
		line += faint(" ›")
	}
	return append(out, "  "+indent+line)
}

// shortPath keeps a path's last folders, as many as fit in w.
func shortPath(p string, w int) string {
	if cellw.String(p) <= w {
		return p
	}
	parts := strings.Split(p, "/")
	s := parts[len(parts)-1]
	for i := len(parts) - 2; i >= 0; i-- {
		next := parts[i] + "/" + s
		if cellw.String(next)+2 > w {
			break
		}
		s = next
	}
	return fit("…/"+s, w)
}

// resetIn says when a usage window resets: the clock time and how long
// until then for the 5-hour window, the day and time for the 7-day one.
func resetIn(at, now time.Time, week bool) string {
	if at.IsZero() {
		return ""
	}
	if !at.After(now) {
		return faint(" reset") // read before a reset that has since passed
	}
	return faint(" ↻" + roughly(at.Sub(now)))
}

// roughly is a duration to one unit: 40m, 3h, 5d.
func roughly(d time.Duration) string {
	switch {
	case d < time.Hour:
		return fmt.Sprintf("%dm", max(1, int(d.Round(time.Minute).Minutes())))
	case d < 48*time.Hour:
		return fmt.Sprintf("%dh", int(d.Round(time.Hour).Hours()))
	}
	return fmt.Sprintf("%dd", int(d.Round(24*time.Hour).Hours()/24))
}

// usageMeter is one plan window: a bar of cells of what's used, coloured
// by how it is going, with a tick for how far through the window we are,
// so being ahead of pace shows at a glance; then the percentage and, with
// when, when it resets.
func usageMeter(label string, pct float64, resets time.Time, window time.Duration, rate float64, now time.Time, w int, when bool) string {
	pace := -1.0 // share of the window gone, when we know when it ends
	if !resets.IsZero() && resets.After(now) {
		pace = 1 - float64(resets.Sub(now))/float64(window)
	}
	c := cGreen
	switch {
	case pct >= 80:
		c = cRed
	case pct >= 50 || (pace >= 0 && pct/100 > pace+0.1):
		c = cYellow // high, or burning faster than the window allows
	}
	fill := min(w, max(0, int(pct/100*float64(w)+0.5)))
	if pct > 0 && fill == 0 {
		fill = 1
	}
	tick := -1
	if pace >= 0 {
		tick = min(w-1, int(pace*float64(w)))
	}
	var b strings.Builder
	for i := range w {
		switch {
		case i == tick && i < fill:
			b.WriteString(paint(cText, "╋"))
		case i == tick:
			b.WriteString(paint(cSub, "┼"))
		case i < fill:
			b.WriteString(paint(c, "━"))
		default:
			b.WriteString(faint("─"))
		}
	}
	s := dim(label+" ") + b.String() + " " + paint(c, fmt.Sprintf("%.0f%%", pct))
	if when {
		s += runsOut(pct, rate, resets, now) + resetIn(resets, now, window > 24*time.Hour)
	}
	return s
}

// runsOut says how long the window lasts at the rate it fills: red when that
// is before it resets, "ok" when it outlasts the reset, nothing until a rate is known.
func runsOut(pct, rate float64, resets, now time.Time) string {
	if rate <= 0 || pct >= 100 {
		return ""
	}
	left := time.Duration((100 - pct) / rate * float64(time.Hour))
	if !resets.IsZero() && resets.After(now) && left >= resets.Sub(now) {
		return faint(" ⌛ok")
	}
	return paint(cRed, " ⌛"+roughly(left))
}

// activeUsage is the current account's plan usage, with when each window
// resets, quiet unless it is high, and near a switch the account rush
// moves on to next with its usage; then every other provider's, a mini
// meter each. It doesn't say whose: the header does, beside it. Short of
// room in w, the detail halves its bars, then drops its reset times, then
// goes, before another provider's meter is left out.
func (m *Model) activeUsage(w int) string {
	for _, av := range m.snap.Accounts {
		if !av.Current {
			continue
		}
		u := av.Quota
		for _, l := range m.snap.Logins {
			if l.Current {
				u = l.Quota // the login sessions run as, in its home or ~/.claude
			}
		}
		if q, ok := m.startQuota(); ok {
			u = q // new sessions run another agent: its account's
		}
		k := agent.Kind(m.startKind())
		if len(u.Windows) == 0 {
			return strings.TrimLeft(m.otherMeters(k, w), " ")
		}
		others := m.otherMeters(k, 1<<16)
		if others != "" {
			others = faint("  │") + others
		}
		for _, st := range []struct {
			cells int
			when  bool
		}{{10, true}, {5, true}, {5, false}} {
			if s := m.usageDetail(u, st.cells, st.when) + others; cellw.String(s) <= w {
				return s
			}
		}
		return strings.TrimLeft(meterRow(m.inUseRows(), w), " ")
	}
	return ""
}

// usageDetail is activeUsage's own account, its meters cells wide.
func (m *Model) usageDetail(u usage.Quota, cells int, when bool) string {
	var parts []string
	for _, win := range u.Windows {
		parts = append(parts, usageMeter(win.Label, win.Percent, win.ResetsAt, win.Span, win.Rate(m.snap.At), m.snap.At, cells, when))
	}
	if next, ok := m.upcoming(u.Used("")); ok {
		// Where sessions go at the switch: glyph, name, and a short
		// bar of its tightest window, in the meters' own style.
		n := next.q.Used("")
		l := lookOf(next.kind)
		parts = append(parts, faint("↪ at "+pct(u.SwitchPoint("", usage.Lead, m.snap.At))+" ")+paint(l.colour(), l.glyph)+" "+paint(cText, next.name())+" "+miniBar(n)+" "+paint(usageColor(n), pct(n)))
	}
	s := m.usageTag() + "  " + strings.Join(parts, "   ")
	if !u.FetchedAt.IsZero() && m.snap.At.Sub(u.FetchedAt) > 3*usage.Every {
		when := u.FetchedAt.Local().Format("15:04")
		if m.snap.At.Sub(u.FetchedAt) > 20*time.Hour {
			when = u.FetchedAt.Local().Format("Mon 15:04")
		}
		s += faint(" as of " + when)
	}
	return s
}

func (m *Model) render() string {
	if m.w == 0 {
		return ""
	}
	m.over = overlayHit{} // kept again by the box drawn on top, if any
	screen := m.quickOver(m.renderScreen()) // the quick ask floats over all but these
	if m.bar != nil {
		return m.overlayBar(screen)
	}
	if m.confirm != nil {
		return m.confirmModal(screen)
	}
	if m.dialog != nil && m.keysTaking() != nil {
		return m.keysModal(screen)
	}
	return screen
}

func (m *Model) renderScreen() string {
	switch m.mode {
	case modeHelp:
		return m.overlayBox(m.listView(), m.helpBody(), min(m.w-4, 80))
	case modeProjects:
		if m.sheet != nil {
			return m.sheetView(m.frame(m.projectsBody(), m.projectsHint()))
		}
		return m.frame(m.projectsBody(), m.projectsHint())
	case modeWall:
		return m.frame(m.wallBody(m.w-4, m.wallH()), m.wallHint())
	}
	if m.dialog != nil {
		if m.sheet != nil { // a sign-in started from Accounts
			return m.sheetView(m.frame(m.dialogBody(m.w-6), ""))
		}
		return m.frame(m.dialogBody(m.w-6), "")
	}
	if m.picker != nil {
		if m.picker.pickerWide() {
			return m.overlayBox(m.listView(), m.pickerBody(m.w-10), m.w-6)
		}
		return m.overlayBox(m.listView(), m.pickerBody(min(m.w-10, 96)), min(m.w-6, 100))
	}
	if m.sheet != nil {
		return m.sheetView(m.listView())
	}
	return m.listView()
}

func (m *Model) frame(body []string, hint string) string {
	head := m.header()
	var b strings.Builder
	for _, l := range head {
		b.WriteString(fit(l, m.w))
		b.WriteByte('\n')
	}
	for _, l := range m.underHead() {
		b.WriteString(l)
		b.WriteByte('\n')
	}
	avail := m.h - len(head) - 4
	// Keep the cursor row in view on long lists.
	start := 0
	if cur := m.frameCursor(body); cur >= avail {
		start = cur - avail + 2
	}
	for i := 0; i < avail; i++ {
		if j := start + i; j < len(body) {
			b.WriteString(fit("  "+body[j], m.w))
		}
		b.WriteByte('\n')
	}
	b.WriteString(faint(strings.Repeat("─", m.w)))
	b.WriteByte('\n')
	if d := m.dialog; d != nil && d.asking != "" {
		typed := string(d.input)
		if d.secret {
			typed = strings.Repeat("•", min(len(d.input), 24))
		}
		hint = paint(cOrange, d.asking+" ❯ ") + paint(cText, typed) + paint(cOrange, "▏")
	}
	b.WriteString(m.statusOr(hint))
	return b.String()
}

// frameCursor finds the highlighted row in a framed body.
func (m *Model) frameCursor(body []string) int {
	for i, l := range body {
		if strings.Contains(l, selBG) {
			return i
		}
	}
	return 0
}

func (m *Model) statusOr(hint string) string {
	reading := ""
	if m.pastingImg {
		reading = paint(cOrange, spinner[m.pasteFrame%len(spinner)]) + dim(" reading the clipboard…")
	}
	if m.status != "" && m.snap.At.Sub(m.statusAt).Seconds() < 6 {
		c := cText
		if m.statusErr {
			c = cRed
		}
		// The spinner goes after a flash, not over it.
		if reading != "" {
			reading = dim(" · ") + reading
		}
		return fit("  "+paint(c, m.status)+reading, m.w)
	}
	if reading != "" {
		return fit("  "+reading, m.w)
	}
	return fit("  "+hint, m.w)
}

// keys are the keys a confirmation waits on, and what each does.
func (c *confirmation) keys() string {
	keys := paint(cOrange, "y") + dim(" "+cmp.Or(c.yesText, "yes"))
	if c.onBang != nil && c.bangText != "" {
		keys += "   " + paint(cOrange, "!") + dim(" "+c.bangText)
	}
	for _, ch := range c.more {
		keys += "   " + paint(cOrange, ch.key) + dim(" "+ch.text)
	}
	if c.onNo != nil {
		return keys + "   " + paint(cOrange, "n") + dim(" "+c.noText) + "   " + paint(cOrange, "esc") + dim(" cancel")
	}
	return keys + "   " + paint(cOrange, "n") + dim(" cancel")
}

// confirmModal draws the question being asked in a box over base: the
// question, its detail, then its keys.
func (m *Model) confirmModal(base string) string {
	bw := min(m.w-4, 72)
	return m.modalOver(base, m.confirmBody(bw), bw, cYellow)
}

// confirmBody is the question's box's inside, for a box bw wide.
func (m *Model) confirmBody(bw int) []string {
	c := m.confirm
	body := []string{paint(cText+bold, c.question)}
	if c.detail != "" {
		for _, l := range wrap(c.detail, bw-4) {
			body = append(body, dim(l))
		}
	}
	// keys are the keys a confirmation waits on, and what each does. A long
	// choice list wraps rather than slice its tail: the last key, n to
	// cancel, must stay on screen.
	body = append(body, "")
	if cellw.String(c.keys()) > bw-4 {
		for _, l := range wrap(c.keys(), bw-4) {
			body = append(body, l)
		}
	} else {
		body = append(body, c.keys())
	}
	return body
}

// modalOver draws body in a box bw wide, edged in col, centred over base
// faded out.
func (m *Model) modalOver(base string, body []string, bw int, col string) string {
	lines := strings.Split(base, "\n")
	for y := range lines {
		lines[y] = faint(unplace(ansi.Strip(fit(lines[y], m.w))))
	}
	box := edgedBox(body, bw, col)
	top, left := max(1, (len(lines)-len(box))/2), (m.w-bw)/2
	m.keepOverlay(left, top, bw, len(box), left+2, top+2)
	return strings.Join(pasteAt(lines, box, top, left), "\n")
}

// keysFit drops the least important pairs (those before the last) until the
// line fits w, so a hint never runs off the screen.
func keysFit(w int, pairs ...string) string {
	for len(pairs) > 2 {
		if s := keys(pairs...); cellw.String(s) <= w {
			return s
		}
		pairs = append(pairs[:len(pairs)-4], pairs[len(pairs)-2:]...)
	}
	return keys(pairs...)
}

// folderKey is the key that changes where a new agent starts, and what it
// does: a worktree's agent offers its worktree or the checkout it came
// from, by name; otherwise the folder picker, when there's a choice.
func (m *Model) folderKey(a *fleet.Agent) (key, label string) {
	if a != nil && inTree(a) && a.Key != m.pickedFor {
		other := filepath.Base(a.Repo)
		if m.startInTree {
			other = filepath.Base(a.Root)
		}
		return convo.KeyWord(m.boundKey("list.worktree")), "start in " + other
	}
	if len(m.startDirs()) > 1 {
		return "ctrl+l", "folder"
	}
	return "", ""
}

// fewKeys is the first n of pairs, then always, fitted to w: a row of keys
// says what matters now and leaves the rest to ?.
func fewKeys(w, n int, pairs, always []string) string {
	pairs = pairs[:min(len(pairs), 2*n):min(len(pairs), 2*n)]
	return keysFit(w, append(pairs, always...)...)
}

// needingYou counts the agents waiting on you, which ctrl+n steps through.
func (m *Model) needingYou() int {
	n := 0
	for _, a := range m.order {
		if a.NeedsYou() {
			n++
		}
	}
	return n
}

func needsLabel(n int) string {
	if n == 1 {
		return "1 needs you"
	}
	return fmt.Sprintf("%d need you", n)
}

// keys renders "key label" pairs with the key brighter than its label.
func keys(pairs ...string) string {
	var parts []string
	for i := 0; i+1 < len(pairs); i += 2 {
		parts = append(parts, paint(cText+bold, convo.KeyWord(pairs[i]))+" "+dim(convo.KeyWord(pairs[i+1])))
	}
	return strings.Join(parts, faint("  ·  "))
}

// layout splits the screen between the list and the preview pane and says
// how many rows the body gets under the header and above the prompt.
func (m *Model) layout() (listW, paneW, bodyH int) {
	listW, paneW = m.widths()
	bodyH = max(3, m.h-m.topH()-m.promptH(m.promptW(listW, paneW)))
	return listW, paneW, bodyH
}

// widths is how the screen splits between the list and the pane, without
// the height, which needs the prompt drawn and so the picker, and #view's
// picker asks which layout is on.
func (m *Model) widths() (listW, paneW int) {
	if m.hostedAlone() {
		return 0, m.w // the one session, full width
	}
	if m.hosted != "" {
		// Hosted's list, beside the session when there's room for both.
		if side := m.sideWidth(); m.w-side-1 >= minPane {
			return side, m.w - side - 1
		}
		return m.w, 0
	}
	listW = m.w
	showing := m.full || m.preview || m.autoSplit()
	if m.zenFull() {
		listW, paneW = 0, m.w // zen is the one agent, full width
		showing = false
	}
	if showing {
		// The list keeps at least a quarter of the screen and 30 columns;
		// if that leaves the pane too narrow to read, there's no split.
		side := m.sideWidth()
		switch {
		case m.chatAlone():
			paneW, listW = m.w, 0
		case m.w-side-1 >= minPane:
			listW, paneW = side, m.w-side-1
		case m.preview:
			paneW, listW = m.w, 0
		}
	}
	return listW, paneW
}

// sideWidth is the list's width in a split: your own share when you've set
// one (shift+← →, dragging the edge, /width), else rush's; never under a
// quarter of the screen or 30 columns. Past those ends, see stepSplit.
func (m *Model) sideWidth() int {
	floor := max((m.w+3)/4, 30)
	side := max(floor, min(m.w*28/100, 64))
	if f := m.store.Config.SideWidth; f > 0 {
		// A width you dragged to wins.
		return max(floor, min(int(float64(m.w)*f+0.5), m.w*3/4))
	}
	// The Session never takes more than its content can use; the spare
	// room goes to Agents. With tiles pinned beside it, they use it.
	if len(m.grid.keys) > 0 {
		return side
	}
	return max(side, m.w-1-maxPane)
}

// maxPane is the widest a Session gets: its rows cap at 124 columns, plus
// the pane's margins.
const maxPane = 128

// setSideWidth stores the list's share, kept between a quarter and a half.
func (m *Model) setSideWidth(cols int) {
	if m.w <= 0 {
		return
	}
	cols = min(cols, m.w-1-minPane) // never so wide the split goes away
	f := float64(cols) / float64(m.w)
	f = max(0.25, min(f, 0.75))
	m.store.Config.SideWidth = f
	m.flash(fmt.Sprintf("list width %.0f%% · release to keep", f*100), false)
}

// stepSplit is shift+← and shift+→ (alt too): they move the list's edge,
// and pushed past either end, the screen goes to the Session alone or the
// list alone. It says whether it took the key.
func (m *Model) stepSplit(grow bool) (tea.Cmd, bool) {
	floor := max((m.w+3)/4, 30)
	if m.w-floor-1 < minPane || m.zenFull() || m.hosted != "" {
		return nil, false // too narrow for a split to step through
	}
	ceil := min(m.w*3/4, m.w-1-minPane)
	switch {
	case m.listW == 0:
		if grow {
			return m.splitAgain(), true
		}
	case !m.chatOpen():
		if !grow {
			return m.splitAgain(), true
		}
	case grow && m.sideWidth()+2 > ceil:
		m.listOnly()
	case !grow && m.sideWidth()-2 < floor:
		return m.sessionOnly(), true
	default:
		d := 2
		if !grow {
			d = -2
		}
		m.setSideWidth(m.sideWidth() + d)
	}
	return nil, true
}

// chatOpen is whether a Session is on screen, beside the list or alone.
func (m *Model) chatOpen() bool { return m.full || m.preview || m.autoSplit() }

// chatAlone is whether the Session has the whole screen: pushed there, or
// opened from Agents alone after you last had one that way.
func (m *Model) chatAlone() bool {
	return m.full || m.preview && m.store.Config.ListOnly && m.store.Config.ChatFull
}

// listOnly hides the Session and gives the screen to Agents, and keeps it
// that way, next time too: a Session opens from there as you last had one.
func (m *Model) listOnly() {
	m.store.Config.View, m.store.Config.ListOnly = "list", true
	_ = m.store.SaveConfig()
	m.preview, m.full, m.paneFocus = false, false, false
	m.flash("just Agents, next time too · shift+← or #view split puts the Session beside them", false)
}

// splitAgain puts Agents and the Session side by side, when there's room,
// and keeps them that way, next time too.
func (m *Model) splitAgain() tea.Cmd {
	if !m.canSplit() {
		m.flash("too narrow for Agents and the Session side by side", true)
		return nil
	}
	if !m.chatOpen() {
		m.preview = !m.wide()
	}
	m.store.Config.SetView("split")
	m.full = false
	_ = m.store.SaveConfig()
	m.flash("Agents and the Session side by side, next time too", false)
	return m.loadPreview()
}

// canSplit is whether the screen is wide enough for Agents and the Session
// side by side.
func (m *Model) canSplit() bool { return m.w-max((m.w+3)/4, 30)-1 >= minPane }

// splitHint is the key back to the split, for the hint row, while one side
// has the screen and there's room for both.
func (m *Model) splitHint(back string) []string {
	if m.zen || m.hosted != "" || !m.canSplit() {
		return nil
	}
	return []string{back + " · #view split", "back to side by side"}
}

// viewNow is which of #view's layouts is on screen.
func (m *Model) viewNow() string {
	l, p := m.widths()
	switch {
	case l == 0:
		return "agent"
	case p == 0:
		return "list"
	}
	return "split"
}

// sessionOnly gives the screen to the picked agent's Session, and keeps it
// that way: Sessions open alone from then on, and rush opens on one.
func (m *Model) sessionOnly() tea.Cmd {
	if m.selected() == nil {
		return nil
	}
	m.preview, m.full = true, true
	m.store.Config.SetView("agent")
	_ = m.store.SaveConfig()
	m.flash("just the Session, next time too · esc for Agents · shift+→ or #view split for both side by side", false)
	return m.loadPreview()
}

// dragSplit follows the list's edge as it's dragged; dropped against
// either side of the screen, that side's pane goes.
func (m *Model) dragSplit(x int) {
	switch {
	case x >= m.w-2:
		if m.chatOpen() {
			m.listOnly()
		}
	case x <= 1:
		if !m.chatAlone() {
			m.sessionOnly()
		}
	default:
		if !m.chatOpen() {
			m.preview = !m.wide()
		}
		m.full = false
		m.store.Config.SetView("split")
		m.setSideWidth(x)
	}
}

// minPane is the narrowest pane worth splitting the screen for: a step row
// with its numbers on the right.
const minPane = 84

// promptW keeps the input under the list when a pane sits beside it, so the
// pane's own input is never stacked over ours.
func (m *Model) promptW(listW, paneW int) int {
	if listW > 0 && paneW > 0 {
		return listW
	}
	return m.w
}

// paneH is the pane's height: beside the list it runs past the prompt to the
// bottom of the screen.
func (m *Model) paneH() int {
	listW, paneW, bodyH := m.layout()
	if listW > 0 && paneW > 0 {
		return bodyH + m.promptH(listW)
	}
	return bodyH
}

func (m *Model) listView() string {
	var head []string
	if !m.zen {
		head = append(m.header(), m.underHead()...)
	}
	listW, paneW, bodyH := m.layout()
	over := m.pickerOverCard() // as the Prompt was measured
	var dock []string
	if paneW == 0 && m.h >= 20+m.dockLines() {
		if f := m.focused(); f != nil {
			for _, l := range m.cardLines(f, m.w-4) {
				dock = append(dock, "  "+l)
			}
		} else {
			dock = append(dock, "", faint("  select an agent to see what it is doing"))
		}
		for len(dock) < 4+m.dockLines() {
			dock = append(dock, "")
		}
	}
	prompt := m.promptLines(m.promptW(listW, paneW))
	split := listW > 0 && paneW > 0
	paneH := bodyH
	if split {
		paneH += len(prompt)
	}
	if bodyH-len(dock) < 5 {
		dock = nil // the list needs a few rows more than the dock does
	}
	bodyH = max(3, bodyH-len(dock))
	m.listTop = len(head)
	m.rowKeys = nil
	var left []string
	m.listW = listW
	if listW > 0 {
		// Getting started sits at the foot of the list while there's room.
		var card []string
		if m.showCard() {
			if card = m.startedLines(listW); bodyH-len(card) < 8 {
				card = nil
			}
		}
		m.cardShown = card != nil
		if card != nil && over {
			// The # picker takes Getting started's rows, so the list
			// doesn't jump when it opens.
			if pick := m.fleetSlashLines(listW); pick != nil {
				card = append(make([]string, max(0, len(card)-len(pick))), pick...)
			}
		}
		// Recall, when on, takes the foot of the list, over the feed: the
		// open chat's notes. The list scrolls in what's left.
		m.side.shownOn = ""
		var recall []string
		if card == nil && m.side.on && m.host != nil && bodyH >= 16 {
			recall = m.sideColumn(listW, min(recallMax, bodyH/2), 0, 0)
			for len(recall) > 0 && strings.TrimSpace(ansi.Strip(recall[len(recall)-1])) == "" {
				recall = recall[:len(recall)-1]
			}
			recall = append(recall, "")
		}
		// The feed docks at the very foot, on the prompt; the list scrolls behind it.
		var docked, dockedKeys []string
		if card == nil {
			docked, dockedKeys = m.streamDock(listW, bodyH)
		}
		left = append([]string{m.columnHeader(listW)}, m.listLines(listW, bodyH-1-len(card)-len(recall)-len(docked))...)
		// What agents did lately fills what the list leaves empty.
		// The next agent's setup is pinned at the foot when there's room.
		var foot, footKeys []string
		if card == nil && bodyH-len(left)-len(docked) >= 6 {
			foot, footKeys = m.footLines(listW)
		}
		foot, footKeys = append(foot, docked...), append(footKeys, dockedKeys...)
		if recall != nil {
			at := bodyH - len(foot) - len(recall)
			if feed, _ := m.feedLines(listW, at-len(left)-1); feed != nil {
				at -= len(feed)
			}
			for len(left) < at {
				left = append(left, "")
			}
			for len(m.rowKeys) < len(left)-1+len(recall) {
				m.rowKeys = append(m.rowKeys, "")
			}
			m.side.y = len(head) + len(left)
			left = append(left, recall...)
		}
		if feed, keys := m.feedLines(listW, bodyH-len(left)-len(foot)-1); card == nil && feed != nil {
			for len(left) < bodyH-len(feed)-len(foot) {
				left = append(left, "")
			}
			for len(m.rowKeys) < len(left)-1 {
				m.rowKeys = append(m.rowKeys, "")
			}
			left, m.rowKeys = append(left, feed...), append(m.rowKeys, keys...)
		}
		if foot != nil {
			for len(left) < bodyH-len(foot) {
				left = append(left, "")
			}
			for len(m.rowKeys) < len(left)-1 {
				m.rowKeys = append(m.rowKeys, "")
			}
			left, m.rowKeys = append(left, foot...), append(m.rowKeys, footKeys...)
		}
		if card != nil {
			for len(left) < bodyH-len(card) {
				left = append(left, "")
			}
			left = append(left[:bodyH-len(card)], card...)
		}
		m.listTop++
	}
	var pane []string
	sessionHeader := false
	if paneW > 0 {
		// Tiles pinned to the grid take a column beside the Session, or a
		// row under it.
		gl := m.gridSplit(paneW-3, paneH)
		sw, sh := gl.sw, gl.sh
		if m.peek.on {
			pane = m.zenPeekLines(paneW-3, paneH)
		} else if m.zen && len(m.zenQueue()) == 0 {
			pane = m.zenQuiet(paneW-3, paneH)
		} else if pane = m.rushPane(sw, sh); pane == nil {
			// A Claude Code agent's Session: its live screen or a summary,
			// switched with [ ], under the same strip a rush session has.
			var body []string
			if m.claudeView == 0 {
				body = m.liveLines(sw)
			}
			if body == nil {
				body = m.previewLines(sw, sh-1)
			}
			pane = append([]string{m.claudeStrip(sw)}, body...)
			if f := m.focused(); m.zenFull() && f != nil {
				pane = append([]string{m.zenBar(f, sw)}, pane...)
			}
		} else {
			sessionHeader = !m.zenFull()
		}
		m.grid.at, m.grid.colX, m.grid.rowY = m.grid.at[:0], 0, 0
		if (gl.col != nil || gl.row != nil) && !m.peek.on {
			pane = m.gridJoin(pane, m.paneX(), len(head), gl)
		}
	}
	var b strings.Builder
	b.Grow(m.frameLen + m.frameLen/8)
	m.paneTop = len(head)
	fadeTurn()
	for _, l := range head {
		fitTo(&b, l, m.w, "")
		b.WriteByte('\n')
	}
	// Rows beside each other: the side without the keys fades back.
	listFade, paneFade := "", ""
	if m.twoSided() {
		if m.sessionFocused() {
			listFade = fade
		} else {
			paneFade = fade
		}
	}
	div := m.divider()
	split2 := func(l, p string, row int) {
		if listFade != "" {
			l = fadeRow(l)
		}
		b.WriteString(listFade)
		fitTo(&b, l, listW, listFade)
		if listFade != "" {
			b.WriteString(reset)
		}
		b.WriteString(div)
		m.paneFrameRow(&b, p, paneW-3, paneFade, sessionHeader, row)
	}
	for i := 0; i < bodyH; i++ {
		l, p := "", ""
		if i < len(left) {
			l = left[i]
		}
		if i < len(pane) {
			p = pane[i]
		}
		switch {
		case listW > 0 && paneW > 0:
			split2(l, p, i)
		case listW > 0:
			fitTo(&b, l, m.w, "")
		default:
			m.paneFrameRow(&b, p, m.w-3, "", sessionHeader, i)
		}
		b.WriteByte('\n')
	}
	for _, l := range dock {
		fitTo(&b, l, m.w, "")
		b.WriteByte('\n')
	}
	m.promptBoxY = len(head) + bodyH + len(dock) + m.promptBoxIdx
	for i, l := range prompt {
		if split {
			p := ""
			if j := bodyH + i; j < len(pane) {
				p = pane[j]
			}
			split2(l, p, bodyH+i)
		} else {
			fitTo(&b, l, m.w, "")
		}
		if i < len(prompt)-1 {
			b.WriteByte('\n')
		}
	}
	m.frameLen = b.Len()
	if len(prompt) == 0 {
		// No Prompt under the body (zen): a last newline would scroll the
		// screen and lose the top row.
		return strings.TrimSuffix(b.String(), "\n")
	}
	return b.String()
}

// Focus: when a session can take the keys, the side without them fades
// back so where you're typing is obvious at a glance; the side with them
// keeps full brightness, an orange marker and an orange box edge.
//
// rush fades it itself, every colour fadeBy of the way to the background,
// not with the terminal's faint, which some (Terminal.app) take so far on
// a theme's own greys that the quiet parts are lost.
const fadeBy = 0.3

var (
	fade  string            // the text colour on the faded side, after every reset
	faded map[string]string // an escape code as it is on the faded side
)

// fadedRows are the rows fadeRow faded this frame and the last, by row:
// most rows are the same frame to frame. fadedFor is the fade they were
// made with.
var fadedRows, fadedRowsOld map[string]string
var fadedFor string

// fadeTurn starts a frame's fading: what the frame before faded stays at
// hand, the rest goes.
func fadeTurn() {
	if fadedFor != fade {
		clear(fadedRows)
		clear(fadedRowsOld)
		fadedFor = fade
	}
	fadedRowsOld, fadedRows = fadedRows, fadedRowsOld
	if fadedRows == nil {
		fadedRows = map[string]string{}
	}
	clear(fadedRows)
}

// fadeRow is s with its text colours faded back.
func fadeRow(s string) string {
	if !strings.Contains(s, "\x1b[") {
		return s
	}
	if f, ok := fadedRows[s]; ok {
		return f
	}
	f, ok := fadedRowsOld[s]
	if !ok {
		f = fadeRowNow(s)
	}
	if fadedRows != nil {
		fadedRows[s] = f
	}
	return f
}

func fadeRowNow(s string) string {
	var b strings.Builder
	b.Grow(len(s) + 16)
	for {
		i := strings.Index(s, "\x1b[")
		if i < 0 {
			b.WriteString(s)
			return b.String()
		}
		j := i + 2
		for j < len(s) && (s[j] >= '0' && s[j] <= '9' || s[j] == ';' || s[j] == ':') {
			j++
		}
		if j == len(s) || s[j] != 'm' {
			b.WriteString(s[:j])
			s = s[j:]
			continue
		}
		b.WriteString(s[:i])
		b.WriteString(fadeCode(s[i : j+1]))
		s = s[j+1:]
	}
}

// fadeCode is one style code on the faded side: its text colour faded,
// and the terminal's own text (a reset, or 39) in the faded text colour.
func fadeCode(code string) string {
	if f, ok := faded[code]; ok {
		return f
	}
	ps := strings.Split(code[2:len(code)-1], ";")
	var out []string
	reset := false
	for i := 0; i < len(ps); i++ {
		switch n := ps[i]; {
		case n == "" || n == "0":
			out, reset = append(out, "0"), true
		case n == "39":
			reset = true
		case n == "2":
			// faint is the terminal's; the fade is already here
		case (n == "38" || n == "48") && i+4 < len(ps) && ps[i+1] == "2":
			if n == "38" {
				c := theme.Mix(theme.RGB{R: atoi8(ps[i+2]), G: atoi8(ps[i+3]), B: atoi8(ps[i+4])}, painted.BG, fadeBy)
				out = append(out, fmt.Sprintf("38;2;%d;%d;%d", c.R, c.G, c.B))
			} else {
				out = append(out, ps[i:i+5]...)
			}
			i += 4
		default:
			out = append(out, n)
		}
	}
	f := "\x1b[" + strings.Join(out, ";") + "m"
	if len(out) == 0 {
		f = ""
	}
	if reset {
		f += fade
	}
	faded[code] = f
	return f
}

func atoi8(s string) uint8 {
	n, _ := strconv.Atoi(s)
	return uint8(min(255, max(0, n)))
}

func (m *Model) twoSided() bool {
	return m.listW > 0 && (m.host != nil || (m.live != nil && m.focused() != nil && m.live.key == m.focused().Key))
}

// paneRow writes a Session row w wide, faded when bg (fade) is set.
func (m *Model) paneRow(b *strings.Builder, p string, w int, bg string) {
	if bg != "" {
		p = fadeRow(p)
		b.WriteString(bg)
	}
	fitTo(b, p, w, bg)
	if bg != "" {
		b.WriteString(reset)
	}
}

// sessionFocused is whether the keys go to the Session: a rush
// conversation, or typing into a Claude Code screen.
func (m *Model) sessionFocused() bool { return m.paneFocus || m.embedded }

// divider leans orange toward the side with focus.
// divider is the edge between Agents and the Session. It stays a quiet
// line (the focused side shows focus itself), a shade brighter when the
// mouse is on it; the pointer turns into a resize arrow there too.
func (m *Model) divider() string {
	if m.dragging || m.divHover {
		return paint(cSub, "│")
	}
	if m.host != nil && m.host.sess != nil {
		return paint(harnessBorder(sessionAgent(m.host)), "│")
	}
	return faint("│")
}

// claudeStrip heads a Claude Code agent's Session with its two views, the
// way a rush session's header carries conversation and overview.
func (m *Model) claudeStrip(w int) string {
	a := m.focused()
	live := a != nil && m.live != nil && m.live.key == a.Key && m.live.ready.Load()
	tab := func(name string, on, avail bool) string {
		switch {
		case on:
			return bgTabOn + paint(cText+bold, " "+name+" ") + reset + bgChrome
		case !avail:
			return faint(" " + name + " ")
		}
		return paint(cSub, " "+name+" ")
	}
	screenOn := m.claudeView == 0 && live
	left := "  " + tab("screen", screenOn, live) + " " + tab("summary", !screenOn, true)
	right := ""
	switch {
	case m.embedded:
		right = paint(cOrange+bold, "typing into it") + dim(" · ctrl+] comes back")
	case screenOn:
		right = dim("typing goes into it · ctrl+] leaves · ctrl+f full screen")
	case !live && a != nil && a.Interactive:
		right = dim(a.Where())
	case !live:
		right = dim("not running · enter resumes it")
	}
	left = withTabHint(left, "v", "views", "", w-cellw.String(right+" ")-2)
	return onBg(bgChrome, spread(left, right+" ", w), w)
}

// rowInset is each step a row sits in under a project, then a worktree.
const rowInset = 2

// Column widths on the right of a row.
const (
	wAct  = 10
	wCPU  = 6
	wRAM  = 7
	wTok  = 7
	wCost = 8
	wAge  = 5
)

// listPreview is the Agents list as it would look now, for a setting that
// changes it: its header and first rows of your own agents, drawn as the
// list draws them, at the list's width or w when that's less.
func (m *Model) listPreview(w, rows int) []string {
	listW, _ := m.widths()
	if listW == 0 {
		listW = m.w
	}
	pw := min(listW, w)
	scroll, keys := m.scroll, m.rowKeys
	m.stackFor = listW
	out := append([]string{m.columnHeader(pw)}, m.listLines(pw, rows)...)
	m.stackFor = 0
	m.scroll, m.rowKeys = scroll, keys
	for len(out) < rows+1 {
		out = append(out, "")
	}
	return out
}

func (m *Model) listLines(w, h int) []string {
	nameCol := m.nameColumn(w)
	two := m.stacked(w, nameCol)
	var all, keys []string
	var cont []bool // a stacked row's second line, which the list never starts on
	selTop, selBottom := -1, -1
	wash := "" // the ground of the agent row being drawn, when it wants you
	emit := func(line, key string, sel bool) {
		if m.broadcast[key] && line != "" {
			line = paint(cGreen, "✓") + line[1:]
		}
		if wash != "" && key != "" {
			if sel && !m.broadcast[key] {
				line = paint(cOrange, "▍") + line[1:]
			}
			line = wash + strings.ReplaceAll(fit(line, w), reset, reset+wash) + reset
		} else if sel {
			if selTop < 0 {
				selTop = len(all)
			}
			selBottom = len(all)
			if !m.broadcast[key] {
				line = paint(cOrange, "▍") + line[1:]
			}
			line = highlight(line, w)
		} else if key != "" && key == m.hover {
			line = hoverLine(line, w)
		}
		all = append(all, line)
		keys = append(keys, key)
		cont = append(cont, false)
	}
	tag := "" // a worktree's, beside its one agent's name instead of heading it
	for i, l := range m.lines {
		switch l.kind {
		case lineSection:
			key := sectionKey(l.title)
			emit(m.sectionLine(l, w), key, key == m.sel)
		case lineProject:
			for _, ln := range strings.Split(m.projectLine(l, w), "\n") {
				emit(ln, "", false)
			}
		case lineTree:
			if soloTree(m.lines, i) { // one agent: it stays under its project, the worktree a tag
				tag = m.treeTag(l)
				continue
			}
			emit(m.treeLine(l, w), "", false)
		case lineBlank:
			emit("", "", false)
		case lineAgent:
			sel := l.agent.Key == m.sel
			inset := l.inset
			if tag != "" {
				inset -= rowInset // under its project, as the heading would have been
			}
			pad := strings.Repeat(" ", max(0, inset-1)) // a column in from the ▍
			rw := w - len(pad)
			if _, ok := m.roomOf(l.agent); ok {
				if strings.HasPrefix(l.agent.Key, roomKeyPrefix) {
					emit(pad+m.roomLine(l.agent, rw, sel), l.agent.Key, sel)
				} else {
					emit(pad+m.roomMemberLine(l.agent, rw, sel), l.agent.Key, sel)
				}
				tag = ""
				continue
			}
			wash = m.rowWash(l.agent, sel)
			emit(pad+m.agentLine(l.agent, rw, w, sel, nameCol-len(pad), two, tag), l.agent.Key, sel)
			tag = ""
			if two && (m.saysWhy(l.agent) || m.hasFilterMatch(l.agent)) {
				emit(pad+m.agentSub(l.agent, rw), l.agent.Key, sel)
				cont[len(cont)-1] = true
			}
			wash = ""
		}
	}
	if len(m.order) == 0 {
		msg := "  No agents yet. Describe a task below to start one."
		if !m.fleetRead {
			msg = "  Reading your agents…"
		}
		if f := m.listFilter; f != nil && len(f.query) > 0 {
			msg = fmt.Sprintf("  No agents match %q.", string(f.query))
			if f.search != nil && !f.done {
				msg = fmt.Sprintf("  No agents match %q yet — still searching…", string(f.query))
			}
		}
		all = append(all, "", dim(msg))
		keys = append(keys, "", "")
		cont = append(cont, false, false)
	}
	// Scrolling up shows the whole row above; down and to the end, a row
	// cut in half at the top gives up its second line instead.
	if selTop >= 0 {
		if selTop-1 < m.scroll {
			m.scroll = max(0, selTop-1)
			if cont[m.scroll] {
				m.scroll--
			}
		}
		if selBottom+1 >= m.scroll+h {
			m.scroll = selBottom + 2 - h
		}
	}
	if m.scroll > len(all)-h {
		m.scroll = max(0, len(all)-h)
	}
	if m.scroll < len(cont) && cont[m.scroll] {
		m.scroll++
	}
	end := min(len(all), m.scroll+h)
	out := append([]string(nil), all[m.scroll:end]...)
	m.rowKeys = keys[m.scroll:end]
	if end < len(all) && len(out) > 0 {
		out[len(out)-1] = faint(fmt.Sprintf("  ↓ %d more lines", len(all)-end))
		m.rowKeys[len(out)-1] = ""
	}
	return out
}

// View tabs are pills: the current one filled orange, the rest a quiet grey.
const tabOn = "\x1b[1;38;2;24;22;20;48;2;217;119;87m"

var tabOff, hoverBG string

// askBG and errBG wash a row that wants you: a question, or an error;
// the Sel ones when it's the row picked.
var askBG, askSelBG, errBG, errSelBG string

// rowWash is the ground a row that wants you is drawn on, picked or not;
// none for the rest.
func (m *Model) rowWash(a *fleet.Agent, sel bool) string {
	var bg, on string
	switch {
	case a.Halted():
		bg, on = errBG, errSelBG
	case a.YourTurn(m.snap.At) || a.JustFinished(m.snap.At):
		return ""
	case a.NeedsYou() || a.Waiting() || a.State == "blocked":
		bg, on = askBG, askSelBG
	}
	if sel {
		return on
	}
	return bg
}

func hoverLine(line string, w int) string {
	line = fit(line, w)
	return hoverBG + strings.ReplaceAll(line, reset, reset+hoverBG) + reset
}

func (m *Model) sectionLine(l listLine, w int) string {
	arrow := faint("▾ ")
	if l.folded {
		arrow = faint("▸ ")
	}
	meta := l.meta
	if l.title == needsSection && m.sharedContext() != "" {
		meta += "  ·  " + m.sharedContext()
	}
	if l.folded {
		head := arrow + paint(cSub+bold, l.title) + "  " + dim(meta)
		room := w - cellw.String(head) - 6
		if room >= 20 && l.peek != "" { // less is a word or two cut off
			head += "   " + faint(fit(l.peek, room))
		}
		return " " + head
	}
	return " " + arrow + rule(l.title, meta, w-5)
}

// cardLines draw the focused row's details as a box: what it is doing now
// in the title, its latest words inside, and its numbers in a footer.
func (m *Model) cardLines(a *fleet.Agent, w int) []string {
	bodyLines := m.dockLines()
	p := m.previews[a.Key].p
	inner := w - 6
	edge := func(s string) string { return paint(cDim, s) }
	line := func(s string) string { return edge("│") + panel("  "+fit(s, inner)+"  ") + edge("│") }

	title := ""
	switch {
	case a.Live() && p.Tool != "":
		arg := oneLine(tildify(p.ToolArg))
		title = paint(cOrange, "● ") + paint(cText+bold, p.Tool) + "  " + paint(cSub, ansi.Truncate(arg, max(10, inner-len(p.Tool)-8), "…"))
	case a.State == "blocked":
		title = paint(cYellow+bold, "waiting on you")
	case a.Busy():
		title = paint(cOrange, "◌ ") + paint(cText, "background work still running")
	case a.Live():
		title = paint(cOrange, "● ") + paint(cText, "working…")
	case a.PID != 0:
		title = dim("idle · still in memory")
	default:
		title = dim("finished " + age(a.Age(m.snap.At)) + " ago")
	}
	tw := cellw.String(title)
	top := edge("╭─ ") + title + edge(" "+strings.Repeat("─", max(0, w-tw-5))+"╮")

	var body []string
	text := p.Text
	if !a.Live() && strings.TrimSpace(a.Detail) != "" && a.Detail != "stopped" {
		text = a.Detail
	}
	if a.State == "blocked" && a.Needs != "" {
		text = a.Needs
	}
	if text == "" {
		text = "…"
	}
	text = mdPlain.Replace(oneLine(text))
	var lines []string
	for _, l := range wrap(text, inner) {
		if strings.TrimSpace(l) != "" {
			lines = append(lines, l)
		}
	}
	for i, l := range lines {
		if i == bodyLines-1 && len(lines) > bodyLines {
			body = append(body, paint(cText, ansi.Truncate(l, inner-24, "…"))+faint("   tab for the full preview"))
			break
		}
		body = append(body, paint(cText, l))
	}

	var cells []string
	if p.Context > 0 {
		f := ctxFill(a, int64(p.Context), agent.ContextWindow(agent.Kind(a.Kind), p.Model))
		of := " of " + tokens(f.Window)
		if f.Compacts() {
			of += " auto-compact"
		}
		cells = append(cells, dim("context ")+ctxBar(f.Pct())+" "+paint(cText, fmt.Sprintf("%.0f%%", f.Pct()))+dim(of))
	}
	u := a.Spend.Usage
	if a.Spend.Cost > 0 {
		cells = append(cells, dim("spent ")+paint(cText, money(a.Spend.Cost)))
		cells = append(cells, dim("out ")+paint(cSub, tokens(u.Output))+dim("  cache ")+paint(cSub, tokens(u.CacheRead)))
	}
	if a.PID != 0 && a.Procs > 0 {
		cells = append(cells, dim(fmt.Sprintf("%d procs ", a.Procs))+paint(cSub, mem(a.Mem)))
	}
	if !a.Live() {
		if el := a.Elapsed(m.snap.At); el > 0 {
			cells = append(cells, dim("ran ")+paint(cSub, dur(el)))
		}
	}
	if c := m.context(a); c != "" {
		cells = append(cells, faint(c))
	}

	for len(body) < bodyLines {
		body = append(body, "")
	}
	out := []string{top}
	for _, l := range body[:bodyLines] {
		out = append(out, line(l))
	}
	out = append(out, edge("├"+strings.Repeat("─", w-2)+"┤"))
	out = append(out, line(strings.Join(cells, edge("   │   "))))
	out = append(out, edge("╰"+strings.Repeat("─", w-2)+"╯"))
	return out
}

func ctxBar(pct float64) string {
	n := min(8, max(0, int(pct/12.5+0.5)))
	c := cSub
	switch {
	case pct >= 80:
		c = cRed
	case pct >= 50:
		c = cYellow
	}
	return paint(c, strings.Repeat("━", n)) + faint(strings.Repeat("━", 8-n))
}

// columnHeader names the list's columns; it stays put while the list scrolls.
// colWidths are the list's right-hand columns at width w. A narrow list
// drops the least useful first, so names keep their room: RUNNING and CPU
// under 96 columns, TOKENS under 76, RAM under 64, cost under 44 (the pane
// header has it).
// Age is TIME's with its two spaces, and a column you've hidden is 0.
func (m *Model) colWidths(w int) (act, cpu, ram, tok, cost, age int) {
	act, cpu, ram, tok, cost, age = wAct, wCPU, wRAM, wTok, wCost, wAge+2
	defer func() {
		c := m.store.Config
		for _, p := range []struct {
			col string
			w   *int
		}{{"running", &act}, {"cpu", &cpu}, {"ram", &ram}, {"tokens", &tok}, {"cost", &cost}, {"time", &age}} {
			if !c.Shows(p.col) {
				*p.w = 0
			}
		}
	}()
	if w < 96 {
		act, cpu = 0, 0
	}
	if w < 76 {
		tok = 0
	}
	if w < 64 {
		ram = 0
	}
	if w < 44 {
		cost = 0
	}
	return
}

func (m *Model) columnHeader(w int) string {
	if f := m.listFilter; f != nil {
		return m.listFilterHeader(f, w)
	}
	wAct, wCPU, wRAM, wTok, wCost, wTime := m.colWidths(w)
	nameCol := m.nameColumn(w)
	sortBy := m.store.Config.SortBy
	if sortBy == "" {
		sortBy = "name"
	}
	label := func(text, mode string) string {
		if mode == sortBy {
			return text + "▾"
		}
		return text
	}
	col := func(text, mode string, width int) string {
		s := right1(label(text, mode), width)
		if mode == sortBy {
			return paint(cSub+bold, s)
		}
		return dim(s)
	}
	name := label("AGENTS", "name")
	left := "  " + fit(name, nameCol+2)
	if m.twoSided() && !m.sessionFocused() {
		left = paint(cOrange, "▍") + " " + fit(name, nameCol+2)
	}
	if sortBy == "name" {
		left = "  " + paint(cSub+bold, fit(name, nameCol+2))
	} else {
		left = dim(left)
	}
	if !m.stacked(w, nameCol) {
		left += dim("LATEST")
	}
	rightW := wAct + wCPU + wRAM + wTok + wCost + wTime + 1
	cols := dim(right1("RUNNING", wAct)) + col("CPU", "cpu", wCPU) + col("RAM", "ram", wRAM) + col("TOKENS", "tokens", wTok) + col("COST", "cost", wCost) + col("TIME", "time", wTime) + " "
	gap := w - cellw.String(left) - rightW
	if gap < 1 {
		return fit(left, w)
	}
	return left + strings.Repeat(" ", gap) + cols
}

// listFilterHeader is the column header while the Agents view's filter
// (alt+f) is open: what's typed, how many agents match, and whether their
// transcripts are still being searched.
func (m *Model) listFilterHeader(f *listFilterState, w int) string {
	label := "  ⌕ filter"
	if len(f.query) > 0 {
		label = "  ⌕ filter: " + string(f.query)
	}
	n := 0
	for _, l := range m.lines {
		if l.kind == lineAgent {
			n++
		}
	}
	meta := fmt.Sprintf("%d agent", n)
	if n != 1 {
		meta += "s"
	}
	if len(f.query) > 0 && f.search != nil && !f.done {
		meta += " · searching…"
	}
	left := paint(cSub+bold, fit(label, max(0, w-cellw.String(meta)-4)))
	gap := w - cellw.String(left) - cellw.String(meta) - 1
	if gap < 1 {
		return fit(left, w)
	}
	return left + strings.Repeat(" ", gap) + dim(meta) + " "
}

// nameColumn is where summaries start: wide enough for most names, never
// more than two fifths of the row.
func (m *Model) nameColumn(w int) int {
	widest := 0
	for _, l := range m.lines {
		if l.kind != lineAgent {
			continue
		}
		n := cellw.String(oneLine(l.agent.DisplayName))
		if b := cellw.String(ansi.Strip(m.badges(l.agent, false))); b > 0 {
			n += b + 1
		}
		widest = max(widest, n)
	}
	if w < 96 {
		// A narrow list is mostly names: give them what the columns leave.
		act, cpu, ram, tok, cost, age := m.colWidths(w)
		return max(12, min(widest, w-4-act-cpu-ram-tok-cost-age-3))
	}
	return max(20, min(widest, (w-30)*2/5))
}

// stacked is when the list is too narrow for a readable summary beside each
// name, or takes no more of the screen than Config.StackAt: rows take two
// lines then, the summary hung under the name.
func (m *Model) stacked(w, nameCol int) bool {
	share := w
	if m.stackFor > 0 {
		share = m.stackFor // a preview, narrower than the list it shows
	}
	if at := m.store.Config.StackPercent(); at > 0 && share*100 <= m.w*at {
		return true
	}
	act, cpu, ram, tok, cost, age := m.colWidths(w)
	return w-3-(act+cpu+ram+tok+cost+age+1)-nameCol-2 < 20
}

// agentSub is a stacked row's second line: its summary, or where it works
// when it has nothing to say, hung from the name above so the two read as one.
func (m *Model) agentSub(a *fleet.Agent, w int) string {
	room := w - 5
	if snip, ok := m.filterSnippet(a, room); ok {
		return "  " + faint("╰ ") + snip
	}
	summary, col, justDone := m.rowSummary(a)
	text := paint(col, tidyFit(summary, room))
	switch {
	case justDone:
		text = paint(cGreen, "just finished") + faint(" · ") + paint(col, tidyFit(summary, room-16))
	case summary == "":
		text = faint(tidyFit(m.context(a), room))
	}
	if by := subLineFade(m.store.Config.SubLine); by > 0 {
		text = fadeText(text, by)
	}
	return "  " + faint("╰ ") + text
}

// subLineFade is how far toward the background the second line goes, from
// Settings' transparency: "50%" is half way. The old dim and faint are
// half and three quarters.
func subLineFade(v string) float64 {
	switch v {
	case "dim":
		return 0.5
	case "faint":
		return 0.75
	}
	n, err := strconv.Atoi(strings.TrimSuffix(v, "%"))
	if err != nil {
		return 0
	}
	return float64(min(90, max(0, n))) / 100
}

var sgrFG = regexp.MustCompile(`38;2;(\d+);(\d+);(\d+)`)

// fadeText is s with each of its text colours by of the way to the
// background, so a state's colour stays itself, only quieter.
func fadeText(s string, by float64) string {
	return sgrFG.ReplaceAllStringFunc(s, func(code string) string {
		p := sgrFG.FindStringSubmatch(code)
		c := theme.Mix(theme.RGB{R: atoi8(p[1]), G: atoi8(p[2]), B: atoi8(p[3])}, painted.BG, by)
		return fmt.Sprintf("38;2;%d;%d;%d", c.R, c.G, c.B)
	})
}

// agentLine is the first line of a row: marker, name, badges, figures, and
// the summary too unless the row is stacked. listW is the whole list's
// width, which says which columns there are, as the header does: a row
// inset under its project is narrower than the header it sits under.
// soloTree is whether the tree line at i heads a single agent.
func soloTree(lines []listLine, i int) bool {
	n := 0
	for _, l := range lines[i+1:] {
		if l.kind != lineAgent {
			break
		}
		n++
	}
	return n == 1
}

func (m *Model) agentLine(a *fleet.Agent, w, listW int, sel bool, nameCol int, stacked bool, tag string) string {
	wAct, wCPU, wRAM, wTok, wCost, wTime := m.colWidths(listW)
	now := m.snap.At
	live := a.Live()
	marker := " "

	switch {
	case a.Checking:
		marker = paint(cSub, "◔")
	case a.Halted():
		marker = paint(cRed, "✗")
	case a.YourTurn(now):
		marker = paint(cGreen, "◆")
	case a.JustFinished(now):
		marker = paint(cGreen, "✓")
	case a.NeedsYou():
		marker = paint(cYellow+bold, "?") // a question: what to look at first
	case a.Waiting():
		marker = paint(cYellow, "?")
	case live:
		marker = paint(cOrange, convo.Spin(a.Kind, m.tick+len(a.ID)))
	case a.Busy():
		marker = paint(cOrange, "◌")
	case a.Done:
		marker = paint(cGreen, "✓")
	case a.PID != 0:
		marker = dim("◦")
	default:
		marker = faint("·") // stopped: every row has a marker, so none reads as missing
	}

	busy := a.Busy()
	resident := !live && a.PID != 0
	act := m.activity(a)
	if wAct == 0 {
		act = ""
	}
	cpuCell := func() string {
		if a.PID == 0 {
			return strings.Repeat(" ", wCPU)
		}
		v := right1(fmt.Sprintf("%.0f%%", a.CPU), wCPU)
		switch {
		case a.CPU >= 50:
			return paint(cYellow, v)
		case a.CPU >= 5:
			return paint(cSub, v)
		default:
			return faint(v)
		}
	}
	ramCell := func(active bool) string {
		if a.PID == 0 {
			return strings.Repeat(" ", wRAM)
		}
		v := right1(mem(a.Mem), wRAM)
		switch {
		case a.Mem >= 4<<30:
			return paint(cYellow, v)
		case active:
			return dim(v)
		default:
			return faint(v)
		}
	}
	tokCell := func(active bool) string {
		if a.Spend.Context <= 0 {
			return strings.Repeat(" ", wTok)
		}
		v := right1(tokens(a.Spend.Context), wTok)
		f := ctxFill(a, a.Spend.Context, agent.ContextWindow(agent.Kind(a.Kind), a.Spend.Model))
		switch {
		case f.Window > 0 && f.Pct() >= 80:
			return paint(cYellow, v)
		case active:
			return dim(v)
		default:
			return faint(v)
		}
	}
	var right string
	switch {
	case live || busy:
		right = act + cpuCell() + ramCell(true) + tokCell(true) + costCell(a.Spend.Cost, wCost)
	case resident:
		right = act + faint(right1(fmt.Sprintf("%.0f%%", a.CPU), wCPU)) + ramCell(false) + tokCell(false) + faint(right1(moneyShort(a.Spend.Cost), wCost))
	default:
		cost := moneyShort(a.Spend.Cost)
		if cost == "–" {
			cost = ""
		}
		right = act + blanks(wCPU+wRAM) + tokCell(false) + faint(right1(cost, wCost))
	}
	switch {
	case wTime == 0:
		right += " "
	case a.Stuck(now):
		right += paint(cYellow, right1(age(a.Quiet(now)), wTime)) + " " // how long it's been silent
	case live && !a.NeedsYou() && !a.Waiting():
		right += dim(right1(dur(m.runFor(a, now)), wTime)) + " " // this run's, not the session's
	case m.groupOf[a.Key] == needsSection:
		waited, col := a.Age(now), cText
		if waited >= 30*time.Minute {
			col = cYellow
		}
		right += paint(col, right1(age(waited), wTime-2)) + " " + cacheWheel(a.Quiet(now)) + " " // the wheel in the column's gutter
	default: // how long it has waited
		right += faint(right1(age(a.Age(now)), wTime)) + " "
	}

	// Bold is for what wants you: the selection and an agent that needs you.
	// Working rows are bright, not bold, so six of them don't drown one.
	nameColor := cSub
	switch {
	case a.YourTurn(now) && !sel:
		nameColor = cGreen + bold // stands out among the working and idle it shares a section with
	case sel || a.NeedsYou() || a.Waiting() || a.Halted():
		nameColor = cText + bold
	case live || busy:
		nameColor = cText
	case a.Pinned:
		nameColor = cText
	default:
		nameColor = cDim // idle, stopped and done step back behind the working
	}
	name := oneLine(a.DisplayName)
	short := m.badges(a, false)
	badges := short
	if ps := m.pluginStatus(a.Key); ps != "" {
		badges = strings.TrimSpace(badges + " " + ps)
	}
	if wAct == 0 {
		// No RUNNING column: its subagents and shells go by the name.
		badges = strings.TrimSpace(badges + " " + strings.TrimSpace(m.activity(a)))
	}
	summary, sumColor, justDone := m.rowSummary(a)
	room := w - 3 - cellw.String(right)
	// Reserve room for identity before shortening a long session title.
	nameRoom := room
	withSummary := (summary != "" && m.saysWhy(a) || m.hasFilterMatch(a)) && !stacked
	full, extra := "", 0
	if sel || a.Key == m.hover {
		full = m.badges(a, true)
		extra = cellw.String(full) - cellw.String(short)
	}
	// The full badge takes its width from the gap before the summary first.
	grew := extra > 0 && withSummary && room-nameCol-extra-2 > 8
	if grew {
		nameCol += extra
	}
	if withSummary && room-nameCol-2 > 8 {
		nameRoom = min(nameRoom, nameCol)
	}
	if badges != "" {
		nameRoom -= cellw.String(badges) + 1
	}
	// Failing that, it never cuts a title that fits whole beside the icon.
	if full != "" && (grew || cellw.String(name) > nameRoom || cellw.String(name) <= nameRoom-extra) {
		if !grew {
			nameRoom -= extra
		}
		badges = strings.Replace(badges, short, full, 1)
	}
	left := paint(nameColor, cellw.Truncate(name, max(0, nameRoom), "…"))
	if badges != "" {
		left += " " + badges
	}
	if tag != "" { // its worktree, which has no heading of its own
		left += "  " + tag
	}
	if p := m.projTitles[folderKey(a)]; p != "" && byTime(m.groupOf[a.Key]) {
		left += "  " + faint(p) // what finished goes by time, without project headings
	}
	if withSummary {
		left = fit(left, nameCol)
		if sw := room - nameCol - 2; sw > 8 {
			if snip, ok := m.filterSnippet(a, sw); ok {
				left += "  " + snip
			} else if justDone {
				left += "  " + paint(cGreen, "just finished") + faint(" · ") + paint(sumColor, fit(summary, sw-16))
			} else {
				left += "  " + paint(sumColor, fit(summary, sw))
			}
		}
	}
	return " " + marker + " " + fit(left, room) + right
}

// saysWhy is whether a row's summary earns its place: a question, a halt
// or a stuck agent says what it wants; anything else is only its name.
func (m *Model) saysWhy(a *fleet.Agent) bool {
	return asks(a) || a.Stuck(m.snap.At)
}

// cacheWheel is how much of the prompt cache's hour is left since the
// agent last wrote, as a wheel ticking round; ⊘ once it has gone, when a
// reply re-reads the whole conversation at full price.
func cacheWheel(quiet time.Duration) string {
	switch left := 1 - float64(quiet)/float64(cacheLife); {
	case left > 0.8:
		return paint(cGreen, "●")
	case left > 0.6:
		return paint(cGreen, "◕")
	case left > 0.4:
		return paint(cYellow, "◑")
	case left > 0.2:
		return paint(cOrange, "◔")
	case left > 0:
		return paint(cRed, "○")
	default:
		return faint("⊘")
	}
}

// rowSummary is the latest thing an agent said or is doing, the colour to
// say it in, and whether it has only just finished.
func (m *Model) rowSummary(a *fleet.Agent) (summary, sumColor string, justDone bool) {
	now := m.snap.At
	live, busy := a.Live(), a.Busy()
	sumColor = cDim
	switch {
	case a.Checking:
		summary, sumColor = "turn ended · checking…", cDim
	case a.Halted():
		summary, sumColor = "stopped · "+oneLine(a.HaltReason()), cRed
	case a.YourTurn(now) && !a.JustFinished(now):
		summary, sumColor = oneLine(a.Detail), cSub
		if summary == "" {
			summary = "finished its turn"
		}
	case a.JustFinished(now):
		summary, sumColor = oneLine(a.Detail), cDim
		justDone = true
	case a.Waiting():
		summary, sumColor = oneLine(a.Needs), cSub
		if summary == "" {
			summary = oneLine(a.Detail)
		}
	case a.State == "blocked":
		summary, sumColor = oneLine(a.Needs), cYellow
		if summary == "" {
			summary = oneLine(a.Detail)
		}
	case live:
		summary, sumColor = oneLine(a.Detail), cDim
		if p := m.previews[a.Key].p; summary == "" && p.Doing != "" {
			summary = oneLine(p.Doing)
		}
		if summary == "" && a.Interactive && !a.Headless {
			summary = "working in a terminal"
		}
		if summary == "" {
			summary, sumColor = "working…", cDim
		}
	case busy:
		summary = backgroundText(a)
	default:
		if f := m.focused(); m.preview || f == nil || f.Key != a.Key {
			summary = oneLine(a.Detail)
		}
		sumColor = cFaint // resting: its last words are there when you look
	}
	if summary == "stopped" {
		summary = ""
	}
	if a.Stuck(now) {
		summary, sumColor = "silent "+age(a.Quiet(now))+" · ctrl+b asks why · "+summary, cYellow
	}
	summary = mdPlain.Replace(summary)
	return
}

// sharedContext is the repository and branch every live agent shares, shown
// once in the section header instead of on each row.
func (m *Model) sharedContext() string {
	shared := ""
	for _, a := range m.order {
		if !a.Live() || a.Interactive {
			continue
		}
		c := m.context(a)
		if shared == "" {
			shared = c
		} else if c != shared {
			return ""
		}
	}
	return shared
}

// dirLabel names a folder by its repository, not a branch: the agent
// decides at the start whether it works there or in a new worktree.
func (m *Model) dirLabel(dir string) string {
	for _, a := range m.snap.Agents {
		if a.Cwd == dir && a.Repo == dir {
			return paint(cText, filepath.Base(a.Repo))
		}
	}
	return paint(cText, shortDir(tildify(dir), 48))
}

// shortDir is a path in at most n cells, its middle left out, so the
// folder's own name always shows: ~/…/langwatch/.worktrees/go-ports.
func shortDir(p string, n int) string {
	if cellw.String(p) <= n {
		return p
	}
	parts := strings.Split(p, "/")
	tail := parts[len(parts)-1]
	for i := len(parts) - 2; i > 0; i-- {
		if cellw.String(parts[0]+"/…/"+parts[i]+"/"+tail) > n {
			break
		}
		tail = parts[i] + "/" + tail
	}
	return ansi.Truncate(parts[0]+"/…/"+tail, n, "…")
}

// context is where a live agent works: repository and branch.
func (m *Model) context(a *fleet.Agent) string {
	return locationSummary(a)
}

// activity is the small column of what an agent is running beside itself:
// subagents (orange, +nested) and background shells.
func (m *Model) activity(a *fleet.Agent) string {
	var parts []string
	if sub := a.Subs; sub.Direct+sub.Nested > 0 {
		v := fmt.Sprintf("↳%d", sub.Direct)
		if sub.Nested > 0 {
			v += fmt.Sprintf("+%d", sub.Nested)
		}
		parts = append(parts, paint(cOrange, v))
	}
	shells := 0
	if a.PID != 0 {
		for _, t := range a.Running {
			if t.Kind == "shell" {
				shells++
			}
		}
	}
	if shells > 0 {
		parts = append(parts, dim(fmt.Sprintf("▸%d", shells)))
	}
	return right1(strings.Join(parts, " "), wAct)
}

// backgroundText says what a finished agent is still waiting on.
func backgroundText(a *fleet.Agent) string {
	kinds := map[string]int{}
	var first string
	for _, b := range a.Background {
		k, label, _ := strings.Cut(b, "\x00")
		kinds[k]++
		if first == "" {
			first = oneLine(label)
		}
	}
	var parts []string
	for _, k := range []string{"shell", "agent", "monitor", "task"} {
		if n := kinds[k]; n > 0 {
			name := map[string]string{"shell": "shell", "agent": "subagent", "monitor": "monitor", "task": "task"}[k]
			if n > 1 {
				name += "s"
			}
			parts = append(parts, fmt.Sprintf("%d %s", n, name))
		}
	}
	if n := a.Subs.Direct + a.Subs.Nested; len(parts) == 0 && n > 0 {
		name := "subagent"
		if n > 1 {
			name += "s"
		}
		parts = append(parts, fmt.Sprintf("%d %s", n, name))
	}
	if len(parts) == 0 {
		parts = append(parts, fmt.Sprintf("%d tasks", a.InFlight))
	}
	s := "background · " + strings.Join(parts, ", ") + " running"
	if first != "" {
		s += " · " + first
	}
	return s
}

func right1(s string, w int) string { return right(s, w) }

// costCell is a working agent's spend: quiet, yellow from $100 the way cpu
// is from 50%.
func costCell(cost float64, w int) string {
	v := right1(moneyShort(cost), w)
	if cost >= 100 {
		return paint(cYellow, v)
	}
	return dim(v)
}

func (m *Model) badges(a *fleet.Agent, full bool) string {
	var parts []string
	actual := a
	if c := m.host; c != nil && c.key == a.Key && c.sess != nil {
		copy := *a
		copy.Kind = string(sessionAgent(c))
		actual = &copy
	}
	if b := rowBadge(actual, full); b != "" {
		// Always name the harness; no provider is an implicit default.
		parts = append(parts, b)
	}
	headed := len(a.PRs) > 0 && m.store != nil && m.splitProjects() && !byTime(m.groupOf[a.Key]) // its heading names its PR
	for i, pr := range a.PRs {
		if headed {
			break
		}
		if i == 2 {
			parts = append(parts, dim(fmt.Sprintf("+%d", len(a.PRs)-2)))
			break
		}
		parts = append(parts, paint(prColor(pr), fmt.Sprintf("#%d", pr.Number)))
	}
	return strings.Join(parts, " ")
}

// prColor is a pull request's state as a colour: open green, draft quiet,
// merged blue, closed dim, red while its checks fail.
func prColor(pr agent.PR) string {
	switch {
	case pr.State == "MERGED":
		return cBlue
	case pr.State == "CLOSED":
		return cDim
	case pr.Checks.Failed > 0:
		return cRed
	case pr.State == "DRAFT":
		return cSub
	}
	return cGreen
}

// noPrompt is when there's no Prompt box: zen answers in the Session's own
// box, and a Session filling a narrow screen has its own box too, so there
// are never two boxes on screen at once.
func (m *Model) noPrompt() bool {
	if m.zenFull() || m.hostedAlone() {
		return true
	}
	l, _ := m.widths()
	return m.host != nil && l == 0 && (m.preview || m.full) && m.mode == modeList
}

// promptBoxAt is the Prompt's box at width w, before its labels.
func (m *Model) promptBoxAt(w int) box {
	b := box{w: w, focused: !m.sessionFocused(), text: m.input, cursor: m.cursorPos(), anchor: m.anchor - 1,
		lead: paint(cOrange, "❯ "), maxRows: min(6, max(1, m.h-m.topH()-4-5)), top: m.promptTop}
	if m.sessionFocused() {
		b.text = nil
	} else if m.chipHot.box == 2 {
		b.hot = m.chipHot.at
	}
	return b.named(m.imgs)
}

// promptH is len(m.promptLines(w)), without drawing them.
func (m *Model) promptH(w int) int {
	if m.noPrompt() {
		return 0
	}
	n := 2 + m.promptBoxAt(w).rows() + 1
	if cmds, _ := m.promptPicker(); len(cmds) > 0 && !m.pickerOverCard() {
		n += min(len(cmds), 6) + 1 // the picker
	}
	return n
}

// pickerOverCard is whether the Prompt's picker is drawn over Getting
// started, the rows at the foot of the list, rather than above the box.
func (m *Model) pickerOverCard() bool {
	return m.cardShown && m.listW > 0 && m.showCard()
}

// promptLines are the input box and a row of key hints. The box's top
// edge says where the text goes and what enter will do; the agent pane's
// own input is the same box, so the two always read alike.
func (m *Model) promptLines(w int) []string {
	if m.noPrompt() {
		return nil
	}
	a := m.selected()
	text := string(m.input)
	b := m.promptBoxAt(w).scrolled()
	m.promptTop = b.top
	m.shelfHits = m.shelfHits[:0]
	// An empty box with nothing asked of it shows no cursor: the list has
	// the keys until you type, rename, or reply.
	b.idle = len(m.input) == 0 && m.inKind == inPrompt
	if m.inKind == inPrompt {
		b.footR = m.boxNote("")
	}
	switch {
	case m.inKind == inRename && a != nil:
		b.topL = dim("rename ") + paint(cText, oneLine(a.DisplayName)) + dim(" · enter saves")
		b.holder = "a new name · empty resets it"
	case m.inKind == inGroup && a != nil:
		b.topL = dim("group for ") + paint(cText, oneLine(a.DisplayName)) + dim(" · enter saves")
		b.holder = "a group name · empty clears it"
	case m.inKind == inReply && a != nil && !a.Rush:
		b.topL = dim("to ") + paint(cText, ansi.Truncate(oneLine(a.DisplayName), 32, "…"))
		if busy(a) {
			// Say it where the message is typed: enter queues, it doesn't send.
			b.topL += dim(" · working, so ") + paint(cOrange, "enter queues")
		}
		b.holder = "a message for this agent · esc leaves reply mode"
	case typingHash(text):
		b.topL = dim("rush command · enter runs it")
		b.topR = m.newPreview(text) // what #new starts, or why it can't
	default:
		dir := m.startDir()
		b.topL = dim("new agent in ") + m.dirLabel(dir)
		b.holder = "describe a task"
		// What it starts as, so a model or profile is never a surprise,
		// beside the others it could: ⌥1…9 or a click takes one.
		room := w - cellw.String(b.topL) - 10
		line, hits := m.shelfLine(dir, room)
		b.topR = line
		if x0 := w - 3 - cellw.String(line); cellw.String(line) <= room {
			for _, h := range hits {
				m.shelfHits = append(m.shelfHits, shelfHit{x0 + h.x0, x0 + h.x1, h.o})
			}
		}
	}
	if m.sessionFocused() {
		// The Session has the keys; this box waits, and says how back.
		b.holder = "esc or ← to come back here"
		if m.embedded {
			b.holder = "ctrl+] or click here to come back"
		}
	}
	var out []string
	if !m.pickerOverCard() {
		out = m.fleetSlashLines(w)
	}
	m.promptBox, m.promptBoxIdx = b, len(out)
	out = append(out, b.lines()...)
	// One row of keys for what you can do right now; ? has the rest.
	var hint string
	needs := m.needingYou()
	switch {
	case m.sessionFocused():
		// The keys are the Session's: only what reaches back here.
		var pairs []string
		if needs > 0 {
			pairs = append(pairs, "ctrl+n", needsLabel(needs))
		}
		back := "esc · ←"
		if m.embedded {
			back = "ctrl+]"
		}
		hint = keysFit(w-4, append(pairs, back, "back to Agents")...)
	case m.inKind == inReply:
		hint = keysFit(w-4, "enter", "send", "↑↓", "another agent", "esc", "done", "?", "guide")
	case m.inKind != inPrompt:
		hint = keysFit(w-4, "enter", "save", "esc", "cancel")
		if m.inKind == inRename {
			hint = keysFit(w-4, "enter", "save", "tab · ↑↓", "save, rename the next", "esc", "cancel")
		}
	case typingHash(text):
		hint = keysFit(w-4, "enter", "run it", "esc", "clear", "?", "guide")
	case len(m.input) > 0:
		pairs := []string{"enter", "start"}
		if key := m.boundKey("list.start"); key != "" {
			pairs = append(pairs, convo.KeyWord(key), "model")
		}
		if fk, fl := m.folderKey(a); fk != "" {
			pairs = append(pairs, fk, fl)
		}
		hint = fewKeys(w-4, 3, pairs, []string{"esc", "clear", "?", "more"})
	case m.peeking():
		back := "esc"
		if from := m.agentByKey(m.peekFrom); from != nil {
			back = "back to " + oneLine(from.DisplayName)
		}
		hint = keysFit(w-4, "↑↓", "pick", "enter · { }", "open it", "esc", back, "shift+← · #view split", "side by side", "?", "guide")
	default:
		// Most useful first: keysFit drops from the end, bar ? guide.
		open, rename := "enter", "ctrl+r"
		if m.store.Config.EnterOn != "open" {
			open, rename = "⌘↓", "enter"
		}
		var pairs []string
		switch {
		case a == nil:
		case a.Rush:
			pairs = append(pairs, open, "open")
		default:
			pairs = append(pairs, open, "open", "ctrl+o", "reply")
		}
		if needs > 0 {
			pairs = append(pairs, "ctrl+n", needsLabel(needs))
		}
		pairs = append(pairs, "ctrl+k", "go anywhere")
		if a != nil {
			pairs = append(pairs, rename, "rename")
		}
		if a != nil && (a.Halted() || a.YourTurn(m.snap.At)) {
			pairs = append([]string{m.boundKey("list.go"), a.ContinueText()}, pairs...)
		}
		if m.newer.Version != "" && !m.updating {
			pairs = append([]string{"#update", "new rush"}, pairs...)
		}
		if n := m.updatesCount(); n > 0 {
			pairs = append(pairs, fmt.Sprintf("%d updates", n), "in Settings")
		}
		if m.rebuilt {
			pairs = append([]string{"#reload", "a newer rush is installed"}, pairs...)
		}
		if m.hosted != "" {
			// Hosted's list: no command bar, and the way back to the session.
			var kept []string
			for i := 0; i+1 < len(pairs); i += 2 {
				if pairs[i] != "ctrl+k" {
					kept = append(kept, pairs[i], pairs[i+1])
				}
			}
			pairs = append([]string{"esc · ctrl+6", "hide Agents"}, kept...)
		}
		// With one side on screen, how to have both is kept in view.
		l, p := m.widths()
		back := "shift+←"
		if l == 0 {
			back = "shift+→"
		}
		if v := m.splitHint(back); v != nil && (l == 0 || p == 0) {
			pairs = append(pairs[:2:2], append(v, pairs[2:]...)...)
		}
		hint = keysFit(w-4, append(pairs, "?", "more")...)
	}
	if m.pastingImg || m.status != "" && m.snap.At.Sub(m.statusAt).Seconds() < 6 {
		hint = strings.TrimRight(m.statusOr(""), " ") // it pads to the screen, not this box
	} else {
		hint = "  " + hint
	}
	return append(out, fit(hint, w))
}

// previewLines is the rich preview: everything about one agent, in ruled
// sections, with the conversation taking whatever height is left.
func (m *Model) previewLines(w, h int) []string {
	a := m.focused()
	if a == nil {
		return []string{"", dim("select an agent to preview it")}
	}
	now := m.snap.At
	p := m.previews[a.Key].p
	section := func(t string) string { return rule(t, "", w) }
	var top, tail []string

	chip := paint(cDim, "finished")
	switch {
	case a.State == "blocked" && !a.Checking:
		chip = paint(cYellow+bold, "needs you")
	case a.Live():
		chip = paint(cOrange+bold, "working")
	case a.Busy():
		chip = paint(cOrange, "background work")
	case a.JustFinished(now):
		chip = paint(cGreen, "just finished")
	case a.PID != 0:
		chip = paint(cSub, "idle")
	}
	top = append(top, paint(cText+bold, oneLine(a.DisplayName))+"   "+chip)
	model := modelWord(a.Kind, firstNonEmpty(p.Model, a.Spend.Model))
	top = append(top, dim(strings.Join(nonEmpty(a.ID, a.Acct.Name, model, "updated "+age(a.Age(now))+" ago"), " · ")))
	loc := paint(cSub, tildify(a.Cwd))
	if a.Branch != "" {
		loc += faint(" · ") + dim(a.Branch)
	}
	top = append(top, loc)
	for _, pr := range a.PRs {
		col := cGreen
		if pr.Checks.Failed > 0 {
			col = cRed
		}
		top = append(top, paint(col, fmt.Sprintf("#%d", pr.Number))+dim(fmt.Sprintf(" %s · checks %d passed, %d failed, %d running", strings.ToLower(pr.State), pr.Checks.Passed, pr.Checks.Failed, pr.Checks.Pending)))
	}

	switch {
	case a.State == "blocked" && a.Needs != "":
		top = append(top, "", section("Needs you"))
		for _, l := range wrap(oneLine(a.Needs), w) {
			top = append(top, paint(cYellow, l))
		}
	case a.Live() && p.Tool != "":
		top = append(top, "", section("Now"))
		top = append(top, paint(cOrange, "● ")+paint(cText+bold, p.Tool)+"  "+dim(ansi.Truncate(oneLine(tildify(p.ToolArg)), w-len(p.Tool)-4, "…")))
	}

	if len(a.TodoItems) > 0 {
		tail = append(tail, "", section(fmt.Sprintf("Todos  %d/%d", a.TodosDone, a.Todos)))
		for i, t := range a.TodoItems {
			if i == 7 {
				tail = append(tail, faint(fmt.Sprintf("+%d more", len(a.TodoItems)-7)))
				break
			}
			switch {
			case t.Done:
				tail = append(tail, paint(cGreen, "☑ ")+faint(fit(t.Label, w-2)))
			case t.Started:
				tail = append(tail, paint(cOrange, "◐ ")+paint(cText, fit(t.Label, w-2)))
			default:
				tail = append(tail, faint("☐ ")+dim(fit(t.Label, w-2)))
			}
		}
	}
	if len(a.Running) > 0 && a.PID != 0 || a.Subs.Spawned > 0 {
		head := "Running"
		if s := a.Subs; s.Spawned > 0 {
			head += fmt.Sprintf("  %d subagents now · %d spawned", s.Direct+s.Nested, s.Spawned)
		}
		tail = append(tail, "", section(head))
		for i, t := range a.Running {
			if i == 5 || a.PID == 0 {
				break
			}
			icon, col := "▸ shell   ", cSub
			if t.Kind == "agent" {
				icon, col = "↳ subagent", cOrange
			} else if t.Kind == "monitor" {
				icon, col = "◎ monitor ", cDim
			}
			since := ""
			if t.StartedAt.Unix() > 0 {
				since = dur(now.Sub(t.StartedAt))
			}
			tail = append(tail, paint(col, icon)+" "+dim(fit(oneLine(tildify(t.Label)), w-20))+faint(right1(since, 7)))
		}
	}
	u := a.Spend.Usage
	tail = append(tail, "", section("Numbers"))
	if p.Context > 0 {
		f := ctxFill(a, int64(p.Context), agent.ContextWindow(agent.Kind(a.Kind), p.Model))
		tail = append(tail, dim(fit("context", 10))+ctxBar(f.Pct())+" "+paint(cText, fmt.Sprintf("%.0f%%", f.Pct()))+dim("  "+f.Of(tokens)))
	}
	tail = append(tail, dim(fit("spent", 10))+paint(cText+bold, money(a.Spend.Cost))+dim(fmt.Sprintf("  in %s · cache read %s · written %s · out %s", tokens(u.Input), tokens(u.CacheRead), tokens(u.CacheWrite5m+u.CacheWrite1h), tokens(u.Output))))
	tail = append(tail, dim(fit("time", 10))+paint(cText, dur(a.Elapsed(now)))+dim(" since "+a.CreatedAt.Local().Format("Mon 15:04")))
	if a.PID != 0 && m.snap.Table != nil {
		tail = append(tail, "", section(fmt.Sprintf("Processes  %d · %s · %.0f%% cpu", a.Procs, mem(a.Mem), a.CPU)))
		nodes := m.snap.Table.Tree(a.PID)
		sort.SliceStable(nodes, func(i, j int) bool { return nodes[i].Footprint > nodes[j].Footprint })
		for i, n := range nodes {
			if i == 4 {
				break
			}
			tail = append(tail, dim(fit(m.shortCmd(n.PID, n.Comm), w-18))+cpuColor(n.CPU, right1(fmt.Sprintf("%.0f%%", n.CPU), 7))+memColor(n.Footprint, right1(mem(n.Footprint), 8)))
		}
	}

	room := h - len(top) - len(tail) - 2
	var conv []string
	if room >= 3 && (len(p.Recent) > 0 || p.First.Text != "") {
		conv = append(conv, "", section("Conversation"))
		asked := func(t string) []string {
			var out []string
			for i, l := range wrap(oneLine(t), w-2) {
				pre := "  "
				if i == 0 {
					pre = paint(cOrange, "› ")
				}
				out = append(out, pre+paint(cText+bold, l))
			}
			return out
		}
		// The message that started it stays on top; the tail fills the rest.
		var first []string
		recent := p.Recent
		if p.First.Text != "" {
			first = asked(p.First.Text)
			if len(first) > 3 {
				first = append(first[:2], ansi.Truncate(first[2], w-3, "")+faint("…"))
			}
			if len(recent) > 0 && recent[0].Role == "user" && recent[0].Text == p.First.Text {
				recent = recent[1:]
			}
		}
		var body []string
		for _, e := range recent {
			switch e.Role {
			case "user":
				body = append(body, asked(e.Text)...)
			case "tool":
				name, arg, _ := strings.Cut(e.Text, "\x00")
				body = append(body, faint("● ")+dim(name)+"  "+faint(ansi.Truncate(oneLine(tildify(arg)), w-len(name)-4, "…")))
			default:
				text := mdPlainNoUnder.Replace(oneLine(e.Text))
				lines := wrap(text, w-2)
				if len(lines) > 4 {
					lines = append(lines[:3], ansi.Truncate(lines[3], w-5, "…"))
				}
				for _, l := range lines {
					body = append(body, "  "+paint(cSub, l))
				}
			}
		}
		keep := room - 2 - len(first)
		if keep < 2 {
			first, keep = nil, room-2
		}
		if len(body) > keep {
			body = append([]string{faint("  ⋯")}, body[len(body)-(keep-1):]...)
		}
		conv = append(append(conv, first...), body...)
	}
	out := append(append(top, conv...), tail...)
	if len(out) > h {
		out = out[:h]
	}
	return out
}

func nonEmpty(xs ...string) []string {
	var out []string
	for _, x := range xs {
		if strings.TrimSpace(x) != "" {
			out = append(out, x)
		}
	}
	return out
}

func roleName(r fleet.Role) string {
	switch r {
	case fleet.RoleDaemon:
		return "Daemon"
	case fleet.RoleView:
		return "Agent views"
	case fleet.RoleWorker:
		return "Agents"
	case fleet.RoleSpare:
		return "Spares — pre-warmed, ready for the next session"
	case fleet.RoleOrphan:
		return "Orphaned — the session that started these has ended"
	default:
		return "Other Claude Code"
	}
}

// helpPages are the guide's tabs: a key and what it does. @id is the key
// action id has now.
var helpPages = []struct {
	name string
	rows [][2]string
}{
	{"✦ Start", [][2]string{
		{"enter", "start an agent"},
		{"@list.folder", "pick its folder"},
		{"@list.worktree", "new sessions from a worktree agent: its worktree or main checkout"},
		{"#", "rush commands"},
		{"/", "agent commands"},
		{"⌘z · ctrl+/", "undo in a box, a cleared one too"},
		{"@prompt.stash", "{verb} · what's typed is back after you send"},
		{"@prompt.history", "{tab} · sent · cleared · replaced, to put back"},
		{"{command}", "the same, with the Prompt empty"},
	}},
	{"▤ Agents", [][2]string{
		{"↑↓", "pick one"},
		{"⌘↓ · { }", "open its Session"},
		{"enter", "rename or open it, as you chose"},
		{"@list.rename", "rename it · tab the next"},
		{"@list.needs", "next one needing you"},
		{"@list.done", "put it away"},
		{"@list.stop", "stop it"},
		{"@list.split", "split the list by project"},
	}},
	{"◈ Around", [][2]string{
		{"@zen", "zen"},
		{"@profile.pick", "the default profile or provider"},
		{"< > · ctrl+\\", "Agents · Projects · Settings"},
		{"[ ]", "a Session's views, with nothing typed"},
		{"[ ]", "a place's pages, or a sheet's tabs"},
		{"{ }", "in Agents, between the list and the Session"},
		{"shift+← →", "resize · past the end, one side alone"},
		{"@list.toggle", "hide or show Agents beside a Session"},
		{"#tips", "Getting started again"},
		{"esc esc", "quit"},
	}},
	{"Session", [][2]string{
		{"@session.enter", "send from the message box"},
		{"@session.toggle", "open or close the selected row"},
		{"@session.history.open", "open all turns (#expand)"},
		{"@session.history.close", "close older turns, keep latest open (#collapse)"},
		{"@session.send", "send now, including queued messages"},
		{"@session.first", "first message · keeps your draft"},
		{"@session.last", "latest message · keeps your draft"},
		{"@session.message", "full user message and attachments"},
		{"@session.pageup", "scroll up (Page Down scrolls down)"},
		{"@session.verbose", "show or hide step details"},
		{"@session.history", "sent messages and saved drafts"},
		{"@session.setup", "model, effort and harness"},
		{"@session.focus", "back to Agents"},
	}},
}

// helpBody shows the current bindings, including user customizations.
func (m *Model) helpBody() []string {
	var tabs []string
	for i, p := range helpPages {
		if i == m.helpPage {
			tabs = append(tabs, tabOn+" "+p.name+" "+reset)
		} else {
			tabs = append(tabs, tabOff+" "+p.name+" "+reset)
		}
	}
	out := []string{strings.Join(tabs, " "), ""}
	page := helpPages[m.helpPage]
	w, _ := m.stash()
	rows := make([][2]string, len(page.rows))
	for i, r := range page.rows {
		k := r[0]
		if id, ok := strings.CutPrefix(k, "@"); ok {
			k = m.boundKey(id)
			if k == "" {
				k = "unbound"
			}
		}
		rows[i] = [2]string{w.Fill(k), w.Fill(r[1])}
	}
	for _, r := range keyRows(rows) {
		out = append(out, r)
		if m.h >= 36 {
			out = append(out, "")
		}
	}
	// Every tab as tall as the tallest, so the box stays put.
	most := 0
	for _, p := range helpPages {
		most = max(most, len(p.rows))
	}
	for range most - len(page.rows) {
		out = append(out, "")
		if m.h >= 36 {
			out = append(out, "")
		}
	}
	out = append(out, dim("ctrl+] x means Ctrl+] then x; alt is Option."), "", keys("[ ]", "tabs", "k", "customize keys", "esc / F1", "close"))
	// Long remapped chords and small terminals must not hide descriptions.
	width := 76
	if m.w > 0 {
		width = min(m.w-8, width)
	}
	var body []string
	for _, line := range out {
		body = append(body, wrap(line, width)...)
	}
	return body
}

// ctxFill is used tokens of a's context against win, its model's window,
// or the smaller one its agent compacts within.
func ctxFill(a *fleet.Agent, used, win int64) agent.Fill {
	var c agent.Compaction
	if a != nil {
		c = a.Compaction
	}
	return agent.FillOf(used, win, c)
}

// ctxLine is a context row's value: its bar, percentage and what fills it.
func ctxLine(f agent.Fill) string {
	return ctxBar(f.Pct()) + " " + paint(cSub, fmt.Sprintf("%.0f%%", f.Pct())) + dim(" · "+f.Of(tokens))
}
