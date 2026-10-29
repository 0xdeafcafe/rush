package ui

import (
	"fmt"
	"os/exec"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/charmbracelet/x/ansi"
)

// A session's own views (rush view set …) sit beside its conversation as
// a sidebar, the one updated last on top, drawn in rush's parts: a title,
// a few numbers, then rows in groups, each with a mark in its tone and a
// link a click opens. The newest one's summary is the session's line in
// the list. Rush reads them from the session's folder, again only when it
// changed, at most once a second.

// sessionViews is what rush last read of one session's views.
type sessionViews struct {
	list    []host.View
	past    []host.View // put away, the one gone last first
	at      time.Time   // when its folder last changed, as read
	checked time.Time
}

// viewsOf are the session's views, read again when they've changed.
func (m *Model) viewsOf(id string) []host.View {
	if id == "" {
		return nil
	}
	if m.agentViews == nil {
		m.agentViews = map[string]*sessionViews{}
	}
	sv := m.agentViews[id]
	if sv == nil {
		sv = &sessionViews{}
		m.agentViews[id] = sv
	}
	if now := time.Now(); now.Sub(sv.checked) >= time.Second {
		sv.checked = now
		if ch := host.ViewsChanged(id); !ch.Equal(sv.at) {
			sv.list, sv.at = host.Views(id)
			sv.past = host.PastViews(id)
		}
	}
	if !slices.ContainsFunc(sv.list, func(v host.View) bool { return v.By != "" }) {
		return sv.list
	}
	// A plugin's views show only while it runs, allowed to draw them: one
	// you've stopped or revoked leaves nothing behind.
	return slices.DeleteFunc(slices.Clone(sv.list), func(v host.View) bool { return v.By != "" && !m.pluginDrawsViews(v.By) })
}

// pastViewsOf are the session's views put away, as viewsOf last read.
func (m *Model) pastViewsOf(id string) []host.View {
	if sv := m.agentViews[id]; sv != nil {
		return sv.past
	}
	return nil
}

// pluginDrawsViews is whether the plugin runs with views in its ui.
func (m *Model) pluginDrawsViews(name string) bool {
	if m.hooks == nil {
		return false
	}
	for _, p := range m.hooks.State().Plugins {
		if p.Name == name {
			return slices.Contains(p.UI, plugin.UIViews)
		}
	}
	return false
}

// viewSummary is the view whose summary a session's row gives the list,
// its newest with one, if any.
func (m *Model) viewSummary(a *fleet.Agent) *host.View {
	if a == nil || !a.Rush {
		return nil
	}
	// The agent's own say it before any a plugin drew.
	vs := m.viewsOf(a.ID)
	for _, plugins := range []bool{false, true} {
		for i := range vs {
			if vs[i].Summary != "" && (vs[i].By != "") == plugins {
				return &vs[i]
			}
		}
	}
	return nil
}

// viewSummaryShort is a view's summary in the list's own words: its mark
// in the view's colour, then each of its parts dim with the list's dots
// between, so "8 milestones, 1 running" reads like the branch beside it.
func viewSummaryShort(v *host.View) string {
	mark, _ := toneMark(v.Tone)
	var parts []string
	for _, p := range strings.FieldsFunc(oneLine(v.Summary), func(r rune) bool { return r == ',' || r == ';' || r == '·' }) {
		if p = strings.TrimSpace(p); p != "" {
			parts = append(parts, dim(p))
		}
	}
	return paint(accentOr(v.Tone, viewAccent(v)), mark) + " " + strings.Join(parts, faint(" · "))
}

// viewsWidth is how wide the sidebar is beside a conversation of width w,
// 0 when the conversation would be left too narrow.
func viewsWidth(w int) int {
	side := min(64, max(42, w*3/8))
	if w-side < 72 {
		return 0
	}
	return side
}

// toneMark is a row's mark and its colour.
func toneMark(tone string) (string, string) {
	switch tone {
	case "good":
		return "✓", cGreen
	case "bad":
		return "✗", cRed
	case "warn":
		return "▲", cYellow
	case "busy":
		return "◐", cBlue
	case "quiet":
		return "·", cDim
	}
	return "•", cSub
}

// viewToneColour is a number's colour for its tone.
func viewToneColour(tone string) string {
	if tone == "" || tone == "quiet" {
		return cBright
	}
	_, c := toneMark(tone)
	return c
}

// toneEdge is the colour of a tone's edges and bars: grey for none.
func toneEdge(tone string) string {
	switch tone {
	case "":
		return cDim
	case "quiet":
		return cFaint
	}
	_, c := toneMark(tone)
	return c
}

