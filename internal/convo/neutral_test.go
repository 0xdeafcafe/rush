package convo

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A session another agent ran, as the host sends it, draws as Claude's do.
func TestNeutralSession(t *testing.T) {
	s := New()
	exit := 2
	s.Apply(event.Init{SessionID: "th-1", Model: "gpt-5", Cwd: "/w"}, at(0))
	s.Apply(host.Sent{Text: "fix the build"}, at(1))
	s.Apply(event.MessageStart{ID: "m1"}, at(2))
	s.Apply(event.Delta{Kind: event.Text, Text: "Looking"}, at(2))
	s.Apply(event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{
		{Kind: event.Text, Text: "Looking at it."},
		{Kind: event.ToolCall, Call: &tool.Call{ID: "c1", Name: "shell", Kind: tool.Shell, Input: tool.Input{Command: "go build ./..."}}},
	}, Tokens: &usage.TokenUsage{Input: 10, Output: 5}}, at(3))
	s.Apply(event.Approval{ID: "a1", Call: tool.Call{ID: "c1", Kind: tool.Shell, Input: tool.Input{Command: "go build ./..."}},
		Options: []event.Option{{ID: "yes", Kind: event.AllowOnce}, {ID: "always", Kind: event.AllowAlways}, {ID: "no", Kind: event.RejectOnce}}}, at(4))
	if p := s.Pending(); len(p) != 1 || p[0].Approval.ID != "a1" || p[0].Tool != "Bash" || !p[0].Approval.Always {
		t.Fatalf("pending = %+v", p)
	}
	s.Apply(host.Answered{ID: "a1"}, at(5))
	s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: "c1", Text: "undefined: x", IsError: true, Exit: &exit}}}}, at(6))
	s.Apply(event.Approval{ID: "a2", Call: tool.Call{ID: "c2", Kind: tool.Edit, Input: tool.Input{Path: "/w/main.go"}}}, at(7))
	if st := s.Step("c2"); st == nil || st.Tool != "Edit" || st.Approval == nil {
		t.Fatalf("an approval for a call not yet seen made no step: %+v", st)
	}
	s.Apply(event.ApprovalCancelled{ID: "a2"}, at(8))
	s.Apply(event.Plan{Todos: []tool.TodoItem{{Label: "build", Status: "in_progress"}}}, at(8))
	s.Apply(event.TurnEnd{Reason: "done", Cost: 0.02}, at(9))

	st := s.Step("c1")
	if st.Status != Failed || exitCode(st) != 2 {
		t.Errorf("c1 = %v, exit %d; want failed with 2", st.Status, exitCode(st))
	}
	if len(s.Tasks) != 1 || s.Tasks[0].Subject != "build" {
		t.Errorf("tasks = %+v", s.Tasks)
	}
	out := plain(s.Render(Options{Width: 110, Now: at(10)}))
	for _, want := range []string{"fix the build", "Looking at it.", "go build"} {
		if !strings.Contains(out, want) {
			t.Errorf("render is missing %q:\n%s", want, out)
		}
	}
}

func TestNeutralQuestionAndInterrupt(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "pick"}, at(0))
	s.Apply(event.Question{ID: "q1", Asks: []event.Ask{{Text: "Which?", Options: []event.Choice{{Label: "A"}, {Label: "B"}}}}}, at(1))
	p := s.Pending()
	if len(p) != 1 || p[0].Tool != "AskUserQuestion" || p[0].Approval.Question == nil {
		t.Fatalf("pending = %+v", p)
	}
	if q := p[0].Approval.Question; len(q.Asks) != 1 || len(q.Asks[0].Options) != 2 {
		t.Errorf("question = %+v", q)
	}
	s.Apply(event.TurnEnd{Reason: "interrupted"}, at(2))
	if tr := s.Turns[0]; tr.Live || !tr.Stopped {
		t.Errorf("turn = live %v, stopped %v; want stopped", tr.Live, tr.Stopped)
	}
}

// A call Claude Code has no tool for keeps its kind, and draws as one.
func TestNeutralKindsWithoutAClaudeTool(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "tidy"}, at(0))
	s.Apply(event.Message{Role: "assistant", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "d1", Name: "delete", Kind: tool.Delete, Input: tool.Input{Path: "/w/old.go"}}},
	}}, at(1))
	st := s.Step("d1")
	if st == nil || st.kind() != tool.Delete || glyphFor(st) != "✎" {
		t.Fatalf("delete step = %+v", st)
	}
	s.Apply(event.TurnEnd{Reason: "done"}, at(2))
	if out := plain(s.Render(Options{Width: 100, Now: at(3)})); !strings.Contains(out, "old.go") {
		t.Errorf("the delete doesn't say what it deleted:\n%s", out)
	}
}

