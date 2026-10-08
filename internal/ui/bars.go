package ui

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/statusline"
	"github.com/0xdeafcafe/rush/internal/sysinfo"
)

// --- rush's own status lines ---

// The top bar (top right of the window, about every agent) and the agent
// header (top of an agent's Session) are status lines you build in
// /statusline, like Claude Code's, from the segments below. What always
// shows stays put: the counts and clanker, an agent's name, state and
// connection, and anything that's wrong.

// barSeg is one thing a rush line can show. draw returns it painted, or
// "" when it has nothing to say.
type barSeg struct {
	id, name, about string
	draw            func(x *barCtx) string
}

// barCtx is what a line is drawn from: the fleet, and for the agent
// header the agent and its conversation.
type barCtx struct {
	m *Model
	t tally
	a *fleet.Agent
	c *hostConn
	// room is what's left of the line's width where a segment starts.
	room int
}

const (
	barTop = iota
	barAgent
)

var topSegs = []barSeg{
	{"plan", "Plan summary", "the default provider’s usage, with fixed window order; add Plan usage for all accounts, meters and reset times", func(x *barCtx) string { return x.m.quietPlan() }},
	{"memory", "Agent memory", "memory held by agents and their processes, with a fixed-width reading", func(x *barCtx) string {
		return dim(fmt.Sprintf("%4.1fG RAM", float64(x.m.snap.Machine.TotalMem)/(1<<30)))
	}},
	{"system", "System alerts", "network trouble, low disk or low battery; quiet while healthy", func(x *barCtx) string { return quietSystem() }},
	{"statushelp", "Status customization", "opens the status-line editor: full CPU, tokens, storage, battery and detailed usage remain available", func(x *barCtx) string { return faint("#statusline details") }},

	{"today", "Spend today", "what every account has spent today", func(x *barCtx) string {
		return paint(cText, money(x.t.today)) + dim(" today")
	}},
	{"usage", "Plan usage", "the account in use: its 5-hour and weekly limits, and when they reset; then every other provider's, the least room first", func(x *barCtx) string {
		return x.m.activeUsage(x.room)
	}},
	{"accounts", "Every account", "each account's shortest limit, of every agent, when there are several", func(x *barCtx) string {
		var parts []string
		for _, r := range x.m.accountRows() {
			// The shortest window: the one that moves.
			if !r.head && len(r.q.Windows) > 0 {
				p := r.q.Windows[0].Percent
				parts = append(parts, dim(r.name()+" ")+paint(usageColor(p), fmt.Sprintf("%.0f%%", p)))
			}
		}
		if len(parts) < 2 {
			return ""
		}
		return strings.Join(parts, "  ")
	}},
	{"agents", "Agents", "how many agents there are, and how many are running", func(x *barCtx) string {
		live, n := 0, 0
		for _, av := range x.m.snap.Accounts {
			live, n = live+av.Live, n+av.Agents
		}
		if n == 0 {
			return ""
		}
		return dim(fmt.Sprintf("%d agents · %d running", n, live))
	}},
	{"ram", "Memory", "what agents and their processes hold", func(x *barCtx) string {
		return dim(mem(x.m.snap.Machine.TotalMem) + " ram")
	}},
	{"tokens", "Tokens", "context tokens held by every agent still working", func(x *barCtx) string {
		var n int64
		for _, a := range x.m.snap.Agents {
			if a.Live() {
				n += a.Spend.Context
			}
		}
		if n == 0 {
			return ""
		}
		return dim(tokens(n) + " tokens")
	}},
	{"cpu", "CPU", "what agents and their processes use", func(x *barCtx) string {
		return dim(fmt.Sprintf("%.0f%% cpu", x.m.snap.Machine.TotalCPU))
	}},
	{"net", "Network", "whether the network has been steady: yellow while the API has lately failed, been slow, or the network changed, offline in red while it can't be reached · #network says more", func(x *barCtx) string {
		return netSeg()
	}},
	{"tmp", "Temp files", "agents' scratch left on disk, when there's much", func(x *barCtx) string {
		if t := x.m.tempTotal(); t >= tempShown {
			return dim(disk(t) + " tmp")
		}
		return ""
	}},
	{"disk", "Free disk", "space left on the disk agents work on: yellow under 20G, red under 5G", func(x *barCtx) string {
		_, d := sysinfo.Now()
		if d.Total == 0 {
			return ""
		}
		v := disk(int64(d.Free)) + " free"
		switch {
		case d.Free < 5<<30:
			return paint(cRed, v)
		case d.Free < 20<<30:
			return paint(cYellow, v)
		}
		return dim(v)
	}},
	{"battery", "Battery", "the battery's charge, ⚡ while plugged in: yellow under 30%, red under 15% off power", func(x *barCtx) string {
		b, _ := sysinfo.Now()
		if !b.Present {
			return ""
		}
		v := fmt.Sprintf("%d%% batt", b.Percent)
		if b.Charging {
			return dim("⚡" + v)
		}
		switch {
		case b.Percent < 15:
			return paint(cRed, v)
		case b.Percent < 30:
			return paint(cYellow, v)
		}
		return dim(v)
	}},
	{"account", "Account", "the agent and account new agents start on, and their profile", func(x *barCtx) string {
		return x.m.startTag()
	}},
	{"clock", "Clock", "the time of day", func(x *barCtx) string {
		return dim(x.m.snap.At.Local().Format("15:04"))
	}},
	{"version", "rush version", "the rush this window runs: yellow once a newer one is installed (#reload), orange while a newer one is out (#update)", func(x *barCtx) string {
		return x.m.versionTag()
	}},
}

