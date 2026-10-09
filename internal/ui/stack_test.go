package ui

import (
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// stackModel is a bench session with three subagents, each with a tail.
func stackModel(t *testing.T) (*Model, *hostConn) {
	m, _ := benchModel(200, 60)
	c := m.host
	now := time.Now()
	c.subTails = map[string]*convo.Tail{}
	for i, id := range []string{"a", "b", "c"} {
		c.subs = append(c.subs, convo.Subagent{ID: id, Type: "child " + id, Description: "does " + id, Mod: now.Add(time.Duration(i) * time.Second).UnixNano()})
		tl := &convo.Tail{Sess: convo.New()}
		tl.Sess.Apply(host.Sent{Text: "task for " + id}, now)
		c.subTails[id] = tl
	}
	return m, c
}

func (m *Model) bandRows(c *hostConn) map[string]int {
	n := map[string]int{}
	for i := c.bodyTop; i < len(c.rowRefs); i++ {
		if strings.HasPrefix(c.rowRefs[i], "stack:") {
			n[c.rowRefs[i]]++
		}
	}
	return n
}

func TestStackBands(t *testing.T) {
	m, c := stackModel(t)
	m.toggleStack(c)
	if m.viewName(c) != "stack" {
		t.Fatalf("view %q", m.viewName(c))
	}
	for range stackFrames {
		m.rushPane(200, 60)
	}
	n := m.bandRows(c)
	if len(n) != 4 {
		t.Fatalf("bands %v", n)
	}
	// The main agent on top is the biggest; the live ones share the rest evenly.
	lo, hi := 1<<30, 0
	for _, id := range []string{"a", "b", "c"} {
		lo, hi = min(lo, n["stack:"+id]), max(hi, n["stack:"+id])
	}
	if hi-lo > 1 || n["stack:"] <= hi {
		t.Fatalf("bands %v: main not biggest or subagents uneven", n)
	}

	// A finished one leaves the stack; the live ones take its rows.
	if c.sess.TaskStatus == nil {
		c.sess.TaskStatus = map[string]string{}
	}
	c.sess.TaskStatus["a"] = "completed"
	m.rushPane(200, 60)
	if d := m.bandRows(c); d["stack:a"] != 0 || d["stack:b"] <= n["stack:b"] {
		t.Fatalf("finished a still shown, or b didn't take its rows: %v -> %v", n, d)
	}
	c.sess.TaskStatus["a"] = ""
	m.rushPane(200, 60)
	if c.rowRefs[c.bodyTop] != "stack:" {
		t.Fatalf("top band is %q, not the main agent", c.rowRefs[c.bodyTop])
	}
	// Most recently active first: c, then b, then a.
	var order []string
	for _, r := range c.rowRefs[c.bodyTop:] {
		if r != "" && (len(order) == 0 || order[len(order)-1] != r) {
			order = append(order, r)
		}
	}
	if !slices.Equal(order, []string{"stack:", "stack:c", "stack:b", "stack:a"}) {
		t.Fatalf("order %v", order)
	}

	// Hover doesn't change the split: every band the same.
	c.pointerHover.ref = "stack:b"
	m.rushPane(200, 60)
	if h := m.bandRows(c); h["stack:b"] != n["stack:b"] {
		t.Fatalf("hover changed the split: %v -> %v", n, h)
	}

	// Clicking a subagent's band watches it; the main band goes back.
	y := m.paneTop + slices.Index(c.rowRefs, "stack:a")
	if !m.clickStack(c, y) || !m.watchingSub(c) || c.subOpen != "a" {
		t.Fatalf("click didn't watch a: view %q open %q", m.viewName(c), c.subOpen)
	}
	m.closeSub(c)
	m.toggleStack(c)
	for range stackFrames {
		m.rushPane(200, 60)
	}
	if !m.clickStack(c, m.paneTop+c.bodyTop) || m.viewName(c) != "conversation" {
		t.Fatalf("main band click: view %q", m.viewName(c))
	}
}

func TestStackPullUp(t *testing.T) {
	m, c := stackModel(t)
	running := func() {
		c.runMemo.key = runsKey{sess: c.sess, applied: c.sess.Applied(), sec: time.Now().Unix()}
		c.runMemo.out, c.runMemo.ok = slices.Clone(c.subs), true
	}
	running()
	m.View()
	running()
	m.View()
	y := slices.Index(c.rowRefs, "subs:title")
	if y < 0 {
		t.Fatal("no subagents panel title in the conversation")
	}
	y += m.paneTop
	x := m.paneX() + 5
	m.Update(tea.MouseClickMsg{X: x, Y: y, Button: tea.MouseLeft})
	m.Update(tea.MouseMotionMsg{X: x, Y: y - 1, Button: tea.MouseLeft})
	if m.viewName(c) != "conversation" {
		t.Fatal("a row of drag opened the stack")
	}
	m.Update(tea.MouseMotionMsg{X: x, Y: y - 3, Button: tea.MouseLeft})
	if m.viewName(c) != "stack" {
		t.Fatalf("pull-up left view %q", m.viewName(c))
	}
	m.Update(tea.MouseReleaseMsg{X: x, Y: y - 3, Button: tea.MouseLeft})
	// It grows: fewer band rows on the first frame than once grown.
	m.rushPane(200, 60)
	first := 0
	for _, v := range m.bandRows(c) {
		first += v
	}
	if !c.sess.Fast {
		t.Fatal("no fast frames while growing")
	}
	for range stackFrames {
		m.rushPane(200, 60)
	}
	last := 0
	for _, v := range m.bandRows(c) {
		last += v
	}
	if first >= last {
		t.Fatalf("didn't grow: %d -> %d", first, last)
	}
}

// The stack has no tab; esc or a click on its frame puts it away.
func TestStackNoTabAndBack(t *testing.T) {
	m, c := stackModel(t)
	if out := ansi.Strip(strings.Join(m.rushPane(200, 60), "\n")); strings.Contains(out, " stack ") {
		t.Fatal("a stack tab shows")
	}
	m.toggleStack(c)
	m.rushPane(200, 60)
	c.input, c.back = nil, 0
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyEscape}, "esc")
	if m.viewName(c) != "conversation" {
		t.Fatalf("esc left %q", m.viewName(c))
	}
	m.toggleStack(c)
	m.rushPane(200, 60)
	if !m.clickStack(c, m.paneTop) || m.viewName(c) != "conversation" {
		t.Fatalf("a frame click left %q", m.viewName(c))
	}
}

