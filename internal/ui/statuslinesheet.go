package ui

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/statusline"
)

// --- /statusline ---

// statusSheet builds status lines: pick what shows and in what order, see
// it drawn with this session's numbers, save. Three of them, one per tab:
//
//   - Claude Code's, under its prompt. Saving sets the account's
//     settings.json statusLine to `rush statusline`, which draws the
//     layout kept in rush's config.
//   - the agent header, at the top of an agent's Session in rush, and
//   - the top bar, at the top right of rush's window: both rush's own,
//     drawn from what rush knows (bars.go), and shown live as you build
//     them.
type statusSheet struct {
	prof agent.Profile
	kind agent.Kind
	// line is the session's agent's own status line; nil hides its tab.
	line agent.StatusLiner
	a    *fleet.Agent
	c    *hostConn
	tab  int
	// agentLay is the agent's own line under its prompt (Claude Code's).
	agentLay statusline.Layout
	bars     statusline.Bars
	was      string // the Claude Code layout as opened, to tell if it changed
	cur      int
	in       statusline.Input
	// current is settings.json's statusLine command now.
	current string
	err     string
	// rnd draws Claude Code's line many times a second without running
	// git, or your own command, each time.
	rnd statusline.Renderer

	// Claude Code's layout and settings.json are read off the UI: its tab
	// says so until they're in. saving is the save being written.
	read   *pending[statusRead]
	loaded bool
	saving *pending[error]
	saved  func(m *Model) // what a save that worked does here

	// For the mouse, as body last drew them: the tabs' row and where each
	// tab ends, the list's first row and the first of its rows showing.
	tabsY, listY, from int
	tabEnds            []int
	// drag is the segment being dragged to a new place.
	drag string
}

const (
	stAgent = iota
	stTop
	stClaude
	statusTabs
)

var statusTabNames = []string{"Agent header", "Top bar", "Claude Code"}

// tabs is how many tabs show: the agent's own only if it has a line.
func (st *statusSheet) tabs() int {
	if st.line == nil {
		return statusTabs - 1
	}
	return statusTabs
}

// tabNames are the tabs showing, the last named for the agent.
func (st *statusSheet) tabNames() []string {
	names := slices.Clone(statusTabNames[:st.tabs()])
	if st.line != nil {
		names[stClaude] = harnessName(string(st.kind))
	}
	return names
}

// statusRead is what the sheet reads when it opens.
type statusRead struct {
	lay     statusline.Layout
	current string
}

func (m *Model) openStatusLine(c *hostConn, a *fleet.Agent) tea.Cmd {
	bars := m.bars
	if bars.Top.Lines == nil {
		bars.Top = statusline.DefaultTop()
	}
	if bars.Agent.Lines == nil {
		bars.Agent = statusline.DefaultAgent()
	}
	k := sessionAgent(c)
	st := &statusSheet{prof: a.Acct, kind: k, a: a, c: c, in: previewInput(c, a),
		bars: statusline.Bars{Top: bars.Top.Clone(), Agent: bars.Agent.Clone()}}
	if agent.Supports(k, agent.FeatureStatusLine) {
		st.line, _ = agent.As[agent.StatusLiner](k)
	}
	line, prof := st.line, a.Acct
	st.read = goPending(func() statusRead {
		r := statusRead{lay: statusline.Load().Clone()}
		if line != nil {
			if cmd, err := line.StatusLine(prof); err == nil {
				r.current = cmd
			}
		}
		return r
	})
	for _, t := range []int{stAgent, stTop} {
		st.tab = t
		st.pad()
	}
	st.tab = stAgent
	st.adopt()
	m.sheet = st
	return st.read.wait()
}

// adopt takes in Claude Code's layout and settings once they're read.
func (st *statusSheet) adopt() {
	r, ok := st.read.take()
	if !ok {
		return
	}
	st.read, st.loaded = nil, true
	st.agentLay, st.current = r.lay, r.current
	// Taken before your own line is folded in, so saving adopts it.
	b, _ := jsonx.Marshal(trimmed(st.agentLay))
	st.was = string(b)
	// A status line of your own isn't lost: it becomes a segment, kept
	// where it was, on the first line.
	if st.current != "" && !statusline.Ours(st.current) && st.agentLay.Custom != st.current {
		st.agentLay.Custom = st.current
		if !st.agentLay.Shown("custom") {
			st.agentLay.Lines = append([][]string{{"custom"}}, st.agentLay.Lines...)
		}
	}
	tab := st.tab
	st.tab = stClaude
	st.pad()
	st.tab = tab
}