// accentOr is a tone's colour, the view's accent for none: for its marks
// and bars.
func accentOr(tone, accent string) string {
	if tone == "" {
		return accent
	}
	return toneEdge(tone)
}

// viewAccent is the colour a view took as its own, rush's orange if none.
func viewAccent(v *host.View) string {
	switch v.Accent {
	case "blue":
		return cBlue
	case "green":
		return cGreen
	case "yellow":
		return cYellow
	case "purple":
		return cQueue
	case "red":
		return cRed
	}
	return cOrange
}

// viewsStrip is how wide the sidebar is folded: a slim box with the
// mark that opens it.
const viewsStrip = 3

// shareBar is the stats' numbers side by side as one bar w wide, each in
// its tone: "" unless two or more are plain numbers above nought, each
// with a tone, as parts of one whole are ("ready", "failing", "to do"):
// "uses" beside "unsure" are not.
func shareBar(ss []host.Stat, w int) string {
	var vals []float64
	sum := 0.0
	for _, s := range ss {
		f, err := strconv.ParseFloat(strings.TrimSpace(s.Value), 64)
		if err != nil || f < 0 || s.Tone == "" {
			return ""
		}
		vals, sum = append(vals, f), sum+f
	}
	if len(vals) < 2 || sum == 0 {
		return ""
	}
	var b strings.Builder
	used, acc := 0, 0.0
	for i, f := range vals {
		acc += f
		n := int(acc/sum*float64(w)+0.5) - used
		if f > 0 && n == 0 && used < w {
			n = 1 // a number above nought always shows
		}
		n = min(n, w-used)
		used += n
		b.WriteString(paint(toneEdge(ss[i].Tone), strings.Repeat("━", n)))
	}
	return b.String() + blanks(w-used)
}

// progressBar is done of total as a bar w wide in col.
func progressBar(frac float64, w int, col string) string {
	n := min(w, max(0, int(frac*float64(w)+0.5)))
	return paint(col, strings.Repeat("━", n)) + faint(strings.Repeat("─", w-n))
}

// spark is vals as a sparkline at most w wide, its newest last.
func spark(vals []float64, w int, col string) string {
	vals = vals[max(0, len(vals)-w):]
	if len(vals) == 0 {
		return ""
	}
	lo, hi := vals[0], vals[0]
	for _, v := range vals {
		lo, hi = min(lo, v), max(hi, v)
	}
	bars := []rune("▁▂▃▄▅▆▇█")
	var b strings.Builder
	for _, v := range vals {
		i := len(bars) / 2
		if hi > lo {
			i = int((v - lo) / (hi - lo) * float64(len(bars)-1))
		}
		b.WriteRune(bars[i])
	}
	return paint(col, b.String())
}

// viewsState is how you've left a session's views beside its
// conversation, and where they were last drawn, for the mouse.
type viewsState struct {
	shut bool // folded to a strip
	past bool // showing the views put away, to bring one back
	// tab is the view shown, by its name: "" or one gone is the newest.
	tab    string
	scroll int             // rows of it scrolled past
	fold   map[string]bool // groups you folded or opened, by view/index
	sorts  map[string]int  // tables you sorted: by column +1, less for down
	open   map[string]bool // rows you opened, by view|row
	hover  int             // the body row under the pointer, +1; 0 for none
	hoverX int             // and its column in the panel, for a button
	hot    bool            // the pointer is on something a click does
	// As last drawn: its column in the pane, its top row on screen, its
	// size, its tabs, what a click on each body row does, and how far it
	// scrolls.
	x, y, w, h int
	tabs       []viewTab
	acts       []viewAct
	most       int
	shown      string // the view shown, by name
	empty      bool   // no view shows now: the strip opens the past ones
	hasPast    bool   // ⟲ along the top edge opens them
	// live are rows rush draws at the top of the view shown, over what
	// the agent drew: the session's subagents (panelsubs.go).
	live []viewLine
	// quiet is why the view shown is greyed, "" for not (viewquiet.go),
	// and what it was last worked out from.
	quiet     string
	quietKey  viewQuietKey
	quietMemo string
}

// viewTab is a tab along the panel's top edge: its columns and view.
type viewTab struct {
	from, to int
	name     string
}

