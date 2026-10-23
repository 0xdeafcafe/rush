package ui

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A subagent run quiet for fleet.StuckSubAfter has outlived its prompt
// cache, so waiting longer buys nothing. rush never kills it: once per run,
// it moves the call its rush session waits on to the background and tells
// the session to look at what the run has done so far.

// nudgeStuckSubs sends each newly stuck subagent's session that nudge.
// Whether a quiet run is still in a tool is read from its transcript, so
// that's asked off the UI goroutine.
func (m *Model) nudgeStuckSubs() tea.Cmd {
	if m.offline || m.following() || m.snap == nil || m.subChecking {
		return nil
	}
	if m.subNudged == nil {
		m.subNudged = map[string]bool{}
	}
	type quiet struct {
		key, id, name string
		subs          []fleet.SubagentTile
	}
	var ask []quiet
	now := m.snap.At
	for _, a := range m.snap.Agents {
		if !a.Rush || a.Past || a.Done || !a.Live() {
			continue
		}
		q := quiet{key: a.Key, id: a.ID, name: a.DisplayName}
		for _, s := range a.Subagents {
			if s.ToolUseID != "" && !s.Mod.IsZero() && now.Sub(s.Mod) >= fleet.StuckSubAfter && !m.subNudged[a.Key+"\x00"+s.ID] {
				q.subs = append(q.subs, s)
			}
		}
		if q.subs != nil {
			ask = append(ask, q)
		}
	}
	if ask == nil {
		return nil
	}
	m.subChecking = true
	return later(func() []quiet {
		for i := range ask {
			ask[i].subs = (&fleet.Agent{Subagents: ask[i].subs}).StuckSubs(now)
		}
		return ask
	}, func(m *Model, stuck []quiet) tea.Cmd {
		m.subChecking = false
		var cmds []tea.Cmd
		for _, q := range stuck {
			for _, s := range q.subs {
				if k := q.key + "\x00" + s.ID; !m.subNudged[k] {
					m.subNudged[k] = true
					cmds = append(cmds, nudgeStuckSub(q.id, q.name, s))
				}
			}
		}
		return tea.Batch(cmds...)
	})
}

func nudgeStuckSub(id, name string, s fleet.SubagentTile) tea.Cmd {
	what := s.Description
	if what == "" {
		what = s.Type
	}
	text := fmt.Sprintf("Your subagent %q has written nothing for %d minutes. rush moved it to the background so you can go on: "+
		"look at what it has done so far. If it's good, continue; if not, stop it and do the work another way.",
		what, int(fleet.StuckSubAfter.Minutes()))
	return func() tea.Msg {
		f := map[string]any{"subagent": s.ID, "type": s.Type, "toolUseId": s.ToolUseID, "why": fmt.Sprintf("its subagent wrote nothing for %d minutes", int(fleet.StuckSubAfter.Minutes()))}
		c, err := host.Dial(id)
		if err != nil {
			f["err"] = err.Error()
			host.LogEvent(id, "stuck-subagent", f)
			return doneMsg{err: err}
		}
		defer c.Close()
		f["backgroundErr"] = errText(c.Background(s.ToolUseID)) // already in the background: the note still helps
		if err := c.SendGuide(text, nil); err != nil {
			if err := c.Send(text); err != nil {
				f["err"] = err.Error()
				host.LogEvent(id, "stuck-subagent", f)
				return doneMsg{err: err}
			}
		}
		host.LogEvent(id, "stuck-subagent", f)
		return doneMsg{text: name + "'s subagent went quiet · moved to the background, " + name + " checks it"}
	}
}

// askStuck asks a rush session silent for quiet what it is waiting on,
// mid-turn, without stopping it.
func askStuck(a *fleet.Agent, quiet time.Duration) tea.Cmd {
	id, name := a.ID, a.DisplayName
	text := fmt.Sprintf("You have written nothing for %d minutes. What are you waiting on? "+
		"If a command or subagent hung, move it to the background or stop it, and carry on another way.", int(quiet.Minutes()))
	return func() tea.Msg {
		c, err := host.Dial(id)
		if err != nil {
			return doneMsg{err: err}
		}
		defer c.Close()
		if err := c.SendGuide(text, nil); err != nil {
			return doneMsg{err: err}
		}
		return doneMsg{text: "asked " + name + " what it's waiting on"}
	}
}