// A step keeps the call its agent made, with what Claude's input has no
// key for: a move's destination.
func TestNeutralStepKeepsItsCall(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "rename"}, at(0))
	s.Apply(event.Message{Role: "assistant", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "m1", Name: "move", Kind: tool.Move, Input: tool.Input{Path: "/w/a.go", To: "/w/b.go"}}},
	}}, at(1))
	st := s.Step("m1")
	if st == nil {
		t.Fatal("no step for the move")
	}
	if c := st.Call(); c.Name != "move" || c.Kind != tool.Move || c.Input.To != "/w/b.go" || st.in().Path != "/w/a.go" {
		t.Fatalf("call = %+v: want the agent's own", c)
	}
}

// A history's prompts are your messages: each starts a turn.
func TestNeutralPromptsStartTurns(t *testing.T) {
	s := New()
	s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.Text, Text: "fix it"}}}, at(0))
	s.Apply(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: "Fixed."}}}, at(1))
	s.Apply(event.TurnEnd{Reason: "done"}, at(2))
	if len(s.Turns) != 1 || s.Turns[0].Prompt != "fix it" {
		t.Fatalf("turns = %+v", s.Turns)
	}
}

// A model switched mid-session is the one the session runs, though the
// agent's init named another.
func TestModelSwitch(t *testing.T) {
	s := New()
	s.Apply(event.Init{Model: "claude-opus-5-5[1m]"}, at(0))
	s.Apply(host.InfoEvent{Info: host.Info{Model: "claude-opus-5-5[1m]"}}, at(1))
	if s.Model != "claude-opus-5-5[1m]" {
		t.Fatalf("model = %q", s.Model)
	}
	s.Apply(host.InfoEvent{Info: host.Info{Model: "sonnet"}}, at(2))
	if s.Model != "sonnet" {
		t.Fatalf("after switch, model = %q", s.Model)
	}
	s.Apply(host.InfoEvent{Info: host.Info{Model: "sonnet", State: "idle"}}, at(3))
	if s.Model != "sonnet" {
		t.Fatalf("model = %q", s.Model)
	}
}

// A background subagent's tool call while the main agent's words stream in
// doesn't make the whole message draw them a second time.
func TestSubagentCallMidStreamKeepsWordsOnce(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "go"}, at(0))
	s.Apply(event.Message{Role: "assistant", ID: "m0", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "task", Name: "Agent", Kind: tool.Subagent}},
	}}, at(1))
	s.Apply(event.MessageStart{ID: "m1"}, at(2))
	s.Apply(event.Delta{Kind: event.Text, Text: "Only vf-runner"}, at(2))
	s.Apply(event.Message{Role: "assistant", Parent: "task", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "b1", Name: "Bash", Kind: tool.Shell, Input: tool.Input{Command: "go vet ./... && echo vet-ok"}}},
	}}, at(3))
	s.Apply(event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{{Kind: event.Text, Text: "Only vf-runner is still running."}}}, at(4))
	n := 0
	for _, it := range s.Turns[len(s.Turns)-1].Items {
		if it.Kind == KText {
			n++
		}
	}
	if n != 1 {
		t.Fatalf("text items = %d, want 1", n)
	}
}

// Between a step's end and the model's next words, the live line says how
// much it reads, and a retried request says why and when it goes again,
// until the model starts answering.
func TestWaitingLine(t *testing.T) {
	s := New()
	exit := 0
	s.Apply(host.Sent{Text: "build it"}, at(0))
	s.Apply(event.Context{Tokens: 182_000}, at(1))
	s.Apply(event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "c1", Name: "shell", Kind: tool.Shell, Input: tool.Input{Command: "go build ./..."}}},
	}}, at(2))
	s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: "c1", Text: "ok", Exit: &exit}}}}, at(4))
	s.Apply(event.Retry{Attempt: 2, Max: 10, Delay: 8e9, Status: 529}, at(10))
	out := plain(s.Render(Options{Width: 120, Now: at(12)}))
	for _, want := range []string{pick(waitings, at(4)) + "…  8s", "~182k tokens in", "↻ retrying · the API is overloaded · attempt 2 of 10 · next in 6s"} {
		if !strings.Contains(out, want) {
			t.Errorf("want %q in:\n%s", want, out)
		}
	}
	s.Apply(event.PartStart{Index: 0, Kind: event.Text}, at(20))
	if out := plain(s.Render(Options{Width: 120, Now: at(21)})); strings.Contains(out, "retrying") {
		t.Errorf("the retry should go once the model answers:\n%s", out)
	}
}

