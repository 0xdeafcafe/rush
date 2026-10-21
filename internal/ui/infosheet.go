package ui

import (
	"fmt"
	"io/fs"
	"slices"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// --- /status, /usage, /stats ---

// infoSheet is Claude Code's /status, /usage and /stats done rush's way,
// as tabs of one sheet: the agent and what it runs on, the plan's limits
// and what's been spent, the account's history, and where its settings
// are. It reads live data every frame, so it follows the agent while it's
// open.
type infoSheet struct {
	conn   string
	tab    int
	scroll [infoTabs]int
	cur    int // the Settings tab's row

	shown    []int // the tabs the session's agent has
	stats    agent.Stats
	statsErr error
	settings []settingsLink // the Settings tab's rows, read when it opens
	// quota is the limits of an agent whose accounts aren't rush's
	// logins, as last read into quotas.json; read when it opens.
	quota *usage.Quota

	// What's read when it opens is read off the UI; until it lands the
	// tabs that show it say so.
	read   *pending[infoRead]
	loaded bool
}

// infoRead is what the sheet reads from disk when it opens.
type infoRead struct {
	stats    agent.Stats
	statsErr error
	counts   *settingsCounts // for the Settings tab
	quota    *usage.Quota
}

// adopt takes in what was read, once it has landed.
func (k *infoSheet) adopt(m *Model) {
	r, ok := k.read.take()
	if !ok {
		return
	}
	k.read, k.loaded = nil, true
	k.stats, k.statsErr, k.quota = r.stats, r.statsErr, r.quota
	c, a := m.sheetConn(k.conn), m.agentByKey(k.conn)
	if r.counts != nil && c != nil && a != nil {
		k.settings = m.settingsLinks(c, a, *r.counts)
	}
}

const (
	infoStatus = iota
	infoContext
	infoUsage
	infoHistory
	infoSettings
	infoTabs
)

var infoTabNames = [infoTabs]string{"Status", "Context", "Usage", "History", "Settings"}

// infoTabNeeds are the screens whose features each tab needs; Status is
// every agent's.
var infoTabNeeds = [infoTabs]string{infoContext: "context", infoUsage: "usage", infoHistory: "stats", infoSettings: "permissions"}

// openInfo opens the sheet on one of its tabs, with the tabs the session's
// agent has.
// errNoStats is an agent that keeps no record of its use.
var errNoStats = fmt.Errorf("no record kept: %w", fs.ErrNotExist)

func (m *Model) openInfo(c *hostConn, tab int) tea.Cmd {
	k := &infoSheet{conn: c.key, tab: tab}
	for t := range infoTabs {
		if need := infoTabNeeds[t]; need == "" || canScreen(c, need) || (t == infoContext && c.sess.Context > 0) {
			k.shown = append(k.shown, t)
		}
	}
	if !slices.Contains(k.shown, tab) {
		k.tab = infoStatus
	}
	if a := m.agentByKey(c.key); a != nil {
		// Read off the UI: the agent's record, its settings files, and
		// the limits last read for it.
		kind, acct, cwd := sessionAgent(c), a.Acct, firstNonEmpty(c.sess.Info.Cwd, a.Cwd)
		stats := slices.Contains(k.shown, infoHistory)
		settings := slices.Contains(k.shown, infoSettings)
		quota := slices.Contains(k.shown, infoUsage) && agent.Kind(a.Kind) != loginsKind
		k.read = goPending(func() infoRead {
			var r infoRead
			if stats {
				r.statsErr = errNoStats
				if sr, ok := agent.As[agent.StatsReader](kind); ok {
					r.stats, r.statsErr = sr.Stats(acct)
				}
			}
			if settings {
				n := readSettingsCounts(kind, acct, cwd)
				r.counts = &n
			}
			if quota {
				q := usage.Load(host.QuotasPath())[host.QuotaKey(acct)]
				r.quota = &q
			}
			return r
		})
		if quota {
			k.quota = &usage.Quota{Problem: "reading its last reading…"}
		}
		k.adopt(m)
	}
	m.sheet = k
	// A fresh count of the context, from a host that can.
	if cl := c.client; cl != nil && c.sess.Info.Proto >= 2 && canScreen(c, "context") {
		go func() { _ = cl.AskContext() }()
	}
	return k.read.wait()
}

// step moves to the next tab shown, or the one before.
func (k *infoSheet) step(by int) {
	i := max(0, slices.Index(k.shown, k.tab))
	k.tab = k.shown[(i+by+len(k.shown))%len(k.shown)]
}

func (k *infoSheet) width(*Model) int { return 100 }

func (m *Model) sheetConn(key string) *hostConn {
	if m.host != nil && m.host.key == key {
		return m.host
	}
	return nil
}

// accountView is the fleet's view of the account a runs on.
func (m *Model) accountView(a *fleet.Agent) (fleet.AccountView, bool) {
	for _, av := range m.snap.Accounts {
		if av.ConfigDir == a.Acct.Dir || av.ConfigDir == "" && a.Acct.Dir == "" {
			return av, true
		}
	}
	return fleet.AccountView{Name: a.Acct.Name, ConfigDir: a.Acct.Dir}, false
}

// infoRow is a label and its value, lined up.
func infoRow(label, value string, w int) string {
	if value == "" {
		value = faint("–")
	}
	return "  " + dim(fit(label, 14)) + " " + ansi.Truncate(value, max(0, w-18), "…")
}

func infoHead(s string) string { return paint(cSub+bold, "  "+s) }

func (k *infoSheet) key(m *Model, _ tea.KeyPressMsg, s string) tea.Cmd {
	k.adopt(m)
	switch s {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "]", "right":
		k.step(1)
	case "[", "left":
		k.step(-1)
	case "up":
		if k.tab == infoSettings {
			k.cur = roundMove(k.cur, -1, len(k.settings))
		} else {
			k.scroll[k.tab] = max(0, k.scroll[k.tab]-1)
		}
	case "down":
		if k.tab == infoSettings {
			k.cur = roundMove(k.cur, 1, len(k.settings))
		} else {
			k.scroll[k.tab]++
		}
	case "pgup":
		k.scroll[k.tab] = max(0, k.scroll[k.tab]-10)
	case "pgdown":
		k.scroll[k.tab] += 10
	case "enter":
		if k.tab == infoSettings && k.cur < len(k.settings) {
			return k.settings[k.cur].open(m)
		}
	}
	return nil
}

func (k *infoSheet) body(m *Model, w, h int) []string {
	k.adopt(m)
	c, a := m.sheetConn(k.conn), m.agentByKey(k.conn)
	name := "Status"
	if a != nil {
		name = firstNonEmpty(a.DisplayName, a.Name, "this agent")
	}
	names := make([]string, 0, len(k.shown))
	for _, t := range k.shown {
		names = append(names, infoTabNames[t])
	}
	program := "its program"
	if a != nil {
		program = harnessName(a.Kind)
	}
	out := []string{sheetTitle(name, "the agent, its account, and "+program, w), "", sheetTabs(names, slices.Index(k.shown, k.tab)), ""}
	if c == nil || a == nil {
		return append(out, dim("  this agent's Session is closed"), "", keysFit(w, "esc", "close"))
	}
	var lines []string
	switch k.tab {
	case infoStatus:
		lines = statusLines(m, c, a, w)
	case infoContext:
		lines = contextLines(c, a, w)
	case infoUsage:
		lines = usageLines(m, c, a, k.quota, w)
	case infoHistory:
		lines = k.historyLines(a, w, h-len(out)-3)
	case infoSettings:
		lines = k.settingsLines(w)
	}
	// Long tabs scroll; the tab row and keys stay put.
	room := max(3, h-len(out)-2)
	k.scroll[k.tab] = max(0, min(k.scroll[k.tab], len(lines)-room))
	if len(lines) > room {
		lines = lines[k.scroll[k.tab] : k.scroll[k.tab]+room]
	}
	out = append(out, lines...)
	switch {
	case k.tab == infoSettings:
		return append(out, "", keysFit(w, "↑↓", "choose", "enter", "open", "[ ]", "tabs", "esc", "close"))
	case len(lines) == room:
		return append(out, "", keysFit(w, "[ ]", "tabs", "↑↓", "scroll", "esc", "close"))
	}
	return append(out, "", keysFit(w, "[ ]", "tabs", "esc", "close"))
}

func sessionID(c *hostConn, a *fleet.Agent) string {
	if c != nil && c.sess.Info.SessionID != "" {
		return c.sess.Info.SessionID
	}
	return a.SessionID
}

// statusLines is the Status tab: the session, its model, the account and
// Claude Code.
func statusLines(m *Model, c *hostConn, a *fleet.Agent, w int) []string {
	s, now := c.sess, time.Now()
	k := sessionAgent(c)
	name, program := harnessName(string(k)), firstNonEmpty(agent.ProgramOf(k), "agent")
	out := []string{infoHead("Session")}
	out = append(out, infoRow("name", paint(cText+bold, firstNonEmpty(s.Info.Name, a.DisplayName, a.Name)), w))
	state := firstNonEmpty(s.Info.State, a.State)
	if c.client != nil && !s.Info.StartedAt.IsZero() {
		state += dim(" · up " + dur(now.Sub(s.Info.StartedAt)))
	}
	out = append(out, infoRow("state", paint(cText, state), w))
	out = append(out, infoRow("session id", dim(sessionID(c, a)), w))
	folder := tildify(firstNonEmpty(s.Info.Cwd, s.Cwd, a.Cwd))
	if a.Branch != "" {
		folder += dim(" on ") + paint(cSub, a.Branch)
	}
	out = append(out, infoRow("folder", folder, w))
	var conn string
	switch {
	case c.client != nil:
		conn = paint(cOrange, "rush") + dim(fmt.Sprintf(" · host pid %d", s.Info.HostPID))
		if s.Info.Stale(m.upd.installed) {
			conn += paint(cYellow, " · older rush than the installed one")
		}
		if s.Info.ClaudePID != 0 {
			conn += dim(fmt.Sprintf(" · %s pid %d", program, s.Info.ClaudePID))
		} else {
			conn += dim(" · " + program + " asleep, wakes on the next message")
		}
	case a.Interactive:
		conn = paint(cSub, name) + dim(" · a terminal session, read from its transcript")
	default:
		conn = paint(cSub, name) + dim(" · its daemon, read from the transcript")
	}
	out = append(out, infoRow("connection", conn, w))
	if a.TranscriptPath != "" {
		out = append(out, infoRow("transcript", dim(tildify(a.TranscriptPath)), w))
	}

	out = append(out, "", infoHead("Model"))
	out = append(out, infoRow("model", paint(cText, convo.PrettyModel(firstNonEmpty(s.Model, s.Info.Model))), w))
	out = append(out, infoRow("effort", paint(cText, s.Effort()), w))
	out = append(out, infoRow("permissions", paint(cText, s.Info.PermissionMode), w))
	if s.Context > 0 {
		out = append(out, infoRow("context", ctxLine(ctxFill(a, int64(s.Context), int64(s.ContextWindow()))), w))
	}

	av, _ := m.accountView(a)
	out = append(out, "", infoHead("Account"))
	who := paint(cText+bold, firstNonEmpty(av.Name, a.Acct.Name))
	if av.Usage.Email != "" {
		who += dim(" · " + av.Usage.Email)
	}
	if lv, ok := m.sessionLogin(c); ok {
		// The one it runs on, which a switch may have left behind.
		who = paint(cText+bold, loginName(lv))
		if e := firstNonEmpty(lv.Usage.Email, lv.Email); e != "" {
			who += dim(" · " + e)
		}
		if !lv.Current {
			who += dim(" · new sessions run on " + currentLoginName(m.snap.Logins))
		}
		av.Usage.Plan, av.Usage.Org, av.Usage.Role = lv.Usage.Plan, lv.Usage.Org, lv.Usage.Role
	}
	out = append(out, infoRow("login", who, w))
	var plan []string
	for _, p := range []string{av.Usage.Plan, av.Usage.Org, av.Usage.Role} {
		if p != "" {
			plan = append(plan, p)
		}
	}
	out = append(out, infoRow("plan", paint(cText, strings.Join(plan, " · ")), w))
	out = append(out, infoRow("config", dim(tildify(a.Acct.Dir)), w))

	out = append(out, "", infoHead(name))
	ver := s.Version
	if ver == "" && c.client == nil {
		ver = faint("not known from a transcript")
	}
	out = append(out, infoRow("version", paint(cText, ver), w))
	if s.NTools > 0 {
		out = append(out, infoRow("tools", paint(cText, fmt.Sprint(s.NTools)), w))
	}
	if len(s.Commands) > 0 {
		out = append(out, infoRow("/ commands", paint(cText, fmt.Sprint(len(s.Commands))), w))
	}
	switch {
	case len(s.MCP) > 0:
		servers := append(s.MCP[:0:0], s.MCP...)
		sort.SliceStable(servers, func(i, j int) bool { return servers[i].Status != "connected" && servers[j].Status == "connected" })
		for i, sv := range servers {
			label := ""
			if i == 0 {
				label = "mcp"
			}
			col := cGreen
			switch sv.Status {
			case "connected":
			case "pending":
				col = cYellow
			default:
				col = cRed
			}
			out = append(out, infoRow(label, paint(col, "● ")+paint(cText, sv.Name)+" "+dim(sv.Status), w))
		}
	case c.client != nil:
		out = append(out, infoRow("mcp", faint("none"), w))
	}
	return out
}

// bigMeter is a window's use as a wide bar, with the share of the window
// gone marked, and when it resets.
func bigMeter(pct float64, resets, now time.Time, window time.Duration, w int) string {
	w = max(10, w)
	pace := -1.0
	if !resets.IsZero() && resets.After(now) {
		pace = 1 - float64(resets.Sub(now))/float64(window)
	}
	col := usageColor(pct)
	if pace >= 0 && pct < 60 && pct/100 > pace+0.1 {
		col = cYellow // burning faster than the window allows
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
			b.WriteString(paint(col, "━"))
		default:
			b.WriteString(faint("─"))
		}
	}
	s := b.String() + " " + paint(col+bold, fmt.Sprintf("%3.0f%%", pct))
	if !resets.IsZero() {
		if resets.After(now) {
			at := resets.Local().Format("15:04")
			if resets.Sub(now) > 20*time.Hour {
				at = resets.Local().Format("Mon 15:04")
			}
			s += dim(" · resets in " + roughly(resets.Sub(now)) + ", " + at)
		} else {
			s += dim(" · has reset since")
		}
	}
	return s
}

