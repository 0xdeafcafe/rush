package ui

import (
	"slices"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

// pickSheet is a plugin's list to choose from, as the plugin gave it: tabs
// that [ and ] go through, filtered as you type, each action on its key.
// What's chosen goes back to the plugin; the sheet only draws.
type pickSheet struct {
	plugin string
	p      plugin.Pick
	tab    int
	cur    int
	filter []rune
	gone   map[string]bool // taken out by an action that stays
}

// openPick shows a plugin's pick, if nothing else has the screen.
func (m *Model) openPick(name string, p *plugin.Pick) {
	if p == nil || m.sheet != nil || m.dialog != nil || m.confirm != nil {
		return
	}
	m.sheet = &pickSheet{plugin: name, p: *p, tab: p.Tab, gone: map[string]bool{}}
}

func (s *pickSheet) width(*Model) int { return 112 }

// shown are the tab's items that match the filter, as the plugin ordered
// them.
func (s *pickSheet) shown() []plugin.PickItem {
	q := strings.ToLower(strings.TrimSpace(string(s.filter)))
	var out []plugin.PickItem
	for _, it := range s.p.Items {
		if it.Tab != s.tab || s.gone[it.ID] {
			continue
		}
		if q == "" || strings.Contains(strings.ToLower(it.Text), q) || strings.Contains(strings.ToLower(it.Meta), q) {
			out = append(out, it)
		}
	}
	return out
}

func (s *pickSheet) count(tab int) int {
	n := 0
	for _, it := range s.p.Items {
		if it.Tab == tab && !s.gone[it.ID] {
			n++
		}
	}
	return n
}

func (s *pickSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle(s.p.Title, s.p.About, w-len(s.plugin)-3) + faint(" · "+s.plugin), ""}
	if s.plugin == stashPlugin {
		out[0] = sheetTitle(s.p.Title, s.p.About, w) // rush's own
	}
	if len(s.p.Tabs) > 0 {
		tabs := make([]string, len(s.p.Tabs))
		for i, t := range s.p.Tabs {
			tabs[i] = t + " " + strconv.Itoa(s.count(i))
		}
		out = append(out, sheetTabs(tabs, s.tab), "")
	}
	out = append(out, dim("find ")+textField(s.filter, len(s.filter), true, "type to search", w-5), "")
	list := s.shown()
	s.cur = max(0, min(s.cur, len(list)-1))
	if len(list) == 0 {
		msg := "nothing here"
		if s.tab < len(s.p.Empty) && s.p.Empty[s.tab] != "" {
			msg = s.p.Empty[s.tab]
		}
		if len(s.filter) > 0 {
			msg = "none of these has that"
		}
		out = append(out, "  "+faint(msg))
	}
	from, to := window(len(list), s.cur, max(3, h-len(out)-4))
	for i := from; i < to; i++ {
		it := list[i]
		meta := faint(it.Meta)
		first, _, _ := strings.Cut(strings.TrimSpace(it.Text), "\n")
		room := max(8, w-4-ansi.StringWidth(meta)-4)
		line := paint(cText, ansi.Truncate(shortImages(first), room, "…"))
		pad := max(1, w-4-ansi.StringWidth(line)-ansi.StringWidth(meta))
		out = append(out, sheetRow(line+strings.Repeat(" ", pad)+meta, i == s.cur, w))
	}
	if len(list) > 0 {
		if lines := strings.Split(strings.TrimSpace(list[s.cur].Text), "\n"); len(lines) > 1 {
			out = append(out, "", faint(ansi.Truncate(strings.Join(lines[1:min(len(lines), 3)], " ⏎ "), w, "…")))
		}
	}
	pairs := []string{"↑↓", "choose"}
	for _, a := range s.p.Actions {
		pairs = append(pairs, a.Key, a.Name)
	}
	if len(s.p.Tabs) > 1 {
		pairs = append(pairs, "[ ]", strings.ToLower(strings.Join(s.p.Tabs, " · ")))
	}
	return append(out, "", keysFit(w, append(pairs, "esc", "close")...))
}

func (s *pickSheet) key(m *Model, k tea.KeyPressMsg, key string) tea.Cmd {
	list := s.shown()
	switch key {
	case "esc", "ctrl+c":
		m.sheet = nil
		return nil
	case "[", "]":
		if n := len(s.p.Tabs); n > 1 {
			s.tab = (s.tab + map[string]int{"]": 1, "[": n - 1}[key]) % n
			s.cur = 0
		}
		return nil
	case "up", "ctrl+p":
		s.cur = max(0, s.cur-1)
		return nil
	case "down", "ctrl+n":
		s.cur = min(len(list)-1, s.cur+1)
		return nil
	case "pgup":
		s.cur = max(0, s.cur-10)
		return nil
	case "pgdown":
		s.cur = min(len(list)-1, s.cur+10)
		return nil
	}
	if i := slices.IndexFunc(s.p.Actions, func(a plugin.PickAction) bool { return a.Key == key }); i >= 0 {
		if s.cur >= len(list) {
			return nil
		}
		a, it := s.p.Actions[i], list[s.cur]
		if a.Stay {
			s.gone[it.ID] = true
		} else {
			m.sheet = nil
		}
		return m.picked(s.plugin, s.p, it.ID, a.Key)
	}
	buf, pos, _ := edit(s.filter, len(s.filter), k, key)
	if pos == len(buf) {
		s.filter = buf
		s.cur = 0
	}
	return nil
}

// picked tells the plugin what was chosen, with the box it's about when
// that box is the one with the keys.
func (m *Model) picked(name string, p plugin.Pick, item, action string) tea.Cmd {
	if m.hooks == nil {
		return nil
	}
	ch := plugin.Picked{Plugin: name, Pick: p.ID, Item: item, Action: action, Box: p.Session}
	if in, who, ok := m.boxState(); ok && who == p.Session {
		ch.Input = in
	}
	return m.hooks.Picked(ch, func(err error) tea.Msg {
		return sheetMsg{apply: func(m *Model) tea.Cmd {
			if err != nil {
				m.fail(name + ": " + err.Error())
			}
			return nil
		}}
	})
}
