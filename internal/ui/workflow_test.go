package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// workflowSession is a session running a workflow two phases in: one
// agent of its first done, one running, one waiting.
func workflowSession(now time.Time) *convo.Session {
	s := convo.New()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "w1", ToolUseID: "t1", Type: "local_workflow", Description: "Port rush to Rust", Backgrounded: true}, now.Add(-5*time.Minute))
	s.Apply(headless.TaskProgress{ID: "w1", Tokens: 980_000, ToolUses: 235, Workflow: []headless.WorkflowEntry{
		{Type: "workflow_phase", Index: 1, Title: "Port"},
		{Type: "workflow_phase", Index: 2, Title: "Check"},
		{Type: "workflow_agent", Index: 1, Label: "port:hl", PhaseTitle: "Port", AgentID: "a1", Model: "claude-sonnet-5-5", State: "done", StartedAt: 1, DurationMs: 180_000, Tokens: 85_000, Result: "Ported photon/hl"},
		{Type: "workflow_agent", Index: 2, Label: "port:claude", PhaseTitle: "Port", AgentID: "a2", State: "progress", StartedAt: now.Add(-2 * time.Minute).UnixMilli(), LastTool: "Bash", Doing: "cargo build"},
		{Type: "workflow_agent", Index: 3, Label: "port:draw", PhaseTitle: "Port", State: "start", QueuedAt: 1},
	}}, now)
	return s
}

// A workflow's row says its phases and how far each is, not that it has
// no output; opened, each agent under its phase, one a pick away.
func TestWorkflowRows(t *testing.T) {
	now := time.Now()
	s := workflowSession(now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	text := func(ls []convo.Line) string {
		var b []string
		for _, l := range ls {
			b = append(b, ansi.Strip(l.Text))
		}
		return strings.Join(b, "\n")
	}
	got := text(m.jobLines(c, convo.Options{Width: 120}))
	t.Logf("closed:\n%s", got)
	for _, want := range []string{"⋔ workflow", "235 steps", "Port 1/3", "· Check"} {
		if !strings.Contains(got, want) {
			t.Errorf("no %q in\n%s", want, got)
		}
	}
	if strings.Contains(got, "no captured output") {
		t.Errorf("a workflow said to have no output:\n%s", got)
	}

	c.open["job:w1"] = true
	lines := m.jobLines(c, convo.Options{Width: 120, Open: c.open})
	got = text(lines)
	t.Logf("opened:\n%s", got)
	for _, want := range []string{"Port  3 agents", "port:hl", "sonnet 5.5", "Ported photon/hl", "› Bash cargo build", "waiting to start", "Check  not started"} {
		if !strings.Contains(got, want) {
			t.Errorf("no %q in\n%s", want, got)
		}
	}
	if strings.Contains(got, "no output yet") {
		t.Errorf("opened, said no output:\n%s", got)
	}
	refs := map[string]bool{}
	for _, l := range lines {
		refs[l.Ref] = true
	}
	if !refs["run:a1"] || !refs["run:a2"] {
		t.Errorf("an agent's row isn't its own: %v", refs)
	}

	// The dock: a workflow is called one, with its phase now.
	dock := ansi.Strip(strings.Join(m.jobsPreview(c, c.dockJobs(), 120), "\n"))
	t.Logf("dock:\n%s", dock)
	for _, want := range []string{"Workflow  1 ", "› ", "Port 1/3 · port:claude"} {
		if !strings.Contains(dock, want) {
			t.Errorf("no %q in the dock:\n%s", want, dock)
		}
	}
	if strings.Contains(dock, "shell") {
		t.Errorf("a workflow called a shell:\n%s", dock)
	}
}

// A workflow's agent stands as its workflow says; once the workflow
// stops, one it hadn't heard finish stopped with it.
func TestWorkflowAgentState(t *testing.T) {
	now := time.Now()
	s := workflowSession(now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{},
		workflows: map[string]*workflowSeen{"w1": {dir: "/s/subagents/workflows/wf_1", w: headless.Workflow{Done: true}}}}
	if _, live := c.subState(convo.Subagent{ID: "a2", Workflow: "wf_1"}); !live {
		t.Error("a running agent isn't")
	}
	if st, live := c.subState(convo.Subagent{ID: "a1", Workflow: "wf_1"}); live || st != "" {
		t.Errorf("a done agent: %q %v", st, live)
	}
	s.Apply(headless.TaskDone{ID: "w1", Status: "stopped"}, now)
	if st, live := c.subState(convo.Subagent{ID: "a2", Workflow: "wf_1"}); live || st != "stopped" {
		t.Errorf("the workflow stopped, its agent: %q %v", st, live)
	}
}

// A finished workflow's output is its progress again, as JSON: its rows
// say it already, so it isn't shown under them.
func TestWorkflowOutputHidden(t *testing.T) {
	now := time.Now()
	s := workflowSession(now)
	out := t.TempDir() + "/w1.output"
	os.WriteFile(out, []byte("{\n  \"workflowProgress\": []\n}\n"), 0o644)
	s.Apply(headless.TaskDone{ID: "w1", Status: "completed", OutputFile: out}, now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{"job:w1": true}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	var b []string
	for _, l := range m.jobLines(c, convo.Options{Width: 120, Open: c.open}) {
		b = append(b, ansi.Strip(l.Text))
	}
	got := strings.Join(b, "\n")
	if strings.Contains(got, "workflowProgress") || !strings.Contains(got, "port:hl") {
		t.Errorf("finished workflow:\n%s", got)
	}
}
