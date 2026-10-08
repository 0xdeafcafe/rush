package ui

import (
	"encoding/json/jsontext"
	"fmt"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func benchJSON(v any) jsontext.Value { b, _ := jsonx.Marshal(v); return b }

// benchConvo is a long rush-mode session with a live turn streaming.
func benchConvo(turns int) *convo.Session {
	s := convo.New()
	s.Info.Cwd = "/work/rush"
	t0 := time.Now().Add(-time.Hour)
	sec := 0
	apply := func(ev any) { s.Apply(ev, t0.Add(time.Duration(sec)*time.Second)); sec++ }
	use := func(id, name string, in any) {
		apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: id, Name: name, Input: benchJSON(in)}}})
	}
	res := func(id, text string, isErr bool, structured any) {
		m := headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: id, Text: text, IsError: isErr}}}
		if structured != nil {
			m.ToolResult = benchJSON(structured)
		}
		apply(m)
	}
	var out strings.Builder
	for i := 0; i < 30; i++ {
		fmt.Fprintf(&out, "ok  \tgithub.com/x/y/pkg%d\t0.%03ds\n", i, i*7)
	}
	answer := "## What changed\n\nThe **renderer** now caches each turn, so `Render` only redraws what moved.\n\n- one\n- two\n\nThat should make streaming feel instant on long sessions."
	for t := 1; t <= turns; t++ {
		id := func(k string) string { return fmt.Sprintf("%s%d", k, t) }
		apply(host.Sent{Text: fmt.Sprintf("turn %d: fix the @internal/convo/render.go wrap then /review", t)})
		apply(headless.Message{Role: "assistant", ID: id("m"), Model: "claude-opus-5-5", Usage: &headless.Usage{InputTokens: 10, OutputTokens: 200, CacheReadInputTokens: 50000},
			Blocks: []headless.Block{{Type: "text", Text: "Looking at how the **pane** draws its `rows`."}}})
		use(id("r"), "Read", map[string]any{"file_path": "/work/rush/internal/ui/view.go"})
		res(id("r"), "…", false, map[string]any{"type": "text", "file": map[string]any{"numLines": 400, "startLine": 1, "totalLines": 1589}})
		use(id("b"), "Bash", map[string]any{"command": "go test ./...", "description": "Run the tests"})
		res(id("b"), out.String(), false, map[string]any{"stdout": out.String(), "stderr": ""})
		use(id("e"), "Edit", map[string]any{"file_path": "/work/rush/internal/convo/render.go"})
		res(id("e"), "ok", false, map[string]any{"structuredPatch": []map[string]any{{"oldStart": 60, "oldLines": 1, "newStart": 60, "newLines": 2,
			"lines": []string{"-\t\tout = append(out, s.turn(t, o)...)", "+\t\tls := s.turn(t, o)", "+\t\tout = append(out, ls...)"}}}})
		apply(headless.Message{Role: "assistant", ID: id("a"), Blocks: []headless.Block{{Type: "text", Text: answer}}})
		apply(headless.Result{Subtype: "success", CostUSD: 0.42})
	}
	apply(host.Sent{Text: "now make streaming quicker"})
	for _, w := range strings.Fields(answer) {
		apply(headless.Delta{Text: w + " "})
	}
	return s
}