// viewAct is what a click on a row of the panel does: open its link, or
// fold or open its group.
type viewAct struct {
	link   string
	fold   string // view/index
	folded bool   // the group is folded now
	// sort is a table's header: a click sorts it by the column under it,
	// cols where each starts in the panel.
	sort string
	cols []int
	// back is a past view's file: a click brings it back as name.
	back, name string
	// open is a row a click opens or shuts, by view|row, opened if it is;
	// row is its key in its view, for what its buttons change.
	open   string
	opened bool
	row    string
	// tick is a checklist item's: a click on its mark ticks it, or
	// unticks it when done.
	tick, done bool
	// run is a subagent's or workflow's ref ("run:<id>", a workflow's):
	// a click watches it.
	run  string
	btns []viewBtn // the buttons along the line
	// lit is the line drawn with the pointer at a column, its button lit.
	lit func(col int) string
}

func (a viewAct) ok() bool {
	return a.link != "" || a.fold != "" || a.sort != "" || a.back != "" || a.open != "" || a.run != "" || len(a.btns) > 0
}

// viewLine is a row of a view's body and what a click on it does.
type viewLine struct {
	s   string
	act viewAct
}

// shownView is the view the tab names, else the newest.
func shownView(vs []host.View, tab string) *host.View {
	for i := range vs {
		if vs[i].Name == tab {
			return &vs[i]
		}
	}
	return &vs[0]
}

// isFolded is whether a view's group is folded: as you left it, else as
// the agent said.
func (st *viewsState) isFolded(v *host.View, gi int) (string, bool) {
	key := v.Name + "/" + strconv.Itoa(gi)
	if f, ok := st.fold[key]; ok {
		return key, f
	}
	return key, v.Groups[gi].Folded
}

// viewsPanel draws a session's views in a box w wide and h tall, beside
// the conversation on its own ground rather than a slab of its own: the
// views as tabs along its top edge with the marks that fold and dismiss
// it, the one shown inside, scrolled as far as st says, and along the
// bottom edge that the agent made it and how much more there is. Folded,
// it's a slim box with the mark that opens it. It notes in st where its
// tabs are and what a click on each row does.
func viewsPanel(vs, past []host.View, w, h int, now time.Time, st *viewsState) []string {
	st.w, st.h, st.tabs, st.acts = w, h, st.tabs[:0], st.acts[:0]
	st.empty, st.hasPast = len(vs) == 0, len(past) > 0
	if h <= 0 || len(vs) == 0 && len(past) == 0 {
		return nil
	}
	if st.shut || st.empty && !st.past {
		mark := paint(cOrange, "✦")
		if st.empty {
			mark = paint(cSub, "⟲") // only past ones: it opens them
		}
		out := make([]string, h)
		for y := range out {
			switch {
			case y == 0:
				out[y] = faint("╭─╮")
			case y == h-1:
				out[y] = faint("╰─╯")
			case y == 1:
				out[y] = faint("│") + paint(cSub, "‹") + faint("│")
			case y == 2:
				out[y] = faint("│") + mark + faint("│")
			default:
				out[y] = faint("│ │")
			}
		}
		return out
	}
	inner := w - 4 // its edges, and a space inside each
	var body []viewLine
	var top, bottom string
	if st.past {
		body, top, bottom = pastBody(past, inner, now), pastTop(w), viewsBottom(w, "", "", 0, 0)
	} else {
		v := shownView(vs, st.tab)
		st.shown = v.Name
		body, top = viewBody(v, inner, st), viewsTop(vs, v, w, st)
	}
	// The box is as tall as what's in it, up to h: ground under it, not
	// an empty box.
	full := h
	h = min(h, len(body)+2)
	st.h = h
	bh := max(0, h-2)
	st.most = max(0, len(body)-bh)
	st.scroll = min(max(0, st.scroll), st.most)
	out := make([]string, 0, h)
	out = append(out, top)
	for i := range bh {
		var l viewLine
		if j := st.scroll + i; j < len(body) {
			l = body[j]
		}
		mid := " " + fit(l.s, inner) + " "
		switch {
		case st.hover != i+1 || !l.act.ok():
		case l.act.lit != nil:
			mid = " " + fit(l.act.lit(st.hoverX), inner) + " "
		default:
			mid = onGround(mid, inner+2, hoverBG)
		}
		out = append(out, faint("│")+mid+faint("│"))
		st.acts = append(st.acts, l.act)
	}
	if h >= 2 {
		if !st.past {
			v := shownView(vs, st.tab)
			when := ago(now.Sub(v.Updated))
			if st.quiet != "" {
				when = st.quiet + " · " + when
			}
			bottom = viewsBottom(w, v.By, when, st.scroll, len(body)-st.scroll-bh)
		}
		out = append(out, bottom)
	}
	for len(out) < full {
		out = append(out, blanks(w))
	}
	return out
}

