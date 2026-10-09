package ui

import (
	"fmt"
	"slices"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// manyAgentsEvents is a turn that launched n background subagents at once,
// each answering "Async agent launched…", then the n notifications that
// woke the session as each finished.
func manyAgentsEvents(n int) []any {
	var evs []any
	apply := func(ev any) { evs = append(evs, ev) }
	apply(host.Sent{Text: "fan out: one lane per module, review each for residue"})
	apply(headless.Message{Role: "assistant", ID: "m0", Blocks: []headless.Block{{Type: "text", Text: "Launching one lane per module."}}})
	for i := range n {
		apply(headless.Message{Role: "assistant", ID: "m0", Blocks: []headless.Block{{Type: "tool_use", ID: fmt.Sprintf("x%d", i), Name: "Agent",
			Input: benchJSON(map[string]any{"description": fmt.Sprintf("lane %d review", i), "prompt": strings.Repeat(fmt.Sprintf("review module %d for residue. ", i), 20), "subagent_type": "general-purpose", "run_in_background": true})}}})
	}
	for i := range n {
		id := fmt.Sprintf("x%d", i)
		apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: id,
			Text: fmt.Sprintf("Async agent launched successfully.\nagentId: a%d (internal ID - do not mention to user. Use SendMessage with to: 'a%d' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes.\nDo not duplicate this agent's work — avoid working with the same files or topics it is using. Briefly tell the user what you launched and end your response.\noutput_file: /tmp/tasks/a%d.output\nIf asked, you can check progress before completion by using Read or Bash tail on the output file.", i, i, i)}},
			ToolResult: benchJSON(map[string]any{"isAsync": true, "status": "async_launched", "agentId": fmt.Sprintf("a%d", i)})})
		ev, _ := headless.Decode([]byte(fmt.Sprintf(`{"type":"system","subtype":"task_started","task_id":"a%d","tool_use_id":"%s","description":"lane %d review","task_type":"local_agent"}`, i, id, i)))
		apply(ev)
	}
	apply(headless.Message{Role: "assistant", ID: "m1", Blocks: []headless.Block{{Type: "text", Text: fmt.Sprintf("Launched %d lanes; I'll report as they finish.", n)}}})
	apply(headless.Result{Subtype: "success"})
	// Each lane works, steps nested under its Agent call in the turn gone by.
	for round := range 3 {
		for i := range n {
			parent, id := fmt.Sprintf("x%d", i), fmt.Sprintf("x%d-%d", i, round)
			apply(headless.Message{Role: "assistant", ParentToolUseID: parent, Blocks: []headless.Block{{Type: "tool_use", ID: id, Name: "Bash", Input: benchJSON(map[string]any{"command": fmt.Sprintf("rg residue modules/m%d", i), "description": "look for residue"})}}})
			apply(headless.Message{Role: "user", ParentToolUseID: parent, Blocks: []headless.Block{{Type: "tool_result", ToolUseID: id, Text: strings.Repeat("modules/x/y.go:12: leftover\n", 8)}}})
		}
	}
	for i := range n {
		note := fmt.Sprintf("<task-notification>\n<task-id>a%d</task-id>\n<tool-use-id>x%d</tool-use-id>\n<status>completed</status>\n<summary>Agent \"lane %d review\" finished</summary>\n<result>%s</result>\n</task-notification>", i, i, i, strings.Repeat("Found two leftovers and removed them. ", 12))
		ev, _ := headless.Decode([]byte(fmt.Sprintf(`{"type":"system","subtype":"task_notification","task_id":"a%d","tool_use_id":"x%d","status":"completed","summary":"lane %d review"}`, i, i, i)))
		apply(ev)
		apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "text", Text: note}}})
		apply(headless.Message{Role: "assistant", ID: fmt.Sprintf("n%d", i), Blocks: []headless.Block{{Type: "text", Text: fmt.Sprintf("Lane %d is done.", i)}}})
		apply(headless.Result{Subtype: "success"})
	}
	return evs
}

