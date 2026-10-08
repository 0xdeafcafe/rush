package ui

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// asked is Claude's AskUserQuestion request as the question it asks.
func asked(r headless.PermissionRequest) *event.Question {
	var n headless.Neutral
	q := n.Event(r)[0].(event.Question)
	return &q
}

func askReq() *event.Question {
	in := map[string]any{"questions": []map[string]any{
		{"question": "Which rules first?", "header": "Lint", "options": []map[string]any{{"label": "no-floating-promises"}, {"label": "explicit return types"}}},
		{"question": "Which packages?", "multiSelect": true, "options": []map[string]any{{"label": "mcp"}, {"label": "skills"}, {"label": "web"}}},
	}}
	b, _ := jsonx.Marshal(in)
	return asked(headless.PermissionRequest{ID: "q1", Tool: "AskUserQuestion", Input: b})
}

func TestAnswerQuestions(t *testing.T) {
	m := &Model{}
	c := &hostConn{}
	req := askReq()
	// A digit answers a single-choice question and moves to the next.
	if _, used := m.questionKey(c, req, "1", true); !used || c.qIdx != 1 {
		t.Fatalf("first answer: used=%v idx=%d", used, c.qIdx)
	}
	// Multi-select: digits toggle, enter confirms and replies.
	m.questionKey(c, req, "1", true)
	m.questionKey(c, req, "3", true)
	m.questionKey(c, req, "3", true)
	m.questionKey(c, req, "2", true)
	if cmd, _ := m.questionKey(c, req, "enter", true); cmd != nil || c.qIdx != 2 {
		t.Fatalf("with two questions, the last answer goes to the review, not straight out: idx=%d", c.qIdx)
	}
	want := map[string]string{"Which rules first?": "no-floating-promises", "Which packages?": "mcp, skills"}
	for k, v := range want {
		if c.qAnswer[k] != v {
			t.Errorf("%q: got %q want %q", k, c.qAnswer[k], v)
		}
	}
	// From the review, a digit goes back to a question to change it; the
	// cursor sits on what was chosen, and answering returns to the review.
	c.cardFocus = true
	m.questionKey(c, req, "1", true)
	if c.qIdx != 0 || c.qCursor != 0 {
		t.Fatalf("back to question 1: idx=%d cursor=%d", c.qIdx, c.qCursor)
	}
	m.questionKey(c, req, "down", true)
	m.questionKey(c, req, "enter", true)
	if c.qIdx != 2 || c.qAnswer["Which rules first?"] != "explicit return types" {
		t.Fatalf("changed answer: idx=%d %v", c.qIdx, c.qAnswer)
	}
	// ← from the review is the last question, with its ticks kept.
	m.questionKey(c, req, "left", true)
	if c.qIdx != 1 || !c.picks(1)[0] || !c.picks(1)[1] || c.picks(1)[2] {
		t.Fatalf("ticks kept: idx=%d %v", c.qIdx, c.qPicks)
	}
	m.questionKey(c, req, "right", true)
	if cmd, used := m.questionKey(c, req, "enter", true); !used || cmd == nil || c.qFor != "" {
		t.Fatal("enter on the review should reply")
	}
	// A lone question sends as soon as it's answered.
	one, _ := jsonx.Marshal(map[string]any{"questions": []map[string]any{{"question": "Go?", "options": []map[string]any{{"label": "yes"}, {"label": "no"}}}}})
	c1 := &hostConn{}
	if cmd, _ := m.questionKey(c1, asked(headless.PermissionRequest{ID: "q2", Tool: "AskUserQuestion", Input: one}), "2", true); cmd == nil {
		t.Fatal("a single question should reply on its answer")
	}
	// Typed text answers in your own words.
	c2 := &hostConn{input: []rune("both, but start with promises")}
	m.questionKey(c2, req, "enter", false)
	if c2.qAnswer["Which rules first?"] != "both, but start with promises" || len(c2.input) != 0 {
		t.Errorf("typed answer: %v", c2.qAnswer)
	}
	// Letters still type while a question is up.
	if _, used := m.questionKey(c2, req, "a", true); used {
		t.Error("a letter shouldn't be taken by the question card")
	}
}

// On a multi-select question, enter on an option only ticks it; the
// Continue button under the options is what moves on.
func TestMultiSelectContinue(t *testing.T) {
	m := &Model{}
	c := &hostConn{cardFocus: true}
	req := askReq()
	m.questionKey(c, req, "1", true) // answer the first, on to the multi-select
	m.questionKey(c, req, "enter", true)
	m.questionKey(c, req, "enter", true)
	if c.qIdx != 1 || c.picks(1)[0] {
		t.Fatalf("enter twice on an option should tick then untick it: idx=%d %v", c.qIdx, c.qPicks)
	}
	m.questionKey(c, req, "enter", true)
	for range 4 { // past the options and "Something else", onto Continue
		m.questionKey(c, req, "down", true)
	}
	if c.qCursor != 4 {
		t.Fatalf("cursor should reach Continue: %d", c.qCursor)
	}
	if _, used := m.questionKey(c, req, "down", true); used {
		t.Error("↓ past Continue should leave the card")
	}
	lines := drawQuestion(c, "", mustQs(req), 100, 0, 0)
	if !strings.Contains(ansi.Strip(strings.Join(lines, "\n")), "Continue") {
		t.Error("the multi-select card should draw a Continue button")
	}
	m.questionKey(c, req, "enter", true)
	if c.qIdx != 2 || c.qAnswer["Which packages?"] != "mcp" {
		t.Fatalf("Continue should confirm the ticks: idx=%d %v", c.qIdx, c.qAnswer)
	}
}

