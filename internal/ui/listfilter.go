package ui

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/fuzzy"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// listFilterState is the Agents view's own filter, opened with ctrl+] f: it
// narrows the list to agents whose name or transcript matches what's typed,
// highlighting the match. Names match at once; what was said is searched in
// the background the same way the command bar's own search does, so a
// large fleet doesn't stall on a keystroke.
type listFilterState struct {
	query  []rune
	gen    int
	search *barSearch
	found  map[string]barFound // agent key -> its hits, as the search reports them
	done   bool
}

// startListFilter opens the filter, empty: nothing narrows until something's
// typed.
func (m *Model) startListFilter() {
	m.listFilter = &listFilterState{}
}

// listFilterKey handles a keypress while the filter is open: typing narrows
// it, esc and backspace step back out. Anything else (the arrows, enter,
// the agent shortcuts) is left for listKey's own switch, which by then only
// sees the filtered rows. ok is false for a key this doesn't use.
func (m *Model) listFilterKey(k tea.KeyPressMsg, s string) (tea.Cmd, bool) {
	f := m.listFilter
	switch s {
	case "esc":
		if f.search != nil {
			f.search.cancel()
			f.search = nil
		}
		if len(f.query) > 0 {
			f.gen++
			f.query, f.found, f.done = nil, nil, false
		} else {
			m.listFilter = nil
		}
		m.rebuild()
		return nil, true
	case "backspace", "ctrl+h":
		if len(f.query) == 0 {
			if f.search != nil {
				f.search.cancel()
			}
			m.listFilter = nil
			m.rebuild()
			return nil, true
		}
		f.query = f.query[:len(f.query)-1]
		return m.applyListFilter(), true
	case "ctrl+w", "alt+backspace", "ctrl+backspace", "ctrl+u", "super+backspace":
		f.query, _, _ = edit(f.query, len(f.query), k, s)
		return m.applyListFilter(), true
	}
	if k.Text == "" || k.Mod&^tea.ModShift != 0 {
		return nil, false
	}
	f.query = append(f.query, []rune(k.Text)...)
	return m.applyListFilter(), true
}

// applyListFilter re-filters the list for what's typed so far, then, once
// there's enough of a word to be worth it, starts searching every agent's
// transcript for it in the background.
func (m *Model) applyListFilter() tea.Cmd {
	f := m.listFilter
	if f.search != nil {
		f.search.cancel()
		f.search = nil
	}
	f.gen++
	f.found, f.done = nil, false
	m.rebuild()
	if !searchable(string(f.query)) {
		return nil
	}
	gen := f.gen
	return tea.Tick(200*time.Millisecond, func(time.Time) tea.Msg { return listFilterPauseMsg(gen) })
}

// searchListFilter starts reading every listed agent's transcript for the
// filter's query in the background; results are merged into
// m.listFilter.found as they arrive.
func (m *Model) searchListFilter() tea.Cmd {
	f := m.listFilter
	var agents []*fleet.Agent
	for _, a := range m.snap.Agents {
		if a.TranscriptPath != "" {
			agents = append(agents, a)
		}
	}
	if len(agents) == 0 {
		f.done = true
		return nil
	}
	f.search = searchAgentTranscripts(f.gen, agents, string(f.query))
	return f.search.waitAs(func(found []barFound, done bool) tea.Msg {
		return listFilterFoundMsg{gen: f.gen, found: found, done: done}
	})
}

type listFilterPauseMsg int

type listFilterFoundMsg struct {
	gen   int
	found []barFound
	done  bool
}

// listFilterMsg handles the filter's own messages; ok is false for
// anything else, so the caller falls through to its usual handling.
func (m *Model) listFilterMsg(msg tea.Msg) (tea.Cmd, bool) {
	switch msg := msg.(type) {
	case listFilterPauseMsg:
		if f := m.listFilter; f != nil && int(msg) == f.gen && f.search == nil {
			return m.searchListFilter(), true
		}
		return nil, true
	case listFilterFoundMsg:
		f := m.listFilter
		if f == nil || msg.gen != f.gen || f.search == nil {
			return nil, true
		}
		if f.found == nil {
			f.found = map[string]barFound{}
		}
		for _, found := range msg.found {
			if found.total > 0 {
				f.found[found.key] = found
			}
		}
		f.done = msg.done
		m.rebuild()
		if !msg.done {
			return f.search.waitAs(func(found []barFound, done bool) tea.Msg {
				return listFilterFoundMsg{gen: f.gen, found: found, done: done}
			}), true
		}
		return nil, true
	}
	return nil, false
}

// listFilterMatch is whether a matches the filter: its name, or (once the
// background search reaches it) something said in it.
func (m *Model) listFilterMatch(a *fleet.Agent) bool {
	f := m.listFilter
	if _, _, ok := fuzzy.Match(string(f.query), a.DisplayName); ok {
		return true
	}
	found, ok := f.found[a.Key]
	return ok && len(found.hits) > 0
}

// hasFilterMatch is whether the filter found a's match in its transcript
// (not just its name), regardless of how much room there is to show it.
func (m *Model) hasFilterMatch(a *fleet.Agent) bool {
	f := m.listFilter
	if f == nil || len(f.query) == 0 {
		return false
	}
	found, ok := f.found[a.Key]
	return ok && len(found.hits) > 0
}

// filterSnippet is the highlighted match the filter found in a's transcript,
// fit to w cells; ok is false when there isn't one (no filter, or a matched
// only by name).
func (m *Model) filterSnippet(a *fleet.Agent, w int) (text string, ok bool) {
	f := m.listFilter
	if f == nil || len(f.query) == 0 || w < 8 {
		return "", false
	}
	found, has := f.found[a.Key]
	if !has || len(found.hits) == 0 {
		return "", false
	}
	h := found.hits[0]
	words := convo.Words(string(f.query))
	return whoGlyph(h.Who) + " " + litTitle(h.Snippet, litWords(h.Snippet, words), w-2), true
}