// manyAgentsConvo is manyAgentsEvents(n) played up to its kth event.
func manyAgentsConvo(n, k int) *convo.Session {
	s := convo.New()
	s.Info.Cwd = "/work/rush"
	evs := manyAgentsEvents(n)
	t0 := time.Now().Add(-time.Hour)
	for i, ev := range evs[:min(k, len(evs))] {
		s.Apply(ev, t0.Add(time.Duration(i)*time.Second))
	}
	return s
}

// shownRows is the transcript rows on screen by absolute row, top first,
// and each one's ref, or its text when it has none.
func shownRows(c *hostConn) (abs []int, text []string) {
	for j := c.bodyTop; j < len(c.rowBody); j++ {
		if i := c.rowBody[j]; i >= 0 && i < len(c.shown) {
			abs = append(abs, c.shownBase+i)
			l := c.shown[i]
			if l.Ref == "" {
				l.Ref = ansi.Strip(l.Text)
			}
			text = append(text, l.Ref)
		}
	}
	return abs, text
}

// undrawn counts transcript rows on screen that weren't drawn in time.
func undrawn(c *hostConn) int {
	start := c.shownTotal - c.scroll - c.bodyRows
	n := 0
	for j := c.bodyTop; j < c.bodyTop+c.bodyRows && j < len(c.rowBody); j++ {
		if r := start + j - c.bodyTop; c.rowBody[j] < 0 && r < c.transcriptRows {
			n++
		}
	}
	return n
}

// The pane over a session that fanned out to 40 agents scrolls as it
// should: the wheel moves the screen three rows a turn all the way to the
// top with every row drawn, Home and the minimap's top reach the first row
// at once, End the last, and the end follows what arrives.
func TestManyAgentsScroll(t *testing.T) {
	var m *Model
	var c *hostConn
	fresh := func() {
		m, _ = benchModel(200, 50)
		c = m.host
		c.sess, c.bodyBuf, c.input = manyAgentsConvo(40, 1<<30), nil, nil
		m.View()
	}
	top := func() bool {
		abs, refs := shownRows(c)
		return len(abs) > 0 && abs[0] <= 1 && slices.Contains(refs, c.sess.TurnRef(0))
	}

	fresh()
	m.Update(tea.KeyPressMsg{Code: tea.KeyHome})
	m.View()
	if !top() || c.scroll != c.scrollCap {
		t.Fatalf("one Home scrolled %d of %d, not to the first row", c.scroll, c.scrollCap)
	}
	m.Update(tea.KeyPressMsg{Code: tea.KeyEnd})
	m.View()
	if abs, _ := shownRows(c); c.scroll != 0 || abs[len(abs)-1] != c.transcriptRows-1 {
		t.Fatalf("End left the pane scrolled %d up", c.scroll)
	}

	fresh()
	if !c.minimap.visible {
		t.Fatal("no minimap at 200 columns")
	}
	m.seekMinimap(c, 0)
	m.View()
	if !top() {
		t.Fatalf("the minimap's top scrolled %d of %d, not to the first row", c.scroll, c.scrollCap)
	}

	fresh()
	var prev []string
	for step := 0; c.scroll < c.scrollCap || step == 0; step++ {
		if step > 400 {
			t.Fatal("the wheel never reached the top")
		}
		was := c.scroll
		m.gate.DrawnAt(time.Time{})
		m.Update(tea.MouseWheelMsg{X: 150, Y: 25, Button: tea.MouseWheelUp})
		m.View()
		_, refs := shownRows(c)
		if n := undrawn(c); n > 0 {
			t.Fatalf("scrolled to %d, %d rows weren't drawn", c.scroll, n)
		}
		if d := c.scroll - was; was > 0 && prev != nil {
			for k := 0; k+d < len(refs) && k < len(prev)-1; k++ {
				if prev[k] != refs[k+d] {
					t.Fatalf("scrolled %d→%d, screen row %d (%q) isn't %d rows down", was, c.scroll, k, prev[k], d)
				}
			}
		}
		prev = refs
	}
	if !top() {
		t.Fatal("the wheel's top isn't the session's first row")
	}

	// At the end, each event as it arrives ends what's shown.
	m.Update(tea.KeyPressMsg{Code: tea.KeyEnd})
	s := convo.New()
	s.Info.Cwd = "/work/rush"
	c.sess, c.bodyBuf = s, nil
	t0 := time.Now().Add(-time.Hour)
	for i, ev := range manyAgentsEvents(40) {
		s.Apply(ev, t0.Add(time.Duration(i)*time.Second))
		c.scrollOnly = false // as the host's lines arriving do
		m.View()
		abs, _ := shownRows(c)
		if c.scroll != 0 || len(abs) == 0 || abs[len(abs)-1] != c.transcriptRows-1 {
			t.Fatalf("event %d: the end isn't followed (scrolled %d)", i, c.scroll)
		}
	}
}