// usageLines is the Usage tab: the plan's limits, what this agent has
// spent, and every account's. other is the limits of an agent whose
// accounts aren't rush's logins, as last read.
func usageLines(m *Model, c *hostConn, a *fleet.Agent, other *usage.Quota, w int) []string { //nolint:gocognit // one section after another, each a few rows
	var out []string
	now := m.snap.At
	if now.IsZero() {
		now = time.Now()
	}
	av, _ := m.accountView(a)
	if other != nil {
		av = fleet.AccountView{Quota: *other}
		av.Usage.Plan = firstNonEmpty(other.Plan, other.Balance)
		av.Usage.Email, av.Usage.Problem, av.Usage.FetchedAt = other.Email, other.Problem, other.FetchedAt
	}
	u := av.Usage
	head := "Plan · " + firstNonEmpty(av.Name, a.Acct.Name)
	if u.Plan != "" {
		head += " · " + u.Plan
	}
	out = append(out, infoHead(head))
	meterW := min(40, max(10, w-60))
	for _, win := range av.Quota.Windows {
		out = append(out, infoRow(win.Name, bigMeter(win.Percent, win.ResetsAt, now, win.Span, meterW), w))
	}
	if len(av.Quota.Windows) == 0 {
		out = append(out, infoRow("limits", faint(firstNonEmpty(u.Problem, "no reading yet: this account's limits come in with the next refresh")), w))
	} else {
		if u.Extra {
			out = append(out, infoRow("extra usage", paint(cText, "on")+dim(" · past the limits, it's billed"), w))
		}
		var note []string
		if !u.FetchedAt.IsZero() {
			note = append(note, "read "+u.FetchedAt.Local().Format("15:04"))
		}
		if u.Problem != "" {
			note = append(note, u.Problem)
		}
		if len(note) > 0 {
			out = append(out, infoRow("", faint(strings.Join(note, " · ")+" · ┼ marks how much of the window has gone"), w))
		}
	}

	s := c.sess
	t := s.Totals(time.Now())
	out = append(out, "", infoHead("This agent"))
	cost := paint(cText+bold, money(t.Cost))
	if t.Turns > 0 {
		cost += dim(fmt.Sprintf(" · %d turns · %s working", t.Turns, dur(t.Working)))
	}
	if s.Partial {
		cost = dim("counting…")
	}
	out = append(out, infoRow("cost", cost, w))
	out = append(out, billedRow(usage.Billing(s.Info.Billing), w)...)
	if t.Requests > 0 && !s.Partial {
		out = append(out, infoRow("requests", paint(cText, fmt.Sprint(t.Requests))+dim(fmt.Sprintf(" · %d tool calls", t.ToolCalls)), w))
		read := t.In + t.CacheRead + t.CacheOut
		hit := ""
		if read > 0 {
			hit = dim(fmt.Sprintf(" · %.0f%% from cache", float64(t.CacheRead)/float64(read)*100))
		}
		out = append(out, infoRow("tokens in", paint(cText, convo.Tokens(read))+hit, w))
		out = append(out, infoRow("", dim(convo.Tokens(t.In)+" new · "+convo.Tokens(t.CacheRead)+" cache read · "+convo.Tokens(t.CacheOut)+" cache written"), w))
		out = append(out, infoRow("tokens out", paint(cText, convo.Tokens(t.Out)), w))
		// By model, when there's more than one (subagents, /model).
		type byModel struct {
			name    string
			in, out int
		}
		per := map[string]*byModel{}
		for _, r := range s.Requests {
			name := convo.PrettyModel(r.Model)
			if per[name] == nil {
				per[name] = &byModel{name: name}
			}
			u := r.Usage
			per[name].in += int(u.Input + u.CacheRead + u.CacheWrite5m + u.CacheWrite1h)
			per[name].out += int(u.Output)
		}
		if len(per) > 1 {
			var rows []*byModel
			for _, b := range per {
				rows = append(rows, b)
			}
			sort.Slice(rows, func(i, j int) bool { return rows[i].in > rows[j].in })
			for i, b := range rows {
				label := ""
				if i == 0 {
					label = "by model"
				}
				out = append(out, infoRow(label, paint(cText, fit(b.name, 18))+dim(convo.Tokens(b.in)+" in · "+convo.Tokens(b.out)+" out"), w))
			}
		}
	}

	if len(m.snap.Accounts) > 0 && other == nil {
		out = append(out, "", infoHead("Every account"))
		for _, x := range m.snap.Accounts {
			var parts []string
			for _, win := range x.Quota.Windows {
				parts = append(parts, dim(win.Label+" ")+paint(usageColor(win.Percent), fmt.Sprintf("%3.0f%%", win.Percent)))
			}
			parts = append(parts, paint(cText, money(x.Today))+dim(" today"))
			if x.Live > 0 {
				parts = append(parts, dim(fmt.Sprintf("%d running", x.Live)))
			}
			name := x.Name
			if x.ConfigDir == a.Acct.Dir {
				name += " ◂"
			}
			out = append(out, infoRow(name, strings.Join(parts, dim("  ·  ")), w))
		}
	}
	return out
}