// versionTag is this window's rush, so two windows can be told apart, and
// what to do when it's behind.
func (m *Model) versionTag() string {
	v := "rush " + m.version
	switch {
	case m.rebuilt:
		return paint(cYellow, v+" · older than installed · #reload")
	case m.newer.Version != "" && !m.updating:
		return paint(cOrange, v+" · "+m.newer.Short()+" is out · #update")
	}
	return faint(v)
}

var agentSegs = []barSeg{
	{"context", "Context", "context fullness; click the header readout for token counts and breakdown", func(x *barCtx) string { return headerContext(x.c, x.a) }},
	{"context-detail", "Context details", "tokens used, context capacity and fullness gauge", func(x *barCtx) string {
		s := x.c.sess
		if s.Context <= 0 {
			return ""
		}
		f := ctxFill(x.a, int64(s.Context), int64(s.ContextWindow()))
		return dim("ctx ") + ctxBar(f.Pct()) + " " + paint(cSub, tokens(f.Used)+"/"+tokens(f.Window)+fmt.Sprintf(" %.0f%%", f.Pct()))
	}},
	{"cost", "Cost", "what the agent has cost so far", func(x *barCtx) string {
		if c := x.c.sess.Info.CostUSD; c > 0 {
			return paint(cText+bold, money(c))
		}
		return ""
	}},
	{"billing", "Billing", "how it's paid for, when not out of a plan: extra usage in yellow, an API key in orange", func(x *barCtx) string {
		switch usage.Billing(x.c.sess.Info.Billing) {
		case usage.Overage:
			return paint(cYellow, "extra usage")
		case usage.Metered:
			return paint(cOrange, "API billed")
		}
		return ""
	}},
	{"folder", "Folder", "the folder it works in", func(x *barCtx) string {
		return dim(locationFolder(x))
	}},
	{"branch", "Git branch", "the branch checked out there", func(x *barCtx) string {
		_, repo, root, branch := sessionLocation(x.a, x.c)
		return paint(cSub, ansi.Truncate(locationBranch(repo, root, branch), max(0, x.room), "…"))
	}},
	{"model", "Model", "the model it runs", func(x *barCtx) string {
		if md := firstNonEmpty(x.c.sess.Model, x.c.sess.Info.Model); md != "" {
			return dim(convo.PrettyModel(md))
		}
		return ""
	}},
	{"rush", "Older rush", "shown when its host runs an older rush than the one installed", func(x *barCtx) string {
		if x.c.client != nil && x.c.sess.Info.Stale(x.m.upd.installed) {
			return paint(cYellow, "older rush")
		}
		return ""
	}},
	{"effort", "Effort", "the effort level", func(x *barCtx) string {
		return dim(x.c.sess.Effort())
	}},
	{"mode", "Permissions", "the permission mode: plan in blue, the ones that don't ask in orange", func(x *barCtx) string {
		mode := x.c.sess.Info.PermissionMode
		switch mode {
		case "":
			return ""
		case "plan":
			return paint(cBlue, mode)
		case "auto", "acceptEdits", "bypassPermissions":
			return paint(cOrange, mode)
		}
		return paint(cSub, mode)
	}},
	{"tmp", "Temp files", "its scratch left on disk, when there's much", func(x *barCtx) string {
		a := x.a
		if a.Temp < tempShown {
			return ""
		}
		t := paint(cSub, disk(a.Temp)+" tmp")
		if a.Temp >= 1<<30 {
			t = paint(cYellow, disk(a.Temp)+" tmp")
		}
		if a.PID == 0 {
			t += dim(" /clean")
		}
		return t
	}},
	{"lines", "Lines changed", "lines it added and removed", func(x *barCtx) string {
		if x.c.sess.Partial {
			return dim("counting…")
		}
		add, del := 0, 0
		for _, f := range x.c.sess.Changes() {
			add, del = add+f.Add, del+f.Del
		}
		if add+del == 0 {
			return ""
		}
		return paint(cGreen, fmt.Sprintf("+%d", add)) + " " + paint(cRed, fmt.Sprintf("−%d", del))
	}},
	{"files", "Files changed", "how many files it changed", func(x *barCtx) string {
		if x.c.sess.Partial {
			return dim("counting…")
		}
		if n := len(x.c.sess.Changes()); n > 0 {
			return dim(fmt.Sprintf("%d file%s", n, plural(n)))
		}
		return ""
	}},
	{"turns", "Turns", "how many of your messages it has had", func(x *barCtx) string {
		if x.c.sess.Partial {
			return dim("counting…")
		}
		if n := len(x.c.sess.Turns); n > 0 {
			return dim(fmt.Sprintf("%d turn%s", n, plural(n)))
		}
		return ""
	}},
	{"time", "Time", "how long since its first turn", func(x *barCtx) string {
		if x.c.sess.Partial {
			return dim("counting…")
		}
		ts := x.c.sess.Turns
		if len(ts) == 0 || ts[0].Start.IsZero() {
			return ""
		}
		return dim(dur(time.Since(ts[0].Start).Round(time.Minute)))
	}},
	{"cache", "Prompt cache", "whether the cache is warm, and for how long: a message after that re-reads everything", func(x *barCtx) string {
		at, cold := x.c.sess.CacheCold(time.Now())
		switch {
		case at.IsZero():
			return ""
		case cold:
			return paint(cYellow, "cache cold") + dim(" "+dur(time.Since(at))+" ago")
		}
		left := time.Until(at)
		col := cGreen
		if left < 10*time.Minute {
			col = cYellow
		}
		return dim("warm till ") + paint(col, at.Local().Format("15:04")) + dim(" · "+dur(left)+" left")
	}},
	{"queue", "Queue", "messages waiting for the turn to end", func(x *barCtx) string {
		if n := len(x.c.sess.Info.Queue); n > 0 {
			return paint(cOrange, fmt.Sprintf("%d queued", n))
		}
		return ""
	}},
	{"account", "Account", "the agent it runs", func(x *barCtx) string {
		return dim(agentName(x.a.Kind))
	}},
	{"session", "Session id", "the conversation's short id", func(x *barCtx) string {
		if id := firstNonEmpty(x.c.sess.Info.SessionID, x.a.SessionID); len(id) >= 8 {
			return faint(id[:8])
		}
		return ""
	}},
	{"clock", "Clock", "the time of day", func(x *barCtx) string {
		return dim(time.Now().Format("15:04"))
	}},
}

