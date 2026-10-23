package ui

import (
	"fmt"
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
	for _, v := range m.viewsOf(a.ID) {
		if v.Summary != "" {
			return v.Summary
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
		return "!", cYellow
	case "busy":
		return "◌", cOrange
	}
	return "·", cDim
}

// viewToneColour is a number's colour for its tone.
func viewToneColour(tone string) string {
	if tone == "" || tone == "quiet" {
		return cBright
	}
	_, c := toneMark(tone)
	return c
}

// viewsStrip is how wide the sidebar is folded: its edge and a mark.
const viewsStrip = 3

// viewsPanel draws views in a column w wide and h tall: first that the
// agent made it (generative UI) and the mark that folds it, then each view,
// its title and when it was updated, its numbers, its groups of rows, and
// its note. Folded, it's a strip with the mark that opens it.
func viewsPanel(vs []host.View, w, h int, now time.Time, shut bool) []string {
	inner := w - 3 // the edge, and a space either side
	var out []string
	add := func(s string) { out = append(out, onBg(panelBG, paint(cOrange, "▍")+" "+fit(s, inner)+" ", w)) }
	if shut {
		out = append(out, onBg(panelBG, paint(cOrange, "▍")+paint(cSub, "‹"), w), onBg(panelBG, paint(cOrange, "▍")+paint(cOrange, "✦"), w))
		for len(out) < h {
			out = append(out, onBg(panelBG, paint(cOrange, "▍"), w))
		}
		return out[:max(0, h)]
	}
	add(spread(paint(cOrange, "✦")+" "+dim("generative UI, made by the agent"), paint(cSub, "›"), inner))
	for vi, v := range vs {
		if vi > 0 {
			add("")
		}
		add("")
		add(spread(paint(cBright+bold, v.Title), dim(ago(now.Sub(v.Updated))), inner))
		if len(v.Stats) > 0 {
			add("")
			cell := inner / len(v.Stats)
			var vals, labels string
			for _, s := range v.Stats {
				vals += fit(paint(viewToneColour(s.Tone)+bold, s.Value), cell)
				labels += fit(dim(s.Label), cell)
			}
			add(vals)
			add(labels)
		}
		for _, g := range v.Groups {
			add("")
			if g.Title != "" {
				add(paint(cSub+bold, g.Title) + dim(fmt.Sprintf("  %d", len(g.Rows))))
			}
			for _, r := range g.Rows {
				mark, col := toneMark(r.Tone)
				meta := ""
				if r.Meta != "" {
					meta = dim(r.Meta)
				}
				room := inner - 2 - cellw.String(meta) - 1
				// A long word on it goes under the row rather than cut its
				// name short.
				if room < 16 || cellw.String(r.Text) > room && cellw.String(meta) > inner/3 {
					room, meta = inner-2, ""
				}
				plain := cellwTrunc(r.Text, room)
				text := paint(cText, plain)
				if r.Link != "" {
					text = link(r.Link, text) // its escapes take no room: laid out by hand
				}
				gap := max(1, inner-2-cellw.String(plain)-cellw.String(meta))
				add(paint(col, mark) + " " + text + strings.Repeat(" ", gap) + meta)
				if meta == "" && r.Meta != "" {
					add("  " + dim(cellwTrunc(r.Meta, inner-2)))
				}
			}
		}
		if v.Note != "" {
			add("")
			for _, l := range wrap(v.Note, inner) {
				add(dim(l))
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

// clickViews is a click on the views sidebar's top row, or anywhere on it
// folded: it folds it to a strip and opens it again.
func (m *Model) clickViews(c *hostConn, x, y int) bool {
	col := x - m.paneX() - c.viewsX
	if c.viewsW == 0 || col < 0 || col >= c.viewsW || y < c.viewsY || !c.viewsShut && y != c.viewsY {
		return false
	}
	c.viewsShut = !c.viewsShut
	return true
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
