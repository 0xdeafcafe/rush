package ui

import (
	"fmt"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/room"
	"github.com/0xdeafcafe/rush/internal/state"
)

// roomSetup is #room new: choosing a room's topic and panel. The room
// itself is a row in the list, shown in the pane as any agent is: see
// roomfeed.go and docs/room.md.
type roomSetup struct {
	topic    []rune
	pos      int
	options  []startOver
	selected []startOver
	cursor   int
	onList   bool
	rounds   int
	dir      string
	// A room on a chat (#discuss): the chat's agent key, its exchanges as
	// the members would read them, oldest first, and how many of the latest
	// go in. fill puts the verdict in the chat's box.
	from  string
	turns []string
	ctx   int
	onCtx bool
	fill  bool
	head  string // where the chat stands, ahead of its turns (#intervene)
}

// openRoom is #room: the newest room's row, else a new room; #room new or
// #room <topic> sets one up.
func (m *Model) openRoom(arg string) tea.Cmd {
	arg = strings.TrimSpace(arg)
	if arg == "" {
		return sheetDo(func() ([]room.Room, error) { return room.List(), nil }, func(m *Model, rs []room.Room, _ error) tea.Cmd {
			if len(rs) == 0 {
				return m.roomSetup("")
			}
			return m.showRoom(rs[0])
		})
	}
	if arg == "new" {
		arg = ""
	}
	return m.roomSetup(arg)
}

func (m *Model) roomSetup(topic string) tea.Cmd {
	s := &roomSetup{topic: []rune(topic), pos: len([]rune(topic)), rounds: room.DefaultRounds, dir: m.startDir(),
		selected: slices.Clone(m.lastPanel)}
	var cmds []tea.Cmd
	for _, route := range m.startRoutes() {
		for _, kind := range route.kinds {
			o := m.startDefaults(kind)
			o.billing = billingOf(route.id, agent.Kind(kind))
			s.options = append(s.options, o)
			cmds = append(cmds, m.loadModels(kind))
		}
	}
	m.sheet = s
	return tea.Batch(cmds...)
}

// showRoom selects room r's row and opens it in the pane.
func (m *Model) showRoom(r room.Room) tea.Cmd {
	m.sel = roomKeyPrefix + r.ID
	m.mode, m.preview, m.paneFocus = modeList, true, true
	return m.loadRooms()
}

func (s *roomSetup) width(m *Model) int { return min(110, m.w-6) }

func (s *roomSetup) body(m *Model, w, h int) []string {
	hint := "tab topic ↔ panel · space choose · ←→ rounds · enter open the room · esc back"
	if s.from != "" {
		hint = "tab topic → context → panel · ←→ turns of context, or rounds · space choose · enter open the room · esc back"
	}
	return append(s.draw(m, w, h-2), "", dim(hint))
}

func (s *roomSetup) paste(text string) {
	if !s.onList && !s.onCtx {
		r := []rune(cleanPaste(text))
		s.topic = insert(s.topic, s.pos, r)
		s.pos += len(r)
	}
}

// choices are each agent here at its default, and at each of its models.
func (s *roomSetup) choices(m *Model) []startOver {
	var out []startOver
	for _, o := range s.options {
		out = append(out, o)
		for _, c := range m.models(o.kind) {
			if c.ID != o.model {
				with := o
				with.model = c.ID
				out = append(out, with)
			}
		}
	}
	return out
}

func roomChoice(o startOver) string {
	s := firstNonEmpty(modelWord(o.kind, o.model), "default model") + " · " + agent.HarnessLabel(agent.Kind(o.kind)) +
		" · " + agent.ProviderLabel(agent.ProviderOf(agent.Kind(o.kind)))
	if o.billing == state.BillingKey {
		s += " · API key"
	}
	return s + "  " + faint(o.model)
}