// waiting is whether the tab showing is Claude Code's, not read yet.
func (st *statusSheet) waiting() bool { return st.tab == stClaude && !st.loaded }

// openTopBar is #statusline from the list: the same sheet, on the top
// bar, with the selected agent (if any) for the other tabs' previews.
func (m *Model) openTopBar(a *fleet.Agent) tea.Cmd {
	c := m.host
	if a == nil {
		a = &fleet.Agent{Acct: m.store.Config.ActiveAccount().Profile(), Kind: m.store.Config.DefaultAgent(), Cwd: m.launchDir}
	}
	if c == nil || c.key != a.Key {
		c = &hostConn{key: a.Key, kind: agent.Kind(a.Kind), sess: convo.New(), open: map[string]bool{}}
	}
	cmd := m.openStatusLine(c, a)
	if st, ok := m.sheet.(*statusSheet); ok {
		st.tab = stTop
	}
	return cmd
}

// width is the window's, on every tab, so the sheet doesn't change size
// as you go between them and the lines show as wide as they really are.
func (st *statusSheet) width(m *Model) int { return m.w }

// lay is the layout of the tab showing.
func (st *statusSheet) lay() *statusline.Layout {
	switch st.tab {
	case stTop:
		return &st.bars.Top
	case stAgent:
		return &st.bars.Agent
	}
	return &st.agentLay
}

func (st *statusSheet) maxLines() int {
	switch st.tab {
	case stClaude:
		return statusline.MaxLines
	case stTop:
		return statusline.TopLines
	}
	return statusline.BarLines
}

// pad gives the layout every line it can have, empty ones included, so
// there's somewhere to move a segment to.
func (st *statusSheet) pad() {
	l := st.lay()
	for len(l.Lines) < st.maxLines() {
		l.Lines = append(l.Lines, nil)
	}
	l.Lines = l.Lines[:st.maxLines()]
}

// segInfo is a segment as the builder lists it.
type segInfo struct{ id, name, about string }

func (st *statusSheet) segs() []segInfo {
	var out []segInfo
	if st.tab == stClaude {
		for _, s := range statusline.Segments {
			if s.ID == "custom" && st.agentLay.Custom == "" {
				continue
			}
			out = append(out, segInfo{s.ID, s.Name, s.About})
		}
		return out
	}
	for _, s := range barSegs(st.which()) {
		out = append(out, segInfo{s.id, s.name, s.about})
	}
	return out
}

func (st *statusSheet) which() int {
	if st.tab == stTop {
		return barTop
	}
	return barAgent
}

func (st *statusSheet) ctx(m *Model) *barCtx {
	return &barCtx{m: m, t: m.tally(), a: st.a, c: st.c}
}

// previewInput is this session as Claude Code would describe it to the
// status line, with made-up numbers where there aren't any yet.
func previewInput(c *hostConn, a *fleet.Agent) statusline.Input {
	var in statusline.Input
	in.SessionID = firstNonEmpty(c.sess.Info.SessionID, a.SessionID, "3f2a9c1e-0000")
	in.Cwd = firstNonEmpty(c.sess.Info.Cwd, a.Cwd)
	in.Workspace.CurrentDir = in.Cwd
	in.Model.DisplayName = firstNonEmpty(c.sess.Model, c.sess.Info.Model, "Opus")
	in.Effort.Level = firstNonEmpty(c.sess.Effort(), "high")
	in.Version = "2.1"
	in.Cost.USD = c.sess.Info.CostUSD
	if in.Cost.USD == 0 {
		in.Cost.USD = 1.27
	}
	in.Cost.DurationMS = 23 * 60 * 1000
	if len(c.sess.Turns) > 0 && !c.sess.Turns[0].Start.IsZero() {
		in.Cost.DurationMS = float64(time.Since(c.sess.Turns[0].Start).Milliseconds())
	}
	in.Cost.LinesAdded, in.Cost.LinesRemoved = 48, 12
	in.Context.Size = 200_000
	if strings.Contains(strings.ToLower(in.Model.DisplayName+c.sess.Info.Model), "1m") {
		in.Context.Size = 1_000_000
	}
	in.Context.Input = c.sess.Context
	if in.Context.Input == 0 {
		in.Context.Input = in.Context.Size * 34 / 100
	}
	return in
}

// slot is a row of the builder: a segment on a line, or not shown
// (line -1).
type slot struct {
	line int
	id   string
}