// BenchmarkManyAgentsFrame is a frame of a session that fanned out to 40
// background agents: wheeling through it once they've all reported, and a
// clock tick's frame while all 40 still run.
func BenchmarkManyAgentsFrame(b *testing.B) {
	b.Run("wheel", func(b *testing.B) {
		m, _ := benchModel(250, 70)
		m.host.sess, m.host.bodyBuf = manyAgentsConvo(40, 1<<30), nil
		m.View()
		b.ReportAllocs()
		i := 0
		for b.Loop() {
			btn := tea.MouseWheelUp
			if i/60%2 == 1 {
				btn = tea.MouseWheelDown
			}
			i++
			m.Update(tea.MouseWheelMsg{X: 245, Y: 35, Button: btn})
			m.View()
		}
	})
	b.Run("running", func(b *testing.B) {
		m, _ := benchModel(250, 70)
		m.host.sess, m.host.bodyBuf = manyAgentsConvo(40, 2+40+80+1), nil // all launched, the turn still live
		m.View()
		b.ReportAllocs()
		for b.Loop() {
			m.tick++
			m.host.scrollOnly = false
			m.View()
		}
	})
}

// Scrolled up over the 40 agents' turn, PgUp and PgDn move the screen ten
// rows a press, and what's shown stays put while the lanes' nested steps,
// their notifications and the turns they wake arrive.
func TestManyAgentsScrollHoldsWhileAgentsWork(t *testing.T) {
	m, _ := benchModel(200, 50)
	c := m.host
	s := convo.New()
	s.Info.Cwd = "/work/rush"
	c.sess, c.bodyBuf, c.input = s, nil, nil
	evs, t0 := manyAgentsEvents(40), time.Now().Add(-time.Hour)
	const launched = 2 + 40 + 80 + 2
	for i, ev := range evs[:launched] {
		s.Apply(ev, t0.Add(time.Duration(i)*time.Second))
	}
	m.View()
	for _, key := range []rune{tea.KeyPgUp, tea.KeyPgUp, tea.KeyPgDown} {
		_, prev := shownRows(c)
		was := c.scroll
		m.Update(tea.KeyPressMsg{Code: key})
		m.View()
		_, refs := shownRows(c)
		d := c.scroll - was
		if d != 10 && d != -10 {
			t.Fatalf("a page key scrolled %d→%d, not ten rows", was, c.scroll)
		}
		if was == 0 {
			continue // leaving the end, the pill takes a row, as with the wheel
		}
		for k := max(0, -d); k+d < len(refs) && k < len(prev); k++ {
			if prev[k] != refs[k+d] {
				t.Fatalf("scrolled %d→%d, screen row %d (%q) isn't %d rows down", was, c.scroll, k, prev[k], d)
			}
		}
	}
	_, held := shownRows(c)
	for i, ev := range evs[launched:] {
		s.Apply(ev, t0.Add(time.Duration(launched+i)*time.Second))
		c.scrollOnly = false
		m.View()
		if _, refs := shownRows(c); len(refs) == 0 || refs[0] != held[0] {
			t.Fatalf("event %d moved the screen: top row %q, was %q (scrolled %d)", launched+i, refs[0], held[0], c.scroll)
		}
	}
}
