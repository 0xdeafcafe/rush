package ui

import (
	"cmp"
	"fmt"
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/room"
)

// discuss is #discuss and ctrl+] t: a room's set-up, on chat c. The topic
// is what's given, else what's typed in its box, else its latest message;
// its latest exchanges go in as context, as many as you choose. With fill
// the verdict goes into the box when it comes, to send on.
func (m *Model) discuss(c *hostConn, topic string, fill bool) tea.Cmd {
	topic = strings.TrimSpace(topic)
	if topic == "" {
		topic = strings.TrimSpace(string(c.input))
	}
	if n := len(c.sess.Turns); topic == "" && n > 0 {
		topic = oneLine(c.sess.Turns[n-1].Prompt)
	}
	return m.chatRoom(c, topic, chatTurns(c.sess, false), 3, fill)
}

// interveneBrief is what #intervene's panel is asked about the chat.
const interveneBrief = "Go through this chat: its plan, its status and its latest turns. Is the agent making progress, and if not, why not (looping, the wrong approach, fixing a symptom, waiting on something)? Cite what you saw: tool calls, errors, times. End with exactly what the agent must do next, written to the agent."

// intervene is #intervene: a room that goes through chat c (its plan,
// status, latest exchanges with their tool calls and times), says why it's
// stuck or going wrong, and puts its instructions to the agent in the box.
func (m *Model) intervene(c *hostConn, focus string) tea.Cmd {
	topic := interveneBrief
	if focus = strings.TrimSpace(focus); focus != "" {
		topic += " The user's concern: " + focus
	}
	cmd := m.chatRoom(c, topic, chatTurns(c.sess, true), 10, true)
	if s, ok := m.sheet.(*roomSetup); ok {
		s.head = chatStatus(c.sess, time.Now())
	}
	return cmd
}

// interveneOn is #intervene @agent: the tagged agent's chat opens, and the
// room's set-up comes once its conversation is in (interveneReady).
func (m *Model) interveneOn(a *fleet.Agent, focus string) tea.Cmd {
	if c := m.host; c != nil && c.key == a.Key && c.ready {
		return m.intervene(c, focus)
	}
	m.intervening = &pendingIntervene{key: a.Key, focus: focus, at: time.Now()}
	return m.goAgent(a)
}

type pendingIntervene struct {
	key, focus string
	at         time.Time
}

// interveneReady sets up the waiting #intervene once its chat is open
// with the whole conversation read, or near enough after a few seconds.
func (m *Model) interveneReady() tea.Cmd {
	p, c := m.intervening, m.host
	if p == nil || c == nil || c.key != p.key || !c.ready || c.sess.Partial && time.Since(p.at) < 5*time.Second {
		if p != nil && time.Since(p.at) > 30*time.Second {
			m.intervening = nil
			m.flash("couldn't open that chat to intervene", true)
		}
		return nil
	}
	m.intervening = nil
	return m.intervene(c, p.focus)
}

// chatStatus is where a chat stands, for a panel going through it: live or
// idle and since when, and its task list.
func chatStatus(s *convo.Session, now time.Time) string {
	var b strings.Builder
	if n := len(s.Turns); n > 0 {
		t := s.Turns[n-1]
		switch {
		case t.Live:
			fmt.Fprintf(&b, "Status: working, turn %d running since %s (%s ago)\n", t.N, t.Start.Format("15:04"), now.Sub(t.Start).Round(time.Minute))
		case !t.End.IsZero():
			fmt.Fprintf(&b, "Status: idle since %s (%s ago)\n", t.End.Format("15:04"), now.Sub(t.End).Round(time.Minute))
		}
	}
	if len(s.Tasks) > 0 {
		b.WriteString("Plan:\n")
		for _, t := range s.Tasks {
			fmt.Fprintf(&b, "- [%s] %s\n", t.Status, t.Subject)
		}
	}
	return strings.TrimSpace(b.String())
}