// slots are the segments line by line, then the ones not shown.
func (st *statusSheet) slots() []slot {
	var out []slot
	l := st.lay()
	for i, ln := range l.Lines {
		for _, id := range ln {
			out = append(out, slot{i, id})
		}
	}
	for _, s := range st.segs() {
		if !l.Shown(s.id) {
			out = append(out, slot{-1, s.id})
		}
	}
	return out
}

// place puts id at pos on line (removing it from wherever it was) and
// moves the cursor with it.
func (st *statusSheet) place(id string, line, pos int) {
	l := st.lay()
	for i, ln := range l.Lines {
		l.Lines[i] = slices.DeleteFunc(ln, func(s string) bool { return s == id })
	}
	if line >= 0 {
		ln := l.Lines[line]
		l.Lines[line] = slices.Insert(ln, max(0, min(pos, len(ln))), id)
	}
	for i, sl := range st.slots() {
		if sl.id == id {
			st.cur = i
		}
	}
}

// listRow is a row of the builder's list: a line's heading, the note
// on an empty line, or a segment (slot, its index in slots()). Line
// len(Lines) is Not shown.
type listRow struct {
	line  int
	head  bool
	empty bool
	slot  int // -1 when it isn't a segment
}

// rows are the list's rows top to bottom, as body draws them.
func (st *statusSheet) rows() []listRow {
	var out []listRow
	l := st.lay()
	i := 0
	for n, ln := range l.Lines {
		out = append(out, listRow{line: n, head: true, slot: -1})
		if len(ln) == 0 {
			out = append(out, listRow{line: n, empty: true, slot: -1})
		}
		for range ln {
			out = append(out, listRow{line: n, slot: i})
			i++
		}
	}
	out = append(out, listRow{line: len(l.Lines), head: true, slot: -1})
	for ; i < len(st.slots()); i++ {
		out = append(out, listRow{line: -1, slot: i})
	}
	return out
}

// mouse picks a tab, or a segment, and drags a segment to where the
// pointer takes it: onto another it takes that one's place, onto a
// line's heading it goes to the end of the line above or the start of the
// one below, whichever it came from, and onto Not shown it's hidden.
func (st *statusSheet) mouse(m *Model, ev mouseEv, x, y int) tea.Cmd {
	st.adopt()
	if st.saving != nil {
		return nil
	}
	switch ev {
	case mouseWheelUp:
		return st.key(m, tea.KeyPressMsg{}, "up")
	case mouseWheelDown:
		return st.key(m, tea.KeyPressMsg{}, "down")
	case mouseRelease:
		st.drag = ""
		return nil
	}
	if ev == mousePress && y == st.tabsY {
		for t, end := range st.tabEnds {
			if x < end {
				if t != st.tab {
					st.tab, st.cur, st.err = t, 0, ""
				}
				break
			}
		}
		return nil
	}
	if st.waiting() {
		return nil
	}
	rows, slots := st.rows(), st.slots()
	at := st.from + y - st.listY
	if y < st.listY || at < 0 || at >= len(rows) {
		return nil
	}
	r := rows[at]
	if ev == mousePress {
		st.drag = ""
		if r.slot >= 0 {
			st.cur, st.drag = r.slot, slots[r.slot].id
		}
		return nil
	}
	if st.drag == "" {
		return nil
	}
	l := st.lay()
	was := slices.IndexFunc(rows, func(r listRow) bool { return r.slot >= 0 && slots[r.slot].id == st.drag })
	if was < 0 || was == at {
		return nil
	}
	from := slots[rows[was].slot]
	down := at > was
	switch {
	case r.empty:
		st.place(st.drag, r.line, 0)
	case r.head && down && r.line == len(l.Lines):
		st.place(st.drag, -1, 0)
	case r.head && down:
		st.place(st.drag, r.line, 0)
	case r.head && r.line == 0:
		st.place(st.drag, 0, 0)
	case r.head:
		st.place(st.drag, r.line-1, len(l.Lines[r.line-1]))
	case slots[r.slot].line < 0:
		if from.line >= 0 {
			st.place(st.drag, -1, 0)
		}
	default:
		to := slots[r.slot]
		pos := slices.Index(l.Lines[to.line], to.id)
		if down && from.line != to.line {
			pos++ // below it: within a line, taking it out already makes room
		}
		st.place(st.drag, to.line, pos)
	}
	return nil
}

