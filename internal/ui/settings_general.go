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
	var secs []section

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

	onInstall := choiceSetting("When rush is installed again", map[bool]string{true: "restart", false: "ask"}[c.RestartOnInstall],
		"What happens once a newer rush is installed (go install, or an update): sessions keep running on the one they started with until they're restarted.",
		[][2]string{
			{"ask", "nothing restarts by itself: #reload runs it here, Settings › Updates restarts the idle sessions."},
			{"restart", "each session restarts on it once it's between turns, with nothing queued, asked or running in the background, and the view reloads."},
		}, func(v string) { c.RestartOnInstall = v == "restart" })

	secs = append(secs, section{title: "Idle and finished", rows: []setting{hibernate, active, justLeft, justLeftN, cleanup, orphans, onInstall}})

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

	lowDisk := choiceSetting("Offer a cleanup when disk is under", offPick(c.CleanupDiskGB, "%dG"),
		"rush watches your free disk itself, without AI. When it's this low it asks whether to start a cleanup agent (with the cleanup plugin on; off, it recommends turning it on): one that reads your worktrees, what's left in /tmp and what plugins like Haven and go-clean report, removes what can go without losing work, says what it did and goes away. Nothing starts until you say yes.",
		[][2]string{
			{"", "it asks once there's less than 20G free."},
			{"10G", "it asks once there's less than 10G free."},
			{"50G", "it asks once there's less than 50G free."},
			{"off", "it never asks about disk."},
		}, func(v string) { c.CleanupDiskGB = offValue(v, "%dG") })
	lowDisk.unset = "20G"
	highRAM := choiceSetting("Offer a cleanup when memory is full for", offPick(c.CleanupRAMMinutes, "%dm"),
		"rush watches memory pressure itself, without AI. When the memory has been under pressure this long it asks whether to start a cleanup agent, which can stop what's holding memory (Haven's stacks, dev servers left running; with the cleanup plugin on) as well as free disk. Nothing starts until you say yes.",
		[][2]string{
			{"", "it asks after 15 minutes under memory pressure."},
			{"5m", "it asks after 5 minutes under memory pressure."},
			{"30m", "it asks after 30 minutes under memory pressure."},
			{"off", "it never asks about memory."},
		}, func(v string) { c.CleanupRAMMinutes = offValue(v, "%dm") })
	highRAM.unset = "15m"
	secs = append(secs, section{title: "Disk and memory", rows: []setting{lowDisk, highRAM}})

	// What keeps a turn from hanging on a stream that went silent: shown,
	// as it's always on.
	stalls := setting{
		label: "Stalled turns",
		what:  "A model's stream can go silent without ending, leaving a turn waiting for good. Claude Code sessions rush runs have Claude Code's own stream watchdog on, which cuts such a stream and tries again, and every turn that has heard nothing from its agent for 2 minutes, with no step running, is marked stalled on its working line. After 5 minutes it moves to Stuck? and rush tells you once; rush never stops it for you.",
		line: func(int) string {
			return fit(paint(cText, "Stalled turns"), 32) + paint(cGreen, "✓ watched") +
				faint(" · Claude Code's stream watchdog on · quiet 2m is marked stalled · 5m tells you")
		},
	}
	secs = append(secs, section{title: "Stalls", rows: []setting{stalls}})
	return secs
}

// offPick is a setting's number as its choice: "" for the default (0),
// "off" for -1, else n in format.
func offPick(n int, format string) string {
	switch {
	case n < 0:
		return "off"
	case n == 0:
		return ""
	}
	return fmt.Sprintf(format, n)
}

// offValue is offPick's choice back as the number.
func offValue(v, format string) int {
	n := 0
	switch v {
	case "off":
		n = -1
	case "":
	default:
		fmt.Sscanf(v, format, &n)
	}
	return n
}

// onOffWord is on or off.
func onOffWord(b bool) string {
	if b {
		return "on"
	}
	return "off"
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

// attributionSetting is whether agents credit themselves, said once for
// every harness: Settings › Harnesses › Every harness.
func (m *Model) attributionSetting() setting {
	c := &m.store.Config
	attribution := choiceSetting("Credit in commits and PRs", c.Everywhere.Attribution,
		"Whether agents add themselves to the commits and pull requests they make (a Co-Authored-By line, a \"Generated with\" note). Set here, rush writes it into every agent's own settings, on every account, and again for any account or agent that turns up later: "+attributionAgents()+". Codex credits itself as your OpenAI account decides, which no setting of its own changes; Gemini, Antigravity, Pi and Kimi never credit themselves.",
		[][2]string{
			{"", "each agent does as its own settings say."},
			{"off", "no agent credits itself."},
			{"on", "every agent credits itself, as it does by default."},
		}, nil)
	attribution.unset = "as each agent is set"
	attribution.run = func(v string) tea.Cmd {
		c.Everywhere.Attribution = v
		return m.applyEverywhere(true)
	}
	return attribution
}