// viewsTop is the panel's top edge: rush's mark, the views' titles as
// tabs, the one shown bright, and at its end › to fold it and × to
// dismiss it.
func viewsTop(vs []host.View, v *host.View, w int, st *viewsState) string {
	right := " " + paint(cSub, "›") + " " + paint(cSub, "×") + " "
	if st.hasPast {
		right = " " + paint(cSub, "⟲") + right
	}
	rw := cellw.String(right)
	room := w - 9 - rw // "╭─ ✦ ", a space and a rule, the marks, "─╮"
	per := max(4, (room-3*(len(vs)-1))/len(vs))
	if len(vs) == 1 {
		per = room
	}
	var b strings.Builder
	col := 5
	for i := range vs {
		if i > 0 {
			if col+3+4 > 5+room {
				break
			}
			b.WriteString(faint(" · "))
			col += 3
		}
		name := cellwTrunc(vs[i].Title, min(per, 5+room-col))
		style := cSub
		if &vs[i] == v {
			style = cBright + bold
		}
		b.WriteString(paint(style, name))
		st.tabs = append(st.tabs, viewTab{col, col + cellw.String(name), vs[i].Name})
		col += cellw.String(name)
	}
	left := faint("╭─ ") + paint(viewAccent(v), "✦") + " " + b.String() + " "
	fill := max(0, w-(col+1)-rw-2)
	return left + faint(strings.Repeat("─", fill)) + right + faint("─╮")
}

// pastTop is the panel's top edge over the past views: a click on it
// goes back.
func pastTop(w int) string {
	left := faint("╭─ ") + paint(cSub, "⟲") + " " + paint(cBright+bold, "Past views") + " "
	right := " " + paint(cSub, "‹ back") + " "
	fill := max(0, w-cellw.String(left)-cellw.String(right)-2)
	return left + faint(strings.Repeat("─", fill)) + right + faint("─╮")
}

// pastBody lists the views put away, the one gone last first, each its
// title and when it went, its summary under it: a click brings it back.
func pastBody(past []host.View, w int, now time.Time) []viewLine {
	out := []viewLine{{}}
	if len(past) == 0 {
		return append(out, viewLine{s: dim("Nothing put away yet.")})
	}
	for i := range past {
		v := &past[i]
		act := viewAct{back: v.File, name: v.Name}
		when := dim(ago(now.Sub(v.Gone)))
		out = append(out, viewLine{spread(paint(viewAccent(v), "◆")+" "+paint(cText, cellwTrunc(v.Title, w-12)), when, w), act})
		if v.Summary != "" {
			out = append(out, viewLine{"  " + dim(cellwTrunc(v.Summary, w-2)), act})
		}
	}
	return append(out, viewLine{}, viewLine{s: faint("a click brings one back, kept until you close it")})
}

// viewsBottom is the panel's bottom edge: who made it, the agent or the
// plugin by, and when it last changed it, and how many rows are scrolled
// past above and below.
func viewsBottom(w int, by, when string, above, below int) string {
	var r []string
	if above > 0 {
		r = append(r, fmt.Sprintf("↑ %d", above))
	}
	if below > 0 {
		r = append(r, fmt.Sprintf("↓ %d more", below))
	}
	right := ""
	if len(r) > 0 {
		right = " " + paint(cSub, strings.Join(r, " · ")) + " "
	}
	left := faint("╰─ ")
	who := "the agent"
	if by != "" {
		who = "plugin " + by
	}
	labels := []string{"made by " + who + " · " + when, "by " + who + " · " + when, when, ""}
	if when == "" {
		labels = []string{"", ""}
	}
	for _, label := range labels {
		if cellw.String(label)+1 <= w-3-cellw.String(right)-3 {
			if label != "" {
				left += dim(label) + " "
			}
			break
		}
	}
	fill := max(0, w-cellw.String(left)-cellw.String(right)-2)
	return left + faint(strings.Repeat("─", fill)) + right + faint("─╯")
}

