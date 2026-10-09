package ui

import (
	"strings"
	"unicode"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// --- a command's argument from a list: /model, /effort ---

// argOptions are the values /name takes for session c's agent, as its
// adapter lists them; nil when it has no list for name.
func argOptions(c *hostConn, name string) []agent.Choice {
	ch, ok := agent.ChoicesOf(sessionAgent(c))
	if !ok {
		return nil
	}
	switch name {
	case "model":
		return ch.Models
	case "effort":
		return ch.Efforts
	}
	return nil
}

// argRunning is what the session runs with for /name, as its host says,
// or for a model, as its answers do.
func argRunning(c *hostConn, name string) string {
	if name == "effort" {
		return c.sess.Effort()
	}
	return firstNonEmpty(c.sess.Info.Model, c.sess.Model)
}

// argNow is which of opts the session runs with, "" for the agent's own
// default: the one last picked here, else the one the running value is,
// else the one whose words it has the most of ("opus[1m]" in a full model
// id with opus and 1m in it). known is false when it's none of them.
func argNow(c *hostConn, name string, opts []agent.Choice) (now string, known bool) {
	if p, ok := c.picked[name]; ok {
		return p, true
	}
	run := argRunning(c, name)
	if run == "" {
		return "", false
	}
	words := func(s string) []string {
		return strings.FieldsFunc(strings.ToLower(s), func(r rune) bool { return !unicode.IsLetter(r) && !unicode.IsDigit(r) })
	}
	has := map[string]bool{}
	for _, w := range words(run) {
		has[w] = true
	}
	best := 0
	for _, o := range opts {
		if o.ID == run {
			return o.ID, true
		}
		ws := words(o.ID)
		all := len(ws) > 0
		for _, w := range ws {
			all = all && has[w]
		}
		if all && len(ws) > best {
			now, best = o.ID, len(ws)
		}
	}
	return now, best > 0
}

// setArg switches the session's /name to id, "" (or "default") for the
// agent's own default.
func (m *Model) setArg(c *hostConn, name, id string) tea.Cmd {
	if c.client == nil && c.sleeping {
		return m.wakeHostThen(c, func(m *Model, next *hostConn) tea.Cmd { return m.setArg(next, name, id) })
	}
	if c.client == nil {
		m.flash("/"+name+" works in rush-mode sessions · /rush moves this one over", true)
		return nil
	}
	if id == "default" {
		id = ""
	}
	if name == "model" && agent.ProviderOf(sessionAgent(c)) == "ollama" {
		o := m.sessionStart(c)
		if o.model != id {
			o.model = id
			return m.switchSession(c, o)
		}
	}
	if c.picked == nil {
		c.picked = map[string]string{}
	}
	was, had := c.picked[name]
	c.picked[name] = id
	set, when := c.client.SetModel, "the next turn"
	if name == "effort" {
		set = c.client.SetEffort
	}
	m.flash(name+": "+firstNonEmpty(id, "default")+" from "+when, false)
	return func() tea.Msg {
		err := set(id)
		if err == nil {
			return nil
		}
		// Not switched: the sheet marks what it runs with again.
		return applyMsg(func(m *Model) tea.Cmd {
			if c.picked[name] == id {
				if had {
					c.picked[name] = was
				} else {
					delete(c.picked, name)
				}
			}
			m.flash(err.Error(), true)
			return nil
		})
	}
}

// choiceSheet is /model or /effort with nothing named: the agent's list,
// its own default first, the one running marked.
type choiceSheet struct {
	conn, name string
	opts       []agent.Choice
	now        int // the running one's row, -1 when not known
	cur        int
}

// openChoices opens the list for /name, reporting whether the agent has one.
func (m *Model) openChoices(c *hostConn, name string) bool {
	opts := argOptions(c, name)
	if name == "model" {
		opts = m.models(string(sessionAgent(c)))
	}
	if name != "model" && name != "effort" {
		return false
	}
	s := &choiceSheet{conn: c.key, name: name, now: -1,
		opts: append([]agent.Choice{{Note: "the agent's own"}}, opts...)}
	now, known := argNow(c, name, opts)
	if run := argRunning(c, name); !known && run != "" {
		// One the list doesn't have still shows, to stay on.
		s.opts = append(s.opts, agent.Choice{ID: run, Note: "running now"})
		now, known = run, true
	}
	if known && now != "" {
		found := false
		for _, o := range s.opts {
			found = found || o.ID == now
		}
		if !found {
			s.opts = append(s.opts, agent.Choice{ID: now, Note: "selected for this session"})
		}
	}
	for i, o := range s.opts {
		if known && o.ID == now {
			s.now, s.cur = i, i
		}
	}
	m.sheet = s
	return true
}

func (s *choiceSheet) width(*Model) int { return 96 }

func (s *choiceSheet) body(m *Model, w, h int) []string {
	about := "for this session, from the next turn"
	if s.name == "effort" {
		about = "for this session, from the next turn"
	}
	if c := m.sheetConn(s.conn); c != nil {
		if s.name == "model" {
			for _, o := range m.models(string(sessionAgent(c))) {
				found := false
				for _, old := range s.opts {
					found = found || old.ID == o.ID
				}
				if !found {
					s.opts = append(s.opts, o)
				}
			}
		}
		if c.client == nil && !c.sleeping {
			about = "native session · r brings its conversation into rush to change settings"
		}
	}
	out := []string{sheetTitle(strings.ToUpper(s.name[:1])+s.name[1:], about, w), ""}
	label := func(o agent.Choice) string { return firstNonEmpty(o.ID, "default") }
	labelW := 0
	for _, o := range s.opts {
		labelW = max(labelW, ansi.StringWidth(label(o)))
	}
	from, to := window(len(s.opts), s.cur, max(1, h-6))
	for i := from; i < to; i++ {
		o := s.opts[i]
		mark := "  "
		if i == s.now {
			mark = paint(cGreen, "✓ ")
		}
		l := label(o)
		line := mark + paint(cText+bold, l+strings.Repeat(" ", labelW-ansi.StringWidth(l))) + "  " +
			dim(ansi.Truncate(o.Note, max(0, w-labelW-8), "…"))
		out = append(out, sheetRow(line, i == s.cur, w))
	}
	return append(out, "", dim("/"+s.name+" <name> takes one not listed"), "",
		keysFit(w, "↑↓", "choose", "enter", "switch", "esc", "cancel"))
}

func (s *choiceSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	switch k {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "up", "shift+tab":
		s.cur = pickerMove(s.cur, len(s.opts), "up")
	case "down", "tab":
		s.cur = pickerMove(s.cur, len(s.opts), "down")
	case "r":
		if c := m.sheetConn(s.conn); c != nil && c.client == nil && !c.sleeping {
			if a := m.agentByKey(c.key); a != nil {
				m.sheet = nil
				return m.moveToRush(a)
			}
		}
	case "enter":
		if c := m.sheetConn(s.conn); c != nil && c.client == nil && !c.sleeping {
			m.flash("Native session · press r to bring its conversation into rush first", true)
			return nil
		}
		m.sheet = nil
		if c := m.sheetConn(s.conn); c != nil {
			return m.setArg(c, s.name, s.opts[s.cur].ID)
		}
	}
	return nil
}