// benchModel is rush at w×h with 30 agents listed and a rush-mode
// session open beside them, its conversation streaming.
func benchModel(w, h int) (*Model, chan []byte) {
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: w, h: h, lastState: map[string]string{}}
	now := time.Now()
	snap := &fleet.Snapshot{At: now}
	for i := 0; i < 30; i++ {
		a := &fleet.Agent{Key: fmt.Sprintf("default/a:%d", i), DisplayName: fmt.Sprintf("agent number %d doing things", i), Acct: claude.DefaultAccount().Profile()}
		a.ID = fmt.Sprintf("%08x", i)
		a.Cwd, a.Repo, a.Branch = "/work/rush", "/work/rush", "main"
		a.UpdatedAt = now.Add(-time.Duration(i) * time.Minute)
		a.Detail = "Looking at how the pane draws its rows and where the wrap happens"
		switch i % 3 {
		case 0:
			a.State, a.PID = "working", 100+i
		case 1:
			a.State = "done"
		default:
			a.State, a.PID, a.Needs = "blocked", 100+i, "which way?"
		}
		snap.Agents = append(snap.Agents, a)
	}
	sel := snap.Agents[0]
	sel.Rush = true
	m.snap = snap
	m.rebuild()
	m.sel, m.preview, m.paneFocus = sel.Key, true, true
	lines := make(chan []byte, 16)
	m.host = &hostConn{kind: "claude", key: sel.Key, id: sel.ID, client: &host.Client{Lines: lines}, sess: benchConvo(300), open: map[string]bool{}, ready: true}
	m.host.input = []rune("a message I am halfway through typing, long enough to wrap onto a second row of the box")
	m.View()
	return m, lines
}

var deltaLine = []byte(`{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"more words "}}}`)

func BenchmarkView(b *testing.B) {
	agent.NeverWait() // as cmd/rush does, or the header stats every agent's home each frame
	for _, sz := range [][2]int{{120, 40}, {250, 70}} {
		b.Run(fmt.Sprintf("%dx%d", sz[0], sz[1]), func(b *testing.B) {
			m, _ := benchModel(sz[0], sz[1])
			b.ReportAllocs()
			for b.Loop() {
				m.View()
			}
		})
		// One batch of streamed deltas through Update, then the frame.
		b.Run(fmt.Sprintf("%dx%d/stream", sz[0], sz[1]), func(b *testing.B) {
			m, _ := benchModel(sz[0], sz[1])
			batch := [][]byte{deltaLine, deltaLine, deltaLine}
			b.ReportAllocs()
			for b.Loop() {
				if l := m.host.sess.Live(); l != nil && len(l.Items) > 0 && len(l.Items[len(l.Items)-1].Text) > 4<<10 {
					l.Items[len(l.Items)-1].Text = ""
				}
				m.Update(hostLinesMsg{key: m.host.key, lines: batch})
				m.View()
			}
		})
		// A key typed into the Session's box, then the frame.
		b.Run(fmt.Sprintf("%dx%d/key", sz[0], sz[1]), func(b *testing.B) {
			m, _ := benchModel(sz[0], sz[1])
			b.ReportAllocs()
			for b.Loop() {
				m.host.input = m.host.input[:80]
				m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
				m.View()
			}
		})
	}
}

func BenchmarkBox(b *testing.B) {
	bx := box{w: 120, focused: true, topL: dim("to ") + paint(cText, "agent"), text: []rune(strings.Repeat("some words to type ", 30)), cursor: 200, anchor: -1,
		lead: paint(cOrange, "❯ "), holder: "a message", maxRows: 6}
	b.ReportAllocs()
	for b.Loop() {
		bx.lines()
	}
}

func BenchmarkHelpers(b *testing.B) {
	s := paint(cSub, "Looking at how the ") + paint(cText+bold, "pane") + paint(cSub, " draws its rows and where the wrap happens; see render.go for the rest.")
	b.Run("fit", func(b *testing.B) {
		b.ReportAllocs()
		for b.Loop() {
			fit(s, 200)
			fit(s, 40)
		}
	})
	b.Run("wrap", func(b *testing.B) {
		b.ReportAllocs()
		for b.Loop() {
			wrap(s, 40)
		}
	})
	b.Run("oneLine", func(b *testing.B) {
		b.ReportAllocs()
		for b.Loop() {
			oneLine("Looking at how the pane\ndraws its rows\tand where the wrap happens")
		}
	})
	b.Run("header", func(b *testing.B) {
		m, _ := benchModel(250, 70)
		b.ReportAllocs()
		for b.Loop() {
			m.header()
		}
	})
}