// viewBody is a view's rows w wide: its summary, a bar of how far along, its numbers as tiles, its groups of rows under
// a rule, folded or not, and its note.
func viewBody(v *host.View, w int, st *viewsState) []viewLine {
	accent := viewAccent(v)
	var out []viewLine
	add := func(act viewAct, ls ...string) {
		for _, s := range ls {
			out = append(out, viewLine{s, act})
		}
	}
	none := viewAct{}
	// room is a blank row between parts: one, never two.
	room := func() {
		if len(out) > 0 && out[len(out)-1].s != "" {
			add(none, "")
		}
	}
	add(none, "")
	if len(st.live) > 0 {
		out = append(out, st.live...)
		room()
	}
	ours := len(out) // what the agent drew starts here
	fade := st.quiet != ""
	if fade && len(st.live) > 0 {
		// Greyed under rush's bright rows it looks broken: folded to its
		// heading, it reads as put away instead. A click opens it, bright.
		key := v.Name + "/§quiet"
		folded, ok := st.fold[key]
		if !ok {
			folded = true
		}
		title := v.Title
		if title == "" {
			title = v.Name
		}
		add(viewAct{fold: key, folded: folded}, viewHeading(title, st.quiet, "", folded, w))
		if folded {
			return out
		}
		add(none, "")
		fade = false // opened: bright as it was
	}
	if hd := viewHead(v, w, accent); len(hd) > 0 {
		add(none, hd...)
		room()
	}
	if t := viewTiles(v.Stats, w); len(t) > 0 {
		add(none, t...)
		room()
	}
	for gi := range v.Groups {
		g := &v.Groups[gi]
		room()
		key, folded := st.isFolded(v, gi)
		if g.Title != "" {
			add(viewAct{fold: key, folded: folded}, viewRuleHead(g.Title, g.Tone, len(g.Rows), folded, w))
		} else {
			folded = false // nothing to open it by
		}
		if folded {
			continue
		}
		if g.Text != "" {
			for _, l := range wrap(g.Text, w) {
				add(none, dim(l))
			}
		}
		bars := viewBars(g.Rows, w, accent)
		under := metaUnder(g.Rows, w)
		cx := blockCtx{accent: accent, st: st, view: v.Name}
		for i := range g.Rows {
			r := &g.Rows[i]
			o := rowOpen{key: host.ItemKey(host.GroupKey(gi), r.ID, r.Text, nil), text: r.Text, meta: r.Meta,
				detail: r.Detail, link: r.Link, actions: r.Actions}
			if r.Value != nil {
				out = append(out, cx.rowLines(o, w, bars(r))...)
				continue
			}
			out = append(out, cx.rowLines(o, w, viewRow(r, w, under)...)...)
		}
	}
	for bi := range v.Blocks {
		room()
		out = append(out, viewBlock(v, bi, w, st, accent)...)
	}
	if v.Note != "" {
		room()
		for _, l := range wrap(v.Note, w-2) {
			add(none, faint("│ ")+dim(l))
		}
	}
	if len(v.Actions) > 0 {
		// The view's own buttons: what it changes is no row of it.
		as := slices.Clone(v.Actions)
		for i := range as {
			as[i].Set = nil
		}
		room()
		out = append(out, btnLines(as, w, 2, accent, "")...)
	}
	room()
	if fade { // greyed: the agent has stopped keeping it
		for i := ours; i < len(out); i++ {
			out[i].s, out[i].act.lit = faint(ansi.Strip(out[i].s)), nil
		}
	}
	return out
}

// viewHead is the view's summary when it has no numbers or bar to say it,
// then the bar of how far along it is, w wide.
func viewHead(v *host.View, w int, accent string) []string {
	var out []string
	if v.Summary != "" && len(v.Stats) == 0 && v.Progress == nil {
		for i, l := range wrap(v.Summary, w-2) {
			lead := "  "
			if i == 0 {
				lead = paint(accentOr(v.Tone, accent), "◆") + " "
			}
			out = append(out, lead+paint(cText, l))
		}
	}
	p := v.Progress
	if p == nil {
		return out
	}
	frac := min(1, max(0, p.Done/p.Total))
	words := fmt.Sprintf("%s of %s", num(p.Done), num(p.Total))
	if p.Label != "" {
		words += " " + p.Label
	}
	pct := fmt.Sprintf("%3d%%", int(frac*100+0.5))
	return append(out, progressBar(frac, w-len(pct)-1, accentOr(p.Tone, accent))+" "+paint(cBright+bold, pct), dim(words))
}

