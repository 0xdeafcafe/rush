package ui

import (
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// The Wall place: every agent at once, a tile each, what it is saying and
// running streaming up its tile as it happens. A tile's edge is the colour
// of its state, and lights up while its transcript is being written; the
// older lines fade. Only what's open is on it, unless a shows the day's.

type wallState struct {
	sel string // the agent picked, by key
	top int    // the first row of tiles shown, when they don't all fit
	all bool   // everything from the last day, not only what's open
	// reading are the transcripts being read for tiles, by agent key.
	reading map[string]bool
	// tiles are where each agent's tile was last drawn, for clicks;
	// above and below are how many weren't, for want of room.
	tiles        []wallTile
	above, below int
	// streams are the tiles' streams as last drawn, by tile: wrapping them
	// is most of a frame, and they change only when a tile's read again.
	streams map[string]wallStreamed
}

// wallStreamed is a tile's stream as drawn, and what it was drawn from.
type wallStreamed struct {
	from  wallStreamFrom
	lines []string
}

// wallStreamFrom is what a tile's stream is drawn from.
type wallStreamFrom struct {
	at          time.Time // the preview's
	n, last     int       // its messages, and the newest one's length
	live        bool
	intent, ink string
	w, room     int
}

type wallTile struct {
	key        string
	x, y, w, h int
}

// wallReadMsg is the tiles' transcript tails, read together off the UI's
// goroutine, so a round of reads is one frame and not one a tile.
type wallReadMsg []wallRead

// wallRead is a tile's transcript tail; ok is false when it hadn't
// changed.
type wallRead struct {
	key string
	e   previewEntry
	ok  bool
}

// Tiles are at least this big; below it they page.
const (
	wallMinW = 40
	wallMinH = 9
)

// wallAgents are the tiles, in an order that doesn't shuffle as they
// work: what needs you first, then what's at work, then the rest, each
// by when it started.
func (m *Model) wallAgents() []*fleet.Agent {
	now := m.snap.At
	var out []*fleet.Agent
	for _, a := range m.snap.Agents {
		if a.Done || a.Past {
			continue
		}
		open := a.Live() || a.Busy() || a.PID != 0 || a.Halted() || a.YourTurn(now)
		if open || m.wall.all && now.Sub(a.UpdatedAt) < workSince {
			out = append(out, a)
		}
	}
	rank := func(a *fleet.Agent) int {
		switch {
		case a.NeedsYou() || a.Halted():
			return 0
		case a.Live():
			return 1
		case a.Busy():
			return 2
		case a.Waiting() || a.YourTurn(now):
			return 3
		case a.PID != 0:
			return 4
		}
		return 5
	}
	sort.SliceStable(out, func(i, j int) bool {
		if ri, rj := rank(out[i]), rank(out[j]); ri != rj {
			return ri < rj
		}
		return out[i].CreatedAt.Before(out[j].CreatedAt)
	})
	return out
}

// wallItem is one tile on the Wall: an open agent, or one of its subagents
// still working. a is always the agent the tile belongs to; sub is set only
// for a subagent's own tile.
type wallItem struct {
	key string
	a   *fleet.Agent
	sub *fleet.SubagentTile
}

// wallItems are the Wall's tiles: wallAgents' agents, each followed at once
// by its own subagents still running, so a fleet busy underneath reads top
// to bottom as it happens rather than behind a count. wallSlots keeps each
// agent's together on a row.
func (m *Model) wallItems() []wallItem {
	agents := m.wallAgents()
	out := make([]wallItem, 0, len(agents))
	for _, a := range agents {
		out = append(out, wallItem{key: a.Key, a: a})
		for i := range a.Subagents {
			s := &a.Subagents[i]
			out = append(out, wallItem{key: a.Key + "\x00" + s.ID, a: a, sub: s})
		}
	}
	return out
}

// wallGroups are the sizes of items' groups: an agent and its subagents.
func wallGroups(items []wallItem) []int {
	var gs []int
	for i, it := range items {
		if i == 0 || it.a != items[i-1].a {
			gs = append(gs, 0)
		}
		gs[len(gs)-1]++
	}
	return gs
}

// wallCell is a tile's place in a row of the Wall: its column, and which
// of the n tiles stacked in that column it is.
type wallCell struct {
	item, col, k, n int
}

// wallRow is a row of the Wall's tiles; joined[c] is set where the tiles
// either side of the gap after column c are one agent's, so their top
// edges join up.
type wallRow struct {
	cells  []wallCell
	joined []bool
}

// wallSubMinH is the least a subagent's tile is, stacked with others.
const wallSubMinH = 5

// wallLayout sets groups of tiles in rows cols wide. A group that fits a
// row sits in it side by side, starting a row of its own when what's left
// of this one is too little. One with more subagents than that has a row
// to itself: the agent down its first column, its subagents stacked up to
// stack high in the others, going on to more rows only when they must.
func wallLayout(groups []int, cols, stack int) []wallRow {
	var rows []wallRow
	cur, col := wallRow{joined: make([]bool, cols)}, 0
	flush := func() {
		if len(cur.cells) > 0 {
			rows = append(rows, cur)
		}
		cur, col = wallRow{joined: make([]bool, cols)}, 0
	}
	i := 0
	for _, g := range groups {
		if g <= cols {
			if col+g > cols {
				flush()
			}
			for j := range g {
				cur.cells = append(cur.cells, wallCell{item: i + j, col: col, n: 1})
				if j > 0 {
					cur.joined[col-1] = true
				}
				col++
			}
			i += g
			continue
		}
		flush()
		rows = append(rows, wallStacked(i, g, cols, stack)...)
		i += g
	}
	flush()
	return rows
}

// wallStacked are the rows of a group too big for one side by side: the
// agent (tile i) down the first column, its g-1 subagents stacked up to
// stack high in the others; with one column, the agent on a row of its own.
func wallStacked(i, g, cols, stack int) []wallRow {
	var rows []wallRow
	cur := wallRow{cells: []wallCell{{item: i, n: 1}}, joined: make([]bool, cols)}
	start := 1
	if cols == 1 {
		rows = append(rows, cur)
		cur, start = wallRow{joined: make([]bool, cols)}, 0
	}
	sc := cols - start
	next, left := i+1, g-1
	for left > 0 {
		take := min(left, sc*stack)
		used := min(take, sc)
		for c := range used {
			n := take/used + boolInt(c < take%used)
			for k := range n {
				cur.cells = append(cur.cells, wallCell{item: next, col: start + c, k: k, n: n})
				next++
			}
		}
		from := start
		if cur.cells[0].item == i {
			from = 0 // the agent's in this row: its edge joins its subagents'
		}
		for c := from; c < start+used-1; c++ {
			cur.joined[c] = true
		}
		left -= take
		rows = append(rows, cur)
		cur = wallRow{joined: make([]bool, cols)}
	}
	return rows
}

// wallGeo is where a tile is on the Wall for moving between them: its
// column, and its top and bottom in rows of wallGeoH.
type wallGeo struct{ col, top, bot int }

const wallGeoH = 60

func wallGeos(layout []wallRow, n int) []wallGeo {
	g := make([]wallGeo, n)
	for r, row := range layout {
		for _, c := range row.cells {
			if c.item < n {
				g[c.item] = wallGeo{c.col, r*wallGeoH + c.k*wallGeoH/c.n, r*wallGeoH + (c.k+1)*wallGeoH/c.n}
			}
		}
	}
	return g
}

// wallStep is the tile next to tile i the way dx or dy goes: the nearest
// above or below, in the nearest column; beside, overlapping it. -1 for
// none.
func wallStep(geo []wallGeo, i, dx, dy int) int {
	cur := geo[i]
	best, bestD := -1, 0
	for j, g := range geo {
		var d int
		switch {
		case j == i:
			continue
		case dy < 0 && g.bot <= cur.top:
			d = (cur.top-g.bot)*100 + abs(g.col-cur.col)
		case dy > 0 && g.top >= cur.bot:
			d = (g.top-cur.bot)*100 + abs(g.col-cur.col)
		case dx != 0 && sign(g.col-cur.col) == dx && g.top < cur.bot && g.bot > cur.top:
			d = abs(g.col-cur.col)*1000 + abs(g.top-cur.top)
		default:
			continue
		}
		if best < 0 || d < bestD {
			best, bestD = j, d
		}
	}
	return best
}

// wallRows is how many rows groups take cols wide in h: fewer subagents
// stack in a column when the rows are shorter.
func wallRows(groups []int, cols, h int) []wallRow {
	layout := wallLayout(groups, cols, max(1, h/wallSubMinH))
	return wallLayout(groups, cols, max(1, h/len(layout)/wallSubMinH))
}

// wallGrid lays groups of tiles out in w×h: the columns, the rows that fit
// on screen, and each row's height. It picks tiles about three and a half
// times as wide as tall (a cell is twice as tall as wide), wasting few.
func wallGrid(groups []int, w, h int) (cols, rows, tileH int) {
	n := 0
	for _, g := range groups {
		n += g
	}
	if n == 0 || w <= 0 || h <= 0 {
		return 1, 1, max(h, 0)
	}
	maxCols := max(1, (w+1)/(wallMinW+1))
	best, bestRows, bestScore := 0, 0, math.Inf(1)
	for c := 1; c <= min(maxCols, n); c++ {
		layout := wallRows(groups, c, h)
		r := len(layout)
		th := h / r
		if th < wallMinH {
			continue
		}
		used := 0
		for _, row := range layout {
			on := map[int]bool{}
			for _, cell := range row.cells {
				on[cell.col] = true
			}
			used += len(on)
		}
		tw := (w - (c - 1)) / c
		score := math.Abs(math.Log(float64(tw)/float64(th)/3.5)) + 1.5*float64(r*c-used)/float64(n)
		if score < bestScore {
			best, bestRows, bestScore = c, r, score
		}
	}
	if best > 0 {
		return best, bestRows, h / bestRows
	}
	// They don't all fit: as many columns as there's room for, and pages
	// of rows.
	rows = max(1, h/wallMinH)
	return min(maxCols, n), rows, h / rows
}

// wallStack is how many subagents stack in a column in rows tileH tall.
func wallStack(tileH int) int { return max(1, tileH/wallSubMinH) }

// wallBody draws the tiles into the frame's body: w wide, h tall.
func (m *Model) wallBody(w, h int) []string {
	items := m.wallItems()
	m.wall.tiles, m.wall.above, m.wall.below = m.wall.tiles[:0], 0, 0
	if len(items) == 0 {
		return m.wallEmpty(w, h)
	}
	m.wallPrune(items)
	groups := wallGroups(items)
	cols, rows, tileH := wallGrid(groups, w, h)
	layout := wallLayout(groups, cols, wallStack(tileH))
	rowOf := make([]int, len(items))
	for r, row := range layout {
		for _, c := range row.cells {
			rowOf[c.item] = r
		}
	}
	pick := m.wallPick(items)
	rows = min(rows, len(layout))
	m.wallScroll(rowOf[pick], len(layout), rows)
	m.wall.above, m.wall.below = wallOffscreen(rowOf, m.wall.top, rows)

	tileW := (w - (cols - 1)) / cols
	extraW := w - (cols - 1) - tileW*cols
	colW, colX := make([]int, cols), make([]int, cols)
	for c, x := 0, 0; c < cols; c++ {
		colW[c], colX[c] = tileW, x
		if c < extraW {
			colW[c]++
		}
		x += colW[c] + 1
	}
	out := make([]string, 0, h)
	for r := range rows {
		th := tileH
		if r == rows-1 {
			th = h - tileH*(rows-1) // the last row takes what's left over
		}
		out = append(out, m.wallRowLines(layout[m.wall.top+r], items, colW, colX, th, len(out), pick)...)
	}
	return out
}

// wallScroll keeps row, the picked tile's, among the rows shown of all.
func (m *Model) wallScroll(row, all, rows int) {
	if row < m.wall.top {
		m.wall.top = row
	} else if row >= m.wall.top+rows {
		m.wall.top = row - rows + 1
	}
	m.wall.top = max(0, min(m.wall.top, all-rows))
}

// wallOffscreen counts the tiles in rows above top, and below the rows
// shown from it; rowOf is each tile's row.
func wallOffscreen(rowOf []int, top, rows int) (above, below int) {
	for _, r := range rowOf {
		switch {
		case r < top:
			above++
		case r >= top+rows:
			below++
		}
	}
	return above, below
}

// wallEmpty is the body when there's nothing to show.
func (m *Model) wallEmpty(w, h int) []string {
	out := make([]string, h)
	msg := "nothing open right now"
	if !m.wall.all {
		msg += faint("  ·  a shows the last day's")
	}
	if h > 0 {
		out[h/2] = blanks((w-cellw.String(msg))/2) + dim(msg)
	}
	return out
}

// wallRowLines draws a row of tiles th tall, the row's top y lines down
// the body, noting where each tile is for clicks.
func (m *Model) wallRowLines(row wallRow, items []wallItem, colW, colX []int, th, y, pick int) []string {
	cols := len(colW)
	lines := make([][]string, cols)
	for _, c := range row.cells {
		ch, y0 := th/c.n, c.k*(th/c.n)
		if c.k == c.n-1 {
			ch = th - y0 // the last in a column takes what's left over
		}
		it := items[c.item]
		t := m.wallTile(it, colW[c.col], ch, c.item == pick)
		for len(t) < ch {
			t = append(t, blanks(colW[c.col]))
		}
		lines[c.col] = append(lines[c.col], t...)
		m.wall.tiles = append(m.wall.tiles, wallTile{key: it.key, x: colX[c.col], y: y + y0, w: colW[c.col], h: ch})
	}
	out := make([]string, 0, th)
	for ly := range th {
		var b strings.Builder
		for c := range cols {
			if c > 0 {
				if ly == 0 && row.joined[c-1] {
					b.WriteString(paint(cSub, "─")) // one agent's tiles, joined along the top
				} else {
					b.WriteByte(' ')
				}
			}
			if ly < len(lines[c]) {
				b.WriteString(lines[c][ly])
			} else {
				b.WriteString(blanks(colW[c]))
			}
		}
		out = append(out, b.String())
	}
	return out
}

func (m *Model) wallPick(items []wallItem) int {
	for i, it := range items {
		if it.key == m.wall.sel {
			return i
		}
	}
	if len(items) > 0 {
		m.wall.sel = items[0].key
	}
	return 0
}

// wallLook is how a tile shows its agent's state: its edge's colour, the
// glyph by its name, and the word for it.
func (m *Model) wallLook(a *fleet.Agent) (edge, glyph, word string) {
	now := m.snap.At
	switch {
	case a.NeedsYou():
		return cYellow, paint(cYellow+bold, "●"), paint(cYellow+bold, "needs you")
	case a.Halted():
		return cRed, paint(cRed, "✕"), paint(cRed, "stopped")
	case a.Live():
		return cOrange, paint(cOrange, convo.Spin(a.Kind, m.tick+len(a.ID))), paint(cOrange, "working")
	case a.Busy():
		return cQueue, paint(cQueue, "◌"), paint(cQueue, lanesLine(a))
	case a.Waiting():
		return cYellow, paint(cYellow, "○"), paint(cYellow, "waiting on you")
	case a.YourTurn(now):
		return cGreen, paint(cGreen, "●"), paint(cGreen, "your turn")
	case a.PID != 0:
		return cEdge, paint(cSub, "○"), dim("idle")
	}
	return cFaint, faint("·"), faint(age(a.Age(now)) + " ago")
}

// wallTile draws one tile, w×h, its edge heavy when it's picked: an
// agent's, or one of its subagents'.
func (m *Model) wallTile(it wallItem, w, h int, picked bool) []string {
	if it.sub != nil {
		return m.wallSubTile(it, w, h, picked)
	}
	return m.wallAgentTile(it.a, w, h, picked)
}

// wallAgentTile draws one agent's tile.
func (m *Model) wallAgentTile(a *fleet.Agent, w, h int, picked bool) []string {
	if w < 8 || h < 3 {
		return nil
	}
	now := m.snap.At
	p := m.previews[a.Key].p
	edge, glyph, word := m.wallLook(a)
	// Its transcript was written in the last couple of seconds: the edge
	// flashes bright.
	if !p.At.IsZero() && now.Sub(p.At) < 2*time.Second && (a.Live() || a.Busy()) {
		edge = cBright
	}
	tl, tr, bl, br, hz, vt := "╭", "╮", "╰", "╯", "─", "│"
	nameC := cText + bold
	if picked {
		tl, tr, bl, br, hz, vt = "┏", "┓", "┗", "┛", "━", "┃"
		nameC = cBright + bold
	}
	iw := w - 4 // inside the edge, a space either side

	// The top edge carries the name and, on the right, the model and age.
	model := modelWord(a.Kind, firstNonEmpty(p.Model, a.Spend.Model))
	if !unmarked(agent.Kind(a.Kind)) {
		model = strings.TrimSpace(agentName(a.Kind) + " " + model)
	}
	meta := strings.Join(nonEmpty(model, age(a.Age(now))), " · ")
	name := oneLine(a.DisplayName)
	roomName := w - 12 - cellw.String(meta)
	if roomName < 8 {
		meta, roomName = "", w-10
	}
	title := glyph + " " + paint(nameC, ansi.Truncate(name, max(roomName, 1), "…"))
	right := ""
	if meta != "" {
		right = " " + dim(meta) + " "
	}
	fill := w - 2 - 1 - cellw.String(title) - 2 - cellw.String(right) - 1
	top := paint(edge, tl+hz) + " " + title + " " + paint(edge, strings.Repeat(hz, max(fill, 0))) + right + paint(edge, hz+tr)
	top = fit(top, w)

	inner := make([]string, 0, h-2)
	// Where: the checkout and branch, and how full its context is.
	where := filepath.Base(a.Repo)
	if a.Repo == "" {
		where = filepath.Base(a.Cwd)
	}
	loc := paint(cSub, where)
	if a.Branch != "" {
		loc += faint(" ⎇ ") + dim(a.Branch)
	}
	ctx := ""
	if p.Context > 0 {
		pct := ctxFill(a, int64(p.Context), agent.ContextWindow(agent.Kind(a.Kind), p.Model)).Pct()
		ctx = ctxBar(pct) + " " + dim(fmt.Sprintf("%2.0f%%", pct))
	}
	inner = append(inner, wallSpread(loc, ctx, iw))

	// The bottom line: what it's doing now, what it costs, its todos.
	var now1 string
	switch {
	case a.NeedsYou() || a.Waiting():
		now1 = word + dim(" · ") + paint(cText, oneLine(firstNonEmpty(a.Needs, a.Detail, p.Text)))
	case a.Halted():
		now1 = word + dim(" · ") + paint(cSub, oneLine(a.Spend.Halt.Text))
	case a.Live() && p.Doing != "":
		now1 = glyph + " " + paint(cText, oneLine(tildify(p.Doing)))
	case a.Live() && a.Detail != "":
		now1 = glyph + " " + paint(cText, oneLine(a.Detail))
	default:
		now1 = word
	}
	var stats []string
	if a.Todos > 0 {
		c := cSub
		if a.TodosDone == a.Todos {
			c = cGreen
		}
		stats = append(stats, paint(c, fmt.Sprintf("☑ %d/%d", a.TodosDone, a.Todos)))
	}
	if n := a.Subs.Direct + a.Subs.Nested; n > 0 {
		stats = append(stats, paint(cQueue, fmt.Sprintf("⑂%d", n)))
	}
	if a.Spend.Cost > 0 {
		stats = append(stats, dim(moneyShort(a.Spend.Cost)))
	}
	foot := wallSpread(now1, strings.Join(stats, " "), iw)

	// Between them, the stream: its latest messages and tool calls, the
	// newest at the bottom, fading as they get older.
	room := h - 2 - 2
	if room > 0 {
		stream := m.wallStreamFor(a.Key, p, a.Live(), a.Intent, iw, room)
		for range room - len(stream) {
			inner = append(inner, blanks(iw))
		}
		inner = append(inner, stream...)
	}
	inner = append(inner, foot)

	return wallFrameClose(top, inner, edge, w, h, bl, br, hz, vt)
}

// wallFrameClose finishes a tile: top (drawn already), inner between the
// side edges, and the bottom edge. Every inner line is exactly w-4 wide
// already (wallSpread's, or a stream's), so none is measured again.
func wallFrameClose(top string, inner []string, edge string, w, h int, bl, br, hz, vt string) []string {
	out := make([]string, 0, h)
	out = append(out, top)
	side := paint(edge, vt)
	for _, l := range inner[:min(len(inner), h-2)] {
		out = append(out, side+" "+l+" "+side)
	}
	out = append(out, paint(edge, bl+strings.Repeat(hz, w-2)+br))
	return out
}

// wallSubTile draws a subagent still working: its type and what it was
// asked, streaming what it's doing, under its parent's repo and branch.
func (m *Model) wallSubTile(it wallItem, w, h int, picked bool) []string {
	if w < 8 || h < 3 {
		return nil
	}
	a, sub := it.a, it.sub
	now := m.snap.At
	p := m.previews[it.key].p
	edge, glyph := cOrange, paint(cOrange, spinner[(m.tick+len(sub.ID))%len(spinner)])
	if !p.At.IsZero() && now.Sub(p.At) < 2*time.Second {
		edge = cBright
	}
	tl, tr, bl, br, hz, vt := "╭", "╮", "╰", "╯", "─", "│"
	nameC := cText + bold
	if picked {
		tl, tr, bl, br, hz, vt = "┏", "┓", "┗", "┛", "━", "┃"
		nameC = cBright + bold
	}
	iw := w - 4

	typ := firstNonEmpty(sub.Type, "subagent")
	name := oneLine(firstNonEmpty(sub.Description, typ))
	// The top edge carries, on the right, what it runs on, as an agent's
	// tile does.
	meta := runsOn(agent.Kind(a.Kind), p.Model, p.Effort)
	roomName := w - 12 - cellw.String(meta)
	if roomName < 8 {
		meta, roomName = "", w-8
	}
	title := glyph + " " + paint(nameC, ansi.Truncate(name, max(roomName, 1), "…"))
	right := ""
	if meta != "" {
		right = " " + dim(meta) + " "
	}
	fill := w - 2 - 1 - cellw.String(title) - 2 - cellw.String(right) - 1
	top := paint(edge, tl+hz) + " " + title + " " + paint(edge, strings.Repeat(hz, max(fill, 0))) + right + paint(edge, hz+tr)
	top = fit(top, w)

	inner := make([]string, 0, h-2)
	where := filepath.Base(a.Repo)
	if a.Repo == "" {
		where = filepath.Base(a.Cwd)
	}
	loc := paint(cSub, where)
	if sub.Worktree != "" {
		loc += faint(" ⎇ ") + dim(sub.Worktree) // its own checkout, not its session's
	}
	inner = append(inner, wallSpread(loc+faint(" · ")+dim(typ), "", iw))

	foot := wallSpread(paint(cOrange, "running")+dim(" · ")+paint(cText, oneLine(firstNonEmpty(sub.Description, "…"))), "", iw)

	room := h - 2 - 2
	if room > 0 {
		stream := m.wallStreamFor(it.key, p, true, sub.Description, iw, room)
		for range room - len(stream) {
			inner = append(inner, blanks(iw))
		}
		inner = append(inner, stream...)
	}
	inner = append(inner, foot)

	return wallFrameClose(top, inner, edge, w, h, bl, br, hz, vt)
}

// wallPrune keeps only the streams of the tiles still on the Wall, once
// enough have come and gone.
func (m *Model) wallPrune(items []wallItem) {
	if len(m.wall.streams) <= 2*len(items) {
		return
	}
	keep := make(map[string]wallStreamed, len(items))
	for _, it := range items {
		if c, ok := m.wall.streams[it.key]; ok {
			keep[it.key] = c
		}
	}
	m.wall.streams = keep
}

// wallStreamFor is wallStream for the tile key, as drawn last time when
// nothing it's drawn from has changed since.
func (m *Model) wallStreamFor(key string, p agent.Preview, live bool, intent string, w, room int) []string {
	f := wallStreamFrom{at: p.At, n: len(p.Recent), live: live, intent: intent, ink: cText + cSub, w: w, room: room}
	if n := len(p.Recent); n > 0 {
		f.last = len(p.Recent[n-1].Text)
	}
	if c, ok := m.wall.streams[key]; ok && c.from == f {
		return c.lines
	}
	if m.wall.streams == nil {
		m.wall.streams = map[string]wallStreamed{}
	}
	lines := wallStream(p, live, intent, w, room)
	for i, l := range lines {
		lines[i] = fit(l, w) // each exactly w wide: the frame needn't measure them again
	}
	m.wall.streams[key] = wallStreamed{from: f, lines: lines}
	return lines
}

// wallStream is the end of what was said and done, room lines of w, oldest
// first. Lines fade with age: the newest two things bright, a few more dim,
// the rest faint. live colours the latest tool call's dot; intent is what
// to show instead, before anything has been said.
func wallStream(p agent.Preview, live bool, intent string, w, room int) []string {
	if len(p.Recent) == 0 {
		// The foot already says its Detail; here, what it was asked.
		say := intent
		if say == "" {
			return []string{faint("…")}
		}
		lines := wrap(oneLine(say), w)
		for i := range lines {
			lines[i] = dim(lines[i])
		}
		return lines[:min(len(lines), room)]
	}
	var out []string
	for back := 0; back < len(p.Recent) && len(out) < room; back++ {
		e := p.Recent[len(p.Recent)-1-back]
		tone := 0
		switch {
		case back >= 5:
			tone = 2
		case back >= 2:
			tone = 1
		}
		ink := func(c string) string {
			switch tone {
			case 1:
				if c == cText {
					return cSub
				}
				return cDim
			case 2:
				return cFaint
			}
			return c
		}
		var lines []string
		switch e.Role {
		case "user":
			ls := wrap(oneLine(e.Text), w-2)
			if len(ls) > 2 {
				ls = append(ls[:1], ansi.Truncate(ls[1], w-3, "")+"…")
			}
			for i, l := range ls {
				lead := "  "
				if i == 0 {
					lead = paint(ink(cBlue), "❯ ")
				}
				lines = append(lines, lead+paint(ink(cBlue), l))
			}
		case "tool":
			tool, arg, _ := strings.Cut(e.Text, "\x00")
			dot := ink(cOrange)
			if back > 0 || !live {
				dot = ink(cSub)
			}
			arg = ansi.Truncate(oneLine(tildify(arg)), max(w-cellw.String(tool)-4, 1), "…")
			lines = append(lines, paint(dot, "● ")+paint(ink(cText)+bold, tool)+" "+paint(ink(cSub), arg))
		default:
			ls := wrap(unmark(mdPlainNoUnder, oneLine(e.Text)), w-2)
			if keep := 3 + 2*boolInt(back == 0); len(ls) > keep {
				ls = append(ls[:keep-1], ansi.Truncate(ls[keep-1], w-3, "")+"…")
			}
			for _, l := range ls {
				lines = append(lines, "  "+paint(ink(cText), l))
			}
		}
		// Prepend whole messages; the oldest may lose its first lines.
		if over := len(out) + len(lines) - room; over > 0 {
			lines = lines[over:]
		}
		out = append(lines, out...)
	}
	return out
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

// wallSpread puts l on the left of w cells and r on the right, cutting l
// first when they don't both fit.
func wallSpread(l, r string, w int) string {
	rw := cellw.String(r)
	if rw == 0 {
		return fit(l, w)
	}
	if rw+2 > w {
		return fit(l, w)
	}
	return fit(l, w-rw-1) + " " + r
}

// wallTick keeps every tile's transcript tail fresh while the Wall is
// open: each is looked at, and read when it grew, off the UI's goroutine.
func (m *Model) wallTick() tea.Cmd {
	items := m.wallItems
	if m.mode != modeWall {
		if len(m.grid.keys) == 0 {
			return nil
		}
		items = m.gridItems // the grid's tiles are read the same way
	}
	if m.wall.reading == nil {
		m.wall.reading = map[string]bool{}
	}
	type want struct {
		key, path string
		kind      agent.Kind
		had       int64
	}
	var wants []want
	for _, it := range items() {
		path := it.a.TranscriptPath
		if it.sub != nil {
			path = it.sub.Path
		}
		if path == "" || m.wall.reading[it.key] || len(wants) >= 24 {
			continue
		}
		m.wall.reading[it.key] = true
		w := want{key: it.key, path: path, kind: it.a.Acct.Kind, had: -1}
		if e, ok := m.previews[it.key]; ok {
			w.had = e.size
		}
		wants = append(wants, w)
	}
	if len(wants) == 0 {
		return nil
	}
	return func() tea.Msg {
		out := make(wallReadMsg, len(wants))
		var wg sync.WaitGroup
		for i, w := range wants {
			wg.Go(func() {
				out[i].key = w.key
				st, err := os.Stat(w.path)
				if err != nil || st.Size() == w.had {
					return
				}
				out[i].ok, out[i].e = true, previewEntry{p: agent.ReadPreview(w.kind, w.path, 128<<10), size: st.Size()}
			})
		}
		wg.Wait()
		return out
	}
}

func (m *Model) onWallRead(msg wallReadMsg) {
	for i := range msg {
		delete(m.wall.reading, msg[i].key)
		if msg[i].ok {
			m.previews[msg[i].key] = msg[i].e
		}
	}
}

func (m *Model) wallHint() string {
	all := "the last day's"
	if m.wall.all {
		all = "only what's open"
	}
	pairs := []string{"←↑↓→", "move", "enter", "open", "a", all, "ctrl+b", "keep going", "esc", "back"}
	if a := m.agentByKey(m.wall.sel); a != nil && a.Halted() {
		pairs[7] = "continue"
	}
	more := ""
	if m.wall.above > 0 {
		more += paint(cSub, fmt.Sprintf("↑ %d above", m.wall.above))
	}
	if m.wall.below > 0 {
		if more != "" {
			more += dim(" · ")
		}
		more += paint(cSub, fmt.Sprintf("↓ %d more", m.wall.below))
	}
	if more == "" {
		return keysFit(m.w-4, pairs...)
	}
	return more + "   " + keysFit(m.w-4-cellw.String(more)-3, pairs...)
}

func (m *Model) wallKey(s string) tea.Cmd {
	items := m.wallItems()
	if len(items) == 0 {
		switch s {
		case "a":
			m.wall.all = !m.wall.all
		case "esc", "q":
			m.setView(placeAgents)
		}
		return nil
	}
	i := m.wallPick(items)
	groups := wallGroups(items)
	cols, _, tileH := wallGrid(groups, m.w-4, m.wallH())
	geo := wallGeos(wallLayout(groups, cols, wallStack(tileH)), len(items))
	// move goes to the tile the way dx or dy goes; across, with none
	// there, to the one before or after it.
	move := func(dx, dy int) {
		j := wallStep(geo, i, dx, dy)
		if j < 0 && dx != 0 && i+dx >= 0 && i+dx < len(items) {
			j = i + dx
		}
		if j >= 0 {
			m.wall.sel = items[j].key
		}
	}
	it := items[i]
	switch s {
	case "esc", "q":
		m.setView(placeAgents)
	case "left", "h":
		move(-1, 0)
	case "right", "l":
		move(1, 0)
	case "up", "k":
		move(0, -1)
	case "down", "j":
		move(0, 1)
	case "home", "g":
		m.wall.sel = items[0].key
	case "end", "G":
		m.wall.sel = items[len(items)-1].key
	case "a":
		m.wall.all = !m.wall.all
	case "enter":
		return m.goAgent(it.a)
	case "ctrl+b", "alt+g":
		if it.sub == nil {
			return m.keepGoing(it.a)
		}
	}
	return nil
}

// wallH is the body's height in the frame: see frame.
func (m *Model) wallH() int { return m.h - m.headH() - 3 }

// wallClick picks the tile under the mouse; a second click opens it.
func (m *Model) wallClick(x, y int) tea.Cmd {
	bx, by := x-2, y-m.headH()-1 // the frame's indent and the row under the header
	for _, t := range m.wall.tiles {
		if bx < t.x || bx >= t.x+t.w || by < t.y || by >= t.y+t.h {
			continue
		}
		double := t.key == m.wall.sel && time.Since(m.lastClick) < 400*time.Millisecond
		m.wall.sel, m.lastClick = t.key, time.Now()
		if double {
			for _, it := range m.wallItems() {
				if it.key == t.key {
					return m.goAgent(it.a)
				}
			}
		}
		return nil
	}
	return nil
}