func mustQs(req *event.Question) []question {
	_, qs := questions(req)
	return qs
}

// With the box empty, y, a and n answer a permission card; once a message
// is under way they type, and enter denies with it as the reason. A digit
// or a bare enter never answers it without focus.
func TestCardsNeedFocus(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	newConn := func() *hostConn {
		c := &hostConn{kind: "claude", sess: convo.New()}
		c.sess.Apply(host.Sent{Text: "go"}, time.Now())
		c.sess.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "Bash", Input: jsontext.Value(`{"command":"ls"}`)}}}, time.Now())
		c.sess.Apply(headless.PermissionRequest{ID: "r1", Tool: "Bash", ToolUseID: "b1"}, time.Now())
		return c
	}
	for _, k := range []string{"enter", "1"} {
		if _, used := m.cardKey(newConn(), k, true); used {
			t.Errorf("%q answered the card without focus", k)
		}
	}
	for _, k := range []string{"y", "a", "n"} {
		if _, used := m.cardKey(newConn(), k, true); !used {
			t.Errorf("%q from an empty box didn't answer the card", k)
		}
		if _, used := m.cardKey(newConn(), k, false); used {
			t.Errorf("%q mid-message answered the card", k)
		}
	}
	c := newConn()
	c.input = []rune("use rg instead")
	if _, used := m.cardKey(c, "enter", false); !used || len(c.input) != 0 {
		t.Fatal("enter with a message should deny with it")
	}
	c = newConn()
	if _, used := m.cardKey(c, "up", true); !used || !c.cardFocus {
		t.Fatal("↑ from an empty box should focus the card")
	}
	if _, used := m.cardKey(c, "esc", true); !used || c.cardFocus {
		t.Fatal("esc should hand the keys back to the box")
	}
	_ = tea.KeyPressMsg{}
}

// A transcript-backed Session that finishes loading after you've moved on
// has no host client; discarding it must not crash.
func TestStaleTranscriptOpen(t *testing.T) {
	m := &Model{hostOpening: "acct/other"}
	m.onHostOpen(hostOpenMsg{key: "acct/old", c: &hostConn{key: "acct/old"}})
	m.hostOpening = "acct/old"
	m.dropHost()
}

func TestZenQueue(t *testing.T) {
	now := time.Now()
	ag := func(key string, age time.Duration, blocked bool) *fleet.Agent {
		a := &fleet.Agent{Key: key, PID: 1}
		a.UpdatedAt = now.Add(-age)
		if blocked {
			a.State = "blocked"
		} else {
			a.State = "working"
		}
		return a
	}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{
		ag("new", time.Minute, true), ag("busy", time.Hour, false), ag("old", 10*time.Minute, true),
	}}, zen: true}
	m.zenPick()
	if m.sel != "old" || !m.paneFocus {
		t.Fatalf("zen should start on the oldest waiting agent, got %q", m.sel)
	}
	m.zenSkip()
	if m.sel != "new" {
		t.Fatalf("ctrl+n should skip to the next, got %q", m.sel)
	}
	// Once answered, it moves on by itself.
	m.snap.Agents[0].State = "working"
	m.zenPick()
	if m.sel != "old" {
		t.Fatalf("an answered agent should give way, got %q", m.sel)
	}
}

func TestSlashQueueTasks(t *testing.T) {
	c := &hostConn{kind: "claude", sess: convo.New(), open: map[string]bool{}}
	c.sess.Commands = []event.Command{{Name: "compact", Description: "summarise"}, {Name: "context"}, {Name: "code-review"}}
	c.input = []rune("/co")
	names := func() (out []string) {
		for _, x := range slashMatches(c) {
			out = append(out, x.Name)
		}
		return
	}
	// rush's and Claude Code's screens first, then the session's, then
	// claude: ones that only have it inside.
	if got := names(); len(got) < 5 || got[0] != "copy" || got[1] != "context" || got[2] != "config" || got[3] != "compact" || !strings.HasPrefix(got[len(got)-1], "claude:") {
		t.Fatalf("matches for /co: %v", got)
	}
	c.input = []rune("/clea")
	if got := names(); len(got) != 1 || got[0] != "clear" {
		t.Fatalf("rush's /clear should match, and #clean shouldn't: %v", got)
	}
	c.input = []rune("/compact now")
	if len(slashMatches(c)) != 0 {
		t.Fatal("the picker closes once arguments start")
	}

	// A transcript-backed session can't take /model; rush says so rather
	// than sending it to Claude.
	m := &Model{snap: &fleet.Snapshot{}}
	if _, ok := m.runRushCommand(c, "/model haiku"); !ok || m.status == "" {
		t.Fatal("/model should be handled by rush")
	}
	if _, ok := m.runRushCommand(c, "/compact"); ok {
		t.Fatal("/compact belongs to Claude Code")
	}

	// Space on a queued message pulls it into the empty box for editing, and
	// holds the queue until it's saved.
	c.input = nil // the command above has been handled
	m.queueLocal("", "first")
	m.queueLocal("", "second")
	c.sel = "q:1"
	if _, used := m.queueKey(c, "space"); !used || string(c.input) != "second" || c.editQ != 2 || c.sel != "" {
		t.Fatalf("edit queued: used=%v input=%q editQ=%d", used, string(c.input), c.editQ)
	}
	if !m.localQ[""].held {
		t.Fatal("editing should hold the queue")
	}
	c.input = []rune("second, better")
	m.sendPane(c, false)
	if q := m.localQ[""]; q.held || q.items[1] != "second, better" || c.editQ != 0 {
		t.Fatalf("after saving: held=%v items=%q", q.held, q.items)
	}

	// Tasks group into now, next and done.
	c.sess.Tasks = []convo.Task{{Subject: "a", Status: "completed"}, {Subject: "b", Active: "Doing b", Status: "in_progress"}, {Subject: "c", Status: "pending"}}
	var out string
	for _, l := range m.taskLines(c, convo.Options{Width: 80}) {
		out += ansi.Strip(l.Text) + "\n"
	}
	for _, want := range []string{"1 of 3 done", "Now  1", "■ Doing b", "Next  1", "Done  1", "✓ a"} {
		if !strings.Contains(out, want) {
			t.Errorf("tasks view missing %q:\n%s", want, out)
		}
	}
}