// Past stackMax, a live subagent is its title row alone.
func TestStackMax(t *testing.T) {
	m, c := stackModel(t)
	old := time.Now().Add(-10 * time.Second)
	for i, id := range []string{"d", "e", "f"} {
		c.subs = append(c.subs, convo.Subagent{ID: id, Type: "child " + id, Mod: old.Add(time.Duration(i) * time.Second).UnixNano()})
		tl := &convo.Tail{Sess: convo.New()}
		tl.Sess.Apply(host.Sent{Text: "task for " + id}, old)
		c.subTails[id] = tl
	}
	m.toggleStack(c)
	for range stackFrames {
		m.rushPane(200, 60)
	}
	n, bands := m.bandRows(c), 0
	for _, v := range n {
		if v > 1 {
			bands++
		}
	}
	if bands != stackMax+1 || len(n) != 7 {
		t.Fatalf("want %d bands of 7: %v", stackMax+1, n)
	}
}

// Dozens of runs live: the title rows past the bands fit the pane.
func TestStackManyLive(t *testing.T) {
	m, c := stackModel(t)
	now := time.Now()
	for i := range 60 {
		id := "x" + strconv.Itoa(i)
		c.subs = append(c.subs, convo.Subagent{ID: id, Type: id, Mod: now.UnixNano()})
		c.subTails[id] = &convo.Tail{Sess: convo.New()}
	}
	if c.liveSubs() < 40 {
		t.Fatalf("%d live", c.liveSubs())
	}
	c.stackGrow = stackFrames
	o := convo.Options{Width: 120, Now: now}
	for _, h := range []int{1, 5, 30} {
		if out := m.stackLines(c, o, h); len(out) > h+1 {
			t.Fatalf("%d rows into %d", len(out), h)
		}
	}
}

// A fold row's click shows all, as ctrl+o; it's no stop for ↑↓.
func TestClickShowAll(t *testing.T) {
	m, c := stackModel(t)
	m.clickRef(c, convo.ShowAllRef)
	if !c.verbose || c.sel == convo.ShowAllRef {
		t.Fatalf("verbose %v, sel %q", c.verbose, c.sel)
	}
	m.clickRef(c, convo.ShowAllRef)
	if c.verbose {
		t.Fatal("a second click folds again")
	}
}
