package plugin

import (
	"fmt"
	"sort"
	"strings"
)

// Describe says, in plain words, what approving p allows: the same text
// `rush plugin check|approve` shows at a terminal, and what a UI's own
// approval dialog shows instead of one.
func Describe(p Plugin) string {
	var b strings.Builder
	w := func(f string, a ...any) { fmt.Fprintf(&b, f, a...) }
	w("%s", p.Name)
	if p.Version != "" {
		w(" %s", p.Version)
	}
	w("\n")
	if p.Description != "" {
		w("  %s\n", printable(p.Description))
	}
	w("\nIt runs %s", strings.Join(p.Command, " "))
	if p.Proto() == ProtoMCP {
		w(" as an MCP server")
	}
	w(", sandboxed:\n")
	w("  • reads its folder, %s, and the system's libraries\n", p.Dir)
	for _, r := range p.Read {
		w("  • also reads %s\n", r)
	}
	if len(p.Write) == 0 {
		w("  • writes only %s\n", DataDir(p.Name))
	} else {
		w("  • writes %s, and %s\n", DataDir(p.Name), strings.Join(p.Write, ", "))
	}
	if len(p.Network) == 0 {
		w("  • no network\n")
	} else {
		w("  • reaches only %s\n", strings.Join(p.Network, ", "))
	}
	w("  • starts no other programs itself, at most %d MB of memory\n", p.Memory()>>20)
	if len(p.Exec) > 0 {
		names := make([]string, 0, len(p.Exec))
		for n := range p.Exec {
			names = append(names, n)
		}
		sort.Strings(names)
		w("\nOutside the sandbox, as you, rush runs for it (with any arguments it adds):\n")
		for _, n := range names {
			w("  • %s\n", strings.Join(p.Exec[n], " "))
		}
	}

	var grants []string
	if p.HasTools() {
		grants = append(grants, "offers its tools to every rush-mode session (each call asks you first, as any tool does)")
	}
	if len(p.Agents) > 0 {
		names := make([]string, 0, len(p.Agents))
		for n := range p.Agents {
			names = append(names, p.Name+":"+n)
		}
		sort.Strings(names)
		grants = append(grants, "adds subagents to every rush-mode session: "+strings.Join(names, ", "))
	}
	if p.Prompt != "" {
		grants = append(grants, fmt.Sprintf("adds %d characters to every rush-mode session's system prompt:\n      %s",
			len(p.Prompt), strings.ReplaceAll(clip(p.Prompt, 400), "\n", "\n      ")))
	}
	if p.Can(CapList) {
		grants = append(grants, "sees every rush-mode session's name, folder, branch, state, cost and context use (not what was said)")
	}
	if p.Can(CapStart) {
		grants = append(grants, fmt.Sprintf("starts sessions on your account in %s, never in a mode that skips asking you;\n"+
			"      tools your Claude Code settings already allow run there without asking", strings.Join(p.Workspaces, ", ")))
	}
	if p.Can(CapRead) {
		grants = append(grants, "follows what the sessions it started say")
	}
	if p.Can(CapSend) {
		grants = append(grants, "sends messages to the sessions it started")
	}
	if p.Can(CapQueue) {
		grants = append(grants, fmt.Sprintf("queues messages, marked as its own, to any session in %s that asks you before acting", strings.Join(p.Workspaces, ", ")))
	}
	if p.Can(CapControl) {
		grants = append(grants, "interrupts and stops the sessions it started")
	}
	if p.Can(CapQueued) {
		grants = append(grants, fmt.Sprintf("sends now, or drops, the messages you queued in any session in %s, by their place in the queue, without seeing them", strings.Join(p.Workspaces, ", ")))
	}
	if p.Sidebar {
		grants = append(grants, "arranges your agent list: groups and names agents in sections of its own, offered as a group-by mode")
	}
	if p.CanUI(UIInput) {
		grants = append(grants, "SEES EVERYTHING YOU TYPE in rush's message boxes, as you type it and when you send or clear it, and may set what's in them")
	}
	if p.CanUI(UIIntercept) {
		grants = append(grants, fmt.Sprintf("is asked before each message you send goes, and may change it or hold it back (it has %v; after that it goes as it was)", InterceptBudget))
	}
	if p.CanUI(UIEvents) {
		grants = append(grants, "hears what happens in rush's screen: sessions seen, opened and left, turns starting and ending, why a session stopped, the network going and coming back; with each, what the agent list shows of the session (never what was said)")
	}
	if p.CanUI(UISend) {
		grants = append(grants, fmt.Sprintf("sends messages, as if you'd typed them, to sessions in %s that ask you before acting", strings.Join(p.Workspaces, ", ")))
	}
	if p.CanUI(UIOverview) {
		grants = append(grants, "adds sections to a Session's overview, and a word or two to its row in the list")
	}
	if p.CanUI(UINotify) {
		grants = append(grants, "shows short notices at the bottom of rush's screen")
	}
	if len(p.Commands) > 0 {
		cs := make([]string, 0, len(p.Commands))
		for _, c := range p.Commands {
			k := ""
			if c.Key != "" {
				k = " (" + c.Key + ", if it's free)"
			}
			cs = append(cs, c.Name+k)
		}
		grants = append(grants, "adds commands you can run and bind to keys: "+strings.Join(cs, ", "))
	}
	if len(p.CLI) > 0 {
		cs := make([]string, 0, len(p.CLI))
		for _, c := range p.CLI {
			cs = append(cs, "rush "+p.Name+" "+c.Name)
		}
		grants = append(grants, "adds commands to rush's CLI, run by the plugin with what it may do: "+strings.Join(cs, ", "))
	}
	if len(p.Settings) > 0 {
		ss := make([]string, 0, len(p.Settings))
		for _, st := range p.Settings {
			ss = append(ss, st.Title)
		}
		grants = append(grants, "offers settings under Settings, Plugins: "+strings.Join(ss, ", "))
	}
	if needs := p.Requires.Needs(); needs != "" {
		w("\nIt runs only with %s.\n", needs)
	}
	if len(grants) > 0 {
		w("\nIt:\n")
		for _, g := range grants {
			w("  • %s\n", g)
		}
	}
	return b.String()
}