func TestClaudeScreensAndFork(t *testing.T) {
	a := &fleet.Agent{Key: "k", ID: "abc", DisplayName: "fixer", Cwd: "/tmp"}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}

	// Screens headless Claude Code can't show open Claude Code's own, or
	// rush's take on them.
	for _, cmd := range []string{"/hooks", "/mcp"} {
		if run, ok := m.runRushCommand(c, cmd); !ok || run == nil {
			t.Errorf("%s should open Claude Code's screen", cmd)
		}
	}
	for _, cmd := range []string{"/plugins", "/plugin"} {
		m.sheet = nil
		if _, ok := m.runRushCommand(c, cmd); !ok {
			t.Errorf("%s should be rush's", cmd)
		}
		if _, is := m.sheet.(*pluginSheet); !is {
			t.Errorf("%s should open the plugins sheet, got %T", cmd, m.sheet)
		}
	}
	m.sheet = nil
	if _, ok := m.runRushCommand(c, "/statusline"); !ok {
		t.Fatal("/statusline should be rush's")
	}
	if _, is := m.sheet.(*statusSheet); !is {
		t.Fatalf("/statusline should open the builder, got %T", m.sheet)
	}
	m.sheet = nil
	// With arguments, /mcp and /config are Claude's to run.
	if _, ok := m.runRushCommand(c, "/mcp enable x"); ok {
		t.Error("/mcp with arguments should go to Claude")
	}
	c.input = []rune("/plug")
	if got := slashMatches(c); len(got) == 0 || got[0].Name != "plugin" {
		t.Fatalf("the picker should offer /plugin: %v", got)
	}

	// Nothing to fork before the first turn.
	if _, ok := m.runRushCommand(c, "/fork"); !ok || m.sheet != nil || m.status == "" {
		t.Fatalf("/fork with no turns: ok=%v status=%q", ok, m.status)
	}
	c.sess.Turns = append(c.sess.Turns, &convo.Turn{N: 1, Prompt: "first"}, &convo.Turn{N: 2, Prompt: "second"}, &convo.Turn{N: 3, Prompt: "third"})
	c.sess.Info.SessionID = "0123456789abcdef"
	if _, ok := m.runRushCommand(c, "/fork try another way"); !ok {
		t.Fatal("/fork should be rush's")
	}
	f, is := m.sheet.(*forkSheet)
	if !is || string(f.name) != "try another way" {
		t.Fatalf("/fork should open its sheet, named: %T", m.sheet)
	}
	// ↓ to Remembers, ← goes back to the oldest choice: up to turn 1.
	f.key(m, tea.KeyPressMsg{}, "down")
	f.key(m, tea.KeyPressMsg{}, "left")
	body := ansi.Strip(strings.Join(f.body(m, 100, 40), "\n"))
	if !strings.Contains(body, "up to turn 1") || !strings.Contains(body, "forgets 2–3") {
		t.Fatalf("fork sheet:\n%s", body)
	}
	f.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil {
		t.Fatal("esc closes the sheet")
	}
}

func TestSlashMidMessage(t *testing.T) {
	c := &hostConn{kind: "claude", sess: convo.New(), open: map[string]bool{}}
	c.local = []event.Command{{Name: "design:design-critique"}, {Name: "pdf"}}
	c.input = []rune("please /crit this")
	c.back = len(" this")
	got := slashMatches(c)
	if len(got) != 1 || got[0].Name != "design:design-critique" {
		t.Fatalf("mid-message matches: %v", got)
	}
	m := &Model{snap: &fleet.Snapshot{}}
	if _, ok := m.slashKey(c, "enter"); !ok {
		t.Fatal("enter should complete")
	}
	if string(c.input) != "please /design:design-critique this" {
		t.Fatalf("completed to %q", string(c.input))
	}
	c.input, c.back = []rune("see /var/folders/x"), 0
	if slashMatches(c) != nil {
		t.Fatal("a path is not a command")
	}
	c.input = []rune("/cl")
	for _, x := range slashMatches(c) {
		if x.Name == "clear" {
			return
		}
	}
	t.Fatal("rush's own commands still show at the start")
}

