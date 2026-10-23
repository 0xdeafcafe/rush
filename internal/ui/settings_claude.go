package ui

// The signed-in agent's own sections of its provider's page: how rush runs it, and
// the definitions its sessions can start as (--agent). What the agent
// itself reads is its adapter's (settings_files.go).

func init() { agentExtras[loginsKind] = (*Model).claudeSections }

func (m *Model) claudeSections() []section {
	run, tuning := m.claudeRunSections()
	secs := []section{run, tuning}
	if def, ok := m.definitionsSection(loginsKind); ok {
		secs = append(secs, def)
	}
	return secs
}

func (m *Model) claudeRunSections() (run, tuning section) {
	cfg := &m.store.Config
	d := &cfg.Dispatch
	runIn := choiceSetting("Run new sessions in", d.RunIn,
		"Where new Claude Code sessions run. rush mode runs Claude Code headless in rush's own host and draws the conversation here; the daemon is Claude Code's own background service and its terminal screen.",
		[][2]string{
			{"", "rush's conversation view, queue, approvals and overview. /rush moves a daemon session over."},
			{"daemon", "Claude Code's background service, shown through its own terminal screen."},
		}, func(v string) { d.RunIn = v })
	runIn.unset = "rush mode"
	onLimit := choiceSetting("When a usage limit hits", d.OnLimit,
		"What a rush-mode session does when a 5-hour or weekly usage limit stops it.",
		[][2]string{
			{"", "it asks once whether to continue by itself when the limit resets."},
			{"auto", "every session continues by itself at the reset, a few seconds apart."},
			{"off", "sessions wait for you after a limit."},
		}, func(v string) { d.OnLimit = v })
	onLimit.unset = "ask each session"
	quick := choiceSetting("Quick start", onOffWord(d.QuickStart()),
		"Starts rush-mode sessions without Claude Code's non-essential network traffic (CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC), for them alone. Claude Code is ready in about 0.3s instead of 0.65s, which you feel on every new session and every message after one has rested.",
		[][2]string{
			{"on", "quicker starts, but no DesignSync, Projects, plugin downloads or live preview in them. Your telemetry settings are unaffected."},
			{"off", "sessions start with everything Claude Code has."},
		}, func(v string) { on := v == "on"; d.Lean = &on })
	compress := choiceSetting("Compress idle transcripts", onOffWord(!cfg.KeepTranscriptsPlain),
		"Transcripts untouched for two days are stored compressed the way macOS stores its own system files: same names, same contents, and Claude Code, grep, your editor and rush read them exactly as before; the system decompresses as they're read (about 30 ms for a 33 MB one). A transcript written to again is stored plainly again. Each is checked byte for byte before it replaces the original.",
		[][2]string{
			{"on", "about a quarter of the disk they took (33 MB → 8 MB for the biggest)."},
			{"off", "transcripts stay as Claude Code writes them."},
		}, func(v string) { cfg.KeepTranscriptsPlain = v == "off" })
	return section{title: "How rush runs it", rows: []setting{runIn, onLimit}},
		section{title: "Tuning", advanced: true, rows: []setting{quick, compress}}
}