func (st *statusSheet) key(m *Model, k tea.KeyPressMsg, s string) tea.Cmd {
	st.adopt()
	if st.saving != nil {
		return nil // until it's written
	}
	switch s {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
		return nil
	case "]":
		st.tab, st.cur, st.err = (st.tab+1)%st.tabs(), 0, ""
		return nil
	case "[":
		st.tab, st.cur, st.err = (st.tab+st.tabs()-1)%st.tabs(), 0, ""
		return nil
	case "enter", "ctrl+s":
		return st.save(m)
	}
	if st.waiting() {
		return nil
	}
	l := st.lay()
	slots := st.slots()
	if len(slots) == 0 {
		return nil
	}
	st.cur = max(0, min(st.cur, len(slots)-1))
	sl := slots[st.cur]
	at := -1
	if sl.line >= 0 {
		at = slices.Index(l.Lines[sl.line], sl.id)
	}
	switch s {
	case "up", "k":
		st.cur = roundMove(st.cur, -1, len(slots))
	case "down", "j":
		st.cur = roundMove(st.cur, 1, len(slots))
	case "space", "x":
		if sl.line >= 0 {
			st.place(sl.id, -1, 0)
			return nil
		}
		// Onto the last line in use.
		line := 0
		for i, ln := range l.Lines {
			if len(ln) > 0 {
				line = i
			}
		}
		st.place(sl.id, line, len(l.Lines[line]))
	case "shift+up", "alt+up", "K":
		switch {
		case sl.line < 0:
		case at > 0:
			st.place(sl.id, sl.line, at-1)
		case sl.line > 0:
			st.place(sl.id, sl.line-1, len(l.Lines[sl.line-1]))
		}
	case "shift+down", "alt+down", "J":
		switch {
		case sl.line < 0:
		case at < len(l.Lines[sl.line])-1:
			st.place(sl.id, sl.line, at+1)
		case sl.line < st.maxLines()-1:
			st.place(sl.id, sl.line+1, 0)
		}
	case "1", "2", "3":
		if line := int(s[0] - '1'); line < st.maxLines() {
			st.place(sl.id, line, len(l.Lines[line]))
		}
	case "s":
		i := slices.Index(statusline.Seps, l.Sep)
		l.Sep = statusline.Seps[(i+1)%len(statusline.Seps)]
	case "c":
		if st.tab == stClaude {
			l.Plain = !l.Plain
		}
	case "r":
		switch st.tab {
		case stTop:
			*l = statusline.DefaultTop()
		case stAgent:
			*l = statusline.DefaultAgent()
		default:
			custom := l.Custom
			*l = statusline.Default()
			l.Custom = custom
		}
		st.pad()
	case "d":
		if st.tab == stClaude {
			return st.turnOff(m)
		}
	}
	return nil
}

// trimmed is a layout as saved: no empty lines at the end.
func trimmed(l statusline.Layout) statusline.Layout {
	l = l.Clone()
	for len(l.Lines) > 1 && len(l.Lines[len(l.Lines)-1]) == 0 {
		l.Lines = l.Lines[:len(l.Lines)-1]
	}
	return l
}

// save keeps all three: rush's own lines at once, and Claude Code's, in
// settings.json too, only when it was changed. The files are written off
// the UI; the sheet stays, saying so, until they are.
func (st *statusSheet) save(m *Model) tea.Cmd {
	bars := statusline.Bars{Top: trimmed(st.bars.Top), Agent: trimmed(st.bars.Agent)}
	msg := "status lines saved"
	l := trimmed(st.agentLay)
	if !l.Shown("custom") {
		l.Custom = "" // let go of it only when it's taken out
	}
	claude := false
	if b, _ := jsonx.Marshal(trimmed(st.agentLay)); st.loaded && st.line != nil && string(b) != st.was {
		claude = true
		msg += " · " + harnessName(string(st.kind)) + " sessions for " + st.prof.Name + " show theirs from their next redraw"
	}
	line, prof := st.line, st.prof
	return st.write(m, func() error {
		if err := statusline.SaveBars(bars); err != nil {
			return err
		}
		if !claude {
			return nil
		}
		if err := statusline.Save(l); err != nil {
			return err
		}
		if err := line.SetStatusLine(prof, statusline.Command()); err != nil {
			return fmt.Errorf("couldn't turn it on for %s: %w", prof.Name, err)
		}
		return nil
	}, func(m *Model) {
		m.bars = bars
		m.flash(msg, false)
	})
}