func TestLocalQueue(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.queueLocal("k", "one")
	m.queueLocal("k", "two")
	if q := m.queueOf(c); !q.local || len(q.items) != 2 {
		t.Fatalf("queue = %+v", q)
	}
	c.sel = "q:0"
	if _, ok := m.queueKey(c, "alt+m"); !ok || m.localQ["k"].items[0] != "one\n\ntwo" {
		t.Fatalf("merge: %q", m.localQ["k"].items)
	}
	m.editLocal(c, 0, "one\n\ntwo", "edited")
	if m.localQ["k"].items[0] != "edited" {
		t.Fatal("edit didn't save")
	}
	if got := withImages("look", []string{"/a.png"}); got != "look\n[image: /a.png]" {
		t.Fatalf("withImages = %q", got)
	}
}

// shift+↑↓ merges the picked message into the one above or below, in
// order; [ and ] move it.
func TestQueueMergeUpDown(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	for _, x := range []string{"a", "b", "c", "d"} {
		m.queueLocal("k", x)
	}
	c.sel = "q:1"
	m.queueKey(c, "shift+up")
	if got := m.localQ["k"].items; strings.Join(got, "|") != "a\n\nb|c|d" || c.sel != "q:0" {
		t.Fatalf("merge up: %q, picked %s", got, c.sel)
	}
	c.sel = "q:1"
	m.queueKey(c, "shift+down")
	if got := m.localQ["k"].items; strings.Join(got, "|") != "a\n\nb|c\n\nd" || c.sel != "q:1" {
		t.Fatalf("merge down: %q, picked %s", got, c.sel)
	}
	m.queueKey(c, "[")
	if got := m.localQ["k"].items; got[0] != "c\n\nd" || c.sel != "q:0" {
		t.Fatalf("move: %q, picked %s", got, c.sel)
	}
}

func TestArgPicker(t *testing.T) {
	c := &hostConn{kind: "claude", sess: convo.New(), open: map[string]bool{}}
	c.sess.Info.Model = "claude-sonnet-5"
	c.input = []rune("/model ")
	got := argMatches(c)
	if len(got) != 6 {
		t.Fatalf("all models offered: %v", got)
	}
	for _, g := range got {
		if g.Name == "model sonnet" && !strings.Contains(g.Description, "now") {
			t.Fatal("current model not marked")
		}
	}
	c.input = []rune("/effort x")
	if got := argMatches(c); len(got) != 1 || got[0].Name != "effort xhigh" {
		t.Fatalf("effort x: %v", got)
	}
	c.input = []rune("/model")
	if argMatches(c) != nil {
		t.Fatal("no argument picker before the space")
	}
}

func TestArtifacts(t *testing.T) {
	s := convo.New()
	now := time.Now()
	pub := func(id, ver string) {
		in, _ := jsonx.Marshal(map[string]string{"file_path": "/tmp/rush-mode.html", "description": "design review"})
		s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: id, Name: "Artifact", Input: in}}}, now)
		s.Apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: id,
			Text: "Published /tmp/rush-mode.html at https://claude.ai/artifact/2pdtkfBi4he8cVWra7qYq6 (Version " + ver + ")"}}}, now)
	}
	pub("a1", "1")
	pub("a2", "2")
	arts := artifacts(s)
	if len(arts) != 1 || arts[0].Version != 2 || arts[0].Versions != 2 || arts[0].About != "design review" {
		t.Fatalf("artifacts = %+v", arts)
	}
}

func TestLocalQueueSends(t *testing.T) {
	a := &fleet.Agent{Key: "k"}
	a.State = "working"
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}}
	m.queueLocal("k", "hi")
	if m.flushLocalQueues() != nil {
		t.Fatal("sent straight away while it works; should gather for a moment")
	}
	m.localQ["k"].since = time.Now().Add(-20 * time.Second)
	if m.flushLocalQueues() == nil || len(m.localQ["k"].items) != 0 {
		t.Fatal("a busy agent never got its queue")
	}
	a.State = "blocked"
	m.queueLocal("k", "later")
	m.localQ["k"].since = time.Now().Add(-time.Minute)
	if m.flushLocalQueues() != nil {
		t.Fatal("sent while it waits on you")
	}
}

func TestQuestionCardDraws(t *testing.T) {
	in := map[string]any{"questions": []map[string]any{
		{"question": "Which layout?", "header": "Layout", "options": []map[string]any{
			{"label": "Split (Recommended)", "description": "Agents left, Session right", "preview": "```\n┌────┬────────┐\n│ A  │ S      │\n└────┴────────┘\n```"},
			{"label": "Stacked", "preview": "┌────────┐\n│ A      │\n├────────┤\n│ S      │\n└────────┘"},
		}},
		{"question": "Theme?", "header": "Theme", "options": []map[string]any{{"label": "dark"}, {"label": "light"}}},
	}}
	b, _ := jsonx.Marshal(in)
	req := asked(headless.PermissionRequest{ID: "q9", Tool: "AskUserQuestion", Input: b})
	m := &Model{}
	c := &hostConn{cardFocus: true}
	draw := func(w int) string {
		var sb strings.Builder
		for _, l := range m.questionCard(c, req, w, 0) {
			if cellw.String(l) > w {
				t.Fatalf("row wider than %d: %q", w, ansi.Strip(l))
			}
			sb.WriteString(ansi.Strip(l) + "\n")
		}
		return sb.String()
	}
	wide := draw(120)
	for _, want := range []string{" Layout ", "○ Theme", "○ send", "0 of 2 answered", "1  Split  ★ recommended", "╭─ Split ─", "│ A  │ S      │", "←→ questions"} {
		if !strings.Contains(wide, want) {
			t.Fatalf("wide card missing %q:\n%s", want, wide)
		}
	}
	// Side by side: the option and the preview share a row.
	sideBySide := false
	for _, l := range strings.Split(wide, "\n") {
		if strings.Contains(l, "Split  ★ recommended") && strings.Contains(l, "╭─ Split") {
			sideBySide = true
		}
	}
	if !sideBySide || strings.Contains(wide, "```") {
		t.Fatalf("preview beside the options, fences dropped:\n%s", wide)
	}
	// Narrow: the preview goes under, and the option says it has one.
	narrow := draw(70)
	if !strings.Contains(narrow, "◇ preview") || !strings.Contains(narrow, "│ A  │ S      │") {
		t.Fatalf("narrow card:\n%s", narrow)
	}
	// Answered, the strip shows the answer and the review lists them.
	m.questionKey(c, req, "1", true)
	m.questionKey(c, req, "2", true)
	review := draw(120)
	for _, want := range []string{"✓ Layout Split", " send ", "Send these answers?", "Layout   ✓ Split", "Theme    ✓ light", "sends your answers"} {
		if !strings.Contains(review, want) {
			t.Fatalf("review missing %q:\n%s", want, review)
		}
	}
}