// A compaction under way takes the working line's place: dots, how long
// it's run, and a time left: guessed until this session has timed one.
func TestCompactingLine(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "go on"}, at(0))
	s.Apply(event.Context{Tokens: 400_000}, at(1))
	s.Apply(event.Status{Busy: true, Text: "compacting"}, at(10))
	out := plain(s.Render(Options{Width: 140, Now: at(70)}))
	name := compaction(at(10))
	if !strings.Contains(out, name+"…  1m 00s") || !strings.Contains(out, "▰") || !strings.Contains(out, "400k tokens to boil down") {
		t.Fatalf("no compacting line:\n%s", out)
	}
	// Guessed: 20s plus 1s per 4k tokens is 2m, 1m gone.
	if !strings.Contains(out, "~1m 00s left") || strings.Contains(out, "as the last one went") {
		t.Fatalf("an untimed compaction should guess what's left:\n%s", out)
	}
	s.Apply(event.Compacted{Trigger: "auto", Before: 400_000, After: 10_000}, at(90))
	if out := plain(s.Render(Options{Width: 140, Now: at(91)})); strings.Contains(out, name+"…") {
		t.Fatalf("done, the dots go:\n%s", out)
	}
	// 80s for 400k: the next, of 200k, should take about 40s.
	s.Apply(event.Context{Tokens: 200_000}, at(100))
	s.Apply(event.Status{Busy: true, Text: "compacting"}, at(100))
	if out := plain(s.Render(Options{Width: 140, Now: at(110)})); !strings.Contains(out, "~30s left, as the last one went") {
		t.Fatalf("a timed compaction should say what's left:\n%s", out)
	}
}

// /compact, and the summary an external compaction starts afresh with,
// are rush's rows, not your messages; the summary is the divider's.
func TestCompactAsksAreNotYours(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "/compact"}, at(0))
	if tn := s.Live(); tn.Prompt != "" || tn.Cause != "/compact" {
		t.Fatalf("/compact drawn as yours: %+v", tn)
	}
	s.Apply(event.TurnEnd{Reason: "done"}, at(1))
	s.Apply(host.Sent{Text: CompactedPrompt("Claude haiku", "did X, doing Y")}, at(2))
	tn := s.Live()
	if tn.Prompt != "" || len(tn.Items) != 1 || tn.Items[0].Kind != KCompact || tn.Items[0].Text != "did X, doing Y" {
		t.Fatalf("summary drawn as yours: %+v", tn)
	}
	out := plain(s.Render(Options{Width: 120, Now: at(3)}))
	if !strings.Contains(out, "context compacted") || !strings.Contains(out, "by Claude haiku") || strings.Contains(out, "did X") {
		t.Fatalf("no divider:\n%s", out)
	}
}

// A compaction rush runs itself, with no turn running, shows in the dock.
func TestDockShowsRushCompaction(t *testing.T) {
	s := New()
	s.Context = 50_000
	s.MarkCompacting(at(0))
	if out := plain(s.Activity(Options{Width: 100, Now: at(5)})); !strings.Contains(out, compaction(at(0))+"…  5s") || !s.Fast {
		t.Fatalf("no dock line: %q", out)
	}
	s.MarkCompacting(time.Time{})
	if len(s.Activity(Options{Width: 100, Now: at(6)})) != 0 {
		t.Fatal("done, the dock line stays")
	}
}

// A URL in your message is linked once: linked twice, the second link
// went round the first's own escape, and the address showed three times.
func TestYourURLLinkedOnce(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "the UI is https://app.visualdiff-check.langwatch.localhost, the API http://127.0.0.1:<port> for haven"}, at(0))
	for _, l := range s.Render(Options{Width: 200, Now: at(1)}) {
		if n := strings.Count(l.Text, "\x1b]8;;http"); n > 0 && n != strings.Count(ansi.Strip(l.Text), "http") {
			t.Errorf("each link once, got %d opened for %d shown: %q", n, strings.Count(ansi.Strip(l.Text), "http"), l.Text)
		}
		if strings.Contains(ansi.Strip(l.Text), "8;;") {
			t.Errorf("a link's escape shows: %q", ansi.Strip(l.Text))
		}
	}
}
