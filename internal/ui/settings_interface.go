package ui

import (
	"github.com/0xdeafcafe/photon/cellw"

	"fmt"
	"slices"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// interfaceSections are how rush looks and what its keys do.
func (m *Model) interfaceSections() []section {
	c := &m.store.Config

	view := choiceSetting("Layout", c.View,
		"How rush lays out Agents and the Session, now and next time. #view and shift+← → change it too.",
		[][2]string{
			{"split", "Agents on the left, the picked agent's Session beside them, when the screen is wide enough."},
			{"agent", "one Session with the whole screen; esc shows Agents, and the next one you open has the whole screen too."},
			{"list", "Agents alone; a Session you open takes the screen until esc."},
			{"", "rush asks which the next time it opens."},
		}, func(v string) {
			if v == "" {
				c.View = ""
				return
			}
			c.SetView(v)
		})
	view.unset = "ask"
	enter := choiceSetting("Enter on an agent", c.EnterOn,
		"What enter does on an agent in the list.",
		[][2]string{
			{"rename", "renames it, as in the Finder; ctrl+o opens it."},
			{"open", "opens its Session; ctrl+r renames it."},
			{"", "rush asks the first time you press it."},
		}, func(v string) { c.EnterOn = v })
	enter.unset = "ask"
	group := choiceSetting("Group by", firstNonEmpty(c.GroupBy, "status"), "How agents are sorted into sections.", [][2]string{
		{"status", "Needs you first (questions, halts, your turn), then Stuck?, Working, and Idle (open, or stopped recently: Settings › General); the rest of the last day under Today, older ones under Earlier."},
		{"agent", "one section per harness (Claude Code, Codex, Copilot…), handy when you run several."},
		{"group", "your own sections; put an agent in one with /group <name>. Ungrouped agents fall back to status."},
	}, func(v string) { c.GroupBy = v })
	group.choices = m.groupModes() // plugins arrange the list too
	split := choiceSetting("Split by project", map[bool]string{true: "on", false: "off"}[m.splitProjects()],
		"Whether each section's agents sit together by the repository they work in, whatever the grouping. ctrl+p, or the toggle at the top of the list, changes it too.",
		[][2]string{
			{"on", "under each section, a line per project with its branch, commits ahead and behind and uncommitted changes; agents in a linked worktree under the repository it came from."},
			{"off", "one run of rows per section, as the sort puts them."},
		}, func(v string) {
			c.SplitBy = map[bool]string{true: "project", false: "none"}[v == "on"]
			m.rebuild()
		})
	sortBy := choiceSetting("Sort rows by", firstNonEmpty(c.SortBy, "name"),
		"The order of rows inside each section. You can also click a column header on the Agents view.",
		[][2]string{
			{"name", "alphabetical, so a row stays put while its agent works."},
			{"recent", "most recently active first; rows move as agents update."},
			{"cost", "most expensive first."},
			{"cpu", "busiest first, by CPU across everything the agent started."},
			{"ram", "heaviest first, by memory across everything the agent started."},
			{"tokens", "biggest context first, by its newest message's tokens."},
			{"time", "longest-running first."},
		}, func(v string) { c.SortBy = v })
	width := ""
	if c.SideWidth > 0 {
		width = fmt.Sprintf("%.0f%%", c.SideWidth*100)
	}
	listWidth := choiceSetting("List width", width,
		"The list's share of the screen beside a Session. Dragging its edge, or shift+← →, changes it too.",
		[][2]string{
			{"", "rush's choice, by the screen's width."},
			{"25%", "a quarter of the screen."},
			{"33%", "a third of the screen."},
			{"50%", "half the screen."},
		}, func(v string) {
			var pct float64
			fmt.Sscanf(v, "%g%%", &pct)
			c.SideWidth = pct / 100
		})
	listWidth.unset = "rush's"
	dock := choiceSetting("Card lines", fmt.Sprint(m.dockLines()),
		"How many lines of the agent's latest words its card under the list shows.",
		[][2]string{{"3", "three lines."}, {"6", "six lines."}, {"10", "ten lines."}, {"15", "fifteen lines."}},
		func(v string) { fmt.Sscanf(v, "%d", &c.DockLines) })
	stackAt := ""
	switch {
	case c.StackAt < 0:
		stackAt = "narrow only"
	case c.StackAt >= 100:
		stackAt = "always"
	case c.StackAt > 0:
		stackAt = fmt.Sprintf("%d%%", c.StackAt)
	}
	stack := choiceSetting("Two-line rows", stackAt,
		"When each agent's row takes two lines, its name and figures above and its latest words under them.",
		[][2]string{
			{"", "when the list has 42% of the screen or less, or is too narrow for words beside the name."},
			{"30%", "when the list has 30% of the screen or less, or is too narrow for words beside the name."},
			{"50%", "when the list has half the screen or less, or is too narrow for words beside the name."},
			{"60%", "when the list has 60% of the screen or less, or is too narrow for words beside the name."},
			{"narrow only", "only when the list is too narrow for words beside the name: one line wherever it fits."},
			{"always", "every row takes two lines, however wide the list is."},
		}, func(v string) {
			switch v {
			case "":
				c.StackAt = 0
			case "narrow only":
				c.StackAt = -1
			case "always":
				c.StackAt = 100
			default:
				fmt.Sscanf(v, "%d%%", &c.StackAt)
			}
		})
	stack.unset = "42%"

	theme := choiceSetting("Theme", c.Theme,
		"What rush's colours are made for. Its text and panels are shades between your terminal's background and text colour, so they follow its theme; its orange, green and red stay, made as easy to read on your background as on rush's own.",
		[][2]string{
			{"", "rush asks the terminal for its background and text, again whenever you come back to it; a terminal that doesn't say gets rush's dark."},
			{"dark", "rush's own dark colours, whatever the terminal says."},
			{"light", "rush's own light colours, for a light terminal that doesn't say it's light."},
		}, func(v string) {
			c.Theme = v
			m.applyColors()
		})
	theme.unset = "match terminal"
	colours := choiceSetting("Colours", map[bool]string{true: "colour-blind", false: "standard"}[c.ColorBlind],
		"How rush tells good from bad: added and removed lines in a diff, done and failed steps and agents.",
		[][2]string{
			{"standard", "green and red."},
			{"colour-blind", "sky blue and amber, which stay apart for red-green and blue-yellow colour blindness alike; + and − still mark every diff line."},
		}, func(v string) {
			c.ColorBlind = v == "colour-blind"
			m.colored = false
			m.applyColors()
		})
	logo := choiceSetting("Logo", map[bool]string{true: "hidden", false: "shown"}[c.HideLogo],
		"The rush bottle at the top left.",
		[][2]string{{"shown", "the bottle beside the counts, five rows tall."}, {"hidden", "text only: the header takes three rows, and the rest goes to the list."}},
		func(v string) { c.HideLogo = v == "hidden" })
	spaces := choiceSetting("Spaces and tabs in diffs", map[bool]string{true: "shown", false: "hidden"}[c.ShowWhitespace],
		"Whether a diff marks its spaces and tabs, as · and →, so a change in indentation shows.",
		[][2]string{{"hidden", "diffs show text only."}, {"shown", "every space is a faint · and every tab a →."}},
		func(v string) {
			c.ShowWhitespace = v == "shown"
			convo.SetShowWhitespace(c.ShowWhitespace)
		})

	subNow := fmt.Sprintf("%d%%", int(subLineFade(c.SubLine)*100+0.5))
	subLine := choiceSetting("Second line transparency", subNow,
		"How see-through the line under each agent's name is: what it's doing, or what it last said. It keeps its state's colour, only quieter.",
		[][2]string{
			{"0%", "solid: as bright as the name."},
			{"25%", "a step back from the name."},
			{"50%", "half way to the background."},
			{"75%", "a shade above the ground, there when you look for it."},
		}, func(v string) {
			c.SubLine = v
			if v == "0%" {
				c.SubLine = ""
			}
		})
	search := choiceSetting("ctrl+k searches transcripts", map[bool]string{true: "on ctrl+enter", false: "as you type"}[c.SearchTranscriptsOnKey],
		"What the command bar (ctrl+k) looks through as you type.",
		[][2]string{
			{"as you type", "agent names, commands and every transcript, as you type."},
			{"on ctrl+enter", "names and commands as you type; ctrl+enter (or ctrl+j) then searches the transcripts."},
		}, func(v string) { c.SearchTranscriptsOnKey = v == "on ctrl+enter" })

	command := choiceSetting("Enter after a /command", map[bool]string{true: "sends", false: "completes"}[c.EnterSendsCommand],
		"What enter does when a message ends in a /command the picker shows, typed in full.",
		[][2]string{
			{"completes", "puts the command in with a space after it; enter again sends the message."},
			{"sends", "sends the message as it is; a command typed in part is still completed."},
		}, func(v string) { c.EnterSendsCommand = v == "sends" })

	rowKey := choiceSetting("Open a picked row with", firstNonEmpty(c.RowKey, "enter and space"),
		"The key that opens or closes the conversation row you've picked. Whichever it is, nothing you've typed is sent while a row is picked.",
		[][2]string{
			{"enter and space", "either opens or closes it."},
			{"enter", "enter does; space says so."},
			{"space", "space does; enter says so."},
		}, func(v string) { c.RowKey = map[bool]string{true: "", false: v}[v == "enter and space"] })

	copying := choiceSetting("Copy on select", map[bool]string{true: "on", false: "off"}[c.CopiesOnSelect()],
		"Whether text you drag over, in a conversation or a message box, goes to the clipboard as you let go.",
		[][2]string{
			{"on", "it's copied as the drag ends, as terminals do."},
			{"off", "it stays selected until cmd+c (where the terminal passes it on) or ctrl+c copies it; for a terminal that copies with cmd+c itself."},
		}, func(v string) {
			on := v == "on"
			c.CopyOnSelect = &on
		})

	// What a setting changes, as it will look: the Agents list and, where
	// there's room, a Session beside it, as the split layout sets them.
	preview := func(w, h int) []string {
		lines := m.listPreview(w-4, max(1, h-3)) // its header and box take three

		bw := 0
		for _, l := range lines {
			bw = max(bw, cellw.String(l))
		}
		list := pbox(paint(cText+bold, "Agents"), dim("your own, as the list shows them"), lines, bw+4, false)
		if sw := w - bw - 5; sw >= 40 {
			sess := pbox(paint(cText+bold, "Session"), dim("an example"), pad(sessionPreview(sw-4, len(lines)), len(lines)), sw, false)
			return sideBySide(list, sess, bw+4)
		}
		return list
	}
	var cols []setting
	for _, col := range [][3]string{
		{"running", "RUNNING", "the subagents and shells each agent has going."},
		{"cpu", "CPU", "how busy everything each agent started is."},
		{"ram", "RAM", "the memory everything each agent started holds."},
		{"tokens", "TOKENS", "how full each agent's context is."},
		{"cost", "COST", "what each agent has spent."},
		{"time", "TIME", "how long each has run; for one that needs you, how long it has waited; for a stuck one, how long it has been silent."},
	} {
		shown := map[bool]string{true: "shown", false: "hidden"}[c.Shows(col[0])]
		cols = append(cols, choiceSetting(col[1], shown, "The "+col[1]+" column: "+col[2]+" A narrow list drops some by itself.",
			[][2]string{{"shown", "when the list is wide enough for it."}, {"hidden", "never: its room goes to names and words."}},
			func(v string) {
				c.HideColumns = slices.DeleteFunc(c.HideColumns, func(h string) bool { return h == col[0] })
				if v == "hidden" {
					c.HideColumns = append(c.HideColumns, col[0])
				}
			}))
	}
	for _, st := range append([]*setting{&view, &theme, &colours, &logo, &spaces, &group, &split, &sortBy, &stack, &subLine}, ptrs(cols)...) {
		st.preview = preview
	}

	minimap := choiceSetting("Conversation minimap", map[bool]string{true: "hidden", false: "shown"}[c.HideMinimap],
		"A miniature of the conversation along its right edge. Click or drag to scroll. Hidden automatically in narrow panes.",
		[][2]string{{"shown", "coloured text shapes and a shaded viewport; click or drag to navigate."}, {"hidden", "all available width goes to the conversation."}},
		func(v string) { c.HideMinimap = v == "hidden" })
	pictures := choiceSetting("Pictures", map[bool]string{true: "pixelated", false: "sharp"}[c.PixelPictures],
		"How images in the conversation are drawn. Sharp needs a terminal with Kitty graphics (Ghostty, Kitty); elsewhere they're always pixelated.",
		[][2]string{{"sharp", "full detail where the terminal can show it."}, {"pixelated", "coloured blocks, softer and quieter."}},
		func(v string) { c.PixelPictures = v == "pixelated"; convo.SetPixelPictures(c.PixelPictures) })
	return []section{
		{title: "Look", rows: []setting{view, theme, colours, logo, spaces, minimap, pictures}},
		{title: "Agents list", rows: []setting{group, split, sortBy, listWidth, dock, stack, subLine, enter, search}},
		{title: "Agents list columns", note: "the figures beside each agent", rows: cols},
		{title: "Message box", rows: []setting{command, rowKey, copying}},
	}
}

func ptrs(ss []setting) []*setting {
	out := make([]*setting, len(ss))
	for i := range ss {
		out[i] = &ss[i]
	}
	return out
}