// unknownSheet asks what to do with a / command rush doesn't know
// (askUnknown): open Claude Code on it, or send it to Claude after all.
type unknownSheet struct {
	conn, line string
	send       func() tea.Cmd
	cur        int // 0 open in Claude Code, 1 send as a message
}

func (k *unknownSheet) width(*Model) int { return 72 }

func (k *unknownSheet) key(m *Model, _ tea.KeyPressMsg, s string) tea.Cmd {
	c, a := m.sheetConn(k.conn), m.agentByKey(k.conn)
	switch s {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "up", "down", "tab", "shift+tab":
		k.cur = 1 - k.cur
	case "enter":
		m.sheet = nil
		if c == nil || a == nil {
			return nil
		}
		if k.cur == 1 {
			return k.send()
		}
		c.input, c.back = c.input[:0], 0
		return m.openScreen(c, a, k.line)
	}
	return nil
}

func (k *unknownSheet) body(m *Model, w, h int) []string {
	name, _, _ := strings.Cut(k.line, " ")
	out := []string{sheetTitle("/"+name, "isn't a command rush knows", w), ""}
	for _, l := range wrap("rush has no view of its own for it, and this session didn't list it among its commands, so it's most likely one of Claude Code's own screens.", w-4) {
		out = append(out, "  "+paint(cSub, l))
	}
	out = append(out, "")
	choices := []struct{ name, about string }{
		{"Open it in Claude Code", "in this agent's folder and account · esc, then ctrl+c twice, comes back"},
		{"Send it to Claude as a message", "as you typed it"},
	}
	for i, ch := range choices {
		out = append(out, sheetRow(paint(cText+bold, ch.name), i == k.cur, w))
		out = append(out, "    "+faint(ansi.Truncate(ch.about, w-6, "…")))
	}
	return append(out, "", keysFit(w, "↑↓", "choose", "enter", "go", "esc", "cancel"))
}

