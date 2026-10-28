package ui

import (
	"fmt"
	"slices"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// --- the plugins that are off until you want them ---

// Optional bundled plugins are useful but not for everyone all the time,
// so they start off. rush asks which you want once, on the first run, and
// again for any that's new since (an update brought it, or what it needs,
// haven say, got installed). Each is asked about once: Settings, Plugins
// turns them on or off after.

// optPick is one optional plugin to ask about.
type optPick struct {
	name, about, category string
	on                    bool // picked in the sheet
}

// optionalMsg is what checkOptional found: the plugins to ask about, and
// the ones already on, which count as asked.
type optionalMsg struct {
	picks     []optPick
	alreadyOn []string
}

// checkOptional looks, off the UI, for optional plugins not asked about
// yet, once a few seconds after start in the view that leads; until the
// sheet can open, each tick tries again.
func (m *Model) checkOptional() tea.Cmd {
	if m.optPicks != nil {
		m.openOptional()
		return nil
	}
	if m.optChecked || m.tick < 5 {
		return nil
	}
	m.optChecked = true
	asked := slices.Clone(m.store.Config.Onboarding.Plugins)
	return func() tea.Msg { return optionalFound(asked, plugin.Bundles(), plugin.BundlesOn()) }
}

// optionalFound is the optional plugins not in asked that run here, by
// category, and the ones you have on already.
func optionalFound(asked []string, bundles []plugin.Bundle, on map[string]bool) optionalMsg {
	var out optionalMsg
	for _, b := range bundles {
		n := b.Manifest.Name
		if !b.Optional || slices.Contains(asked, n) || b.Manifest.Unmet() != "" {
			continue
		}
		if on[n] {
			out.alreadyOn = append(out.alreadyOn, n)
			continue
		}
		out.picks = append(out.picks, optPick{name: n, about: b.Manifest.Description, category: b.Manifest.CategoryOf().Title})
	}
	order := func(c string) int {
		return slices.IndexFunc(plugin.Categories, func(x plugin.Category) bool { return x.Title == c })
	}
	slices.SortStableFunc(out.picks, func(a, b optPick) int { return order(a.category) - order(b.category) })
	return out
}

// gotOptional keeps what checkOptional found and offers it.
func (m *Model) gotOptional(msg optionalMsg) {
	if len(msg.alreadyOn) > 0 {
		m.markAsked(msg.alreadyOn)
	}
	if len(msg.picks) > 0 {
		m.optPicks = msg.picks
		m.openOptional()
	}
}

// markAsked records plugins as asked about.
func (m *Model) markAsked(names []string) {
	o := &m.store.Config.Onboarding
	for _, n := range names {
		if !slices.Contains(o.Plugins, n) {
			o.Plugins = append(o.Plugins, n)
		}
	}
	_ = m.store.SaveConfig()
}

// openOptional opens the sheet once the screen is free.
func (m *Model) openOptional() {
	if len(m.optPicks) == 0 || m.sheet != nil || m.dialog != nil || m.confirm != nil || m.mode != modeList {
		return
	}
	o := m.store.Config.Onboarding
	m.sheet = &optionalSheet{picks: m.optPicks, never: len(o.Plugins) == 0, first: len(o.Plugins) == 0 && len(o.Steps) == 0}
	m.optPicks = nil
}

// optionalSheet ticks which optional plugins to turn on.
type optionalSheet struct {
	picks []optPick
	first bool // the first run
	never bool // never asked before, on the first run or not
	cur   int
	busy  bool
}

func (s *optionalSheet) width(*Model) int { return 100 }

func (s *optionalSheet) body(m *Model, w, h int) []string {
	title, text := "Plugins", "New plugins came with rush, or can run here now. "
	switch {
	case s.first:
		text = "Welcome to rush. These plugins come with it. "
	case s.never:
		text = "These plugins come with rush. "
	}
	text += "They're off until you want them: useful, but not for everyone all the time. " +
		"Tick the ones you want; Settings › Plugins turns any of them on or off later."
	out := []string{sheetTitle(title, "off until you want them", w), ""}
	for _, l := range wrap(text, w-2) {
		out = append(out, "  "+l)
	}
	out = append(out, "")
	// Each row: its tick, its name and its description cut to the line;
	// the one under the cursor is described in full below the list. The
	// list and the description keep the height of the longest, so the box
	// doesn't grow and shrink as the cursor moves.
	var about []string
	if s.cur < len(s.picks) {
		about = wrap(s.picks[s.cur].about, w-4)
	}
	aboutH, cats := 0, 0
	for i, p := range s.picks {
		aboutH = max(aboutH, len(wrap(p.about, w-4)))
		if i == 0 || p.category != s.picks[i-1].category {
			cats++
		}
	}
	room := max(3, h-len(out)-aboutH-5-cats)
	from, to := window(len(s.picks), s.cur, room)
	listEnd := len(out) + min(len(s.picks), room) + cats
	cat := ""
	for i := from; i < to; i++ {
		p := s.picks[i]
		if p.category != cat {
			cat = p.category
			out = append(out, "  "+faint(cat))
		}
		box := dim("○ ")
		if p.on {
			box = paint(cGreen, "● ")
		}
		name := fmt.Sprintf("%-19s", p.name)
		line := box + paint(cText+bold, name) + dim(ansi.Truncate(p.about, max(0, w-26), "…"))
		out = append(out, sheetRow(line, i == s.cur, w))
	}
	for len(out) < listEnd {
		out = append(out, "")
	}
	out = append(out, "")
	for _, l := range about {
		out = append(out, "    "+dim(l))
	}
	for range aboutH - len(about) {
		out = append(out, "")
	}
	k := keysFit(w, "↑↓", "move", "space", "tick", "enter", "turn on ticked", "esc", "not now")
	if s.busy {
		k = paint(cYellow, "⋯ turning them on")
	}
	return append(out, "", k)
}

func (s *optionalSheet) key(m *Model, k tea.KeyPressMsg, str string) tea.Cmd {
	if s.busy {
		return nil
	}
	switch str {
	case "up", "shift+tab", "k":
		s.cur = max(0, s.cur-1)
	case "down", "tab", "j":
		s.cur = min(len(s.picks)-1, s.cur+1)
	case "space", " ", "x":
		if s.cur < len(s.picks) {
			s.picks[s.cur].on = !s.picks[s.cur].on
		}
	case "enter":
		return s.apply(m)
	case "esc", "ctrl+c", "n":
		for i := range s.picks {
			s.picks[i].on = false
		}
		return s.apply(m)
	}
	return nil
}

// apply turns on what's ticked, and records every plugin shown as asked
// about, so none is asked about again.
func (s *optionalSheet) apply(m *Model) tea.Cmd {
	var shown, on []string
	for _, p := range s.picks {
		shown = append(shown, p.name)
		if p.on {
			on = append(on, p.name)
		}
	}
	m.markAsked(shown)
	if len(on) == 0 {
		m.sheet = nil
		m.flash("no plugins turned on · Settings › Plugins has them", false)
		return nil
	}
	s.busy = true
	return sheetDo(func() ([]string, error) {
		var failed []string
		for _, n := range on {
			if err := plugin.SetBundled(n, true); err != nil {
				failed = append(failed, n+": "+err.Error())
			}
		}
		_ = hooks.Reload()
		return failed, nil
	}, func(m *Model, failed []string, _ error) tea.Cmd {
		if m.sheet == s {
			m.sheet = nil
		}
		if len(failed) > 0 {
			m.flash("couldn't turn on "+failed[0], true)
			return nil
		}
		m.flash(fmt.Sprintf("%d plugins on · Settings › Plugins changes them", len(on)), false)
		return nil
	})
}