func (s *roomSetup) key(m *Model, k tea.KeyPressMsg, key string) tea.Cmd {
	switch key {
	case "esc":
		m.sheet = nil
		return nil
	case "tab", "shift+tab":
		switch {
		case s.from != "" && !s.onList && !s.onCtx:
			s.onCtx = true // topic, then context, then the panel
		case s.onCtx:
			s.onCtx, s.onList = false, true
		default:
			s.onList = !s.onList
		}
		return nil
	case "enter":
		return s.create(m)
	}
	if s.onCtx {
		switch key {
		case "left", "-":
			s.ctx = max(0, s.ctx-1)
		case "right", "+", "=":
			s.ctx = min(len(s.turns), s.ctx+1)
		}
		return nil
	}
	if !s.onList {
		s.topic, s.pos, _ = edit(s.topic, s.pos, k, key)
		return nil
	}
	choices := s.choices(m)
	switch key {
	case "up":
		s.cursor = max(0, s.cursor-1)
	case "down":
		s.cursor = min(max(0, len(choices)-1), s.cursor+1)
	case "left", "-":
		s.rounds = max(2, s.rounds-1)
	case "right", "+", "=":
		s.rounds = min(12, s.rounds+1)
	case "space", " ":
		if s.cursor < len(choices) {
			o := choices[s.cursor]
			if i := slices.Index(s.selected, o); i >= 0 {
				s.selected = slices.Delete(s.selected, i, i+1)
			} else if len(s.selected) < 6 {
				s.selected = append(s.selected, o)
			}
		}
	}
	return nil
}

// create opens the room set up: its panel's sessions start in its driver.
func (s *roomSetup) create(m *Model) tea.Cmd {
	topic := strings.TrimSpace(string(s.topic))
	switch {
	case topic == "":
		m.flash("give the room a topic first", false)
		return nil
	case len(s.selected) < 2:
		m.flash("choose at least two agents: tab, then space", false)
		return nil
	}
	r := room.Room{Topic: topic, Cwd: s.dir, Rounds: s.rounds, From: s.from, Brief: s.brief()}
	m.lastPanel = slices.Clone(s.selected)
	var taken []string
	for _, o := range s.selected {
		cfg := m.configAs(o, s.dir)
		if err := cfg.UseAgent(o.kind); err != nil {
			m.flash(err.Error(), true)
			return nil
		}
		name := room.Name(cfg.Kind, cfg.Model, taken)
		taken = append(taken, name)
		mb := room.Member{Name: name, Config: cfg}
		for _, c := range m.models(o.kind) {
			if c.ID == o.model {
				mb.Context = int(c.Context)
			}
		}
		r.Members = append(r.Members, mb)
	}
	return sheetDo(func() (room.Room, error) {
		r, err := room.Create(r)
		if err == nil {
			err = room.Start(r.ID)
		}
		return r, err
	}, func(m *Model, r room.Room, err error) tea.Cmd {
		if err != nil {
			m.flash("couldn't open the room: "+err.Error(), true)
			return nil
		}
		if m.sheet == s {
			m.sheet = nil
		}
		if s.from != "" {
			return m.joinedOpened(r, s.fill)
		}
		return m.showRoom(r)
	})
}

func (s *roomSetup) draw(m *Model, w, h int) []string {
	out := []string{paint(cText+bold, "Topic"), "  " + textField(s.topic, s.pos, !s.onList && !s.onCtx, "What should they argue about?", w-4), "",
		paint(cText+bold, "Folder") + "  " + paint(cSub, tildify(s.dir)) + dim("  where they read and run things: the selected agent's, as for a new session"), ""}
	if s.from != "" {
		n := paint(cBright, fmt.Sprintf("← %d →", s.ctx))
		if s.onCtx {
			n = paint(cOrange+bold, fmt.Sprintf("← %d →", s.ctx))
		}
		out = append(out, paint(cText+bold, "Context")+"  "+n+dim("  "+ctxWords(s.ctx, len(s.turns))), "")
	}
	out = append(out,
		paint(cText+bold, "Panel")+dim(fmt.Sprintf("  %d chosen · two or more, mixed models argue best", len(s.selected)))+
			dim("   rounds at most: ")+paint(cBright, fmt.Sprint(s.rounds)), "")
	choices := s.choices(m)
	if len(choices) == 0 {
		return append(out, paint(cYellow, "No runnable agents. Add one in Settings first."))
	}
	s.cursor = min(s.cursor, len(choices)-1)
	from, to := window(len(choices), s.cursor, max(1, h-len(out)))
	for i := from; i < to; i++ {
		mark := "○ "
		if slices.Contains(s.selected, choices[i]) {
			mark = paint(cGreen, "● ")
		}
		out = append(out, sheetRow(mark+roomChoice(choices[i]), s.onList && i == s.cursor, w))
	}
	return out
}