// write runs f off the UI, then, if it worked, closes the sheet and does
// done; if not, the sheet stays open and says why.
func (st *statusSheet) write(m *Model, f func() error, done func(m *Model)) tea.Cmd {
	st.err, st.saved = "", done
	st.saving = goPending(f)
	st.landed(m) // at once, when it was written at once
	return st.saving.then(func(m *Model) tea.Cmd {
		st.landed(m)
		return nil
	})
}

// landed takes in a save once it's written.
func (st *statusSheet) landed(m *Model) {
	err, ok := st.saving.take()
	if !ok {
		return
	}
	st.saving = nil
	if err != nil {
		st.err = err.Error()
		return
	}
	if m.sheet == st {
		m.sheet = nil
	}
	st.saved(m)
}

func (st *statusSheet) turnOff(m *Model) tea.Cmd {
	switch {
	case st.current == "":
		m.flash("there's no status line to turn off", false)
		return nil
	case !statusline.Ours(st.current):
		st.err = "that status line is your own command, not rush's: it's left alone"
		return nil
	}
	line, prof := st.line, st.prof
	return st.write(m, func() error { return line.SetStatusLine(prof, "") }, func(m *Model) {
		m.flash("status line turned off for "+prof.Name, false)
	})
}

// sample is one segment on its own, as the line would show it now.
func (st *statusSheet) sample(m *Model, id string) string {
	if st.tab == stClaude {
		l := statusline.Layout{Lines: [][]string{{id}}, Plain: st.agentLay.Plain, Custom: st.agentLay.Custom}
		s, _, _ := strings.Cut(st.rnd.Render(st.in, l, st.prof.Dir, time.Now()), "\n")
		return s
	}
	if s, ok := findBarSeg(st.which(), id); ok {
		return s.draw(st.ctx(m))
	}
	return ""
}