// claudeSheet says, before it does, that a claude: command (claudeCloud)
// opens a real Claude Code: a fresh one, in this agent's folder and on its
// account, not this conversation.
type claudeSheet struct{ conn, line string }

func (k *claudeSheet) width(*Model) int { return 72 }

func (k *claudeSheet) key(m *Model, _ tea.KeyPressMsg, s string) tea.Cmd {
	switch s {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "enter":
		m.sheet = nil
		c, a := m.sheetConn(k.conn), m.agentByKey(k.conn)
		if c == nil || a == nil {
			return nil
		}
		c.input, c.back = c.input[:0], 0
		return m.openScreen(c, a, k.line)
	}
	return nil
}

func (k *claudeSheet) body(m *Model, w, h int) []string {
	name, _, _ := strings.Cut(k.line, " ")
	about := ""
	for _, c := range claudeCloud {
		if c.Name == name {
			about = c.Description
		}
	}
	out := []string{sheetTitle("claude:"+name, about, w), ""}
	banner := "  " + paint(cOrange, "↗ ") + paint(cText+bold, "This opens a real Claude Code")
	out = append(out, onBg(bgChrome, banner, w), "")
	for _, l := range wrap("/"+name+" is Claude Code's own, for your account or Claude's cloud, and rush leaves it to Claude Code. rush hands the terminal to a fresh Claude Code in this agent's folder, on its account, and opens /"+k.line+" there. It isn't this conversation.", w-4) {
		out = append(out, "  "+paint(cSub, l))
	}
	out = append(out, "")
	for _, l := range wrap("When you're done: esc, then ctrl+c twice, and you're back here.", w-4) {
		out = append(out, "  "+paint(cSub, l))
	}
	return append(out, "", keysFit(w, "enter", "open Claude Code", "esc", "cancel"))
}

// billedRow says how an agent's requests are paid for; none until the
// agent has said.
func billedRow(b usage.Billing, w int) []string {
	var v string
	switch b {
	case usage.Plan:
		v = paint(cText, "plan") + dim(" · out of the allowance")
	case usage.Overage:
		v = paint(cYellow, "extra usage") + dim(" · the allowance is spent: each request is paid for")
	case usage.Metered:
		v = paint(cOrange, "API key") + dim(" · every token is paid for")
	default:
		return nil
	}
	return []string{infoRow("billed", v, w)}
}
