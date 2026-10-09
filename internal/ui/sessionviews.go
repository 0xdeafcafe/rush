package ui

import (
	"fmt"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// --- queue view ---

// The queue's keys never need alt, which a Mac's option key doesn't send
// unless the terminal is told to. ↑ from the empty box picks the last
// queued message in the dock; a picked message takes plain keys
// (anything else goes back to typing), and ctrl+enter always sends
// everything waiting, now.

// queueHint is the keys for a picked queued message.
func queueHint(q queued, w int) string {
	hold := "hold"
	if q.held {
		hold = "let go"
	}
	pairs := []string{"space", "edit", "shift+↑↓", "merge up/down", "[ ]", "move", "s", "send this now", "g", "steer with it", "G", "steer with all", "⌫", "drop", "h", hold}
	if !q.local {
		how := "one per turn"
		if q.separate {
			how = "all together"
		}
		pairs = append(pairs, "o", how)
	}
	return keysFit(w, append(pairs, "esc", "done")...)
}

// picked1 draws a row as picked.
func picked1(text string, w int, focused bool) string {
	bar := faint("▍")
	if focused {
		bar = paint(cOrange, "▍")
	}
	return selBG + strings.ReplaceAll(bar+fit(text, w-1)[1:], reset, reset+selBG) + reset
}

// queueSel is the queued message picked in the dock.
func queueSel(c *hostConn, n int) (int, bool) {
	var i int
	if _, err := fmt.Sscanf(c.sel, "q:%d", &i); err != nil || i < 0 || i >= n {
		return 0, false
	}
	return i, true
}

// queueKey acts on a picked queued message. It reports whether it used
// the key.
func (m *Model) queueKey(c *hostConn, s string) (tea.Cmd, bool) {
	q := m.queueOf(c)
	i, ok := queueSel(c, len(q.items))
	if !ok {
		return nil, false
	}
	switch s {
	case "space", "e":
		if len(c.input) > 0 {
			m.flash("Finish or stash your message before editing the queue", false)
			return nil, true
		}
		// Edit it in the box; enter there saves it back in place. The
		// queue holds meanwhile, so it doesn't go half edited.
		c.pastes = pastes{}
		c.input, c.back, c.editQ, c.editWas = c.pastes.unfold(q.items[i]), 0, i+1, q.items[i]
		c.sel = ""
		var cmd tea.Cmd
		if c.editHeld = !q.held; c.editHeld {
			cmd = m.holdQueue(c, true)
		}
		return cmd, true
	case "shift+up":
		// Into the one above: it comes first, so the order holds.
		if i == 0 {
			m.flash("nothing before it to merge with", true)
			return nil, true
		}
		c.sel, c.selMoved = fmt.Sprintf("q:%d", i-1), true
		return m.queueEdit(c, "merge", i-1, 0), true
	case "shift+down":
		return m.queueEdit(c, "merge", i, 0), true
	case "[", "]", "alt+up", "alt+down":
		to := i - 1
		if s == "]" || s == "alt+down" {
			to = i + 1
		}
		if to < 0 || to >= len(q.items) {
			return nil, true
		}
		c.sel, c.selMoved = fmt.Sprintf("q:%d", to), true
		return m.queueEdit(c, "move", i, to), true
	case "m", "alt+m":
		return m.queueEdit(c, "merge", i, 0), true
	case "s":
		return m.queueEdit(c, "send", i, 0), true
	case "g":
		return m.steerQueue(c, i, false), true
	case "G", "shift+g":
		return m.steerQueue(c, i, true), true
	case "backspace", "delete", "ctrl+x":
		return m.queueEdit(c, "drop", i, 0), true
	case "h", "alt+h":
		c.editHeld = false // yours now, not the edit's
		return m.holdQueue(c, !q.held), true
	case "o", "alt+o":
		if q.local {
			return nil, true
		}
		on := !q.separate
		c.sess.Info.QueueSeparate = on
		return hostCmd(func() error { return c.client.QueueSeparately(on) }), true
	}
	return nil, false
}

// queueEdit changes the queue here at once, so the view and the next key
// see it, and tells the host of a rush session, naming the message by
// its place and text as you saw it.
func (m *Model) queueEdit(c *hostConn, op string, i, to int) tea.Cmd {
	items := slices.Clone(m.queueOf(c).items)
	// A rush session's queued images go wherever their message does.
	qi := make([][]string, len(items))
	qe := make([]*event.Exchange, len(items))
	if c.client != nil {
		copy(qi, c.sess.Info.QueueImages)
		copy(qe, c.sess.Info.QueueExchanges)
	}
	was, im, origin := items[i], qi[i], qe[i]
	switch op {
	case "move":
		items = slices.Insert(slices.Delete(items, i, i+1), to, was)
		qi = slices.Insert(slices.Delete(qi, i, i+1), to, im)
		qe = slices.Insert(slices.Delete(qe, i, i+1), to, origin)
	case "merge":
		if i+1 >= len(items) {
			m.flash("nothing after it to merge with", true)
			return nil
		}
		if qe[i] != nil || qe[i+1] != nil {
			m.flash("agent messages keep their sender · send them separately with s", false)
			return nil
		}
		items[i] += "\n\n" + items[i+1]
		items = slices.Delete(items, i+1, i+2)
		qi[i] = slices.Concat(im, qi[i+1])
		qi = slices.Delete(qi, i+1, i+2)
		qe = slices.Delete(qe, i+1, i+2)
	case "send", "drop":
		items = slices.Delete(items, i, i+1)
		qi = slices.Delete(qi, i, i+1)
		qe = slices.Delete(qe, i, i+1)
		// The pick stays where it was, on the next message.
		if len(items) == 0 {
			c.sel = ""
		} else {
			c.sel = fmt.Sprintf("q:%d", min(i, len(items)-1))
		}
	}
	if sq, sa := m.subQueue(c); sq != nil {
		sq.items = items
		if op == "send" {
			return tellSub(c, sa, was)
		}
		return nil
	}
	if c.client == nil {
		q := m.localQ[c.key]
		q.items = items
		if op == "send" {
			if a := m.agentByKey(c.key); a != nil {
				return reply(a, was)
			}
		}
		return nil
	}
	c.sess.Info.Queue, c.sess.Info.QueueImages, c.sess.Info.QueueExchanges = items, qi, qe
	cl := c.client
	return hostCmd(func() error {
		switch op {
		case "move":
			return cl.MoveQueued(i, was, to)
		case "merge":
			return cl.MergeQueued(i, was)
		case "send":
			return cl.SendQueued(i, was)
		}
		return cl.RemoveQueued(i, was)
	})
}

// holdQueue holds the queue or lets it go.
func (m *Model) holdQueue(c *hostConn, on bool) tea.Cmd {
	if sq, _ := m.subQueue(c); sq != nil {
		sq.held = on
		return nil
	}
	if c.client == nil {
		q := m.localQueueOf(c.key)
		q.held, q.fails, q.retry = on, 0, time.Time{}
		return nil
	}
	c.sess.Info.QueueHeld = on
	cl := c.client
	return hostCmd(func() error { return cl.HoldQueue(on) })
}

// sendQueueNow sends everything waiting straight away, and extra (what's
// in the box) after it, as one message: whether the agent is working,
// waiting on you or the queue is held, and in the order you wrote them.
func (m *Model) sendQueueNow(c *hostConn, extra string) tea.Cmd {
	if sq, sa := m.subQueue(c); sq != nil {
		return m.sendSubQueueNow(c, sq, sa, extra)
	}
	queued := m.queueOf(c).items
	if c.client != nil && hasQueuedExchange(c) && (len(queued) > 1 || extra != "") {
		m.flash("agent messages keep their sender · send them separately with s", false)
		return nil
	}
	items := slices.Clone(queued)
	if extra != "" {
		items = append(items, extra)
	}
	if len(items) == 0 {
		m.flash("nothing queued", false)
		return nil
	}
	if c.client == nil {
		a := m.agentByKey(c.key)
		if !canQueue(a) {
			return nil
		}
		q := m.localQueueOf(c.key)
		q.items, q.fails, q.retry = items, 0, time.Time{}
		return m.sendLocal(c.key, a, q)
	}
	n := len(queued)
	c.sess.Info.Queue, c.sess.Info.QueueImages, c.sess.Info.QueueExchanges = nil, nil, nil
	cl := c.client
	return hostCmd(func() error {
		if n == 0 {
			return cl.SendNow(extra)
		}
		text, err := joinQueued(cl, queued, items)
		if err != nil {
			return err
		}
		return cl.SendQueued(0, text)
	})
}

// joinQueued merges the queue into its first message, so their images go
// too, and writes it out as items (the queue, maybe with more after), for
// sending as one.
func joinQueued(cl *host.Client, queued, items []string) (string, error) {
	for k := 1; k < len(queued); k++ {
		if err := cl.MergeQueued(0, strings.Join(queued[:k], "\n\n")); err != nil {
			return "", err
		}
	}
	text := host.JoinQueue(items)
	return text, cl.EditQueued(0, strings.Join(queued, "\n\n"), text)
}

// steerQueue hands the turn under way queued message i, or all of them as
// one, without stopping it. A command the turn waits on goes to the
// background first, so it reads them at its next step, not when that ends.
func (m *Model) steerQueue(c *hostConn, i int, all bool) tea.Cmd {
	sq, _ := m.subQueue(c)
	if c.client == nil || sq != nil {
		// Sent as they'd go anyway: these are read at the next step already.
		if all {
			return m.sendQueueNow(c, "")
		}
		return m.queueEdit(c, "send", i, 0)
	}
	if c.sess.Info.Proto < 7 {
		m.flash("this session's host is older than this rush · /restart it to steer with the queue", true)
		return nil
	}
	queued := slices.Clone(m.queueOf(c).items)
	if all && len(queued) > 1 && hasQueuedExchange(c) {
		m.flash("agent messages keep their sender · steer with each separately using g", false)
		return nil
	}
	was := queued[i]
	if all {
		c.sess.Info.Queue, c.sess.Info.QueueImages, c.sess.Info.QueueExchanges = nil, nil, nil
		i, c.sel = 0, ""
	} else {
		qi := make([][]string, len(queued))
		copy(qi, c.sess.Info.QueueImages)
		c.sess.Info.Queue = slices.Delete(slices.Clone(queued), i, i+1)
		c.sess.Info.QueueImages = slices.Delete(qi, i, i+1)
		qe := make([]*event.Exchange, len(queued))
		copy(qe, c.sess.Info.QueueExchanges)
		c.sess.Info.QueueExchanges = slices.Delete(qe, i, i+1)
		if c.sel = ""; len(c.sess.Info.Queue) > 0 {
			c.sel = fmt.Sprintf("q:%d", min(i, len(c.sess.Info.Queue)-1))
		}
	}
	blocked := c.sess.Info.Proto >= 3 && slices.ContainsFunc(c.sess.RunningJobs(), func(j *convo.Job) bool {
		return !j.Background && c.jobKind(j) != "subagent"
	})
	what := "it"
	if all && len(queued) > 1 {
		what = "the queue"
	}
	if blocked {
		m.flash("steering with "+what+" · moved the command it waited on to the background so it reads it now", false)
	} else {
		m.flash("steering with "+what+" · it reads it at its next step, nothing stopped", false)
	}
	cl := c.client
	return hostCmd(func() error {
		if all {
			text, err := joinQueued(cl, queued, queued)
			if err != nil {
				return err
			}
			was = text
		}
		if err := cl.SteerQueued(i, was); err != nil {
			return err
		}
		if blocked {
			return cl.Background("")
		}
		return nil
	})
}

// endQueueEdit lets the queue go again if editing held it.
func (m *Model) endQueueEdit(c *hostConn) tea.Cmd {
	c.editQ = 0
	if !c.editHeld {
		return nil
	}
	c.editHeld = false
	return m.holdQueue(c, false)
}

// --- tasks view ---

func (m *Model) taskLines(c *hostConn, o convo.Options) []convo.Line {
	w := o.Width
	var out []convo.Line
	line := func(text string) { out = append(out, convo.Line{Text: fit(text, w)}) }
	_, done, total := c.sess.Current()
	line("  " + paint(cSub+bold, "Tasks") + "  " + dim(fmt.Sprintf("%d of %d done", done, total)))
	line("  " + faint(strings.Repeat("─", max(0, w-4))))
	if total == 0 {
		line("")
		line("    " + dim("No task list yet. It appears when the agent plans its work."))
		return out
	}
	group := func(title, status, mark string) {
		var ts []convo.Task
		for _, t := range c.sess.Tasks {
			if t.Status == status {
				ts = append(ts, t)
			}
		}
		if len(ts) == 0 {
			return
		}
		line("")
		line("  " + paint(cSub+bold, title) + "  " + dim(fmt.Sprint(len(ts))))
		for _, t := range ts {
			label := t.Subject
			col := cSub
			switch status {
			case "in_progress":
				label, col = firstNonEmpty(t.Active, t.Subject), cText+bold
			case "completed":
				col = cDim
			}
			for j, r := range wrap(oneLine(label), max(20, w-10)) {
				lead := "    " + mark + " "
				if j > 0 {
					lead = "      "
				}
				line(lead + paint(col, r))
			}
		}
	}
	group("Now", "in_progress", paint(cOrange, "■"))
	group("Next", "pending", dim("☐"))
	group("Done", "completed", paint(cGreen, "✓"))
	return out
}

// --- slash commands ---

// rushCommands are handled by rush itself rather than sent to Claude.
// They keep Claude Code's / names; rush's other commands take # (see
// fleetCommands). Claude Code's that rush already does its own way run
// rush's (/diff opens the changes view, /cd moves the agent, …). A new one
// rush adds goes in fleetCommands, not here (TestSlashIsOnlyClaudeCodes).
var rushCommands = []event.Command{
	{Name: "clear", Description: "start this agent afresh, named by your next message; what it had is kept (/rewind)"},
	{Name: "fork", Description: "carry on in a copy of this conversation, as a new agent (this one stays as it is)", ArgumentHint: "[name]"},
	{Name: "rewind", Description: "go back to before one of your messages and try again; the path you leave is kept as a branch"},
	{Name: "model", Description: "pick the model for the next turn, or name one", ArgumentHint: "[model]"},
	{Name: "effort", Description: "pick the effort (applies from the next start), or name one", ArgumentHint: "[level]"},
	{Name: "plan", Description: "plan mode on, or off again: Claude plans and asks before it changes anything"},
	{Name: "diff", Description: "what changed: this session's edits and the working tree (the changes view)"},
	{Name: "tasks", Description: "what's running: shells, monitors and subagents, to stop or background"},
	{Name: "copy", Description: "copy Claude's last answer; /copy 2 the one before", ArgumentHint: "[n]"},
	{Name: "rename", Description: "rename the agent, or type the new name", ArgumentHint: "[name]"},
	{Name: "cd", Description: "tell the agent to work in another folder from now on", ArgumentHint: "[path]"},
	{Name: "stop", Description: "stop the agent; its conversation stays, and a message wakes it"},
	{Name: "background", Description: "leave it running in the background and go back to Agents"},
	{Name: "resume", Description: "past conversations: they're in Agents, and a message carries one on"},
	{Name: "help", Description: "a short guide to rush"},
	{Name: "btw", Description: "a side question in a panel over the chat (ctrl+b): not added to the conversation; ctrl+f makes it a chat of its own", ArgumentHint: "[question]"},
	{Name: "export", Description: "the conversation as text: copy it, or save it to a file", ArgumentHint: "[file]"},
	{Name: "subtask", Description: "send a subagent off with the task; Claude carries on, and reports back when it's done", ArgumentHint: "<task>"},
}

// rushAliases are Claude Code's other names for commands rush does.
var rushAliases = map[string]string{"bashes": "tasks", "bg": "background", "continue": "resume", "name": "rename",
	"branch": "fork", "checkpoint": "rewind", "undo": "rewind", "reset": "clear", "new": "clear"}

// claudeScreens are Claude Code's own screens. rush draws most of them
// itself (rushScreens); the rest hand the terminal to Claude Code on that
// screen, and come back when you leave it. Given arguments, /mcp and
// /config go to Claude as usual.
var claudeScreens = []event.Command{
	{Name: "status", Description: "this agent, its account, Claude Code's version and MCP servers"},
	{Name: "context", Description: "what fills the context window, by category"},
	{Name: "usage", Description: "the plan's 5-hour and weekly limits, and what's been spent"},
	{Name: "stats", Description: "your history: activity by day, tokens by model, busiest hours"},
	{Name: "skills", Description: "the skills and / commands Claude can use: search, use, edit"},
	{Name: "plugin", Description: "installed plugins on/off, discover and install, marketplaces"},
	{Name: "mcp", Description: "MCP servers: connect, sign in, tools"},
	{Name: "hooks", Description: "the hooks that run around tools, prompts and sessions"},
	{Name: "permissions", Description: "allow and deny rules for tools"},
	{Name: "memory", Description: "memory, CLAUDE.md and the other files Claude reads (the memory view)"},
	{Name: "config", Description: "Claude Code's settings (Settings › Providers)"},
	{Name: "statusline", Description: "build status lines: this header, rush's top bar, and Claude Code's"},
}

// rushScreens are the ones rush draws itself (rushScreen).
var rushScreens = map[string]bool{"status": true, "context": true, "usage": true, "stats": true, "plugin": true, "skills": true,
	"memory": true, "config": true, "statusline": true, "permissions": true, "hooks": true}

// screenAliases are other names Claude Code takes for the same screens.
var screenAliases = map[string]string{"plugins": "plugin", "marketplace": "plugin", "settings": "config", "cost": "usage",
	"allowed-tools": "permissions", "version": "status"}

// claudeCloud are Claude Code's screens for your account and Claude's
// cloud, which rush leaves to Claude Code: named claude:<name> in the
// picker, they open a real Claude Code after saying so (claudeSheet).
var claudeCloud = []event.Command{
	{Name: "login", Description: "sign in to an Anthropic account"},
	{Name: "logout", Description: "sign out"},
	{Name: "upgrade", Description: "upgrade the plan for higher limits"},
	{Name: "usage-credits", Description: "usage credits past the limits, or ask your admin for them"},
	{Name: "rate-limit-options", Description: "what to do when a limit is reached"},
	{Name: "privacy-settings", Description: "privacy settings"},
	{Name: "setup-bedrock", Description: "Amazon Bedrock sign-in, region and models"},
	{Name: "setup-vertex", Description: "Google Vertex AI sign-in, project, region and models"},
	{Name: "teleport", Description: "send a session to the cloud, or bring one back from claude.ai"},
	{Name: "remote-control", Description: "drive a session from claude.ai or the app"},
	{Name: "session", Description: "a cloud session's URL and QR code"},
	{Name: "remote-env", Description: "the default environment for cloud agents"},
	{Name: "web-setup", Description: "cloud sessions with your GitHub account"},
	{Name: "cloud-plugins", Description: "whether cloud sessions use this machine's plugins"},
	{Name: "desktop", Description: "carry on in Claude Desktop"},
	{Name: "mobile", Description: "a QR code for the Claude app"},
	{Name: "ultraplan", Description: "plan in the cloud"},
	{Name: "ultrareview", Description: "a deep multi-agent review in the cloud"},
	{Name: "autofix-pr", Description: "watch a pull request and fix what fails"},
	{Name: "install-github-app", Description: "Claude on GitHub Actions for a repo"},
	{Name: "install-slack-app", Description: "the Claude Slack app"},
	{Name: "design-login", Description: "design-system access for /design-sync"},
	{Name: "design-consent", Description: "let Claude reach your Design projects"},
	{Name: "design-revoke", Description: "take that back"},
	{Name: "artifacts", Description: "browse your published and shared artifacts"},
	{Name: "workflows", Description: "browse running and completed workflows"},
	{Name: "daemon", Description: "Claude Code's background services and routines"},
}

// cloudAliases are Claude Code's other names for claudeCloud's.
var cloudAliases = map[string]string{"extra-usage": "usage-credits", "tp": "teleport", "rc": "remote-control", "remote": "session",
	"app": "desktop", "ios": "mobile", "android": "mobile"}

// claudeCloudName is the claudeCloud command name is, by any of its names
// (login, claude:login, tp).
func claudeCloudName(name string) (string, bool) {
	name = strings.TrimPrefix(name, "claude:")
	if n, ok := cloudAliases[name]; ok {
		name = n
	}
	return name, slices.ContainsFunc(claudeCloud, func(c event.Command) bool { return c.Name == name })
}

// claudeCloudPicks is claudeCloud as the picker offers it: claude:login.
var claudeCloudPicks = func() []event.Command {
	out := make([]event.Command, 0, len(claudeCloud))
	for _, c := range claudeCloud {
		c.Name = "claude:" + c.Name
		out = append(out, c)
	}
	return out
}()

// offCommands are Claude Code's that do nothing in rush, and why: its own
// terminal's, the odds and ends, and the ones rush hasn't done yet. They
// stay out of the picker, and say so when typed.
var offCommands = func() map[string]string {
	out := map[string]string{}
	for _, n := range strings.Fields("theme color tui scroll-speed focus brief terminal-setup voice keybindings ide chrome exit quit vim") {
		out[n] = "it's for Claude Code's own terminal"
	}
	for _, n := range strings.Fields("release-notes feedback bug share update install import powerup wellbeing breaks break-reminder downtime stickers radio heapdump passes agents plugin-types workflow-launch-exec") {
		out[n] = "rush leaves it out"
	}
	out["loops"] = "Claude Code has it switched off; ask Claude to /loop instead"
	return out
}()

// openableScreens are the screens the session's agent can open.
func openableScreens(c *hostConn) []event.Command {
	var out []event.Command
	for _, s := range claudeScreens {
		if canScreen(c, s.Name) {
			out = append(out, s)
		}
	}
	return out
}

// claudeScreen is the Claude Code screen a command opens, if it's one.
func claudeScreen(name string) (string, bool) {
	if n, ok := screenAliases[name]; ok {
		name = n
	}
	ok := slices.ContainsFunc(claudeScreens, func(c event.Command) bool { return c.Name == name })
	return name, ok
}

// slashWord finds the /word being typed at the cursor, at the start of the
// message or after a space. A word with a second slash is a path.
func slashWord(c *hostConn) (start, end int, q string, ok bool) {
	in, pos := c.input, len(c.input)-c.back
	start = pos
	for start > 0 && !unicode.IsSpace(in[start-1]) {
		start--
	}
	if start >= len(in) || in[start] != '/' || start == pos {
		return 0, 0, "", false
	}
	end = pos
	for end < len(in) && !unicode.IsSpace(in[end]) {
		end++
	}
	q = string(in[start+1 : pos])
	if strings.Contains(q, "/") {
		return 0, 0, "", false
	}
	return start, end, strings.ToLower(q), true
}

// slashMatches is what the picker offers for the /word at the cursor:
// rush's own commands (at the start of a message only), the session's,
// then the custom commands and skills found on disk.
func slashMatches(c *hostConn) []event.Command {
	start, _, q, ok := slashWord(c)
	if !ok {
		return nil
	}
	lists := [][]event.Command{c.sess.Commands, c.local}
	switch {
	case start == 0 && !ownScreens(c):
		// Claude Code's own screens aren't another agent's; the ones rush
		// draws are, when its agent has what they show.
		lists = append([][]event.Command{sessionCommands(c), openableScreens(c)}, lists...)
	case start == 0:
		lists = append([][]event.Command{sessionCommands(c), openableScreens(c)}, lists...)
		lists = append(lists, claudeCloudPicks)
	}
	var out []event.Command
	seen := map[string]bool{}
	// Another name for one of Claude Code's screens (/plugins, /bug) finds
	// it first.
	if name, ok := screenAliases[q]; ok && start == 0 && canScreen(c, name) {
		if i := slices.IndexFunc(claudeScreens, func(c event.Command) bool { return c.Name == name }); i >= 0 {
			seen[name] = true
			out = append(out, claudeScreens[i])
		}
	}
	for _, list := range lists {
		for _, cmd := range list {
			// What rush leaves out or leaves to Claude Code isn't offered
			// under Claude Code's own name.
			_, cloud := claudeCloudName(cmd.Name)
			if seen[cmd.Name] || offCommands[cmd.Name] != "" || cloud && !strings.HasPrefix(cmd.Name, "claude:") {
				continue
			}
			// claude:login is found by login, or by claude: itself.
			name := strings.ToLower(cmd.Name)
			if !strings.HasPrefix(q, "claude:") {
				name = strings.TrimPrefix(name, "claude:")
			}
			if strings.Contains(name, q) {
				seen[cmd.Name] = true
				out = append(out, cmd)
			}
		}
	}
	// What you typed exactly, then names starting with it, then the rest.
	rank := func(c event.Command) int {
		n := strings.TrimPrefix(strings.ToLower(c.Name), "claude:")
		switch {
		case n == q || screenAliases[q] == c.Name:
			return 0
		case strings.HasPrefix(n, q):
			return 1
		}
		return 2
	}
	sort.SliceStable(out, func(i, j int) bool { return rank(out[i]) < rank(out[j]) })
	return out
}

// loadLocal fills in the commands and skills on disk for the agent's
// account and folder, as read at most 30 seconds ago (see commandsOf).
func (m *Model) loadLocal(c *hostConn) {
	a := m.agentByKey(c.key)
	if a == nil {
		return
	}
	c.local, c.skills = c.local[:0], map[string]bool{}
	found := m.sessionCommands(c, a)
	for _, f := range found {
		c.local = append(c.local, event.Command{Name: f.Name, Description: f.Description, ArgumentHint: f.ArgumentHint})
		if f.Skill {
			c.skills[f.Name] = true
		}
	}
}

// sessionCommands are the commands and skills on disk for the agent's
// account and folder, as last read; nil when its agent lists none.
func (m *Model) sessionCommands(c *hostConn, a *fleet.Agent) []agent.Command {
	k := sessionAgent(c)
	ad, ok := agent.Get(k)
	if _, lists := ad.(agent.Commander); !ok || !lists {
		return nil
	}
	p := a.Acct
	if ps := agent.ProfilesOf(ad); p.Dir == "" && len(ps) > 0 {
		p = ps[0]
	}
	return m.commandsOf(k, p, firstNonEmpty(c.sess.Info.Cwd, a.Cwd))
}

// argMatches is the picker for a command's argument: "/model so" offers
// the models starting with so, the current one marked.
func argMatches(c *hostConn) []event.Command {
	text := string(c.input)
	if c.back != 0 || !strings.HasPrefix(text, "/") || strings.ContainsAny(text, "\n") {
		return nil
	}
	name, q, ok := strings.Cut(text[1:], " ")
	opts := argOptions(c, name)
	if !ok || opts == nil || strings.Contains(q, " ") {
		return nil
	}
	now, known := argNow(c, name, opts)
	var out []event.Command
	for _, o := range append(opts, agent.Choice{ID: "default", Note: "the agent's own"}) {
		if !strings.HasPrefix(o.ID, strings.ToLower(q)) {
			continue
		}
		d := o.Note
		if known && (now == o.ID || now == "" && o.ID == "default") {
			d = strings.TrimPrefix(d+" · now", " · ")
		}
		out = append(out, event.Command{Name: name + " " + o.ID, Description: d})
	}
	return out
}

// paneArgs are the completions for a command's argument in session c's
// box: its /model or /effort.
func (m *Model) paneArgs(c *hostConn) []event.Command { return argMatches(c) }

// slashLines draws the picker above the message box.
func (m *Model) slashLines(c *hostConn, w int) []string {
	if cmds := m.hashMatches(c.input, c.back); len(cmds) > 0 {
		c.slashSel = max(0, min(c.slashSel, len(cmds)-1))
		return pickerRows(cmds, c.slashSel, w, "#", func(string) string { return "" }, "rush's, for this agent · ↑↓ · tab completes · enter runs")
	}
	if cmds := m.mentionMatches(c.input, c.back); len(cmds) > 0 {
		c.slashSel = max(0, min(c.slashSel, len(cmds)-1))
		return pickerRows(cmds, c.slashSel, w, "@", func(string) string { return "" }, mentionHow)
	}
	cmds := m.paneArgs(c)
	if cmds == nil {
		if _, _, _, ok := slashWord(c); !ok {
			return nil
		}
		m.loadLocal(c)
		cmds = slashMatches(c)
	}
	if len(cmds) == 0 {
		return nil
	}
	c.slashSel = max(0, min(c.slashSel, len(cmds)-1))
	st, _, _, _ := slashWord(c)
	tag := func(name string) string {
		switch {
		case slices.ContainsFunc(rushCommands, func(a event.Command) bool { return a.Name == name }):
			return paint(cOrange, " rush")
		case st == 0 && rushScreens[name]:
			return paint(cOrange, " rush")
		case st == 0 && (strings.HasPrefix(name, "claude:") || slices.ContainsFunc(claudeScreens, func(a event.Command) bool { return a.Name == name })):
			return paint(cSub, " claude code ↗")
		case c.skills[name]:
			return paint(cBlue, " skill")
		}
		return ""
	}
	how := "↑↓ choose · tab completes · enter runs"
	if st > 0 {
		how = "↑↓ choose · tab or enter completes"
	}
	return pickerRows(cmds, c.slashSel, w, "/", tag, how)
}

// pickerRows draws a command picker: up to six commands around the
// selected one, then a row saying how to use it.
func pickerRows(cmds []event.Command, sel, w int, lead string, tag func(string) string, how string) []string {
	start := max(0, sel-5)
	end := min(len(cmds), start+6)
	nameW := 0
	for _, cmd := range cmds[start:end] {
		nameW = max(nameW, len([]rune(cmd.Name))+1)
	}
	nameW = min(nameW, 28)
	var out []string
	for i := start; i < end; i++ {
		cmd := cmds[i]
		name := paint(cBright+bold, fit(lead+cmd.Name, nameW+1))
		t := tag(cmd.Name)
		desc := dim(ansi.Truncate(convo.KeyWord(oneLine(cmd.Description)), max(10, w-nameW-7-ansi.StringWidth(t)), "…"))
		row := "   " + name + "  " + desc + t
		if i == sel {
			out = append(out, onBg(selBG, paint(cOrange, " ▸ ")+strings.TrimPrefix(row, "   "), w))
		} else {
			out = append(out, onBg(bgChrome, row, w))
		}
	}
	more := ""
	if len(cmds) > end-start {
		more = fmt.Sprintf("%d of %d · ", sel+1, len(cmds))
	}
	return append(out, onBg(bgChrome, "   "+dim(more+how), w))
}

// slashKey drives the picker while a command is being typed.
func (m *Model) slashKey(c *hostConn, s string) (tea.Cmd, bool) {
	if cmd, used := m.paneHashKey(c, s); used {
		return cmd, true
	}
	if args := m.paneArgs(c); len(args) > 0 {
		c.slashSel = max(0, min(c.slashSel, len(args)-1))
		switch s {
		case "up":
			c.slashSel = roundMove(c.slashSel, -1, len(args))
		case "down":
			c.slashSel = roundMove(c.slashSel, 1, len(args))
		case "tab":
			c.input, c.back = []rune("/"+args[c.slashSel].Name), 0
		case "enter":
			c.input, c.back = []rune("/"+args[c.slashSel].Name), 0
			return m.sendPane(c, false), true
		default:
			return nil, false
		}
		return nil, true
	}
	cmds := slashMatches(c)
	if len(cmds) == 0 {
		return nil, false
	}
	start, end, _, _ := slashWord(c)
	complete := func(tail string) {
		rest := c.input[end:]
		if len(rest) > 0 && unicode.IsSpace(rest[0]) {
			tail = ""
		}
		name := []rune("/" + cmds[c.slashSel].Name + tail)
		c.input = append(append(append([]rune{}, c.input[:start]...), name...), rest...)
		c.back = len(rest)
	}
	switch s {
	case "up":
		c.slashSel = roundMove(c.slashSel, -1, len(cmds))
		return nil, true
	case "down":
		c.slashSel = roundMove(c.slashSel, 1, len(cmds))
		return nil, true
	case "tab":
		complete(" ")
		return nil, true
	case "enter":
		// A command that is the whole message runs; one mid-message is
		// completed, and enter again sends the message.
		if start == 0 && strings.TrimSpace(string(c.input[end:])) == "" {
			c.input, c.back = []rune("/"+cmds[c.slashSel].Name), 0
			return m.sendPane(c, false), true
		}
		// Typed in full: its name, without claude:, or another name for it.
		typed, name := strings.ToLower(string(c.input[start+1:end])), cmds[c.slashSel].Name
		if (typed == strings.ToLower(name) || "claude:"+typed == strings.ToLower(name) || screenAliases[typed] == name) && m.store.Config.EnterSendsCommand {
			return nil, false // typed in full, there's nothing to complete
		}
		complete(" ")
		return nil, true
	}
	return nil, false
}

// runRushCommand handles the commands rush answers itself. It reports
// whether text was one of them.
func (m *Model) runRushCommand(c *hostConn, text string) (tea.Cmd, bool) {
	name, arg, _ := strings.Cut(strings.TrimPrefix(strings.TrimSpace(text), "/"), " ")
	arg = strings.TrimSpace(arg)
	a := m.agentByKey(c.key)
	if n, ok := rushAliases[name]; ok {
		name = n
	}
	if why := offCommands[name]; why != "" {
		m.flash("/"+name+" isn't in rush: "+why, true)
		return nil, true
	}
	if _, cloud := claudeCloudName(name); !canRun(c, name) || cloud && !ownScreens(c) {
		m.flash("/"+name+" isn't something "+harnessName(string(sessionAgent(c)))+" can do", true)
		return nil, true
	}
	if cloud, ok := claudeCloudName(name); ok && a != nil { // ownScreens, above
		m.sheet = &claudeSheet{conn: c.key, line: strings.TrimSpace(cloud + " " + arg)}
		return nil, true
	}
	switch name {
	case "done":
		// Putting it away closes its conversation rather than showing the
		// next agent's; on a narrow screen the list comes back. Zen goes
		// on to the next agent that needs you.
		if a == nil || a.Done || m.zen {
			return m.markDone(a), true
		}
		cmd := m.markDone(a)
		if m.confirm != nil && m.confirm.onYes != nil {
			yes := m.confirm.onYes
			m.confirm.onYes = func() tea.Cmd { m.leavePane(); return yes() }
			return cmd, true
		}
		m.leavePane()
		return cmd, true
	case "clean":
		return m.askClean(a), true
	case "rush":
		if a == nil {
			return nil, true
		}
		return m.moveToRush(a), true
	case "clear":
		if a == nil {
			return nil, true
		}
		if c.client == nil || !agent.Supports(sessionAgent(c), agent.FeatureRewind) {
			return m.startHosted("", a.Cwd), true // a new session beside it
		}
		return m.clearInPlace(c, a), true
	case "fork":
		if a == nil {
			return nil, true
		}
		m.openFork(c, a, arg)
		return nil, true
	case "handoff": // rush's, so #fork now; typed the old way it still runs
		if a == nil {
			return nil, true
		}
		at := m.statusAt
		cmd := m.handoffTo(c, a, arg)
		if m.statusAt == at {
			m.flash("rush's own commands start with #: #fork", false)
		}
		return cmd, true
	case "compact":
		if a != nil {
			if cmd, ok := m.compactTyped(c, a, arg); ok {
				return cmd, true
			}
		}
	case "agent", "profile": // rush's, so #use now; typed the old way it still runs
		return m.setupCommand(c, strings.TrimSpace(text), strings.TrimSpace(text))
	case "rewind":
		if a == nil {
			return nil, true
		}
		return m.openRewind(c, a), true
	case "plan":
		if c.client == nil {
			m.flash("/plan works in rush-mode sessions · /rush moves this one over", true)
			return nil, true
		}
		mode := "plan"
		if c.sess.Info.PermissionMode == "plan" || arg == "off" {
			mode = "default"
		}
		return m.setPermission(c, mode), true
	case "diff":
		if !m.showView(c, "changes") {
			m.flash("no changes view for this agent", true)
		}
		return nil, true
	case "tasks":
		if !m.showView(c, "background") && !m.showView(c, "subagents") && !m.showView(c, "tasks") {
			m.flash("nothing running in the background", false)
		}
		return nil, true
	case "copy":
		n := 1
		if v, err := strconv.Atoi(arg); err == nil && v > 0 {
			n = v
		}
		t := c.sess.LastAnswer(n)
		if t == "" {
			m.flash("no answer to copy yet", true)
			return nil, true
		}
		m.copyText(t)
		m.flash(fmt.Sprintf("copied Claude's answer · %d lines", strings.Count(t, "\n")+1), false)
		return nil, true
	case "rename", "stop", "help":
		if a == nil {
			return nil, true
		}
		return m.command(a, strings.TrimSpace("#"+name+" "+arg)), true
	case "cd":
		if a == nil {
			return nil, true
		}
		if arg == "" {
			return m.openMovePicker(a), true
		}
		return m.command(a, "#cd "+arg), true
	case "btw":
		return m.openBtw(c, arg), true
	case "export":
		dir := c.sess.Info.Cwd
		if a != nil {
			dir = firstNonEmpty(dir, a.Cwd)
		}
		return m.openExport(c, dir, arg), true
	case "subtask":
		if arg == "" {
			m.flash("what's the task? /subtask <task>", true)
			return nil, true
		}
		return m.sendAs(c, subtaskPrompt(arg)), true
	case "background":
		m.leavePane()
		m.flash("it carries on in the background · it's in Agents", false)
		return nil, true
	case "resume":
		m.leavePane()
		m.flash("past conversations are in Agents: pick one, and a message carries it on", false)
		return nil, true
	case "model", "effort":
		if arg == "" && m.openChoices(c, name) {
			return m.loadModels(string(sessionAgent(c))), true
		}
		return m.setArg(c, name, arg), true
	}
	if screen, ok := claudeScreen(name); ok && (arg == "" || screen != "mcp" && screen != "config") && a != nil {
		if cmd, ok := m.rushScreen(c, a, screen); ok {
			return cmd, true
		}
		return m.openScreen(c, a, screen), true
	}
	return nil, false
}

// cmdName is a / command's name as Claude Code takes it: not a path.
var cmdName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_:.-]*$`)

// askUnknown asks what to do with a / command that neither rush nor this
// session knows, which headless Claude Code would only answer with "isn't
// available in this environment": most likely it's one of Claude Code's
// own screens. y opens Claude Code on it (esc, then ctrl+c twice, comes
// back); n sends it to Claude as a message, by send.
func (m *Model) askUnknown(c *hostConn, text string, send func() tea.Cmd) bool {
	a := m.agentByKey(c.key)
	// Claude Code sessions are typed into, and know their own commands;
	// until the session has said which it has, there's nothing to go by.
	// An agent with no screens of its own has none to open.
	if a == nil || c.client == nil || len(c.sess.Commands) == 0 || !ownScreens(c) {
		return false
	}
	line := strings.TrimSpace(strings.TrimPrefix(text, "/"))
	name, _, _ := strings.Cut(line, " ")
	if strings.Contains(line, "\n") || !cmdName.MatchString(name) {
		return false
	}
	m.loadLocal(c)
	if _, ok := claudeCloudName(name); ok || offCommands[name] != "" || rushAliases[name] != "" || screenAliases[name] != "" {
		return false
	}
	for _, list := range [][]event.Command{rushCommands, claudeScreens, c.sess.Commands, c.local} {
		if slices.ContainsFunc(list, func(k event.Command) bool { return strings.EqualFold(k.Name, name) }) {
			return false
		}
	}
	m.sheet = &unknownSheet{conn: c.key, line: line, send: send}
	return true
}

// rushScreen is rush's own take on one of Claude Code's screens, where it
// has one; the rest open Claude Code's.
func (m *Model) rushScreen(c *hostConn, a *fleet.Agent, screen string) (tea.Cmd, bool) {
	switch screen {
	case "plugin":
		return m.openPlugins(c, a), true
	case "memory":
		if m.showView(c, "memory") {
			return nil, true
		}
	case "config":
		m.openAgentSettings(sessionAgent(c))
		return nil, true
	case "statusline":
		m.openStatusLine(c, a)
		return nil, true
	case "skills":
		m.openSkills(c, a)
		return nil, true
	case "permissions":
		m.openPermissions(c, a)
		return nil, true
	case "hooks":
		return m.openHooks(c, a), true
	case "status":
		m.openInfo(c, infoStatus)
		return nil, true
	case "context":
		m.openInfo(c, infoContext)
		return nil, true
	case "usage":
		m.openInfo(c, infoUsage)
		return nil, true
	case "stats":
		m.openInfo(c, infoHistory)
		return nil, true
	}
	return nil, false
}

// showView switches the Session to one of its views by name.
func (m *Model) showView(c *hostConn, name string) bool {
	for i, v := range m.views(c) {
		if v == name {
			c.view = i
			return true
		}
	}
	return false
}

// openScreen hands the terminal to Claude Code on one of its own screens,
// for the agent's account and folder. Leaving it (esc, or ctrl+c twice)
// brings rush back; a hosted session reloads its plugins after /plugin.
func (m *Model) openScreen(c *hostConn, a *fleet.Agent, screen string) tea.Cmd {
	key, k := c.key, sessionAgent(c)
	sc, ok := agent.As[agent.Screener](k)
	if !ok {
		m.flash(harnessName(string(k))+" has no screens of its own", true)
		return nil
	}
	hint := "\033[2m  rush · " + harnessName(string(k)) + "'s /" + screen + " · when you're done: esc, then ctrl+c twice to come back\033[0m"
	acct, cwd := a.Acct, firstNonEmpty(c.sess.Info.Cwd, a.Cwd)
	return func() tea.Msg {
		cmd := sc.Screen(acct, cwd, screen, hint) // it reads the account's settings
		return tea.ExecProcess(cmd, func(err error) tea.Msg { return screenDoneMsg{key: key, screen: screen, err: err} })()
	}
}

type screenDoneMsg struct {
	key, screen string
	err         error
}

// onScreenDone is rush coming back from a Claude Code screen.
func (m *Model) onScreenDone(msg screenDoneMsg) tea.Cmd {
	if msg.err != nil {
		m.flash("Claude Code's /"+msg.screen+": "+msg.err.Error(), true)
		return nil
	}
	c := m.host
	if msg.screen != "plugin" || c == nil || c.key != msg.key || c.client == nil {
		return nil
	}
	// A running session only sees newly enabled plugins once it reloads
	// them; mid-turn, that's left to you.
	if c.sess.Info.State == "working" {
		m.flash("plugins change for "+c.sess.Info.Name+" after /reload-plugins, once this turn ends", false)
		return nil
	}
	cl := c.client
	return hostCmd(func() error { return cl.Send("/reload-plugins") })
}

func hasQueuedExchange(c *hostConn) bool {
	return slices.ContainsFunc(c.sess.Info.QueueExchanges, func(e *event.Exchange) bool { return e != nil })
}
