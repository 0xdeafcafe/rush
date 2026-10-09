package ui

import (
	"fmt"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	photonframe "github.com/0xdeafcafe/photon/frame"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

func longAnswerScrollModel() *Model {
	m, _ := benchModel(200, 60)
	c := m.host
	s := convo.New()
	s.Apply(host.Sent{Text: "Investigate and research"}, time.Now())
	var answer strings.Builder
	for i := range 150 {
		fmt.Fprintf(&answer, "Paragraph %d has enough words to read while scrolling through the answer.\n\n", i)
	}
	s.Apply(headless.Message{Role: "assistant", ID: "answer", Blocks: []headless.Block{{Type: "text", Text: answer.String()}}}, time.Now())
	s.Apply(headless.Result{Subtype: "success"}, time.Now())
	c.sess, c.bodyBuf, c.shown, c.scroll = s, nil, nil, 0
	m.View()
	return m
}

func TestScrollLongAnswerKeepsPositionAcrossRedraws(t *testing.T) {
	m := longAnswerScrollModel()
	c := m.host
	for _, direction := range []tea.MouseButton{tea.MouseWheelUp, tea.MouseWheelDown} {
		for i := range 30 {
			before := c.scroll
			want := before + 3
			if direction == tea.MouseWheelDown {
				want = max(0, before-3)
			}
			want = min(want, c.scrollCap)
			m.gate.DrawnAt(time.Time{})
			m.Update(tea.MouseWheelMsg{X: 195, Y: 30, Button: direction})
			m.View()
			if c.scroll != want {
				t.Fatalf("wheel %v step %d: scroll %d -> %d, want %d", direction, i, before, c.scroll, want)
			}
			row := c.rowBody[c.bodyTop+2] // below the pinned prompt
			shown := c.shown[row].Text
			c.scrollOnly = false
			m.View()
			if c.scroll != want {
				t.Fatalf("redraw after wheel %v step %d: scroll %d, want %d", direction, i, c.scroll, want)
			}
			if got := c.shown[c.rowBody[c.bodyTop+2]].Text; got != shown {
				t.Fatalf("redraw after wheel %v step %d changed the visible row", direction, i)
			}
		}
	}
}

// A mostly vertical trackpad gesture can contain horizontal wheel events.
// Those must not undo the vertical movement, including within a held frame.
func TestScrollDiagonalGestureDoesNotRubberBand(t *testing.T) {
	m := longAnswerScrollModel()
	c := m.host
	c.scroll, c.scrollOnly = 30, true
	m.View()
	for _, held := range []bool{false, true} {
		for _, vertical := range []tea.MouseButton{tea.MouseWheelUp, tea.MouseWheelDown} {
			for _, horizontal := range []tea.MouseButton{tea.MouseWheelLeft, tea.MouseWheelRight} {
				before := c.scroll
				m.gate.DrawnAt(time.Time{})
				if held {
					m.gate.DrawnAt(time.Now())
				}
				m.Update(tea.MouseWheelMsg{X: 195, Y: 30, Button: vertical})
				m.View()
				want := before + 3
				if vertical == tea.MouseWheelDown {
					want = before - 3
				}
				for range 4 {
					m.Update(tea.MouseWheelMsg{X: 195, Y: 30, Button: horizontal})
					m.View()
					if c.scroll != want {
						t.Fatalf("%v after %v moved scroll from %d to %d", horizontal, vertical, want, c.scroll)
					}
				}
				m.Update(photonframe.TickMsg{})
				m.View()
				if c.scroll != want {
					t.Fatalf("frame after %v/%v moved scroll from %d to %d", vertical, horizontal, want, c.scroll)
				}
			}
		}
	}
}

func TestScrolledAnswerStaysPutWhenContentHeightChanges(t *testing.T) {
	m := longAnswerScrollModel()
	c := m.host
	c.scroll, c.scrollOnly = 80, true
	m.View()
	read := func() string { return c.shown[c.rowBody[c.bodyTop+2]].Text }
	want := read()
	total := c.shownTotal
	for _, queue := range [][]string{{"queued message one", "queued message two"}, nil} {
		c.sess.Info.Queue = queue
		c.scrollOnly = false
		m.View()
		if c.shownTotal == total {
			t.Fatal("fixture did not change content height")
		}
		if got := read(); got != want {
			t.Fatalf("content height %d -> %d moved the visible answer; anchor %+v", total, c.shownTotal, c.top)
		}
		total = c.shownTotal
	}
}

func TestScrollStaysOnRowsAsEarlierTurnHeightsAreMeasured(t *testing.T) {
	m, _ := benchModel(200, 60)
	c := m.host
	s := convo.New()
	for turn := range 20 {
		s.Apply(host.Sent{Text: fmt.Sprintf("Question %d", turn)}, time.Now())
		var answer strings.Builder
		for line := range 5 + turn*5 {
			fmt.Fprintf(&answer, "Answer %d, paragraph %d.\n\n", turn, line)
		}
		s.Apply(headless.Message{Role: "assistant", ID: fmt.Sprint(turn), Blocks: []headless.Block{{Type: "text", Text: answer.String()}}}, time.Now())
		s.Apply(headless.Result{Subtype: "success"}, time.Now())
	}
	c.sess, c.shown, c.bodyBuf, c.scroll = s, nil, nil, 0
	c.historyMode = convo.HistoryOpen
	c.scrollOnly = false
	m.View()
	c.scroll, c.scrollOnly = 30, true
	m.View()
	heightsChanged := 0
	for n := range 500 {
		row := c.rowBody[c.bodyTop+2]
		if row < 3 || c.scrollCap-c.scroll < 3 {
			break
		}
		want := c.shown[row-3].Text
		total := c.shownTotal
		c.scroll += 3
		c.scrollOnly = true
		m.View()
		if c.shownTotal != total {
			heightsChanged++
		}
		if got := c.shown[c.rowBody[c.bodyTop+2]].Text; got != want {
			t.Fatalf("step %d jumped while total changed %d -> %d", n, total, c.shownTotal)
		}
		c.scrollOnly = false
		m.View()
		if c.shownTotal != total {
			heightsChanged++
		}
		if got := c.shown[c.rowBody[c.bodyTop+2]].Text; got != want {
			t.Fatalf("step %d snapped back on redraw", n)
		}
	}
	if heightsChanged == 0 {
		t.Fatal("fixture never measured an earlier turn")
	}
}
