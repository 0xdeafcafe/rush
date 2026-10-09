package ui

import (
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// conversationLater is an agent row's conversation, told for a hand-off,
// as a func for the Cmd that hands it off: the open pane's is told now,
// on the UI goroutine that owns it; one read from its transcript or
// through its adapter is read when the func is called.
func (m *Model) conversationLater(a *fleet.Agent) func() agent.Conversation {
	kind, name, cwd := agent.Kind(a.Kind), a.DisplayName, a.Cwd
	tell := func(sess *convo.Session) agent.Conversation {
		c := sess.Conversation(kind)
		if c.Name == "" {
			c.Name = name
		}
		if c.Cwd == "" {
			c.Cwd = cwd
		}
		return c
	}
	switch {
	case m.host != nil && m.host.key == a.Key:
		c := tell(m.host.sess)
		return func() agent.Conversation { return c }
	case a.TranscriptPath != "":
		path := a.TranscriptPath
		return func() agent.Conversation { return tell(convo.History(path, time.Time{})) }
	}
	s := agent.Session{ID: a.SessionID, Name: a.DisplayName, Transcript: a.History,
		Profile: agent.Profile{Kind: kind, Dir: a.Acct.Dir}}
	return func() agent.Conversation { return tell(agentHistory(kind, s, time.Time{})) }
}

// handoffTargets are the agents a conversation on from can be handed to:
// installed, able to run here, and able to start from another's.
func handoffTargets(from agent.Kind) []agent.Adapter {
	var out []agent.Adapter
	for _, a := range host.Installed() {
		if a.Kind() != from && agent.Supports(a.Kind(), agent.FeatureHandoffIn) {
			out = append(out, a)
		}
	}
	return out
}

// handoffTo starts a new session on agent to, opened with this one's
// conversation so far. This one is left as it is.
func (m *Model) handoffTo(c *hostConn, a *fleet.Agent, to string) tea.Cmd {
	from := sessionAgent(c)
	if to == "" {
		var names []string
		for _, t := range handoffTargets(from) {
			names = append(names, string(t.Kind()))
		}
		if len(names) == 0 {
			m.flash("no other installed harness can take this conversation on", true)
			return nil
		}
		m.flash("#fork to which harness? "+strings.Join(names, ", "), false)
		return nil
	}
	k := agent.Kind(strings.ToLower(to))
	if k == from {
		m.flash("this conversation is already on "+agentName(string(k)), true)
		return nil
	}
	if !agent.Supports(k, agent.FeatureHandoffIn) {
		m.flash(agentName(string(k))+" can't take a conversation on from another harness", true)
		return nil
	}
	cfg := host.Config{Cwd: a.Cwd, Name: a.DisplayName + " · on " + agentName(string(k)),
		IdleStop: host.Duration(m.store.Config.Dispatch.Rest())}
	if err := cfg.UseAgent(string(k)); err != nil {
		m.flash(err.Error(), true)
		return nil
	}
	conv := m.conversationLater(a)
	m.flash("handing "+a.DisplayName+" to "+agentName(string(k))+"…", false)
	return func() tea.Msg {
		cfg.Prompt = agent.Handoff(conv()).Text
		hc, err := host.Spawn(cfg)
		if err != nil {
			return doneMsg{err: err}
		}
		return hostStartedMsg{id: hc.ID, name: cfg.Name}
	}
}
