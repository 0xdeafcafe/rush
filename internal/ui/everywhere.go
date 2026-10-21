package ui

import (
	"fmt"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/events"
)

// applyEverywhere writes what General's Every agent says into each agent's
// own settings, where they don't say it already: when it's changed (say),
// at start, and as accounts turn up.
func (m *Model) applyEverywhere(say bool) tea.Cmd {
	v := m.store.Config.Everywhere.Attribution
	if v == "" {
		return nil
	}
	return func() tea.Msg {
		changed := agent.ApplyAttribution(v == "on")
		var done, failed []string
		for _, c := range changed {
			f := map[string]any{"agent": string(c.Kind), "profile": c.Profile, "on": v == "on"}
			if c.Err != nil {
				f["err"] = c.Err.Error()
				failed = append(failed, c.String())
			} else {
				done = append(done, agentName(string(c.Kind)))
			}
			events.Log("attribution-set", f)
		}
		switch {
		case len(failed) > 0:
			return doneMsg{err: fmt.Errorf("couldn't set attribution in %s", strings.Join(failed, ", "))}
		case say && len(done) > 0:
			return doneMsg{text: "attribution " + v + " in " + strings.Join(slices.Compact(done), ", ")}
		case say:
			return doneMsg{text: "attribution was already " + v + " in every agent"}
		}
		return nil
	}
}

// attributionAgents names the agents rush can set attribution in.
func attributionAgents() string {
	var names []string
	for _, a := range agent.All() {
		if _, ok := a.(agent.Attributor); ok {
			names = append(names, agentName(string(a.Kind())))
		}
	}
	return strings.Join(slices.Compact(names), ", ")
}
