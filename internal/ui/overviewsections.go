package ui

import (
	"fmt"
	"strings"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/efficiency"
)

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
