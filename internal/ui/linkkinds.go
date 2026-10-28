package ui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/ghpr"
)

// A linkHandler is what the UI does with links of a kind convo knows
// (convo.LinkKind): what a click on one opens in rush, and what its menu
// offers before Open in browser and Copy link. A kind with no handler
// still draws its badge; a click on it does what any link's does.
type linkHandler struct {
	// open opens what u is to in rush, with what its kind said of it;
	// nil leaves the click to the terminal.
	open func(m *Model, u string, v any) tea.Cmd
	// acts are what the menu on u offers first.
	acts func(m *Model, u string, v any) []linkAct
}

// linkHandlers are the handlers by kind's name. A kind's badge is in
// convo; what can be done with it is here.
var linkHandlers = map[string]linkHandler{
	convo.LinkGitHubPR: {
		open: func(m *Model, u string, v any) tea.Cmd { pr := v.(ghpr.PR); return m.openPRLink(u, &pr) },
		acts: prLinkActs,
	},
}

// knownLink is the handler for u and what its kind said of it, when a
// kind knows u, has answered, and the UI does something with it.
func knownLink(u string) (linkHandler, any, bool) {
	kind, _, v, ok := convo.LinkInfo(u)
	if !ok {
		return linkHandler{}, nil, false
	}
	h, ok := linkHandlers[kind]
	return h, v, ok
}

// openKnownLink opens a link of a kind rush knows in rush, reporting
// whether it did.
func (m *Model) openKnownLink(u string) (tea.Cmd, bool) {
	h, v, ok := knownLink(u)
	if !ok || h.open == nil {
		return nil, false
	}
	return h.open(m, u, v), true
}

// openPRLink opens a linked pull request in its sheet, with its checks,
// reviews and conversation, and merging, approving and commenting on it.
func (m *Model) openPRLink(u string, pr *ghpr.PR) tea.Cmd {
	s := &prSheet{url: u, num: pr.Number, last: *pr}
	m.sheet = s
	return s.readComments()
}

// prLinkActs are what the menu on a pull request's link offers: opening
// it in rush, and merging it straight from there when it's open.
func prLinkActs(m *Model, u string, v any) []linkAct {
	pr := v.(ghpr.PR)
	acts := []linkAct{{"Open in rush", func(m *Model) tea.Cmd { return m.openPRLink(u, &pr) }}}
	if pr.State == "OPEN" {
		acts = append(acts, linkAct{"Merge…", func(m *Model) tea.Cmd {
			cmd := m.openPRLink(u, &pr)
			if s, ok := m.sheet.(*prSheet); ok {
				s.askMerge(m, &pr)
			}
			return cmd
		}})
	}
	return acts
}
