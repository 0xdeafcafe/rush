package ui

import (
	"cmp"
	"fmt"
	"math"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
)

// The feed's graph is who's been talking to whom: each agent that has
// messaged another is a mark set round an ellipse, and a line joins each
// pair that have, labelled with how many messages went between them,
// brighter the busier, orange while fresh. g in the feed switches to it
// and back; it keeps to the open project tab.

// graphFresh is how long a pair's line stays orange after a message.
const graphFresh = 2 * time.Minute

// graphEdge is a pair of agents that have messaged each other.
type graphEdge struct {
	a, b string // their @handles
	n    int    // messages between them, both ways
	last time.Time
}

// feedGraph is the agents in posts that have messaged another, busiest
// first, and each pair of them that has.
func feedGraph(posts []streamPost) (nodes []string, edges []graphEdge) {
	at := map[[2]string]int{}
	sent := map[string]int{}
	for _, p := range posts {
		if p.to == "" {
			continue
		}
		a, b := p.author.Username(), p.to
		if b < a {
			a, b = b, a
		}
		i, ok := at[[2]string{a, b}]
		if !ok {
			i, at[[2]string{a, b}] = len(edges), len(edges)
			edges = append(edges, graphEdge{a: a, b: b})
		}
		edges[i].n++
		if p.at.After(edges[i].last) {
			edges[i].last = p.at
		}
		for _, h := range []string{a, b} {
			if _, ok := sent[h]; !ok {
				nodes = append(nodes, h)
			}
			sent[h]++
		}
	}
	slices.SortStableFunc(nodes, func(x, y string) int { return cmp.Compare(sent[y], sent[x]) })
	return nodes, edges
}

func (s *communitySheet) graphBody(m *Model, w, h int) []string {
	out := []string{sheetTitle("the feed 🐓", "who's been talking to whom", w)}
	if keys, names := s.projects(m); keys != nil {
		out = append(out, "", feedTabs(names, slices.Index(keys, s.project), w))
	}
	nodes, edges := feedGraph(s.shown(m))
	footer := append([]string{""}, keysControls(w, "g", "Timeline", "[ ]", "Project", "esc", "Close")...)
	s.rows = nil // nothing here to click
	if len(edges) == 0 {
		out = append(out, "", paint(cText, "No agents have messaged each other yet."),
			dim("Their messages to each other land here as they send them."))
	} else {
		total, busiest := 0, edges[0]
		for _, e := range edges {
			total += e.n
			if e.n > busiest.n {
				busiest = e
			}
		}
		out = append(out, "", dim(fmt.Sprintf("%d agents · %d messages · busiest %s ↔ %s, %d", len(nodes), total, busiest.a, busiest.b, busiest.n)), "")
		out = append(out, drawGraph(nodes, edges, w, max(3, h-len(out)-len(footer)), time.Now())...)
	}
	for len(out) < h-len(footer) {
		out = append(out, "")
	}
	out = append(out, footer...)
	for i := range out {
		out[i] = fit(out[i], w)
	}
	return out
}

// graphCell is one cell of the graph as it's drawn.
type graphCell struct {
	r   rune
	ink string
}

// drawGraph lays nodes round an ellipse in a w×h box, joins each edge's
// pair with a line and its count, and writes each agent's name by its
// mark: before it on the left half, after it on the right.
func drawGraph(nodes []string, edges []graphEdge, w, h int, now time.Time) []string {
	grid := make([][]graphCell, h)
	for y := range grid {
		grid[y] = make([]graphCell, w)
		for x := range grid[y] {
			grid[y][x] = graphCell{r: ' '}
		}
	}
	set := func(x, y int, r rune, ink string) {
		if y >= 0 && y < h && x >= 0 && x < w {
			grid[y][x] = graphCell{r, ink}
		}
	}
	nameW := 0 // the longest name, up to a quarter of the width
	for _, n := range nodes {
		nameW = max(nameW, min(cellw.String(n), w/4))
	}
	// The marks: round an ellipse from the top, clockwise; two face each other.
	cx, cy := float64(w-1)/2, float64(h-1)/2
	rx, ry := max(1, cx-float64(nameW)-3), max(0, cy-1)
	pos := map[string][2]int{}
	start := -math.Pi / 2
	if len(nodes) == 2 {
		start = math.Pi
	}
	for i, n := range nodes {
		a := start + 2*math.Pi*float64(i)/float64(len(nodes))
		pos[n] = [2]int{int(math.Round(cx + rx*math.Cos(a))), int(math.Round(cy + ry*math.Sin(a)))}
	}
	busy := 0
	for _, e := range edges {
		busy = max(busy, e.n)
	}
	// Quiet pairs first, so the busy ones are drawn over them.
	order := slices.Clone(edges)
	slices.SortStableFunc(order, func(x, y graphEdge) int { return cmp.Compare(x.n, y.n) })
	for _, e := range order {
		ink := cFaint
		switch {
		case now.Sub(e.last) < graphFresh:
			ink = cOrange
		case e.n*2 >= busy:
			ink = cSub
		case e.n*4 >= busy:
			ink = cDim
		}
		p, q := pos[e.a], pos[e.b]
		line := graphLine(p, q)
		for _, c := range line[1 : len(line)-1] {
			set(c[0], c[1], graphStroke(p, q), ink)
		}
		mid, label := line[len(line)/2], fmt.Sprint(" ", e.n, " ")
		for i, r := range label {
			set(mid[0]-len(label)/2+i, mid[1], r, ink+bold)
		}
	}
	for _, n := range nodes {
		p, tint := pos[n], handleTint(n)
		name := cellw.Truncate(n, nameW, "…")
		set(p[0], p[1], '●', tint)
		x := p[0] + 2
		if float64(p[0]) < cx {
			x = p[0] - 1 - cellw.String(name)
		}
		set(x-1, p[1], ' ', "")
		for _, r := range name {
			set(x, p[1], r, tint+bold)
			x++
		}
		set(x, p[1], ' ', "")
	}
	out := make([]string, h)
	for y, row := range grid {
		var b strings.Builder
		for x := 0; x < len(row); {
			end := x
			for end < len(row) && row[end].ink == row[x].ink {
				end++
			}
			run := make([]rune, 0, end-x)
			for _, c := range row[x:end] {
				run = append(run, c.r)
			}
			if row[x].ink == "" {
				b.WriteString(string(run))
			} else {
				b.WriteString(paint(row[x].ink, string(run)))
			}
			x = end
		}
		out[y] = b.String()
	}
	return out
}

// graphLine is the cells from p to q, both ends in.
func graphLine(p, q [2]int) [][2]int {
	dx, dy := abs(q[0]-p[0]), -abs(q[1]-p[1])
	sx, sy := cmp.Compare(q[0], p[0]), cmp.Compare(q[1], p[1])
	var out [][2]int
	x, y, e := p[0], p[1], dx+dy
	for {
		out = append(out, [2]int{x, y})
		if x == q[0] && y == q[1] {
			return out
		}
		e2 := 2 * e
		if e2 >= dy {
			e, x = e+dy, x+sx
		}
		if e2 <= dx {
			e, y = e+dx, y+sy
		}
	}
}

// graphStroke is the box-drawing stroke for a line from p to q, a cell
// being about twice as tall as it's wide.
func graphStroke(p, q [2]int) rune {
	dx, dy := q[0]-p[0], q[1]-p[1]
	switch {
	case abs(dx) >= 4*abs(dy):
		return '─'
	case abs(dx) <= abs(dy):
		return '│'
	case (dx > 0) == (dy > 0):
		return '╲'
	}
	return '╱'
}
