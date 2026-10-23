package ui

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
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
	at      time.Time // when its folder last changed, as read
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
		}
	}
	return sv.list
}

// viewSummary is the line a session's newest view gives the list, if any.
func (m *Model) viewSummary(a *fleet.Agent) string {
	if a == nil || !a.Rush {
		return ""
	}
	vs := m.viewsOf(a.ID)
	for i := range vs {
		if vs[i].Summary != "" {
			return vs[i].Summary
		}
	}
	return ""
}

// viewsWidth is how wide the sidebar is beside a conversation of width w,
// 0 when the conversation would be left too narrow.
func viewsWidth(w int) int {
	side := min(56, max(38, w/3))
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

// accentOr is a tone's colour, rush's orange for none, as the panel's
// own edge is: for a view's title and its bar.
func accentOr(tone string) string {
	if tone == "" {
		return cOrange
	}
	return toneEdge(tone)
}

// viewsStrip is how wide the sidebar is folded: its edge and a mark.
const viewsStrip = 3

// onTile is s on a stat's lighter ground, kept so after each styled piece.
func onTile(s string) string {
	return selBG + strings.ReplaceAll(s, reset, reset+selBG) + reset
}

// shareBar is the stats' numbers side by side as one bar w wide, each in
// its tone: "" unless two or more are plain numbers above nought.
func shareBar(ss []host.Stat, w int) string {
	var vals []float64
	sum := 0.0
	for _, s := range ss {
		f, err := strconv.ParseFloat(strings.TrimSpace(s.Value), 64)
		if err != nil || f < 0 {
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

// viewsPanel draws views in a column w wide and h tall: first that the
// agent made it (generative UI) and the mark that folds it, then each view,
// its title and when it was updated, a bar of how far along, its numbers
// as tiles with a bar of their shares, its groups of rows under a rule,
// and its note. Folded, it's a strip with the mark that opens it.
func viewsPanel(vs []host.View, w, h int, now time.Time, shut bool) []string {
	inner := w - 3 // the edge, and a space either side
	var out []string
	add := func(ls ...string) {
		for _, s := range ls {
			out = append(out, onBg(panelBG, paint(cOrange, "▍")+" "+fit(s, inner)+" ", w))
		}
	}
	if shut {
		out = append(out, onBg(panelBG, paint(cOrange, "▍")+paint(cSub, "‹"), w), onBg(panelBG, paint(cOrange, "▍")+paint(cOrange, "✦"), w))
		for len(out) < h {
			out = append(out, onBg(panelBG, paint(cOrange, "▍"), w))
		}
		return out[:max(0, h)]
	}
	add(spread(paint(cOrange, "✦")+" "+dim("generative UI, made by the agent"), paint(cSub, "›  ×"), inner))
	for vi := range vs {
		v := &vs[vi]
		add("")
		if vi > 0 {
			add(faint(strings.Repeat("┄", inner)), "")
		}
		add(viewHead(v, inner, now)...)
		add(viewTiles(v.Stats, inner)...)
		for _, g := range v.Groups {
			add("")
			add(viewGroup(g, inner)...)
		}
		if v.Note != "" {
			add("")
			for _, l := range wrap(v.Note, inner-2) {
				add(faint("│ ") + dim(l))
			}
		}
	}
	add("")
	for len(out) < h {
		add("")
	}
	if len(out) > h && h > 0 {
		out = append(out[:h-1], onBg(panelBG, paint(cOrange, "▍")+" "+dim(fmt.Sprintf("%d more rows · rush view list", len(out)-h+1)), w))
	}
	return out
}

// viewHead is a view's title, when it was updated, and the bar of how
// far along it is, w wide.
func viewHead(v *host.View, w int, now time.Time) []string {
	out := make([]string, 1, 3)
	out[0] = spread(paint(accentOr(v.Tone), "◆")+" "+paint(cBright+bold, v.Title), dim(ago(now.Sub(v.Updated))), w)
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
	return append(out, progressBar(frac, w-len(pct)-1, accentOr(p.Tone))+" "+paint(cBright+bold, pct), dim(words))
}

// viewTiles are a view's numbers as tiles side by side, w wide, each with
// its trend if any has one, then the bar of their shares.
func viewTiles(ss []host.Stat, w int) []string {
	n := len(ss)
	if n == 0 {
		return nil
	}
	cell := (w - (n - 1)) / n // a column between tiles
	var tiles [3]strings.Builder
	trends := false
	for i, s := range ss {
		gap := " "
		if i == n-1 {
			cell, gap = w-(cell+1)*(n-1), "" // the last takes what's left
		}
		edge := paint(toneEdge(s.Tone), "▌")
		tiles[0].WriteString(onTile(edge+" "+fit(paint(viewToneColour(s.Tone)+bold, s.Value), cell-2)) + gap)
		tiles[1].WriteString(onTile(edge+" "+fit(paint(cSub, s.Label), cell-2)) + gap)
		tiles[2].WriteString(onTile(edge+" "+fit(spark(s.Trend, cell-3, toneEdge(s.Tone)), cell-2)) + gap)
		trends = trends || len(s.Trend) > 0
	}
	out := []string{"", tiles[0].String(), tiles[1].String()}
	if trends {
		out = append(out, tiles[2].String())
	}
	if bar := shareBar(ss, w); bar != "" {
		out = append(out, bar)
	}
	return out
}

// viewGroup is a group's rule with its title and count, then its rows,
// w wide.
func viewGroup(g host.Group, w int) []string {
	var out []string
	if g.Title != "" {
		col := cSub
		if g.Tone != "" {
			col = toneEdge(g.Tone)
		}
		head := paint(col, "▾ ") + paint(col+bold, g.Title) + dim(fmt.Sprintf("  %d ", len(g.Rows)))
		out = append(out, head+faint(strings.Repeat("─", max(0, w-cellw.String(head)))))
	}
	for i := range g.Rows {
		out = append(out, viewRow(&g.Rows[i], w)...)
	}
	return out
}

// viewRow is a row w wide: its mark, its text and link, and at its end
// its bar or the word on it, which goes on a line under it when there's
// no room for both.
func viewRow(r *host.Row, w int) []string {
	mark, col := toneMark(r.Tone)
	meta, under := "", r.Meta
	switch {
	case r.Progress != nil:
		meta = progressBar(*r.Progress, 6, col) + paint(cSub, fmt.Sprintf(" %3d%%", int(*r.Progress*100+0.5)))
	case r.Meta != "":
		meta, under = dim(r.Meta), ""
	}
	room := w - 2 - cellw.String(meta) - 1
	// A long word on it goes under the row rather than cut its name short.
	if under == "" && (room < 16 || cellw.String(r.Text) > room && cellw.String(meta) > w/3) {
		room, meta, under = w-2, "", r.Meta
	}
	if r.Link != "" {
		room -= 2 // its ↗
	}
	plain := cellwTrunc(r.Text, room)
	text := paint(cText, plain)
	if r.Link != "" {
		text = link(r.Link, text) + paint(cFaint, " ↗") // its escapes take no room: laid out by hand
		plain += " ↗"
	}
	gap := max(1, w-2-cellw.String(plain)-cellw.String(meta))
	out := []string{paint(col, mark) + " " + text + strings.Repeat(" ", gap) + meta}
	if under != "" {
		out = append(out, faint("└ ")+dim(cellwTrunc(under, w-2)))
	}
	return out
}

// num is a number as short as it reads: 14, not 14.000000.
func num(f float64) string { return strconv.FormatFloat(f, 'f', -1, 64) }

// clickViews is a click on the views sidebar's top row, or anywhere on it
// folded: its × dismisses it, anywhere else folds it to a strip, and the
// strip opens it again.
func (m *Model) clickViews(c *hostConn, x, y int) bool {
	col := x - m.paneX() - c.viewsX
	if c.viewsW == 0 || col < 0 || col >= c.viewsW || y < c.viewsY || !c.viewsShut && y != c.viewsY {
		return false
	}
	if !c.viewsShut && col >= c.viewsW-4 { // "×  " at the row's end
		c.viewsGone, c.viewsShut = time.Now(), false
		return true
	}
	c.viewsShut = !c.viewsShut
	return true
}

// viewsSince is the views to show after you dismissed them at gone: all of
// them once the agent has changed one since, else none.
func viewsSince(vs []host.View, gone time.Time) []host.View {
	if gone.IsZero() || slices.ContainsFunc(vs, func(v host.View) bool { return v.Updated.After(gone) }) {
		return vs
	}
	return nil
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
func paneViewsSide(out []string, top, h, contentW, side int, vs []host.View, shut bool) {
	col := viewsPanel(vs, side, h, time.Now(), shut)
	for y := 0; y < h && top+y < len(out); y++ {
		out[top+y] = fit(out[top+y], contentW) + reset + strings.TrimRight(col[y], "")
	}
}
