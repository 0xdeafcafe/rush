package convo

import (
	"fmt"
	"sort"
	"strings"

	"github.com/0xdeafcafe/photon/theme"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// --- what fills the context window ---

// ctxHues are the categories' colours, each its own hue so neighbours in
// the bar stay apart: what rush is given (blue, violet), what's plugged in
// (green, teal, pink), what's read in (white, yellow), what's said
// (orange, as rush's own).
var ctxHues = map[string]theme.RGB{
	"System prompt": {R: 86, G: 140, B: 230},
	"System tools":  {R: 160, G: 130, B: 220},
	"MCP tools":     {R: 127, G: 191, B: 138},
	"MCP server":    {R: 70, G: 180, B: 170},
	"Custom agents": {R: 220, G: 120, B: 170},
	"Memory":        {R: 235, G: 230, B: 220},
	"Skills":        {R: 225, G: 190, B: 80},
	"Messages":      {R: 217, G: 119, B: 87},
}

// ctxInk is each of ctxHues fitted to the terminal's ground.
var ctxInk = map[string]string{}

// ctxColour is a category's colour, the same in the overview and /context.
func ctxColour(name string) string {
	best := ""
	for prefix := range ctxHues {
		if strings.HasPrefix(name, prefix) && len(prefix) > len(best) {
			best = prefix
		}
	}
	if best == "" {
		return cDim
	}
	return ctxInk[best]
}

// ctxParts are the categories in the window, in Claude Code's order:
// what's used, then the auto-compact buffer, then what's free. Deferred
// tools aren't in the window, so they're left out.
func ctxParts(u *usage.Context) (used []usage.ContextPart, buffer, free int) {
	for _, c := range u.Parts {
		switch {
		case c.Deferred || c.Kind == "deferred":
		case c.Kind == "buffer":
			buffer += c.Tokens
		case c.Kind == "free":
			free += c.Tokens
		default:
			used = append(used, c)
		}
	}
	return used, buffer, free
}

// ContextBar is the window as one stacked bar w cells wide: each category
// in its colour, then the auto-compact buffer, then what's free.
func ContextBar(u *usage.Context, w int) string {
	if u == nil || u.Max <= 0 || w <= 0 {
		return ""
	}
	used, buffer, _ := ctxParts(u)
	var b strings.Builder
	n, at := 0, 0
	for _, c := range used {
		at += c.Tokens
		end := min(w, (at*w+u.Max/2)/u.Max)
		if c.Tokens > 0 && end <= n && n < w {
			end = n + 1 // everything there shows, if only as one cell
		}
		b.WriteString(paint(ctxColour(c.Name), strings.Repeat("█", max(0, end-n))))
		n = max(n, end)
	}
	bufAt := w - (buffer*w+u.Max/2)/u.Max
	if n < bufAt {
		b.WriteString(faint(strings.Repeat("─", bufAt-n)))
		n = bufAt
	}
	b.WriteString(faint(strings.Repeat("░", max(0, w-n))))
	return b.String()
}

// ContextLegend is a row per category: its colour, name, tokens and share.
func ContextLegend(u *usage.Context, lead string) []string {
	if u == nil || u.Max <= 0 {
		return nil
	}
	used, buffer, free := ctxParts(u)
	pct := func(n int) string { return fmt.Sprintf("%5.1f%%", float64(n)/float64(u.Max)*100) }
	var out []string
	for _, c := range used {
		out = append(out, lead+paint(ctxColour(c.Name), "█ ")+text(fitTo(c.Name, 20))+" "+text(fmt.Sprintf("%6s", tokens(c.Tokens)))+" "+dim(pct(c.Tokens)))
	}
	if buffer > 0 {
		out = append(out, lead+faint("░ ")+dim(fitTo("Auto-compact buffer", 20))+" "+dim(fmt.Sprintf("%6s", tokens(buffer)))+" "+faint(pct(buffer)))
	}
	out = append(out, lead+faint("─ ")+dim(fitTo("Free", 20))+" "+dim(fmt.Sprintf("%6s", tokens(free)))+" "+faint(pct(free)))
	return out
}

// ContextDetail is what's inside the categories: the biggest skills, MCP
// servers, agents, memory files and tools' calls and results.
func ContextDetail(u *usage.Context, lead string, each int) []string { //nolint:gocognit // one group per part, each the same few steps
	if u == nil {
		return nil
	}
	var out []string
	group := func(title string, rows [][2]string) {
		if len(rows) == 0 {
			return
		}
		if len(out) > 0 {
			out = append(out, "")
		}
		out = append(out, lead+paint(cSub+bold, title))
		for i, r := range rows {
			if i == each {
				out = append(out, lead+"  "+dim(fmt.Sprintf("… %d more", len(rows)-each)))
				break
			}
			out = append(out, lead+"  "+text(fitTo(r[0], 34))+" "+dim(r[1]))
		}
	}
	type nt struct {
		name string
		n    int
	}
	sorted := func(xs []nt) [][2]string {
		sort.SliceStable(xs, func(i, j int) bool { return xs[i].n > xs[j].n })
		var rows [][2]string
		for _, x := range xs {
			rows = append(rows, [2]string{x.name, tokens(x.n)})
		}
		return rows
	}
	if len(u.Skills.Each) > 0 {
		var xs []nt
		for _, s := range u.Skills.Each {
			xs = append(xs, nt{s.Name, s.Tokens})
		}
		group(fmt.Sprintf("Skills · %d of %d listed · %s", u.Skills.Included, u.Skills.Total, tokens(u.Skills.Tokens)), sorted(xs))
	}
	if len(u.MCPTools) > 0 {
		by := map[string]int{}
		n := map[string]int{}
		for _, t := range u.MCPTools {
			by[t.Server] += t.Tokens
			n[t.Server]++
		}
		var xs []nt
		for s, t := range by {
			xs = append(xs, nt{fmt.Sprintf("%s (%d tools)", s, n[s]), t})
		}
		group("MCP servers", sorted(xs))
	}
	if len(u.Agents) > 0 {
		var xs []nt
		for _, a := range u.Agents {
			xs = append(xs, nt{a.Type, a.Tokens})
		}
		group("Custom agents", sorted(xs))
	}
	if len(u.MemoryFiles) > 0 {
		var xs []nt
		for _, f := range u.MemoryFiles {
			xs = append(xs, nt{f.Path, f.Tokens})
		}
		group("Memory files", sorted(xs))
	}
	m := u.Messages
	if m.ToolCalls+m.ToolResults+m.User+m.Assistant+m.Attachments > 0 {
		xs := []nt{{"tool results", m.ToolResults}, {"tool calls", m.ToolCalls}, {"Claude's words", m.Assistant}, {"your messages", m.User}, {"attachments", m.Attachments}}
		var keep []nt
		for _, x := range xs {
			if x.n > 0 {
				keep = append(keep, x)
			}
		}
		group("Messages", sorted(keep))
	}
	if len(m.ByTool) > 0 {
		var xs []nt
		for _, t := range m.ByTool {
			xs = append(xs, nt{t.Name, t.Calls + t.Results})
		}
		group("Messages by tool", sorted(xs))
	}
	return out
}

// ContextGrid is the window as rows of cells, as Claude Code's /context
// draws it: each cell a share of the window, in its category's colour;
// the free space dotted and the auto-compact buffer shaded at the end.
func ContextGrid(u *usage.Context, cols, rows int) []string {
	if u == nil || u.Max <= 0 || cols <= 0 || rows <= 0 {
		return nil
	}
	used, buffer, _ := ctxParts(u)
	n := cols * rows
	cells := make([]string, 0, n)
	at := 0
	for _, c := range used {
		at += c.Tokens
		end := min(n, (at*n+u.Max/2)/u.Max)
		if c.Tokens > 0 && end <= len(cells) && len(cells) < n {
			end = len(cells) + 1
		}
		for len(cells) < end {
			cells = append(cells, paint(ctxColour(c.Name), "■"))
		}
	}
	bufAt := n - (buffer*n+u.Max/2)/u.Max
	for len(cells) < bufAt {
		cells = append(cells, faint("·"))
	}
	for len(cells) < n {
		cells = append(cells, faint("░"))
	}
	out := make([]string, rows)
	for r := range rows {
		out[r] = strings.Join(cells[r*cols:(r+1)*cols], " ")
	}
	return out
}

// ContextWindow is the size of the session's context window: as its agent
// said, else as known for its model.
func (s *Session) ContextWindow() int {
	if s.Window > 0 {
		return s.Window
	}
	return int(agent.ContextWindow(agent.Kind(s.Info.Kind), firstNonEmpty(s.Model, s.Info.Model)))
}