// A question that explains first and asks last shows the ask apart.
func TestSplitAsk(t *testing.T) {
	for _, tc := range []struct{ in, lead, ask string }{
		{"Which layout?", "", "Which layout?"},
		{"The check blocked this. Proceed?", "The check blocked this.", "Proceed?"},
		{"Context here.\n\nWhich one, e.g. this?", "Context here.", "Which one, e.g. this?"},
		{"Use e.g. foo?", "", "Use e.g. foo?"},
		{"Pick one. Done.", "", "Pick one. Done."},
	} {
		if lead, ask := splitAsk(tc.in); lead != tc.lead || ask != tc.ask {
			t.Errorf("%q: got %q / %q", tc.in, lead, ask)
		}
	}
}

// A long question keeps to its height by folding what isn't under the
// cursor, and a description is never cut while there's room.
func TestQuestionCardFolds(t *testing.T) {
	long := strings.Repeat("a reason that goes on ", 20)
	in := map[string]any{"questions": []map[string]any{{"question": "Go?", "options": []map[string]any{
		{"label": "one", "description": long}, {"label": "two", "description": long}, {"label": "three", "description": long},
	}}}}
	b, _ := jsonx.Marshal(in)
	req := asked(headless.PermissionRequest{ID: "q5", Tool: "AskUserQuestion", Input: b})
	m := &Model{}
	c := &hostConn{cardFocus: true}
	full := m.questionCard(c, req, 100, 0)
	if s := ansi.Strip(strings.Join(full, "\n")); strings.Contains(s, "…") {
		t.Fatalf("cut with room to spare:\n%s", s)
	}
	folded := m.questionCard(c, req, 100, 16)
	if len(folded) > 16 || len(folded) >= len(full) {
		t.Fatalf("folded to %d rows of %d", len(folded), len(full))
	}
}

func TestAnswersCarryPreview(t *testing.T) {
	in := map[string]any{"questions": []map[string]any{
		{"question": "Which?", "options": []map[string]any{{"label": "a", "preview": "A!"}, {"label": "b"}}},
		{"question": "And?", "options": []map[string]any{{"label": "x"}}},
	}}
	b, _ := jsonx.Marshal(in)
	req := asked(headless.PermissionRequest{ID: "q3", Tool: "AskUserQuestion", Input: b})
	_, qs := questions(req)
	var got struct {
		Questions   []any                        `json:"questions"`
		Answers     map[string]string            `json:"answers"`
		Annotations map[string]map[string]string `json:"annotations"`
	}
	_ = jsonx.Unmarshal(answerInput(req, qs, map[string]string{"Which?": "a", "And?": "my own"}), &got)
	if len(got.Questions) != 2 || got.Answers["Which?"] != "a" || got.Answers["And?"] != "my own" {
		t.Fatalf("answers: %+v", got)
	}
	if got.Annotations["Which?"]["preview"] != "A!" || got.Annotations["And?"] != nil {
		t.Fatalf("annotations: %+v", got.Annotations)
	}
}

// rush's commands take #: the Prompt's picker offers them as they're
// typed, then a command's choices after a space; enter runs one or
// completes it. A heading or an issue number is still a message.
func TestFleetSlash(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.paneFocus = false
	m.input = []rune("#vie")
	got, lead := m.promptPicker()
	if lead != "#" || len(got) == 0 || got[0].Name != "view" {
		t.Fatalf("#vie offers %v", got)
	}
	if _, ok := m.fleetSlashKey("enter"); !ok || string(m.input) != "#view " {
		t.Fatalf("enter on #view, which needs an argument, left %q", string(m.input))
	}
	m.input = []rune("#view ")
	if got, _ := m.promptPicker(); len(got) != 3 || got[0].Name != "view split" || got[1].Name != "view agent" {
		t.Fatalf("#view offers %v", got)
	}
	m.fleetSlashKey("down")
	m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	if string(m.input) != "#view agent" {
		t.Fatalf("tab left %q", string(m.input))
	}
	for _, in := range []string{"#nothing", "# Plan", "#123 is broken", "hello #so"} {
		m.input = []rune(in)
		if got, _ := m.promptPicker(); got != nil {
			t.Fatalf("%q offers %v", in, got)
		}
	}
	if isHashCmd("# Plan") || isHashCmd("#123") || !isHashCmd("#stop") {
		t.Fatal("a # command is # and a letter")
	}
	m.input, m.paneFocus = []rune("#so"), true
	if got, _ := m.promptPicker(); got != nil {
		t.Fatal("no Prompt picker while the Session has the keys")
	}

	// In the Session's box # offers the same commands, for its agent.
	c := m.host
	c.input, c.back = []rune("#pi"), 0
	if got := m.hashMatches(c.input, c.back); len(got) != 1 || got[0].Name != "pin" {
		t.Fatalf("the Session's #pi offers %v", got)
	}
	if l := m.slashLines(c, 80); len(l) != 2 || !strings.Contains(ansi.Strip(l[0]), "#pin") {
		t.Fatalf("the Session's picker draws %q", l)
	}
}

