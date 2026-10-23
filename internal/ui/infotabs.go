package ui

import (
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io/fs"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
)

// --- the History and Settings tabs of infoSheet ---

// contextLines is the Context tab: what fills the window, as a grid with
// its legend beside it, then what's inside each part.
func contextLines(c *hostConn, a *fleet.Agent, w int) []string {
	s := c.sess
	u := s.Usage
	if u == nil || u.Max <= 0 {
		var out []string
		if s.Context > 0 {
			out = append(out, infoRow("context", ctxLine(ctxFill(a, int64(s.Context), int64(s.ContextWindow()))), w), "")
		}
		why := "the breakdown comes when this turn ends"
		if note := agent.FeatureOf(sessionAgent(c), agent.FeatureContext).Note; note != "" {
			why = harnessName(string(sessionAgent(c))) + " says " + note
		}
		switch {
		case !canScreen(c, "context"):
			why = harnessName(string(sessionAgent(c))) + " reports context usage without a detailed breakdown"
		case c.sleeping || s.Info.Sleeping:
			why = "this session is idle; the breakdown refreshes when it resumes"
		case c.client == nil:
			why = "a breakdown needs a rush-mode session: sessions outside rush only say how full it is"
		case s.Info.Proto < 2:
			why = "this agent's host is older than the breakdown: it comes once the host restarts (it does after resting idle)"
		case s.Info.ClaudePID == 0:
			why = harnessName(string(sessionAgent(c))) + " is asleep: the breakdown comes when it next wakes"
		}
		return append(append(out, dim("  "+why)), contextSince(c, w)...)
	}
	// Its own count of where it compacts wins over rush's.
	f := ctxFill(a, int64(u.Total), int64(u.Max))
	meta := convo.Tokens(u.Total) + " of " + tokens(f.Window) + fmt.Sprintf(" · %.0f%%", f.Pct())
	if u.AutoCompact && u.AutoCompactAt > 0 {
		meta += " · auto-compacts at " + convo.Tokens(u.AutoCompactAt)
	}
	if f.Compacts() {
		meta += " · model " + tokens(f.Model)
	}
	out := []string{infoHead(convo.PrettyModel(u.Model)) + "  " + dim(meta) + faint("  · #slim drops what it never uses"), ""}
	grid := convo.ContextGrid(u, 20, 10)
	legend := convo.ContextLegend(u, "")
	for i := range max(len(grid), len(legend)) {
		l, r := strings.Repeat(" ", 39), ""
		if i < len(grid) {
			l = grid[i]
		}
		if i < len(legend) {
			r = legend[i]
		}
		out = append(out, "  "+l+"    "+r)
	}
	if d := convo.ContextDetail(u, "  ", 6); len(d) > 0 {
		out = append(append(out, ""), d...)
	}
	when := "just now"
	if ago := time.Since(u.At); ago >= time.Minute {
		when = roughly(ago) + " ago"
	}
	out = append(out, "", faint("  counted "+when+", from the last reply and local estimates"))
	return append(out, contextSince(c, w)...)
}

// contextSince is how the context grew turn by turn, and what changed at
// its last start, each after a blank row; nothing when neither is known.
func contextSince(c *hostConn, w int) []string {
	s := c.sess
	var out []string
	for _, ls := range [][]string{
		convo.ContextGrowthLines(s.ContextGrowth(), "  ", w-4),
		convo.ContextChangeLines(s.Info.ContextChange, harnessName(string(sessionAgent(c))), "  ", w, time.Now()),
	} {
		if len(ls) > 0 {
			out = append(append(out, ""), ls...)
		}
	}
	return out
}

