package ui

import (
	"path/filepath"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

// --- a plugin a project you're working in ships ---

// projectPluginsMsg is what plugin.InProjects found in the repositories
// open sessions work in.
type projectPluginsMsg struct{ offers []plugin.Offer }

// checkProjectPlugins looks, off the UI, in the repositories open sessions
// work in whenever that set changes, and offers what was found once the
// screen is free.
func (m *Model) checkProjectPlugins() tea.Cmd {
	m.openPluginOffer()
	seen := map[string]bool{}
	for _, a := range m.snap.Agents {
		if !a.Past {
			seen[a.Repo], seen[a.Root] = true, true
		}
	}
	delete(seen, "")
	roots := make([]string, 0, len(seen))
	for r := range seen {
		roots = append(roots, r)
	}
	slices.Sort(roots)
	key := strings.Join(roots, "\x00")
	if key == m.offerRoots {
		return nil
	}
	m.offerRoots = key
	return func() tea.Msg { return projectPluginsMsg{offers: plugin.InProjects(roots)} }
}

// openPluginOffer offers the first of m.offers, unless something already
// has the screen, as openPluginApproval does.
func (m *Model) openPluginOffer() {
	if len(m.offers) == 0 || m.sheet != nil || m.dialog != nil || m.confirm != nil || m.mode != modeList {
		return
	}
	// It comes unasked and Install runs the project's code: an enter meant
	// for the list mustn't pick it.
	m.sheet = &pluginOfferSheet{o: m.offers[0], cur: 1}
}

// pluginOfferSheet is Install or Not now for a plugin a project ships.
// Installing runs the project's install.sh, as you; what it builds then
// waits on approval like any plugin.
type pluginOfferSheet struct {
	o    plugin.Offer
	cur  int // 0 Install, 1 Not now
	busy bool
	err  string
}

func (s *pluginOfferSheet) width(*Model) int { return 100 }

func (s *pluginOfferSheet) body(m *Model, w, h int) []string {
	what := "ships a plugin"
	if s.o.Update {
		what = "ships a new version of"
	}
	title := s.o.Name
	if s.o.Version != "" {
		title += " " + s.o.Version
	}
	out := []string{sheetTitle("Plugin", title+" · in "+filepath.Base(s.o.Project), w), ""}
	text := filepath.Base(s.o.Project) + " " + what + " " + s.o.Name + ". " + s.o.Description +
		"\n\nInstalling runs this, as you and outside the sandbox:\n  sh " + tildify(s.o.InstallScript()) +
		"\n\nThen rush shows what the plugin may do, and asks you to approve it."
	for _, l := range wrap(text, w-2) {
		out = append(out, "  "+l)
	}
	out = append(out, "")
	choices := []struct{ what, about string }{
		{"Install", "build it, then review what it may do"},
		{"Not now", "asks again when the project has a new version"},
	}
	for i, c := range choices {
		out = append(out, sheetRow(paint(cText+bold, c.what)+"  "+dim(c.about), i == s.cur, w))
	}
	if s.err != "" {
		out = append(out, "", paint(cRed, s.err))
	}
	k := keysFit(w, "↑↓", "choose", "enter", "pick", "esc", "not now")
	if s.busy {
		k = paint(cYellow, "⋯ installing")
	}
	return append(out, "", k)
}

func (s *pluginOfferSheet) key(m *Model, k tea.KeyPressMsg, str string) tea.Cmd {
	if s.busy {
		return nil
	}
	switch str {
	case "up", "shift+tab", "down", "tab":
		s.cur = 1 - s.cur
	case "y":
		s.cur = 0
		return s.pick(m)
	case "n", "esc", "ctrl+c":
		s.cur = 1
		return s.pick(m)
	case "enter":
		return s.pick(m)
	}
	return nil
}

func (s *pluginOfferSheet) pick(m *Model) tea.Cmd {
	s.busy, s.err = true, ""
	o, install := s.o, s.cur == 0
	return sheetDo(func() (bool, error) {
		if install {
			return true, plugin.Install(o)
		}
		return false, plugin.Skip(o)
	}, func(m *Model, installed bool, err error) tea.Cmd {
		if m.sheet != s {
			return nil
		}
		if err != nil {
			s.busy, s.err = false, err.Error()
			return nil
		}
		m.sheet = nil
		m.offers = slices.DeleteFunc(m.offers, func(x plugin.Offer) bool { return x.Name == o.Name })
		if installed {
			m.flash(o.Name+" installed · approve it to run it", false)
			return m.checkPluginApprovals()
		}
		m.flash("left "+o.Name+" uninstalled", false)
		m.openPluginOffer()
		return nil
	})
}
