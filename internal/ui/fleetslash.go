package ui

import (
	"slices"
	"sort"
	"strings"
	"unicode"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// rush's own commands start with #, so / is always Claude's: in the
// Prompt it starts a session with one of Claude's commands or skills, in
// a Session it goes to the agent. A # command in the Prompt acts on the
// selected agent, in a Session on that Session's agent.

// fleetCommands are rush's # commands, the ones command() runs. A hint in
// <> needs an argument; one in [] can go without.
var fleetCommands = []event.Command{
	{Name: "expand", Description: "open all conversation turns; tool details keep their own folds"},
	{Name: "collapse", Description: "preview older turns, keeping the latest turn open"},
	{Name: "model", Description: "choose the current session’s model", ArgumentHint: "[model]"},
	{Name: "effort", Description: "choose the current session’s reasoning effort", ArgumentHint: "[level]"},
	{Name: "agent", Description: "what the next session starts as, or in a session what it switches to, then a message to send it; alone opens the sheet (Shift+Tab)", ArgumentHint: "[harness:account[:effort]] [message]"},
	{Name: "done", Description: "move the agent to Done (alt+d); its idle process stops"},
	{Name: "room", Description: "a full-screen group chat: fresh agents argue a topic to a verdict, and you're in it", ArgumentHint: "[new|topic]"},
	{Name: "discuss", Description: "a room on this chat: agents argue it, with its latest turns as context, and the verdict comes back here", ArgumentHint: "[topic]"},
	{Name: "intervene", Description: "two agents go through this chat (or @agent's): plan, status, turns and tool calls; they tell you why it's stuck, and what the agent must do goes in its box", ArgumentHint: "[@agent] [concern]"},
	{Name: "broadcast", Description: "send one message to several agents: all, @names, or alone to click them in the list", ArgumentHint: "[all|@agent…] [message]"},
	{Name: "feed", Description: "the feed your agents share: blockers, fixes, heads-ups; on|off switches it for all of rush; open|closed lets agents cross projects", ArgumentHint: "[on|off|open|closed|new]"},
	{Name: "go", Description: "tell the agent to keep going (alt+g); after an error, to continue"},
	{Name: "away", Description: "leave the session to carry on while you're gone: at each check-in (every 30m unless you say) it's told to keep going, and what it asks or needs approved is saved for when you're back, until the time's up or it's all done; off, or alone, says what happened", ArgumentHint: "[3h [every 15m]|off]"},
	{Name: "loop", Description: "tell the agent to keep going each time it's idle at a check-in (every 30m unless you say), till it's all done or the time you give is up; questions wait for you as ever", ArgumentHint: "[3h] [every 15m]|off"},
	{Name: "back", Description: "what happened while you were away: check-ins, its last words, what it saved for you; ends away"},
	{Name: "stop", Description: "stop the agent"},
	{Name: "rm", Description: "delete the session, and its worktree when that's safe"},
	{Name: "kill", Description: "kill the agent and everything it started"},
	{Name: "perm", Description: "choose session permissions (also in Shift+Tab)", ArgumentHint: "[mode]"},
	{Name: "yolo", Description: "explicitly enable this harness’s supported bypass permission mode"},
	{Name: "handoff", Description: "carry this conversation on in another harness, in a new session, from a summary (this one stays as it is)", ArgumentHint: "<harness>"},
	{Name: "compact", Description: "compact this session with a model of your choosing: its own (keeps the cache), a cheaper Claude, or a local Ollama one"},
	{Name: "slim", Description: "what this session carries every request and never uses (MCP servers, subagents, skills): drop them for it alone, or compact it (#optimise)"},
	{Name: "clean", Description: "delete the agent's temp work; all does every finished agent", ArgumentHint: "[all]"},
	{Name: "cd", Description: "tell the agent to work in another folder from now on", ArgumentHint: "[path]"},
	{Name: "rename", Description: "rename the agent, or type the new name", ArgumentHint: "[name]"},
	{Name: "pin", Description: "pin or unpin the agent in its harness"},
	{Name: "pr", Description: "open the agent's pull request"},
	{Name: "full", Description: "open a Claude Code agent full screen, in Claude Code"},
	{Name: "rush", Description: "move the agent into rush mode (a terminal one is copied, not stopped)"},
	{Name: "new", Description: "start an agent on any harness, provider, model and effort, once; defaults stay as they are", ArgumentHint: "[harness@provider[:account]] [model] [effort] [task]"},
	{Name: "with", Description: "what the next session starts as, once: #new without a task", ArgumentHint: "[harness@provider[:account]] [model] [effort]"},
	{Name: "profile", Description: "the profile the next session starts under (which providers it runs, and what it does at a limit), or in a session one it switches to; alone says which", ArgumentHint: "[name] [message]"},
	{Name: "mackeys", Description: "send Terminal.app's ⌘← → ⌘⌫ ⌘⌦ ⌘Z on to rush through Hammerspoon, installed with brew if need be; alone says whether it's on", ArgumentHint: "[on|off]"},
	{Name: "ghostty", Description: "put Ghostty on LangWatch's light and dark themes, following the system; off puts its own colours back; alone says whether it's on", ArgumentHint: "[on|off]"},
	{Name: "statusline", Description: "build the top bar, the agent header and Claude Code's status line"},
	{Name: "network", Description: "whether the API answers, the network rush is on and how fast it moves, and what waits for it (#net)"},
	{Name: "view", Description: "Agents and the Session side by side, the agent's Session alone, or Agents alone (shift+← →)", ArgumentHint: "<split|agent|list>"},
	{Name: "stash", Description: "what you set aside, sent, cleared and replaced, to put back in the box (ctrl+r · ctrl+s sets what's typed aside)"},
	{Name: "guide", Description: "ask the guide, a fast model that sees your sessions: what's next, which one needs you, where something is; it opens the one it means", ArgumentHint: "[question]"},
	{Name: "ask", Description: "ask rush about itself, or have it change a setting for you: an agent in rush's own folder, with its guide", ArgumentHint: "[question]"},
	{Name: "help", Description: "a short guide to rush"},
	{Name: "update", Description: "install the newest rush, with go install; #reload runs it"},
	{Name: "reload", Description: "run the installed rush in this one's place, where you were; sessions carry on; all reloads the other rush views first", ArgumentHint: "[all]"},
	{Name: "tips", Description: "Getting started and tips from the top; off puts them away", ArgumentHint: "[off]"},
	{Name: "quit", Description: "leave rush"},
}

// fleetAliases are other names command() answers to.
var fleetAliases = map[string]string{"permissions": "perm", "optimise": "slim", "optimize": "slim", "trim": "slim", "bloat": "slim", "undone": "done", "delete": "rm", "move": "cd", "exit": "quit", "history": "stash", "drafts": "stash", "net": "network", "afk": "away", "flight": "away", "twotter": "feed", "twatter": "feed", "twitter": "feed", "twattr": "feed", "community": "feed", "chirp": "feed", "chirps": "feed"}

// fleetNeedsAgent are # commands that act on the selected or focused agent;
// the bar offers them only once one's in view. The rest are rush-wide.
var fleetNeedsAgent = map[string]bool{
	"done": true, "go": true, "away": true, "loop": true, "back": true, "stop": true, "rm": true, "kill": true,
	"clean": true, "cd": true, "rename": true,
	"pin": true, "pr": true, "full": true, "rush": true, "compact": true, "slim": true, "handoff": true,
}

// isHashCmd is whether text is a # command: # and a letter, so a Markdown
// heading (# Plan) or an issue (#123) is still a message.
func isHashCmd(text string) bool {
	r := []rune(text)
	return len(r) >= 2 && r[0] == '#' && unicode.IsLetter(r[1])
}

// typingHash is whether a # command is being typed, # alone included.
func typingHash(text string) bool { return text == "#" || isHashCmd(text) }

// isFleetCommand is whether name (without its prefix) is one of rush's.
func isFleetCommand(name string) bool {
	if fleetAliases[name] != "" {
		return true
	}
	for _, c := range fleetCommands {
		if c.Name == name {
			return true
		}
	}
	return false
}

// fleetArgs are what a command offers once you've typed a space, and which
// of them is in use now.
func (m *Model) fleetArgs(name string) (opts []string, now string) {
	switch name {
	case "with":
		for _, a := range m.agentOrder() {
			opts = append(opts, string(a.Kind()))
		}
		return opts, m.store.Config.DefaultAgent()
	case "profile":
		for _, p := range m.store.Config.AllProfiles() {
			opts = append(opts, p.Name)
		}
		// Each agent is a profile too: its provider in its harness.
		for _, a := range m.agentOrder() {
			if _, ok := m.store.Config.ProfileNamed(string(a.Kind())); ok && !slices.Contains(opts, string(a.Kind())) {
				opts = append(opts, string(a.Kind()))
			}
		}
		return opts, m.startProfile(m.startDir()).Name
	case "clean":
		return []string{"all"}, ""
	case "away":
		return []string{"1h", "2h", "3h", "5h", "8h", "off"}, ""
	case "loop":
		return []string{"every 15m", "every 30m", "every 1h", "off"}, ""
	case "broadcast":
		opts = []string{"all"}
		for _, a := range m.broadcastAll() {
			opts = append(opts, "@"+mentionName(a))
		}
		return opts, ""
	case "view":
		return []string{"split", "agent", "list"}, m.viewNow()
	}
	return nil, ""
}

// filterCommands keeps the commands whose name contains q, those starting
// with it first. An argument hint leads the description.
func filterCommands(q string, lists ...[]event.Command) []event.Command {
	var out []event.Command
	seen := map[string]bool{}
	for _, list := range lists {
		for _, cmd := range list {
			if seen[cmd.Name] || !strings.Contains(strings.ToLower(cmd.Name), q) {
				continue
			}
			seen[cmd.Name] = true
			if cmd.ArgumentHint != "" {
				cmd.Description = strings.TrimSpace(cmd.ArgumentHint + "  " + cmd.Description)
			}
			out = append(out, cmd)
		}
	}
	// A plugin's plugin:name starts with q when its name does.
	starts := func(name string) bool {
		name = strings.ToLower(name)
		_, short, _ := strings.Cut(name, ":")
		return strings.HasPrefix(name, q) || strings.HasPrefix(short, q)
	}
	sort.SliceStable(out, func(i, j int) bool { return starts(out[i].Name) && !starts(out[j].Name) })
	return out
}

// hashMatches is what the picker offers for a # command being typed: the
// commands containing what's typed, or after a space, the command's
// choices.
func (m *Model) hashMatches(in []rune, back int) []event.Command {
	text := string(in)
	if back != 0 || !strings.HasPrefix(text, "#") || strings.ContainsAny(text, "\n") ||
		len(in) > 1 && !unicode.IsLetter(in[1]) {
		return nil
	}
	name, q, hasArg := strings.Cut(text[1:], " ")
	if hasArg && (name == "new" || name == "with") {
		return m.newArgs(name, q)
	}
	if hasArg && (name == "agent" || name == "profile") {
		return m.setupArgs(text, back, m.hashStart)
	}
	if !hasArg {
		return filterCommands(strings.ToLower(name), append(m.availableFleetCommands(), m.pluginHashCommands()...))
	}
	opts, now := m.fleetArgs(name)
	if strings.Contains(q, " ") {
		return nil
	}
	var out []event.Command
	for _, o := range opts {
		if !strings.HasPrefix(strings.ToLower(o), strings.ToLower(q)) {
			continue
		}
		d := ""
		if o == now {
			d = "now"
		}
		out = append(out, event.Command{Name: name + " " + o, Description: d})
	}
	return out
}

// hashStart is what #agent and #profile change: the open Session's start
// when its box has the keys, else the next session's.
func (m *Model) hashStart() startOver {
	if m.paneFocus && m.host != nil {
		return m.sessionStart(m.host)
	}
	return m.nextStart(m.startDir())
}

// newSessionCommands are the commands and skills on disk of the agent a
// new session would run, for its profile and the folder it would start
// in, from memory: they're read in the background, and none show until
// they have been.
func (m *Model) newSessionCommands() []agent.Command {
	k := agent.Kind(m.startKind())
	p, ok := m.agentProfile(k)
	if !ok {
		p = agent.Profile{Kind: k}
	}
	return m.commandsOf(k, p, m.startDir())
}

// promptPicker is what the Prompt's picker offers, and the prefix its
// commands take: # for rush's, / for a new session's.
func (m *Model) promptPicker() ([]event.Command, string) {
	if m.inKind != inPrompt || m.sessionFocused() || !m.acceptsText() {
		return nil, ""
	}
	if cmds := m.hashMatches(m.input, m.back); len(cmds) > 0 {
		return cmds, "#"
	}
	if cmds := m.mentionMatches(m.input, m.back); len(cmds) > 0 {
		return cmds, "@"
	}
	text := string(m.input)
	if m.back != 0 || !strings.HasPrefix(text, "/") || strings.ContainsAny(text[1:], " \n/") {
		return nil, ""
	}
	var list []event.Command
	for _, f := range m.newSessionCommands() {
		list = append(list, event.Command{Name: f.Name, Description: f.Description, ArgumentHint: f.ArgumentHint})
	}
	return filterCommands(strings.ToLower(text[1:]), list), "/"
}

// fleetSlashLines draws the Prompt's picker above its box.
func (m *Model) fleetSlashLines(w int) []string {
	cmds, lead := m.promptPicker()
	if len(cmds) == 0 {
		return nil
	}
	m.slashSel = max(0, min(m.slashSel, len(cmds)-1))
	if lead == "#" {
		return pickerRows(cmds, m.slashSel, w, "#", func(string) string { return "" }, "↑↓ · tab completes · enter runs")
	}
	if lead == "@" {
		return pickerRows(cmds, m.slashSel, w, "@", func(string) string { return "" }, mentionHow)
	}
	skills := map[string]bool{}
	for _, f := range m.newSessionCommands() {
		skills[f.Name] = f.Skill
	}
	tag := func(name string) string {
		if skills[name] {
			return paint(cBlue, " skill")
		}
		return ""
	}
	return pickerRows(cmds, m.slashSel, w, "/", tag, "↑↓ · tab completes · enter starts it")
}

// fleetSlashKey drives the Prompt's picker while a command is being typed.
func (m *Model) fleetSlashKey(s string) (tea.Cmd, bool) {
	cmds, lead := m.promptPicker()
	if len(cmds) == 0 {
		return nil, false
	}
	m.slashSel = max(0, min(m.slashSel, len(cmds)-1))
	pick := cmds[m.slashSel]
	switch s {
	case "up", "down":
		m.slashSel = pickerMove(m.slashSel, len(cmds), s)
	case "tab", "enter":
		if lead != "@" {
			break
		}
		m.input, m.back = completeMention(m.input, m.back, pick.Name)
		m.slashSel = 0
		return nil, true
	}
	switch s {
	case "up", "down":
	case "tab":
		m.input, m.back, m.slashSel = completed(lead, pick, true), 0, 0
	case "enter":
		m.input, m.back, m.slashSel = completed(lead, pick, false), 0, 0
		if !needsArg(pick) {
			return m.submit(), true
		}
	default:
		return nil, false
	}
	return nil, true
}

// paneHashKey drives the # picker in a Session's box: its commands act on
// that Session's agent.
func (m *Model) paneHashKey(c *hostConn, s string) (tea.Cmd, bool) {
	cmds, lead := m.hashMatches(c.input, c.back), "#"
	if len(cmds) == 0 {
		cmds, lead = m.mentionMatches(c.input, c.back), "@"
	}
	if len(cmds) == 0 {
		return nil, false
	}
	c.slashSel = max(0, min(c.slashSel, len(cmds)-1))
	pick := cmds[c.slashSel]
	switch s {
	case "up", "down":
		c.slashSel = pickerMove(c.slashSel, len(cmds), s)
	case "tab", "enter":
		if lead != "@" {
			break
		}
		c.input, c.back = completeMention(c.input, c.back, pick.Name)
		c.slashSel = 0
		return nil, true
	}
	switch s {
	case "up", "down":
	case "tab":
		c.input, c.back, c.slashSel = completed(lead, pick, true), 0, 0
	case "enter":
		c.input, c.back, c.slashSel = completed(lead, pick, false), 0, 0
		if !needsArg(pick) {
			text := string(c.input)
			c.input = c.input[:0]
			return m.command(m.agentByKey(c.key), text), true
		}
	default:
		return nil, false
	}
	return nil, true
}

func pickerMove(sel, n int, s string) int {
	if s == "up" {
		return roundMove(sel, -1, n)
	}
	return roundMove(sel, 1, n)
}

// roundMove is a menu's cursor moved d rows: ↑ on the first row goes to
// the last, ↓ on the last to the first.
func roundMove(cur, d, n int) int {
	if n <= 0 {
		return 0
	}
	return ((cur+d)%n + n) % n
}

// needsArg is whether a picked command can't run without an argument.
func needsArg(c event.Command) bool { return strings.HasPrefix(c.ArgumentHint, "<") }

// completed is the box's text once a command is picked: with a space to
// type its argument after when it takes one, or always with tab.
func completed(lead string, c event.Command, tab bool) []rune {
	out := []rune(lead + c.Name)
	if needsArg(c) || tab && c.ArgumentHint != "" {
		out = append(out, ' ')
	}
	return out
}

// legacyCommand runs a Prompt command typed the old way, /stop for #stop,
// when Claude has no command of that name, and says what it's called now.
func (m *Model) legacyCommand(text string) (tea.Cmd, bool) {
	f := strings.Fields(text)
	name := strings.ToLower(strings.TrimPrefix(f[0], "/"))
	if !isFleetCommand(name) {
		return nil, false
	}
	for _, c := range m.newSessionCommands() {
		if c.Name == name {
			return nil, false
		}
	}
	at := m.statusAt
	cmd := m.command(m.selected(), "#"+strings.TrimPrefix(text, "/"))
	if m.statusAt == at {
		m.flash("rush's commands start with # now: #"+name+" · / starts a session with one of Claude's", false)
	}
	return cmd, true
}

// Commands offered here must be meaningful for the agent currently targeted.
func (m *Model) availableFleetCommands() []event.Command {
	a := m.selected()
	if m.paneFocus && m.host != nil {
		a = m.agentByKey(m.host.key)
	}
	var out []event.Command
	for _, c := range fleetCommands {
		if fleetNeedsAgent[c.Name] && a == nil {
			continue
		}
		if a != nil {
			k := agent.Kind(a.Kind)
			if !agentCanRun(k, c.Name) {
				continue
			}
			if (c.Name == "native" || c.Name == "full") && !agent.Supports(k, agent.FeatureScreen) {
				continue
			}
			if c.Name == "full" && (a.Rush || a.Interactive || a.Past) {
				continue
			}
			if c.Name == "pin" {
				if _, ok := agent.As[agent.Pinner](a.Acct.Kind); !ok {
					continue
				}
			}
			if c.Name == "handoff" && (m.host == nil || m.host.key != a.Key) {
				continue
			}
			if c.Name == "slim" && (m.host == nil || m.host.key != a.Key || m.host.client == nil || m.host.sess.Usage == nil) {
				continue
			}

		}
		out = append(out, c)
	}
	return out
}