// chatRoom is a room's set-up on chat c, with its latest ctx turns as
// context to start.
func (m *Model) chatRoom(c *hostConn, topic string, turns []string, ctx int, fill bool) tea.Cmd {
	cmd := m.roomSetup(topic)
	if s, ok := m.sheet.(*roomSetup); ok {
		s.from, s.turns, s.ctx, s.fill = c.key, turns, min(ctx, len(turns)), fill
		if cwd := c.sess.Info.Cwd; cwd != "" {
			s.dir = cwd
		}
	}
	return cmd
}

// chatTurns are a chat's exchanges as a member reads them, oldest first:
// what was asked and what the agent said, each cut to a few KB. With
// steps, a reviewer's: when each began, every tool call, what the failed
// ones said and how the turn ended.
func chatTurns(s *convo.Session, steps bool) []string {
	var out []string
	for _, t := range s.Turns {
		var b strings.Builder
		if steps && !t.Start.IsZero() {
			fmt.Fprintf(&b, "[turn %d, %s]\n", t.N, t.Start.Format("2006-01-02 15:04"))
		}
		if p := strings.TrimSpace(t.Prompt); p != "" {
			b.WriteString("User: " + p + "\n")
		}
		for _, it := range t.Items {
			if it.Kind == convo.KText && strings.TrimSpace(it.Text) != "" {
				b.WriteString("Agent: " + strings.TrimSpace(it.Text) + "\n")
			}
			if st := it.Step; steps && it.Kind == convo.KStep && st != nil {
				fmt.Fprintf(&b, "Tool: %s %s\n", st.Tool, cutRunes(oneLine(string(st.Input)), 200))
				if st.Status == convo.Failed {
					b.WriteString("  failed: " + cutRunes(oneLine(st.Output), 300) + "\n")
				}
			}
		}
		if steps && t.Err != "" {
			b.WriteString("Turn ended badly: " + t.Err + "\n")
		}
		if x := b.String(); x != "" {
			out = append(out, cutRunes(x, 4<<10))
		}
	}
	return out
}

func cutRunes(s string, n int) string {
	if r := []rune(s); len(r) > n {
		return string(r[:n]) + "…"
	}
	return s
}

// brief is the context chosen: the chat's latest ctx exchanges.
func (s *roomSetup) brief() string {
	parts := s.turns[max(0, len(s.turns)-s.ctx):]
	if s.head != "" {
		parts = append([]string{s.head}, parts...)
	}
	return strings.Join(parts, "\n---\n")
}

// ctxWords says what the context count means.
func ctxWords(n, total int) string {
	switch {
	case total == 0:
		return "the chat has nothing to give yet"
	case n == 0:
		return "the topic alone"
	case n == 1:
		return "the latest exchange of the chat"
	}
	return fmt.Sprintf("the latest %d of the chat's %d exchanges", n, total)
}

// joinedOpened is a room on a chat started: you stay in the chat, the
// room's row joins the chat's in the list, and its verdict comes back.
func (m *Model) joinedOpened(r room.Room, fill bool) tea.Cmd {
	m.flash("room opened on this chat · it shows above the box · the verdict comes back here", false)
	return m.loadRooms()
}

// discussTyped is enter in the discuss send mode: a room's set-up on what's
// typed, the box cleared.
func (m *Model) discussTyped(c *hostConn) tea.Cmd {
	topic := c.pastes.expand(string(c.input), true)
	c.input, c.back, c.pastes = c.input[:0], 0, pastes{}
	return m.discuss(c, topic, true)
}

// dockRoomPrefix marks the dock rows of a room running on the chat.
const dockRoomPrefix = "dockroom:"

// openDockRoom opens the room a dock row ref names, as showRoom does
// (the caller reloads the rooms). It reports whether ref was one.
func (m *Model) openDockRoom(ref string) bool {
	id, ok := strings.CutPrefix(ref, dockRoomPrefix)
	if !ok {
		return false
	}
	m.sel = roomKeyPrefix + id
	m.mode, m.preview, m.paneFocus = modeList, true, true
	return true
}

// chatRoomOn is the newest room opened from chat c still running, if any.
func (m *Model) chatRoomOn(c *hostConn) *room.Summary {
	for i := range m.rooms.list {
		if s := &m.rooms.list[i]; s.From == c.key && s.Over == "" && !s.Verdict {
			return s
		}
	}
	return nil
}