// A message for a job Claude Code has let go of carries the conversation on
// in rush mode instead of failing.
func TestJobGoneMovesToRush(t *testing.T) {
	bin := t.TempDir() // Claude Code is installed
	os.WriteFile(filepath.Join(bin, "claude"), []byte("#!/bin/sh\n"), 0o755)
	t.Setenv("PATH", bin)
	agent.Recheck()
	t.Cleanup(agent.Recheck)
	a := &fleet.Agent{Key: "acct/gone", DisplayName: "gone", Kind: "claude"}
	a.SessionID, a.State = "sess", "done"
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}}
	m.order = m.snap.Agents
	_, cmd := m.update(jobGoneMsg{key: a.Key, text: "carry on"})
	if cmd == nil || !strings.HasPrefix(m.status, "moving gone") {
		t.Fatalf("expected a move to rush mode, got status %q", m.status)
	}
}

func TestQueueKeys(t *testing.T) {
	a := &fleet.Agent{Key: "k"}
	a.State = "working"
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.host = c
	for _, s := range []string{"a", "b", "c", "d"} {
		m.queueLocal("k", s)
	}
	items := func() string { return strings.Join(m.localQ["k"].items, ",") }
	key := func(s string) tea.Cmd {
		k := tea.KeyPressMsg{}
		if len(s) == 1 {
			k.Text = s
		}
		return m.paneKey(k, s)
	}

	// ↑ from the empty box picks the last queued message, in the dock.
	key("up")
	if c.sel != "q:3" {
		t.Fatalf("↑ picked %q", c.sel)
	}
	// Moving follows the message, however fast the keys come.
	key("up")
	key("up")
	key("up")
	key("]")
	key("]")
	if items() != "b,c,a,d" || c.sel != "q:2" {
		t.Fatalf("moved to %s, sel %s", items(), c.sel)
	}
	// Plain keys: ⌫ drops, the pick staying on the next one; h holds.
	key("backspace")
	if items() != "b,c,d" || c.sel != "q:2" {
		t.Fatalf("dropped: %s, sel %s", items(), c.sel)
	}
	key("h")
	if !m.localQ["k"].held || len(c.input) != 0 {
		t.Fatal("h should hold, not type")
	}
	// A key it doesn't use goes back to typing.
	key("w")
	if c.sel != "" || string(c.input) != "w" {
		t.Fatalf("typing: sel %q input %q", c.sel, string(c.input))
	}
	// ctrl+enter sends the queue, held or not, with what's typed last.
	if key("ctrl+enter") == nil || items() != "" {
		t.Fatalf("ctrl+enter left %s", items())
	}
	if key("ctrl+enter") != nil {
		t.Fatal("nothing to send")
	}
}

func TestQueueHintNoAlt(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.queueLocal("k", "fix the tests")
	if h := ansi.Strip(queueHint(m.queueOf(c), 200)); strings.Contains(h, "alt") {
		t.Errorf("the queue's keys shouldn't need alt: %s", h)
	}
}

// ↓ from the box goes nowhere; ↑ goes up through what's above it: the
// queue.
func TestUpFromTheBox(t *testing.T) {
	a := &fleet.Agent{Key: "k"}
	a.State = "working"
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}, paneFocus: true}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.host = c
	m.queueLocal("k", "one")
	key := func(s string) { m.paneKey(tea.KeyPressMsg{}, s) }
	key("down")
	if c.sel != "" {
		t.Fatalf("↓ from the box picked %q", c.sel)
	}
	key("up")
	if c.sel != "q:0" {
		t.Fatalf("↑: on %q, want q:0", c.sel)
	}
}

// Answering a card from the card hands the keys on to the card that comes
// next, a plan to approve after a question, rather than back to the box;
// a key pressed between the two means you've moved on, and it doesn't.
func TestCardFocusCarriesOn(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, host: c, paneFocus: true}
	one, _ := jsonx.Marshal(map[string]any{"questions": []map[string]any{{"question": "Go?", "options": []map[string]any{{"label": "yes"}, {"label": "no"}}}}})
	ask := func(tool, tu, id string, in jsontext.Value) {
		c.sess.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: tu, Name: tool, Input: in}}}, time.Now())
		c.sess.Apply(headless.PermissionRequest{ID: id, Tool: tool, ToolUseID: tu, Input: in}, time.Now())
	}
	c.sess.Apply(host.Sent{Text: "go"}, time.Now())
	ask("AskUserQuestion", "b1", "q1", one)
	a := &fleet.Agent{DisplayName: "x"}
	m.paneKey(tea.KeyPressMsg{}, "up")
	if cmd := m.paneKey(tea.KeyPressMsg{}, "enter"); cmd == nil || c.cardFocus {
		t.Fatalf("enter on an option should answer the lone question: focus %v", c.cardFocus)
	}
	m.paneDock(a, c, 100, 40) // the old card still up, not yet settled
	if c.cardFocus {
		t.Fatal("the card just answered shouldn't take the keys back")
	}
	c.sess.Apply(host.Answered{ID: "q1"}, time.Now())
	ask("ExitPlanMode", "b2", "p1", jsontext.Value(`{"plan":"do it"}`))
	m.paneDock(a, c, 100, 40)
	if !c.cardFocus {
		t.Fatal("the next card should have the keys")
	}

	// Answered, then a key before the next one: it stays with the box,
	// and the agent's card waits in the dock rather than as a modal.
	m.paneKey(tea.KeyPressMsg{}, "n")
	c.sess.Apply(host.Answered{ID: "p1"}, time.Now())
	m.paneKey(tea.KeyPressMsg{}, "down")
	ask("Bash", "b3", "r1", jsontext.Value(`{"command":"ls"}`))
	m.paneDock(a, c, 100, 40)
	if c.cardFocus || m.cardModal(c) {
		t.Fatal("a key between cards should leave the keys in the box")
	}
}