// viewTiles are a view's numbers side by side, w wide, with a quiet rule
// between them: each in its tone over its label and its trend if any has
// one, then the bar of their shares.
func viewTiles(ss []host.Stat, w int) []string {
	n := len(ss)
	if n == 0 {
		return nil
	}
	cell := (w - 3*(n-1)) / n // " │ " between tiles
	var rows [3]strings.Builder
	trends := false
	for i, s := range ss {
		cw := cell
		if i == n-1 {
			cw = w - (cell+3)*(n-1) // the last takes what's left
		}
		if i > 0 {
			for r := range rows {
				rows[r].WriteString(faint(" │ "))
			}
		}
		rows[0].WriteString(fit(paint(viewToneColour(s.Tone)+bold, s.Value), cw))
		rows[1].WriteString(fit(paint(cSub, s.Label), cw))
		rows[2].WriteString(fit(spark(s.Trend, cw, toneEdge(s.Tone)), cw))
		trends = trends || len(s.Trend) > 0
	}
	out := []string{rows[0].String(), rows[1].String()}
	if trends {
		out = append(out, rows[2].String())
	}
	if bar := shareBar(ss, w); bar != "" {
		out = append(out, bar)
	}
	return out
}

// viewRuleHead is a group's or block's rule with its title and count (n,
// none below 0), ▾ open or ▸ folded, w wide.
func viewRuleHead(title, tone string, n int, folded bool, w int) string {
	count := ""
	if n >= 0 {
		count = strconv.Itoa(n)
	}
	return viewHeading(title, count, tone, folded, w)
}

// viewHeading is a part's heading, w wide: ▾ open or ▸ folded, its title
// bright (in its tone if it has one), and a few quiet words on it (a
// count). No rule: the room above it sets it apart.
func viewHeading(title, count, tone string, folded bool, w int) string {
	col := cBright
	if tone != "" {
		col = toneEdge(tone)
	}
	caret := "▾ "
	if folded {
		caret = "▸ "
	}
	if count != "" {
		count = "  " + count
	}
	title = cellwTrunc(title, max(4, w-2-cellw.String(count)))
	return paint(cSub, caret) + paint(col+bold, title) + dim(count)
}

// viewBars draws a group's rows that have a value as a chart, w wide:
// each its mark and text in a column, a bar as long as its value against
// the largest, and the number, or its meta, at the end.
func viewBars(rows []host.Row, w int, accent string) func(*host.Row) string {
	most, textW, numW := 0.0, 0, 0
	words := func(r *host.Row) string {
		if r.Meta != "" {
			return r.Meta
		}
		return num(*r.Value)
	}
	for i := range rows {
		if r := &rows[i]; r.Value != nil {
			most = max(most, *r.Value)
			textW = max(textW, cellw.String(r.Text))
			numW = max(numW, cellw.String(words(r)))
		}
	}
	numW = min(numW, w/4)
	textW = min(textW, w*2/5)
	barW := max(4, w-2-textW-1-1-numW)
	return func(r *host.Row) string {
		mark, col := toneMark(r.Tone)
		frac := 0.0
		if most > 0 {
			frac = *r.Value / most
		}
		bar := accentOr(r.Tone, accent)
		text := paint(cText, fit(cellwTrunc(r.Text, textW), textW))
		if r.Link != "" {
			text = link(r.Link, text)
		}
		n := min(barW, max(0, int(frac*float64(barW)+0.5)))
		if *r.Value > 0 && n == 0 {
			n = 1 // above nought always shows
		}
		return paint(col, mark) + " " + text + " " + paint(bar, strings.Repeat("━", n)) + faint(strings.Repeat("─", barW-n)) + " " + paint(cSub, fmt.Sprintf("%*s", numW, cellwTrunc(words(r), numW)))
	}
}

// viewRow is a row w wide: its mark, its text and link, and at its end
// its bar or the word on it. The word goes on the lines under the text,
// lined up with it, when under says so (metaUnder) or it has a bar.
func viewRow(r *host.Row, w int, under bool) []string {
	mark, col := toneMark(r.Tone)
	meta, below := "", r.Meta
	switch {
	case r.Progress != nil:
		meta = progressBar(*r.Progress, 6, col) + paint(cSub, fmt.Sprintf(" %3d%%", int(*r.Progress*100+0.5)))
	case r.Meta != "" && !under:
		meta, below = dim(r.Meta), ""
	}
	room := w - 2
	if meta != "" {
		room -= cellw.String(meta) + 2
	}
	if r.Link != "" {
		room -= 2 // its ↗
	}
	plain := cellwTrunc(r.Text, room)
	text := paint(cText, plain)
	if r.Link != "" {
		text = link(r.Link, text) + paint(cSub, " ↗") // its escapes take no room: laid out by hand
		plain += " ↗"
	}
	gap := max(1, w-2-cellw.String(plain)-cellw.String(meta))
	return append([]string{paint(col, mark) + " " + text + strings.Repeat(" ", gap) + meta}, metaLines(below, w)...)
}

