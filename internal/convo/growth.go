package convo

import (
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// --- how the context grew, turn by turn, and what changed at a restart ---

// ContextStep is one turn's context: what its last request sent (input,
// cache read and cache written), and, if the agent compacted during it,
// how big the context was when it did.
type ContextStep struct {
	Turn      int // the turn's N
	Tokens    int
	Compacted int
}

// ContextGrowth is the context each turn ended with, from the requests
// already recorded: the main agent's only, as subagents have their own.
// Turns that sent nothing and didn't compact are left out. Kept until the
// next event, as the Context tab asks every frame.
func (s *Session) ContextGrowth() []ContextStep {
	if m := &s.growthMemo; m.steps != nil && m.applied == s.applied {
		return m.steps
	}
	steps := s.contextGrowth()
	s.growthMemo.applied, s.growthMemo.steps = s.applied, steps
	if steps == nil {
		s.growthMemo.steps = []ContextStep{} // known to be none
	}
	return s.growthMemo.steps
}

func (s *Session) contextGrowth() []ContextStep {
	n := len(s.Turns)
	if n == 0 {
		return nil
	}
	tokens := make([]int, n)
	sent := make([]bool, n)
	for _, r := range s.Requests {
		if r.Run != "" || r.Agent != "" {
			continue
		}
		// The turn it went in: the last one started by then.
		i := sort.Search(n, func(i int) bool { return s.Turns[i].Start.After(r.At) }) - 1
		if i < 0 {
			i = 0
		}
		u := r.Usage
		tokens[i] = int(u.Input + u.CacheRead + u.CacheWrite5m + u.CacheWrite1h)
		sent[i] = true
	}
	var out []ContextStep
	for i, t := range s.Turns {
		step := ContextStep{Turn: t.N, Tokens: tokens[i]}
		for _, it := range t.Items {
			if it.Kind == KCompact && it.Compact != nil && it.Compact.Before > step.Compacted {
				step.Compacted = it.Compact.Before
			}
		}
		if sent[i] || step.Compacted > 0 {
			out = append(out, step)
		}
	}
	return out
}

// ContextGrowthLines draws ContextGrowth in w cells: a bar per turn, the
// newest at the right, the turns it compacted in yellow; then where it
// started and ended, its peak, and the compactions with the size each
// compacted from.
func ContextGrowthLines(steps []ContextStep, lead string, w int) []string {
	if len(steps) < 2 || w < 8 {
		return nil
	}
	from := max(0, len(steps)-w)
	shown := steps[from:]
	peak := 0
	for _, st := range shown {
		peak = max(peak, st.Tokens, st.Compacted)
	}
	if peak == 0 {
		return nil
	}
	levels := []rune("▁▂▃▄▅▆▇█")
	var b strings.Builder
	for _, st := range shown {
		v := max(st.Tokens, st.Compacted)
		g := string(levels[min(len(levels)-1, v*len(levels)/(peak+1))])
		if st.Compacted > 0 {
			b.WriteString(paint(cYellow, g))
			continue
		}
		b.WriteString(paint(ctxColour("Messages"), g))
	}
	first, last := shown[0], shown[len(shown)-1]
	caption := fmt.Sprintf("#%d %s → #%d %s · peak %s", first.Turn, tokens(first.Tokens), last.Turn, tokens(last.Tokens), tokens(peak))
	if from > 0 {
		caption = fmt.Sprintf("last %d turns · ", len(shown)) + caption
	}
	out := []string{lead + b.String(), lead + dim(caption)}
	var comp []string
	for _, st := range steps {
		if st.Compacted > 0 {
			comp = append(comp, fmt.Sprintf("#%d from %s", st.Turn, tokens(st.Compacted)))
		}
	}
	if len(comp) > 0 {
		more := ""
		if len(comp) > 3 {
			more, comp = fmt.Sprintf(" · %d before", len(comp)-3), comp[len(comp)-3:]
		}
		out = append(out, lead+paint(cYellow, "▮ ")+dim("compacted "+strings.Join(comp, " · ")+more))
	}
	return out
}

// ContextChangeLines draws what changed since the agent's last start:
// tools and MCP servers added and removed, rush's prompt sections (its
// estimate) and the agent's own categories (its count). The agent's base
// prompt isn't among them: rush never sees its text.
func ContextChangeLines(c *usage.Change, harness, lead string, w int, now time.Time) []string {
	if c.Empty() {
		return nil
	}
	head := lead + paint(cSub+bold, "Since the last start")
	if !c.At.IsZero() {
		head += dim(" · restarted " + dur(now.Sub(c.At).Truncate(time.Second)) + " ago")
	}
	out := []string{head}
	names := func(xs []string) string {
		return cellw.Truncate(strings.Join(xs, ", "), max(10, w-len(lead)-20), "…")
	}
	// A server's tools go with it, counted, rather than listed.
	of := func(tools []string, server string) (n int, rest []string) {
		for _, t := range tools {
			if strings.HasPrefix(t, "mcp__"+server+"__") {
				n++
				continue
			}
			rest = append(rest, t)
		}
		return n, rest
	}
	added, removed := c.ToolsAdded, c.ToolsRemoved
	mark := func(plus bool) string {
		if plus {
			return paint(cGreen, "+ ")
		}
		return paint(cRed, "− ")
	}
	for _, sv := range c.ServersAdded {
		n, rest := of(added, sv)
		added = rest
		out = append(out, lead+"  "+mark(true)+text("MCP server "+sv)+dim(toolCount(n)))
	}
	for _, sv := range c.ServersRemoved {
		n, rest := of(removed, sv)
		removed = rest
		out = append(out, lead+"  "+mark(false)+text("MCP server "+sv)+dim(toolCount(n)))
	}
	if len(added) > 0 {
		out = append(out, lead+"  "+mark(true)+text(plural(len(added), "tool"))+dim("  "+names(added)))
	}
	if len(removed) > 0 {
		out = append(out, lead+"  "+mark(false)+text(plural(len(removed), "tool"))+dim("  "+names(removed)))
	}
	for _, d := range c.Sections {
		out = append(out, lead+"  "+deltaRow(d, "rush's prompt: "+d.Name, "")+faint(" · estimated"))
	}
	if len(c.Parts) > 0 {
		out = append(out, lead+"  "+dim("as counted by "+harness+":"))
		for _, d := range c.Parts {
			out = append(out, lead+"    "+deltaRow(d, d.Name, paint(ctxColour(d.Name), "█ ")))
		}
	}
	return out
}

func toolCount(n int) string {
	if n == 0 {
		return ""
	}
	return " · " + plural(n, "tool")
}

// deltaRow is one size that changed: + for added, − for gone, ~ for
// changed, with its tokens before and after.
func deltaRow(d usage.Delta, name, swatch string) string {
	switch {
	case d.Added:
		return paint(cGreen, "+ ") + swatch + text(fitTo(name, 30)) + " " + dim(fmt.Sprintf("%6s", fineTokens(d.Now)))
	case d.Gone:
		return paint(cRed, "− ") + swatch + text(fitTo(name, 30)) + " " + dim(fmt.Sprintf("%6s", fineTokens(d.Was)))
	}
	sign, col := "+", cYellow
	if d.Change() < 0 {
		sign, col = "−", cGreen
	}
	return faint("~ ") + swatch + text(fitTo(name, 30)) + " " + dim(fmt.Sprintf("%6s → %s ", fineTokens(d.Was), fineTokens(d.Now))) + paint(col, sign+fineTokens(abs(d.Change())))
}

// fineTokens is tokens with a decimal under 10k, where a change of a few
// hundred would otherwise round away.
func fineTokens(n int) string {
	if n >= 1000 && n < 10_000 {
		return strings.TrimSuffix(fmt.Sprintf("%.1f", float64(n)/1e3), ".0") + "k"
	}
	return tokens(n)
}

func abs(n int) int {
	if n < 0 {
		return -n
	}
	return n
}