// historyLines is the History tab: the agent's own record of the
// account's use (Claude Code's /stats), by day and by model, fitted to h.
func (k *infoSheet) historyLines(a *fleet.Agent, w, h int) []string {
	if !k.loaded {
		return []string{dim("  reading " + harnessName(a.Kind) + "'s record…")}
	}
	st := k.stats
	if k.statsErr != nil {
		why := harnessName(a.Kind) + " hasn't kept a record for this account yet"
		if !errors.Is(k.statsErr, fs.ErrNotExist) {
			why = "couldn't read its record: " + k.statsErr.Error()
		}
		return []string{dim("  " + why)}
	}
	out := []string{infoHead("All time · " + a.Acct.Name)}
	since := ""
	if !st.First.IsZero() {
		since = dim(" since " + st.First.Local().Format("2 Jan 2006"))
	}
	out = append(out, infoRow("sessions", paint(cText, thousands(int64(st.Sessions)))+dim(" · "+thousands(int64(st.Messages))+" messages")+since, w))
	if st.Longest > 0 {
		out = append(out, infoRow("longest", paint(cText, dur(st.Longest))+dim(fmt.Sprintf(" · %s messages · %s", thousands(int64(st.LongestMsgs)), st.LongestAt.Local().Format("2 Jan"))), w))
	}
	if peak := hourSpark(st.Hours); peak != "" {
		out = append(out, infoRow("by hour", peak, w))
		out = append(out, infoRow("", faint("0     6     12    18   23"), w))
	}

	// Days: as many as fit, newest last, bars by messages.
	days := st.Days
	room := max(5, h-len(out)-4-min(4, len(st.Models)))
	if len(days) > room {
		days = days[len(days)-room:]
	}
	if len(days) > 0 {
		out = append(out, "", infoHead(fmt.Sprintf("Last %d days", len(days))))
		top := 1
		for _, d := range days {
			top = max(top, d.Messages)
		}
		barW := max(8, min(30, w-70))
		for _, d := range days {
			fill := d.Messages * barW / top
			if d.Messages > 0 && fill == 0 {
				fill = 1
			}
			bar := paint(cOrange, strings.Repeat("━", fill)) + faint(strings.Repeat("─", barW-fill))
			nums := fmt.Sprintf("%7s msgs  %3d sessions  %6s tools  %6s tok", short(d.Messages), d.Sessions, short(d.ToolCalls), bigTokens(d.TotalTokens()))
			out = append(out, "  "+dim(fit(d.Date.Format("Mon 2 Jan"), 12))+" "+bar+" "+dim(nums))
		}
	}
	if len(st.Models) > 0 {
		out = append(out, "", infoHead("By model"))
		for _, t := range st.Models {
			out = append(out, infoRow(convo.PrettyModel(t.Model), paint(cText, fit(bigTokens(t.Total()), 9))+dim(bigTokens(t.Out)+" out · "+bigTokens(t.CacheRead)+" from cache"), w))
		}
	}
	if st.Computed != "" {
		out = append(out, "", faint("  as Claude Code last counted it, up to "+st.Computed))
	}
	return out
}

// bigTokens is a token count to three figures, up to billions.
func bigTokens(n int64) string {
	if n >= 1_000_000_000 {
		return fmt.Sprintf("%.1fB", float64(n)/1e9)
	}
	return convo.Tokens(int(n))
}

// short is a count to three figures: 950, 12.6k, 1.3M.
func short(n int) string {
	switch {
	case n >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(n)/1e6)
	case n >= 10_000:
		return fmt.Sprintf("%.0fk", float64(n)/1e3)
	case n >= 1000:
		return fmt.Sprintf("%.1fk", float64(n)/1e3)
	}
	return fmt.Sprint(n)
}

// hourSpark draws the sessions started in each hour as one bar per hour.
func hourSpark(h [24]int) string {
	top := 0
	for _, n := range h {
		top = max(top, n)
	}
	if top == 0 {
		return ""
	}
	bars := []rune("▁▂▃▄▅▆▇█")
	var b strings.Builder
	for _, n := range h {
		if n == 0 {
			b.WriteString(faint("·"))
			continue
		}
		b.WriteString(paint(cOrange, string(bars[min(len(bars)-1, n*len(bars)/(top+1))])))
	}
	return b.String()
}

// settingsLink is a row of the Settings tab: an area of settings, what it
// holds now, and the sheet that edits it.
type settingsLink struct {
	name, value, about string
	screen             string // the screen it opens, whose features it needs
	claude             bool   // opens Claude Code's own screen
	open               func(m *Model) tea.Cmd
}

// settingsCounts is what the Settings tab shows of the settings files
// and commands, read off the UI.
type settingsCounts struct {
	allow, ask, deny, hooks, plugins, env, skills, cmds int
	model, mode, line                                   string
}

// readSettingsCounts reads every settings file a session of kind on acct
// in cwd reads, and its commands and skills. It reads the disk.
func readSettingsCounts(kind agent.Kind, acct agent.Profile, cwd string) settingsCounts {
	var n settingsCounts
	for i, f := range settingsFiles(kind, acct, cwd) {
		if s, err := settingsfile.Load(f.path); err == nil {
			n.add(s, i == 0)
		}
	}
	if cm, ok := agent.As[agent.Commander](kind); ok {
		for _, f := range cm.Commands(acct, cwd) {
			if f.Skill {
				n.skills++
			} else {
				n.cmds++
			}
		}
	}
	return n
}