// metaUnder is whether rows w wide put their words under their text: when
// any one's doesn't fit beside it whole, all of them do, so they line up.
func metaUnder(rows []host.Row, w int) bool {
	return slices.ContainsFunc(rows, func(r host.Row) bool {
		return r.Meta != "" && r.Progress == nil && !metaFits(r.Text, r.Meta, r.Link != "", w)
	})
}

// metaFits is whether a row's text and its word sit side by side, w wide,
// with its mark, a gap of two and its link's ↗.
func metaFits(text, meta string, link bool, w int) bool {
	n := 2 + cellw.String(oneLine(text)) + 2 + cellw.String(meta)
	if link {
		n += 2
	}
	return n <= w
}

// metaLines are a row's words under its text, w wide: lined up with the
// text, quiet, two lines at most.
func metaLines(meta string, w int) []string {
	if meta == "" {
		return nil
	}
	ls := wrap(meta, w-2)
	if len(ls) > 2 {
		ls = []string{ls[0], cellwTrunc(ls[1]+" "+ls[2], w-2)}
	}
	out := make([]string, len(ls))
	for i, l := range ls {
		out[i] = "  " + dim(l)
	}
	return out
}

// num is a number as short as it reads: 14, not 14.000000.
func num(f float64) string { return strconv.FormatFloat(f, 'f', -1, 64) }

// viewsAt is where x, y falls on the session's views, if it does: the
// column and row inside them.
func (m *Model) viewsAt(c *hostConn, x, y int) (col, row int, ok bool) {
	st := &c.views
	col, row = x-m.paneX()-st.x, y-st.y
	return col, row, st.w > 0 && col >= 0 && col < st.w && row >= 0 && row < st.h
}

// clickViews is a click on the session's views. On the top edge a tab
// shows its view, × dismisses them and anywhere else folds them to a
// strip; the strip opens them again. A group's title folds or opens it,
// a row with a link opens it, and the bottom edge scrolls a page, or
// back to the top from the end.
func (m *Model) clickViews(c *hostConn, x, y int) (bool, tea.Cmd) {
	col, row, ok := m.viewsAt(c, x, y)
	if !ok {
		return false, nil
	}
	st := &c.views
	switch {
	case st.shut || st.empty && !st.past:
		st.shut, st.past = false, st.empty
	case st.past && row == 0:
		st.past, st.scroll = false, 0
	case row == 0:
		for _, t := range st.tabs {
			if col >= t.from && col < t.to {
				if st.tab != t.name {
					st.tab, st.scroll = t.name, 0
				}
				return true, nil
			}
		}
		if col >= st.w-5 && col <= st.w-3 { // " × " at the edge's end: put away
			if c.sess != nil && st.shown != "" {
				_ = host.DropView(c.sess.Info.ID, st.shown)
				m.refreshViews(c.sess.Info.ID)
			}
			return true, nil
		}
		if st.hasPast && col >= st.w-9 && col <= st.w-8 { // ⟲
			st.past, st.scroll = true, 0
			return true, nil
		}
		st.shut = true
	case row == st.h-1:
		if st.scroll >= st.most {
			st.scroll = 0
		} else {
			st.scroll = min(st.most, st.scroll+max(1, st.h-3))
		}
	case row-1 < len(st.acts):
		switch a := st.acts[row-1]; {
		case len(a.btns) > 0:
			for _, b := range a.btns {
				if col >= b.from && col < b.to {
					return true, m.viewAction(c, st.shown, a.row, b.a)
				}
			}
		case a.run != "":
			if m.isWorkflowRef(c, a.run) {
				m.openWorkflow(c, a.run)
			} else if id, ok := strings.CutPrefix(a.run, "run:"); ok {
				c.sel = a.run
				m.watchRun(c, id)
			}
		case a.tick && col <= 3: // its mark, past the edge and a space
			done := !a.done
			return true, m.viewAction(c, st.shown, a.row, host.Action{Set: &host.Change{Done: &done}})
		case a.open != "":
			if st.open == nil {
				st.open = map[string]bool{}
			}
			st.open[a.open] = !a.opened
		case a.fold != "":
			if st.fold == nil {
				st.fold = map[string]bool{}
			}
			st.fold[a.fold] = !a.folded
		case a.sort != "":
			c := 0
			for i, from := range a.cols {
				if col >= from {
					c = i + 1
				}
			}
			if st.sorts == nil {
				st.sorts = map[string]int{}
			}
			// By it, then the other way, then as the agent had it.
			switch st.sorts[a.sort] {
			case c:
				st.sorts[a.sort] = -c
			case -c:
				delete(st.sorts, a.sort)
			default:
				st.sorts[a.sort] = c
			}
		case a.back != "":
			if c.sess != nil {
				if err := host.BringBack(c.sess.Info.ID, a.back); err != nil {
					m.fail("couldn't bring it back: " + err.Error())
				} else {
					st.past, st.tab, st.scroll = false, a.name, 0
					m.refreshViews(c.sess.Info.ID)
				}
			}
		case a.link != "":
			go func() { _ = exec.Command("open", a.link).Run() }()
			m.flash("opened "+strings.TrimPrefix(a.link, "https://"), false)
		}
	}
	return true, nil
}