func barSegs(which int) []barSeg {
	if which == barTop {
		return topSegs
	}
	return agentSegs
}

func findBarSeg(which int, id string) (barSeg, bool) {
	for _, s := range barSegs(which) {
		if s.id == id {
			return s, true
		}
	}
	return barSeg{}, false
}

func usageColor(p float64) string {
	switch {
	case p >= 85:
		return cRed
	case p >= 60:
		return cYellow
	}
	return cGreen
}

// barLayout is the line being drawn: the one saved, or the one being built
// in /statusline, so the real header shows your changes as you make them.
func (m *Model) barLayout(which int) statusline.Layout {
	if st, ok := m.sheet.(*statusSheet); ok {
		switch {
		case which == barTop && st.tab == stTop:
			return st.bars.Top
		case which == barAgent && st.tab == stAgent:
			return st.bars.Agent
		}
	}
	if which == barTop {
		if m.bars.Top.Lines == nil {
			return statusline.DefaultTop()
		}
		return m.bars.Top
	}
	if m.bars.Agent.Lines == nil {
		return statusline.DefaultAgent()
	}
	return m.bars.Agent
}

// barMore ends a line that had to leave segments out for room.
func barMore() string { return faint(" ⋯") }

// barLine draws line i of a rush line, at most w wide. When it's too
// wide, the segments last on the line go first, whole, and the line ends
// with ⋯; which went is kept for /statusline to point out.
func (m *Model) barLine(which, i int, x *barCtx, w int) string {
	l := m.barLayout(which)
	if i >= len(l.Lines) || w <= 0 {
		return ""
	}
	sep := l.Sep
	if sep == "" {
		sep = " · "
	}
	var parts, ids []string
	for _, id := range l.Lines[i] {
		if s, ok := findBarSeg(which, id); ok {
			x.room = w - ansi.StringWidth(strings.Join(append(parts, ""), dim(sep)))
			if p := s.draw(x); p != "" {
				parts, ids = append(parts, p), append(ids, id)
			}
		}
	}
	line := strings.Join(parts, dim(sep))
	var dropped []string
	if ansi.StringWidth(line) > w {
		for len(parts) > 0 && ansi.StringWidth(strings.Join(parts, dim(sep)))+ansi.StringWidth(barMore()) > w {
			k := leastKept(ids)
			dropped = append(dropped, ids[k])
			parts, ids = slices.Delete(parts, k, k+1), slices.Delete(ids, k, k+1)
		}
		line = strings.Join(parts, dim(sep)) + barMore()
		if len(parts) == 0 {
			line = strings.TrimLeft(barMore(), " ")
		}
	}
	if m.barDrops == nil {
		m.barDrops = map[int][]string{}
	}
	m.barDrops[which*10+i] = dropped
	return line
}

