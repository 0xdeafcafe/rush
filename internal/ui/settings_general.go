package ui

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/menubar"
)

// General controls session lifecycle, notifications and background work.
func (m *Model) generalSections() []section {
	c := &m.store.Config
	d := &c.Dispatch
	var secs []section

	rest := choiceSetting("Rest idle sessions after", restValue(d.RestMinutes),
		"How long an idle rush-mode session keeps its harness running. An idle harness holds 150-580 MB; after this it stops, and your next message starts it again in about a second. The host, the conversation and its queue stay, and so does the prompt cache.",
		[][2]string{
			{"", "the agent stops a moment after its turn, with nothing left running in the background."},
			{"2 min", "the agent stays up 2 minutes after its turn."},
			{"5 min", "the agent stays up 5 minutes after its turn."},
			{"10 min", "the agent stays up 10 minutes after its turn."},
			{"30 min", "the agent stays up half an hour after its turn."},
		}, func(v string) {
			d.RestMinutes = 0
			fmt.Sscanf(v, "%d", &d.RestMinutes)
		})
	rest.unset = "at once"

	hib := "off"
	if c.Hibernate.AfterMinutes > 0 {
		hib = fmt.Sprintf("%dm", c.Hibernate.AfterMinutes)
	}
	hibernate := choiceSetting("Hibernate finished agents", hib,
		"A finished agent's process stays in memory (often 300–800 MB) until it is stopped. Hibernating stops it; the conversation is kept and enter resumes it.",
		[][2]string{
			{"off", "finished agents stay in memory until you stop them (ctrl+x)."},
			{"15m", "one that finished and sat idle 15 minutes is stopped."},
			{"30m", "one that finished and sat idle 30 minutes is stopped."},
			{"60m", "one that finished and sat idle an hour is stopped."},
		}, func(v string) {
			c.Hibernate.AfterMinutes = 0
			fmt.Sscanf(v, "%dm", &c.Hibernate.AfterMinutes)
		})

	active := choiceSetting("Keep stopped agents in Active for", activeValue(c.ActiveMinutes),
		"An agent whose process has stopped (rested, hibernated or closed) stays in Active with the rest of what's in play for this long, then moves to Today.",
		[][2]string{
			{"", "it moves to Today 5 minutes after it was last active."},
			{"10m", "it moves to Today 10 minutes after it was last active."},
			{"30m", "it moves to Today half an hour after it was last active."},
			{"60m", "it moves to Today an hour after it was last active."},
			{"120m", "it moves to Today two hours after it was last active."},
			{"off", "it moves to Today as soon as its process stops."},
		}, func(v string) {
			switch v {
			case "off":
				c.ActiveMinutes = -1
			case "":
				c.ActiveMinutes = 0
			default:
				fmt.Sscanf(v, "%dm", &c.ActiveMinutes)
			}
		})
	active.unset = "5m"

	justLeft := choiceSetting("Keep agents in Just left for", activeValue(c.JustLeftMinutes),
		"An agent that leaves Idle goes to Just left, most recent first, so one you missed is still easy to find; after this long it moves to Today.",
		[][2]string{
			{"", "it stays two hours after it was last active."},
			{"30m", "it stays half an hour after it was last active."},
			{"60m", "it stays an hour after it was last active."},
			{"480m", "it stays eight hours after it was last active."},
			{"off", "nothing goes to Just left; it moves straight to Today."},
		}, func(v string) {
			switch v {
			case "off":
				c.JustLeftMinutes = -1
			case "":
				c.JustLeftMinutes = 0
			default:
				fmt.Sscanf(v, "%dm", &c.JustLeftMinutes)
			}
		})
	justLeft.unset = "120m"
	justLeftN := choiceSetting("Just left holds at most", countValue(c.JustLeftCount),
		"The most recent stay in Just left; older ones move on to Today.",
		[][2]string{
			{"", "the 10 most recent."},
			{"5", "the 5 most recent."},
			{"20", "the 20 most recent."},
			{"50", "the 50 most recent."},
		}, func(v string) {
			c.JustLeftCount = 0
			fmt.Sscanf(v, "%d", &c.JustLeftCount)
		})
	justLeftN.unset = "10"

	cleanup := choiceSetting("Clean up done work after", cleanupValue(c.CleanupHours),
		convo.KeyWord("What happens to an agent's worktree and temp work once you've marked it done (alt+d) and left it alone. Stopping an agent never removes anything. A worktree goes only if git says every change in it is committed and pushed; its branch stays. One that isn't is kept, and Projects says why on the worktree."),
		[][2]string{
			{"", "done work that's committed and pushed goes 3 hours after it was last touched."},
			{"1h", "done work that's committed and pushed goes an hour after it was last touched."},
			{"12h", "done work that's committed and pushed goes 12 hours after it was last touched."},
			{"24h", "done work that's committed and pushed goes a day after it was last touched."},
			{"off", "nothing goes by itself; x on a worktree in Projects removes it."},
		}, func(v string) {
			switch v {
			case "off":
				c.CleanupHours = -1
			case "":
				c.CleanupHours = 0
			default:
				fmt.Sscanf(v, "%dh", &c.CleanupHours)
			}
		})
	cleanup.unset = "3h"

	orphans := choiceSetting("Orphaned processes", map[bool]string{true: "keep", false: "end"}[c.KeepOrphans],
		"What happens to what a session was running once the session itself has ended: a dev server, a watcher, a test run left going. Projects › System lists them either way.",
		[][2]string{
			{"end", "they're ended two minutes after their session ended, gently and then firmly."},
			{"keep", "they run until you end them, with x or X in Projects › System."},
		}, func(v string) { c.KeepOrphans = v == "keep" })

	secs = append(secs, section{title: "Idle and finished", rows: []setting{rest, hibernate, active, justLeft, justLeftN, cleanup, orphans}})

	notify := choiceSetting("Notify when an agent needs you", onOffWord(!c.Quiet),
		"A macOS notification when an agent starts waiting on you (a question or a permission), not when you have already seen it.",
		[][2]string{{"on", "you are notified once per new question."}, {"off", "the Needs you section is the only signal."}},
		func(v string) { c.Quiet = v == "off" })
	menu := choiceSetting("Menu bar icon", onOffWord(c.MenuBar),
		"rush in the menu bar: every account's usage, what's working, and who needs you, with a badge. A question with a few answers can be answered from its notification's buttons, a permission allowed or denied. The first time, it's built with Xcode's Swift compiler (a few seconds).",
		[][2]string{{"on", "it opens with rush, and its menu can open it at login; rush's own notifications give way to its."}, {"off", "no menu bar icon."}}, nil)
	menu.run = func(v string) tea.Cmd {
		c.MenuBar, c.MenuBarAsked = v == "on", true
		if !c.MenuBar {
			// It reads its lock file and runs pkill: off the UI.
			return func() tea.Msg { menubar.Stop(); return nil }
		}
		return m.startMenuBar()
	}
	secs = append(secs, section{title: "When an agent needs you", rows: []setting{notify, menu}})

	feed := choiceSetting("The feed", onOffWord(c.Feed),
		"The feed your agents share: blockers, fixes and heads-ups, docked at the foot of the list. Off, agents are not told about it and rush feed refuses. #feed on|off says the same.",
		[][2]string{{"off", "no feed: agents are not told and rush feed refuses."}, {"on", "agents may chirp and reply; the latest chirps dock at the foot of the list."}},
		func(v string) { c.Feed = v == "on" })
	secs = append(secs, section{title: "The feed", rows: []setting{feed}})

	// What keeps a turn from hanging on a stream that went silent: shown,
	// as it's always on.
	stalls := setting{
		label: "Stalled turns",
		what:  "A model's stream can go silent without ending, leaving a turn waiting for good. Claude Code sessions rush runs have Claude Code's own stream watchdog on, which cuts such a stream and tries again, and every turn that has heard nothing from its agent for 2 minutes, with no step running, is marked stalled on its working line.",
		line: func(int) string {
			return fit(paint(cText, "Stalled turns"), 32) + paint(cGreen, "✓ watched") +
				faint(" · Claude Code's stream watchdog on · a turn quiet 2m is marked stalled")
		},
	}
	secs = append(secs, section{title: "Stalls", rows: []setting{stalls}})
	return secs
}

// onOffWord is on or off.
func onOffWord(b bool) string {
	if b {
		return "on"
	}
	return "off"
}

func restValue(min int) string {
	if min <= 0 {
		return ""
	}
	return fmt.Sprintf("%d min", min)
}

func cleanupValue(h int) string {
	switch {
	case h < 0:
		return "off"
	case h > 0:
		return fmt.Sprintf("%dh", h)
	}
	return ""
}

// countValue is a count setting as its choice says it: "" for the default.
func countValue(n int) string {
	if n <= 0 {
		return ""
	}
	return fmt.Sprint(n)
}

// activeValue is ActiveMinutes as its choice says it.
func activeValue(n int) string {
	switch {
	case n < 0:
		return "off"
	case n == 0:
		return ""
	}
	return fmt.Sprintf("%dm", n)
}