// chatRoomRows are the dock's rows for the room running on chat c: its
// topic and round, then each member and the first line of what it said
// last, the one speaking spun.
func (m *Model) chatRoomRows(c *hostConn, w int) []string {
	r := m.chatRoomOn(c)
	if r == nil {
		return nil
	}
	rounds := cmp.Or(r.Rounds, room.DefaultRounds)
	right := paint(cSub, fmt.Sprintf("round %d/%d · %d agents", r.Round, rounds, len(r.Members))) + "  "
	if r.Paused {
		right = paint(cYellow, "paused") + dim(" · ") + right
	}
	title := paint(cBlue, "⇶ room") + dim(" · ") + paint(cText+bold, cellw.Truncate(oneLine(r.Topic), max(10, w-cellw.String(ansi.Strip(right))-14), "…"))
	out := []string{spread("  "+title, right, w)}
	for i, mb := range r.Members {
		mark, name := " ", paint(cText, mb.Name)
		if slices.Contains(r.Speaking, mb.Name) {
			mark, name = spinOf(agent.Kind(mb.Config.Kind), m.tick+i), paint(cOrange+bold, mb.Name)
		}
		out = append(out, cellw.Truncate("    "+mark+" "+name+"  "+dim(oneLine(r.Last[mb.Name])), w-2, "…"))
	}
	return out
}

// verdictRows are the dock's card for the verdict waiting on chat c.
func (m *Model) verdictRows(c *hostConn, w int) []string {
	v, ok := m.verdicts[c.key]
	if !ok {
		return nil
	}
	// What it is and what it's waiting for, then the topic it was asked,
	// then each member's last word whole.
	out := []string{" " + paint(cBlue+bold, "⇶ the room on this chat has a verdict") + dim(" · send it to this agent?")}
	out = append(out, "   "+dim(cellw.Truncate("asked: "+oneLine(v.Topic), max(10, w-6), "…")), "")
	for _, l := range strings.Split(strings.TrimSpace(v.Final), "\n") {
		for _, r := range wrap(l, max(20, w-6)) {
			out = append(out, "   "+paint(cText, r))
		}
	}
	return append(out, "", "   "+keys("enter", "send it to this agent", "e", "edit it first", "esc", "drop it"))
}

// verdictText is a verdict as it goes to the agent.
func verdictText(s room.Summary) string {
	return "The room's verdict on " + oneLine(s.Topic) + ":\n" + strings.TrimSpace(s.Final)
}

// verdictKey answers the verdict card, while the box is empty and no row
// is picked: enter sends it on, e puts it in the box to edit, esc drops it.
func (m *Model) verdictKey(c *hostConn, s string, empty bool) (tea.Cmd, bool) {
	v, ok := m.verdicts[c.key]
	// Behind any other card; on it, or with nothing else picked.
	if !ok || !empty || cardKind(c) != "" || c.sel != "" && !c.cardFocus || s != "enter" && s != "e" && s != "esc" {
		return nil, false
	}
	delete(m.verdicts, c.key)
	c.cardFocus = false
	if s == "esc" {
		m.flash("verdict dropped", false)
		return nil, true
	}
	c.input, c.back = []rune(verdictText(v)), 0
	if s == "e" {
		return nil, true
	}
	return m.sendPane(c, false), true
}

// verdictsBack hands each finished room on a chat its verdict back: into
// the chat's box when it's open and the box is free (or the discuss send
// mode asked), else a line in the feed.
func (m *Model) verdictsBack(list []room.Summary) {
	if m.verdictsPicked == nil {
		m.verdictsPicked = map[string]bool{}
	}
	for _, s := range list {
		if s.From == "" || !s.Verdict || m.verdictsPicked[s.ID] {
			continue
		}
		m.verdictsPicked[s.ID] = true
		if m.verdicts == nil {
			m.verdicts = map[string]room.Summary{}
		}
		m.verdicts[s.From] = s
		if c := m.host; c != nil && c.key == s.From {
			m.flash("the room's verdict is in · enter sends it on", false)
			continue
		}
		if a := m.agentByKey(s.From); a != nil {
			m.noteEvent(a, cBlue, "⇶", "the room's verdict is in")
		}
	}
}