// barKeep is how long a segment holds on when its line is short of room:
// the higher, the later it goes. The rest are 3.
var barKeep = map[string]int{
	"clock": 9, "time": 9,
	"plan": 8, "usage": 8, "context": 8, "system": 8,
	"today": 7, "model": 7, "queue": 7,
	"agents": 6, "effort": 6, "mode": 6, "folder": 6, "branch": 6,
	"account": 5, "cost": 5, "billing": 5, "battery": 5,
	"cache":    4,
	"accounts": 2, "context-detail": 2,
	"version": 1, "rush": 1, "statushelp": 1, "session": 1,
}

// leastKept is which of ids goes first: the lowest barKeep, the last of
// those.
func leastKept(ids []string) int {
	keep := func(id string) int {
		if k, ok := barKeep[id]; ok {
			return k
		}
		return 3
	}
	at := len(ids) - 1
	for i := len(ids) - 2; i >= 0; i-- {
		if keep(ids[i]) < keep(ids[at]) {
			at = i
		}
	}
	return at
}

// barDropped is the segments the header last left out for room.
func (m *Model) barDropped(which int) map[string]bool {
	out := map[string]bool{}
	for i := range statusline.TopLines {
		for _, id := range m.barDrops[which*10+i] {
			out[id] = true
		}
	}
	return out
}