// refreshViews reads the session's views again at once: you changed them.
func (m *Model) refreshViews(id string) {
	if sv := m.agentViews[id]; sv != nil {
		sv.list, sv.at = host.Views(id)
		sv.past = host.PastViews(id)
	}
}

// wheelViews scrolls the session's views when the pointer is over them.
func (m *Model) wheelViews(c *hostConn, x, y int, up bool) bool {
	if _, _, ok := m.viewsAt(c, x, y); !ok || c.views.shut || c.views.empty && !c.views.past {
		return false
	}
	d := 3
	if up {
		d = -3
	}
	c.views.scroll = min(max(0, c.views.scroll+d), c.views.most)
	return true
}

// hoverViews lights the row of the session's views under the pointer
// when a click on it does something, and says whether that changed.
func (m *Model) hoverViews(x, y int) bool {
	c := m.host
	if c == nil {
		return false
	}
	st := &c.views
	hover, hot := 0, false
	col, row, ok := m.viewsAt(c, x, y)
	if ok {
		switch {
		case st.shut, st.empty && !st.past, row == 0:
			hot = true
		case row == st.h-1:
			hot = st.most > 0
		case row-1 < len(st.acts) && st.acts[row-1].ok():
			hover, hot = row, true
			if bs := st.acts[row-1].btns; len(bs) > 0 {
				hot = slices.ContainsFunc(bs, func(b viewBtn) bool { return col >= b.from && col < b.to })
			}
		}
	}
	changed := hover != st.hover || hover > 0 && st.acts[hover-1].lit != nil && col != st.hoverX
	st.hover, st.hot, st.hoverX = hover, hot, col
	return changed
}

// ago is how long ago, in a word or two.
func ago(d time.Duration) string {
	switch {
	case d < 10*time.Second:
		return "just now"
	case d < time.Minute:
		return fmt.Sprintf("%ds ago", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	}
	return fmt.Sprintf("%dh ago", int(d.Hours()))
}

// paneViewsSide puts the session's views beside the conversation's body
// rows, out[top:top+h], in the last side columns of contentW+side.
func paneViewsSide(out []string, top, h, contentW, side int, vs, past []host.View, st *viewsState) {
	col := viewsPanel(vs, past, side, h, time.Now(), st)
	for y := 0; y < len(col) && top+y < len(out); y++ {
		out[top+y] = fit(out[top+y], contentW) + reset + col[y]
	}
}

// sideViews is the session's views folded into the column beside the
// conversation, w wide: how many there are, then each view's title and
// what it says of itself, and each row's ref. A click opens them, on a
// view's row at that view.
func sideViews(vs, past []host.View, w int) (rows, refs []string) {
	var how []string
	if len(vs) > 0 {
		how = append(how, fmt.Sprintf("%d", len(vs)))
	}
	if len(past) > 0 {
		how = append(how, fmt.Sprintf("%d put away", len(past)))
	}
	open := "open ›"
	if len(vs) == 0 {
		open = "bring one back ›"
	}
	rows = []string{spread(sideHead("Views", dim(strings.Join(how, " · "))), paint(cSub, open)+" ", w)}
	refs = []string{sideViewsRef}
	const shown = 3
	for _, v := range vs[:min(len(vs), shown)] {
		title := firstNonEmpty(oneLine(v.Title), v.Name)
		row := "   " + paint(viewAccent(&v), "▸") + " " + paint(cText, title)
		if v.Summary != "" {
			row += "  " + dim(oneLine(v.Summary))
		}
		rows = append(rows, fit(row, w))
		refs = append(refs, sideViewsRef+":"+v.Name)
	}
	if more := len(vs) - shown; more > 0 {
		rows = append(rows, faint(fmt.Sprintf("   +%d more", more)))
		refs = append(refs, sideViewsRef)
	}
	return rows, refs
}