func (st *statusSheet) body(m *Model, w, h int) []string {
	st.adopt()
	about := map[int]string{
		stAgent:  "the top of an agent's Session: right of its name, and under it",
		stTop:    "the top right of rush, about every agent at once",
		stClaude: "what " + harnessName(string(st.kind)) + " shows under its prompt · " + st.prof.Name,
	}[st.tab]
	out := []string{sheetTitle("Status lines", about, w), "", "  " + sheetTabs(st.tabNames(), st.tab), ""}
	st.tabsY, st.tabEnds = 2, nil
	end := 2
	for _, n := range st.tabNames() {
		end += ansi.StringWidth(n) + 5 // and the "  ·  " after it
		st.tabEnds = append(st.tabEnds, end-2)
	}
	if st.waiting() {
		return append(out, dim("  reading "+harnessName(string(st.kind))+"'s settings…"), "", keysFit(w, "[ ]", "tab", "esc", "cancel"))
	}
	l := st.lay()

	// What the real header, drawn just before this, had no room for.
	var dropped map[string]bool
	if st.tab != stClaude {
		dropped = m.barDropped(st.which())
	}
	// The preview is the real thing, set into the screen's own ground as
	// it will sit there, and live behind the sheet too.
	cw := w - 4
	if st.tab == stClaude {
		line := st.rnd.Render(st.in, *l, st.prof.Dir, time.Now())
		if line == "" {
			line = faint("(nothing to show: space adds a segment)")
		}
		name := harnessName(string(st.kind))
		out = append(out, dim("  preview, under "+name+"'s prompt, with this session's numbers"))
		well := []string{faint(strings.Repeat("─", cw-2)), paint(cText, "❯ ")}
		for _, ln := range strings.Split(line, "\n") {
			well = append(well, ansi.Truncate(ln, cw-2, "…"))
		}
		out = append(out, cutout(well, cw)...)
		out = append(out, "")
		switch {
		case st.current == "":
			out = append(out, dim("  "+name+" has no status line for this account yet."))
		case statusline.Ours(st.current):
			out = append(out, dim("  "+name+" draws this now: saving updates it."))
		default:
			out = append(out, dim("  Your own status line is kept, as the segment Your own line: move it, or take it out."))
		}
	} else {
		where := "  preview · the agent header, live on the real one too"
		var well []string
		if st.tab == stTop {
			where = "  preview · rush's top, live on the real one too"
			// The header as it's drawn at this width.
			wasW := m.w
			m.w = cw - 2
			well = m.header()
			m.w = wasW
		} else {
			pw := cw - 2
			_, paneW, _ := m.layout()
			if paneW > 0 {
				pw = min(pw, paneW) // as wide as it really is, when that fits
			}
			well = m.paneHeader(st.a, st.c, pw)
			if paneW == 0 {
				dropped = m.barDropped(barAgent) // no pane behind: the preview's
			}
			well = append(well, faint("  ⏺ the conversation goes on here…"))
		}
		out = append(out, dim(where))
		out = append(out, cutout(well, cw)...)
		out = append(out, "")
		note := "  When there isn't room, the segments last on a line go first, and it ends ⋯. What's wrong always shows."
		if len(dropped) > 0 {
			note = paint(cYellow, "  ■") + dim(" hasn't room at this width: it's left out, and the line ends ⋯. What's wrong always shows.")
		}
		out = append(out, dim(note))
	}
	out = append(out, "")

	slots := st.slots()
	st.cur = max(0, min(st.cur, len(slots)-1))
	listH := max(4, h-len(out)-5)
	var list []string
	selAt := 0
	infos := st.segs()
	row := func(i int) {
		sl := slots[i]
		var seg segInfo
		for _, s := range infos {
			if s.id == sl.id {
				seg = s
			}
		}
		mark := paint(cGreen, "■")
		if sl.line < 0 {
			mark = dim("□")
		}
		noRoom := sl.line >= 0 && dropped[sl.id]
		if noRoom {
			mark = paint(cYellow, "■")
		}
		if sl.id == st.drag {
			mark = paint(cOrange, "↕")
		}
		if i == st.cur {
			selAt = len(list)
		}
		sample := st.sample(m, sl.id)
		if sample == "" {
			sample = faint("(nothing right now)")
		}
		about := seg.about
		if sl.id == "custom" && st.tab == stClaude {
			about = st.agentLay.Custom
		}
		if noRoom {
			about = paint(cYellow, "no room ⋯ move it earlier, or widen the window")
		}
		text := "  " + mark + " " + paint(cText, fit(seg.name, 16)) + fit(sample, 24) + "  " + faint(about)
		list = append(list, sheetRow(ansi.Truncate(text, w-3, "…"), i == st.cur, w))
	}
	lineName := func(n int) string {
		switch {
		case n == len(l.Lines):
			return "Not shown"
		case st.tab == stAgent && n == 0:
			return "Line 1 · right of the name"
		case st.tab == stAgent:
			return "Line 2 · under the name"
		}
		return "Line " + strconv.Itoa(n+1)
	}
	for _, r := range st.rows() {
		switch {
		case r.head:
			list = append(list, paint(cSub+bold, "  "+lineName(r.line)))
		case r.empty:
			list = append(list, faint("      empty · drag a segment here, or shift+↓ or "+strconv.Itoa(r.line+1)))
		default:
			row(r.slot)
		}
	}
	from, to := window(len(list), selAt, listH)
	// While dragging the list stays put unless the segment would leave it,
	// so what's under the pointer is what was.
	if st.drag != "" && st.from+listH <= len(list) && selAt >= st.from && selAt < st.from+listH {
		from, to = st.from, st.from+listH
	}
	st.listY, st.from = len(out), from
	out = append(out, list[from:to]...)
	out = append(out, "")
	switch {
	case st.saving != nil:
		out = append(out, paint(cOrange, "  "+spinner[m.tick%len(spinner)])+dim(" saving…"))
	case st.err != "":
		out = append(out, paint(cRed, "  "+st.err))
	}
	sepName := strings.TrimSpace(l.Sep)
	if sepName == "" {
		sepName = "space"
	}
	lines := "1 2"
	if st.maxLines() == 3 {
		lines = "1 2 3"
	}
	// Most needed first: what doesn't fit is left off the end.
	pairs := []string{"space", "show/hide", "shift+↑↓ or drag", "move", "[ ]", "tab", "enter", "save", "esc", "cancel", lines, "to line", "s", "separator " + sepName}
	if st.tab == stClaude {
		colour := "on"
		if l.Plain {
			colour = "off"
		}
		pairs = append(pairs, "c", "colour "+colour, "d", "off")
	}
	return append(out, keysFit(w, append(pairs, "r", "reset")...))
}

// cutout sets lines into a well of the terminal's own background, cw wide
// and indented to line up with the sheet's text, so what's in it looks as
// it will on screen rather than on the sheet's panel.
func cutout(lines []string, cw int) []string {
	const ground = "\x1b[49m"
	row := func(l string) string { return "  " + onBg(ground, " "+l, cw) }
	out := []string{row("")}
	for _, l := range lines {
		out = append(out, row(l))
	}
	return append(out, row(""))
}