// A confirmation, like sending to a cold cache, asks in a box over the
// screen, not on the bottom line.
func TestConfirmModal(t *testing.T) {
	m := &Model{w: 100, h: 20, snap: &fleet.Snapshot{}}
	m.confirm = &confirmation{question: "Send to a cold cache?", detail: "its prompt cache expired 9m ago"}
	if strings.Contains(ansi.Strip(m.statusOr("hint")), "cold cache") {
		t.Error("a confirmation shouldn't also ask on the bottom line")
	}
	out := ansi.Strip(m.confirmModal(strings.Repeat("behind\n", 19) + "behind"))
	for _, w := range []string{"╭", "Send to a cold cache?", "expired 9m ago", "y yes", "n cancel", "behind"} {
		if !strings.Contains(out, w) {
			t.Errorf("modal missing %q:\n%s", w, out)
		}
	}
}

// A question that comes while you're typing takes the box to itself: what
// you'd typed is set aside, and back once the question's answered.
func TestQuestionHoldsTheBox(t *testing.T) {
	m, _ := benchModel(120, 40)
	c := m.host
	c.input, c.back = []rune("and also the tests"), 3
	ask(m)
	m.holdForQuestion(c)
	if len(c.input) != 0 || c.qHeld == nil {
		t.Fatalf("while asking, the box is empty: %q", string(c.input))
	}
	m.holdForQuestion(c) // a second pass with the question still up keeps it held
	c.sess.Apply(event.ApprovalCancelled{ID: "q1"}, time.Now())
	m.holdForQuestion(c)
	if string(c.input) != "and also the tests" || c.back != 3 || c.qHeld != nil {
		t.Fatalf("answered, the box is back: %q back=%d", string(c.input), c.back)
	}
}

// An answer typed while a question waits stays in the box when more of
// the conversation arrives, goes with its question as you move between
// them, and one sent in your own words is shown on the card and back in
// the box on returning to it.
func TestQuestionKeepsTypedAnswer(t *testing.T) {
	m, _ := benchModel(120, 40)
	c := m.host
	ask(m)
	m.holdForQuestion(c) // the box was empty when it came
	c.input = []rune("both, promises first")
	m.holdForQuestion(c) // more arrives while typing
	if string(c.input) != "both, promises first" {
		t.Fatalf("typed answer wiped: %q", string(c.input))
	}
	req := c.sess.Pending()[0].Approval.Question
	m.questionKey(c, req, "enter", false)
	if c.qIdx != 1 || len(c.input) != 0 {
		t.Fatalf("answered, on to the next: idx=%d box=%q", c.qIdx, string(c.input))
	}
	c.input = []rune("only mc")
	c.cardFocus = true
	m.questionKey(c, req, "left", true)
	if string(c.input) != "both, promises first" {
		t.Fatalf("back on question 1, its answer is in the box: %q", string(c.input))
	}
	if out := ansi.Strip(strings.Join(drawQuestion(c, "", qsOf(req), 120, 0, 0), "\n")); !strings.Contains(out, "✓ both, promises first") {
		t.Errorf("the card should show the answer in your own words:\n%s", out)
	}
	m.questionKey(c, req, "right", true)
	if string(c.input) != "only mc" {
		t.Fatalf("back on question 2, what was typed there: %q", string(c.input))
	}
}

// A question that goes with an answer half-written, and another that
// comes after it, leave the answer in the box: it isn't set aside as a
// draft, and the message held before comes back once the box is empty.
func TestQuestionNextKeepsHalfAnswer(t *testing.T) {
	m, _ := benchModel(120, 40)
	c := m.host
	c.input = []rune("my message")
	ask(m)
	m.holdForQuestion(c)
	c.input = []rune("half an ans")
	c.sess.Apply(event.ApprovalCancelled{ID: "q1"}, time.Now())
	m.holdForQuestion(c)
	ask(m)
	m.holdForQuestion(c)
	if string(c.input) != "half an ans" {
		t.Fatalf("the half-written answer went: %q", string(c.input))
	}
	c.sess.Apply(event.ApprovalCancelled{ID: "q1"}, time.Now())
	c.input = nil
	m.holdForQuestion(c)
	if string(c.input) != "my message" {
		t.Fatalf("the held message is back once the box is empty: %q", string(c.input))
	}
}

func qsOf(req *event.Question) []question {
	_, qs := questions(req)
	return qs
}