// add counts one settings file in; the first is the account's own, whose
// env block is the one shown.
func (n *settingsCounts) add(s *settingsfile.File, first bool) {
	var rules []string
	for _, kind := range []struct {
		key string
		n   *int
	}{{"permissions.allow", &n.allow}, {"permissions.ask", &n.ask}, {"permissions.deny", &n.deny}} {
		rules = nil
		if s.Get(kind.key, &rules) {
			*kind.n += len(rules)
		}
	}
	var hs map[string][]jsontext.Value
	if s.Get("hooks", &hs) {
		for _, list := range hs {
			n.hooks += len(list)
		}
	}
	var on map[string]bool
	if s.Get("enabledPlugins", &on) {
		for _, v := range on {
			if v {
				n.plugins++
			}
		}
	}
	if first {
		n.env = len(s.Env())
	}
	n.model = firstNonEmpty(s.String("model"), n.model)
	n.mode = firstNonEmpty(s.String("permissions.defaultMode"), n.mode)
	n.line = firstNonEmpty(s.String("statusLine.command"), n.line)
}

// settingsLinks are the Settings tab's rows, from what was read of the
// settings files the agent's session reads.
func (m *Model) settingsLinks(c *hostConn, a *fleet.Agent, n settingsCounts) []settingsLink {
	allow, ask, deny, hooks, plugins, env, skills, cmds := n.allow, n.ask, n.deny, n.hooks, n.plugins, n.env, n.skills, n.cmds
	model, mode, line := n.model, n.mode, n.line
	join := func(parts ...string) string {
		var out []string
		for _, p := range parts {
			if p != "" {
				out = append(out, p)
			}
		}
		return strings.Join(out, " · ")
	}
	count := func(n int, one, many string) string {
		switch n {
		case 0:
			return ""
		case 1:
			return "1 " + one
		}
		return fmt.Sprintf("%d %s", n, many)
	}
	switch {
	case strings.Contains(line, "rush"):
		line = "rush's"
	case line != "":
		line = "your own command"
	}
	connected := 0
	for _, s := range c.sess.MCP {
		if s.Status == "connected" {
			connected++
		}
	}
	mcp := ""
	if len(c.sess.MCP) > 0 {
		mcp = fmt.Sprintf("%d of %d connected", connected, len(c.sess.MCP))
	}
	k := sessionAgent(c)
	all := []settingsLink{
		{name: harnessName(string(k)), value: join(model, count(env, "env var", "env vars")), about: "settings.json and the environment every session starts with",
			open: func(m *Model) tea.Cmd { m.sheet = nil; m.openAgentSettings(k); return nil }},
		{name: "Permissions", screen: "permissions", value: join(mode, count(allow, "allow", "allow"), count(ask, "ask", "ask"), count(deny, "deny", "deny")), about: "what tools may do without asking",
			open: func(m *Model) tea.Cmd { return m.openPermissions(c, a) }},
		{name: "Hooks", screen: "hooks", value: count(hooks, "hook", "hooks"), about: "commands run around tools, prompts and sessions",
			open: func(m *Model) tea.Cmd { return m.openHooks(c, a) }},
		{name: "Plugins", screen: "plugin", value: count(plugins, "on", "on"), about: "installed plugins, discover more, marketplaces",
			open: func(m *Model) tea.Cmd { return m.openPlugins(c, a) }},
		{name: "Skills", screen: "skills", value: join(count(skills, "skill", "skills"), count(cmds, "command", "commands")), about: "what Claude picks up, and your own / commands",
			open: func(m *Model) tea.Cmd { m.openSkills(c, a); return nil }},
		{name: "Status line", screen: "statusline", value: line, about: "this header, rush's top bar, and Claude Code's",
			open: func(m *Model) tea.Cmd { return m.openStatusLine(c, a) }},
		{name: "Memory", screen: "memory", about: "CLAUDE.md and the other files Claude reads",
			open: func(m *Model) tea.Cmd { m.sheet = nil; m.showView(c, "memory"); return nil }},
		{name: "MCP servers", screen: "mcp", value: mcp, about: "connect, sign in, tools", claude: true,
			open: func(m *Model) tea.Cmd { m.sheet = nil; return m.openScreen(c, a, "mcp") }},
	}
	var out []settingsLink
	for _, l := range all {
		if canScreen(c, l.screen) {
			out = append(out, l)
		}
	}
	return out
}

// settingsLines is the Settings tab.
func (k *infoSheet) settingsLines(w int) []string {
	if !k.loaded {
		return []string{dim("  reading the settings files…")}
	}
	var out []string
	for i, l := range k.settings {
		value := l.value
		if value == "" {
			value = "–"
		}
		tag := ""
		if l.claude {
			tag = paint(cSub, "  claude code ↗")
		}
		line := paint(cText+bold, fit(l.name, 14)) + " " + paint(cText, fit(value, 26)) + " " + faint(ansi.Truncate(l.about, max(0, w-48-ansi.StringWidth(tag)), "…")) + tag
		out = append(out, sheetRow(line, i == k.cur, w))
	}
	return out
}
