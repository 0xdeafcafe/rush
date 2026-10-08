package ui

import (
	"fmt"
	"strings"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/efficiency"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// leftLines are what the agent at key wrote outside its project and
// scratch, as last looked at: the places, a few, and what they come to.
func (m *Model) leftLines(key string, w int) []convo.Line {
	a := m.agentByKey(key)
	if a == nil || len(a.Left) == 0 {
		return nil
	}
	var mine []fleet.LeftItem
	var size int64
	for _, l := range m.clean.outside {
		if l.Agent.Key == key && !l.Gone {
			mine, size = append(mine, l), size+l.Size
		}
	}
	head := fmt.Sprintf("    %d places written outside it", len(a.Left))
	if mine != nil {
		head = fmt.Sprintf("    %d places written outside it, %d still there · %s", len(a.Left), len(mine), disk(size))
	}
	out := []convo.Line{{Text: fit(dim(head+" · Projects, c cleans up"), w)}}
	for i, l := range mine {
		if i == 5 {
			out = append(out, convo.Line{Text: dim(fmt.Sprintf("      and %d more", len(mine)-5))})
			break
		}
		out = append(out, convo.Line{Text: fit("      "+paint(cSub, tildify(l.Path))+dim(" · "+disk(l.Size)+leftNote(l)), w)})
	}
	return out
}

// leftNote is what holds a left item back, or that it's scratch.
func leftNote(l fleet.LeftItem) string {
	switch {
	case l.Why() != "":
		return " · " + l.Why()
	case l.Temp:
		return " · scratch"
	}
	return " · outside temp"
}

// overviewSections are the Overview's own rows past the session's report:
// the pages it published, and a link to the Memory tab.
func (m *Model) overviewSections(c *hostConn, o convo.Options) []convo.Line {
	w := o.Width
	var out []convo.Line
	head := func(title, meta string) {
		h := "  " + faint("▾") + " " + paint(cSub+bold, title)
		if meta != "" {
			h += "  " + dim(meta)
		}
		out = append(out, convo.Line{}, convo.Line{Text: fit(h+" "+faint(strings.Repeat("─", max(0, w-cellwidth(h)-3))), w)})
	}
	if arts := c.artifactsOf(); len(arts) > 0 {
		head("Artifacts", fmt.Sprintf("%d published · enter opens one", len(arts)))
		out = append(out, m.artifactLines(c, o)[2:]...) // past its own heading
	}
	if t := c.sess.Info.TempDir; t != "" {
		// Its scratch, to find what it left behind.
		head("Scratch", "TMPDIR for it and everything it runs")
		row := "    " + paint(cText, tildify(t))
		if a := m.agentByKey(c.key); a != nil && a.Temp > 0 {
			row += dim(" · " + disk(a.Temp))
		}
		out = append(out, convo.Line{Text: fit(row, w)})
		out = append(out, m.leftLines(c.key, w)...)
	}
	if canScreen(c, "memory") {
		// A link to the Memory tab, with what it costs.
		files := m.memoryOf(c)
		var up int64
		for _, f := range files {
			up += f.Up
		}
		head("Memory", "")
		row := "    " + paint(cBlue, "→ ") + paint(cText, fmt.Sprintf("%d files", len(files))) + dim(fmt.Sprintf(" · ≈%s tokens every session", efficiency.Tokens(up)))
		right := dim("enter opens the Memory tab") + "  "
		if c.sel == "go:memory" {
			bar := faint("▍")
			if o.Focused {
				bar = paint(cOrange, "▍")
			}
			out = append(out, convo.Line{Text: selBG + strings.ReplaceAll(bar+fit(spread(row, right, w), w)[1:], reset, reset+selBG) + reset, Ref: "go:memory"})
		} else {
			out = append(out, convo.Line{Text: fit(spread(row, right, w), w), Ref: "go:memory"})
		}
	}
	return out
}