// With enter set to send, a message ending in a command typed in full
// sends: completing it would only add a space. A partial word still
// completes, and by default a full one does too.
func TestSlashFullWordSends(t *testing.T) {
	c := &hostConn{kind: "claude", sess: convo.New(), open: map[string]bool{}}
	c.local = []event.Command{{Name: "pdf"}, {Name: "pdf-tools"}}
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}}
	c.input, c.back = []rune("then run /pdf"), 0
	if _, used := m.slashKey(c, "enter"); !used || string(c.input) != "then run /pdf " {
		t.Fatalf("by default a full word completes, got %q", string(c.input))
	}
	m.store.Config.EnterSendsCommand = true
	c.input, c.back = []rune("then run /pdf"), 0
	if _, used := m.slashKey(c, "enter"); used || string(c.input) != "then run /pdf" {
		t.Fatalf("enter was taken by the picker, box now %q", string(c.input))
	}
	c.input = []rune("then run /pd")
	if _, used := m.slashKey(c, "enter"); !used || string(c.input) != "then run /pdf " {
		t.Fatalf("a partial word still completes, got %q", string(c.input))
	}
}

// The last question answered in your own words goes to the review with
// Send answers picked, so enter sends them.
func TestReviewPicksSend(t *testing.T) {
	m, _ := benchModel(120, 40)
	c := m.host
	ask(m)
	req := c.sess.Pending()[0].Approval.Question
	qs := qsOf(req)
	for i := range qs {
		c.cardFocus = false
		c.input = []rune("mine " + fmt.Sprint(i))
		m.questionKey(c, req, "enter", false)
	}
	if c.qIdx != len(qs) || !c.cardFocus {
		t.Fatalf("on the review with the keys on the card: idx=%d focus=%v", c.qIdx, c.cardFocus)
	}
}

// Pointing at a queued message says what its keys do, and clicking it
// picks it for them.
func TestQueueHover(t *testing.T) {
	a := &fleet.Agent{Key: "k"}
	a.State = "working"
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}, store: &state.Store{}}
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), open: map[string]bool{}}
	m.host = c
	m.queueLocal("k", "first")
	m.queueLocal("k", "second")
	dock := m.paneDock(a, c, 120, 40)
	y := slices.IndexFunc(dock, func(l string) bool { return strings.Contains(ansi.Strip(l), "second") })
	if !c.queueHover(c.dockY+y) || c.qHover != 2 {
		t.Fatalf("hovering line %d found %d", y, c.qHover-1)
	}
	if head := ansi.Strip(strings.Join(m.paneDock(a, c, 120, 40), "\n")); strings.Contains(head, "steer with all") || !strings.Contains(head, "sends within 15s") {
		t.Errorf("hovering changed the queue's header:\n%s", head)
	}
	m.clickRow(c, 0, c.dockY+y)
	if c.sel != "q:1" {
		t.Errorf("clicking picked %q", c.sel)
	}
	if !c.queueHover(-1) || c.qHover != 0 {
		t.Error("moving off kept the hover")
	}
}

// v shows the option under the cursor whole, its preview uncut, and with
// none says so; a click on an option moves the cursor there, and a second
// picks it.
func TestQuestionFullViewAndClicks(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, host: c, paneFocus: true}
	tall := strings.TrimSuffix(strings.Repeat("row\n", 20), "\n") + "\nlast row"
	in, _ := jsonx.Marshal(map[string]any{"questions": []map[string]any{{"question": "Which?", "options": []map[string]any{
		{"label": "tall", "preview": tall}, {"label": "plain", "description": "no picture"}}}}})
	c.sess.Apply(host.Sent{Text: "go"}, time.Now())
	c.sess.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "AskUserQuestion", Input: in}}}, time.Now())
	c.sess.Apply(headless.PermissionRequest{ID: "q1", Tool: "AskUserQuestion", ToolUseID: "b1", Input: in}, time.Now())
	req := c.sess.Pending()[0].Approval.Question
	a := &fleet.Agent{DisplayName: "x"}
	draw := func() string { return ansi.Strip(strings.Join(m.paneDock(a, c, 100, 80), "\n")) }

	c.cardFocus = true
	if strings.Contains(draw(), "last row") {
		t.Fatal("the list should cut a tall preview")
	}
	m.questionKey(c, req, "v", true)
	if out := draw(); !strings.Contains(out, "last row") || strings.Contains(out, "plain") {
		t.Fatalf("full view should show the whole preview and only the option under the cursor:\n%s", out)
	}
	m.questionKey(c, req, "down", true)
	if out := draw(); !strings.Contains(out, "no preview for this option") || !strings.Contains(out, "no picture") {
		t.Fatalf("full view of an option without a preview:\n%s", out)
	}
	m.questionKey(c, req, "esc", true)
	if c.qFull {
		t.Fatal("esc should leave full view")
	}

	c.qCursor = 0
	draw()
	row := -1
	for r, k := range c.qHit {
		if k == "@1" && (row < 0 || r < row) {
			row = r
		}
	}
	y := c.dockY + c.cardTop + c.qHitTop + row
	if cmd, ok := m.clickCard(c, m.paneX()+6, y); !ok || cmd != nil || c.qCursor != 1 {
		t.Fatalf("a click on an option should move the cursor there: ok=%v cursor=%d", ok, c.qCursor)
	}
	if cmd, ok := m.clickCard(c, m.paneX()+6, y); !ok || cmd == nil {
		t.Fatal("a second click should pick it")
	}
}
