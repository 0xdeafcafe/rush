package headless

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A workflow's progress says its phases and how each agent stands: an
// agent queued, then running, then done, reads as the last word of it,
// with what the earlier ones said kept.
func TestNeutralWorkflowProgress(t *testing.T) {
	var n Neutral
	p := neutral(t, &n, `{"type":"system","subtype":"task_progress","task_id":"w1","usage":{"total_tokens":900,"tool_uses":4},"workflow_progress":[
		{"type":"workflow_phase","index":1,"title":"Port"},
		{"type":"workflow_phase","index":2,"title":"Check"},
		{"type":"workflow_agent","index":1,"label":"port:hl","phaseIndex":1,"phaseTitle":"Port","model":"claude-sonnet-5-5","state":"start","queuedAt":1},
		{"type":"workflow_agent","index":1,"agentId":"a1","state":"progress","startedAt":2000,"lastToolName":"Bash","lastToolSummary":"cargo build"},
		{"type":"workflow_agent","index":2,"label":"port:draw","phaseTitle":"Port","state":"start","queuedAt":1},
		{"type":"workflow_agent","index":3,"label":"port:claude","phaseTitle":"Port","agentId":"a3","state":"done","startedAt":1000,"durationMs":5000,"tokens":300,"resultPreview":"ported"},
		{"type":"workflow_log","index":1}
	]}`)[0].(event.TaskProgress)
	if !reflect.DeepEqual(p.Phases, []string{"Port", "Check"}) {
		t.Errorf("phases = %q", p.Phases)
	}
	if len(p.Agents) != 3 {
		t.Fatalf("agents = %+v", p.Agents)
	}
	if a := p.Agents[0]; a.Label != "port:hl" || a.ID != "a1" || a.State != "running" || a.Model != "claude-sonnet-5-5" || a.LastTool != "Bash" || a.Doing != "cargo build" {
		t.Errorf("a queued agent that started = %+v", a)
	}
	if a := p.Agents[1]; a.State != "queued" {
		t.Errorf("one not started = %+v", a)
	}
	if a := p.Agents[2]; a.State != "done" || a.Result != "ported" || a.Tokens != 300 || a.Took.Seconds() != 5 {
		t.Errorf("one done = %+v", a)
	}
}

// A run rush didn't hear: its journal says which agents started and which
// gave back, and its script the phases it will go through.
func TestReadWorkflowJournal(t *testing.T) {
	session := t.TempDir()
	dir := filepath.Join(session, "subagents", "workflows", "wf_1")
	os.MkdirAll(dir, 0o755)
	os.MkdirAll(filepath.Join(session, "workflows", "scripts"), 0o755)
	os.WriteFile(filepath.Join(dir, "journal.jsonl"), []byte(`{"type":"launched"}
{"type":"started","key":"k1","agentId":"a1","label":"port:hl","phase":"Port"}
{"type":"started","key":"k2","agentId":"a2","label":"port:draw","phase":"Port"}
{"type":"result","key":"k1","agentId":"a1","result":{"summary":"ported hl","tests":3}}
{"type":"started","key":"k3","agentId":"a3","label":"port:check","phase":"Port"}
{"type":"failed","key":"k3","agentId":"a3"}
`), 0o644)
	os.WriteFile(filepath.Join(session, "workflows", "scripts", "port-wf_1.js"), []byte(`export const meta = {
  name: 'port',
  description: 'Port it',
  phases: [
    { title: 'Port', detail: 'in parallel' },
    { title: "Check" },
  ],
}
phase('Port')
`), 0o644)
	w := ReadWorkflow(dir)
	if w.Done || !reflect.DeepEqual(w.Phases, []string{"Port", "Check"}) {
		t.Errorf("phases = %q, done %v", w.Phases, w.Done)
	}
	if len(w.Agents) != 3 || w.Agents[0].State != "done" || w.Agents[0].Result != "ported hl" || w.Agents[1].State != "running" || w.Agents[1].ID != "a2" || w.Agents[2].State != "failed" {
		t.Errorf("agents = %+v", w.Agents)
	}

	// Ended, it wrote its whole progress: that's what it says.
	os.WriteFile(filepath.Join(session, "workflows", "wf_1.json"), []byte(`{"phases":[{"title":"Port"}],"workflowProgress":[
		{"type":"workflow_agent","index":1,"label":"port:hl","phaseTitle":"Port","agentId":"a1","state":"done","tokens":10},
		{"type":"workflow_agent","index":2,"label":"port:draw","phaseTitle":"Port","agentId":"a2","state":"error","error":"boom"}]}`), 0o644)
	w = ReadWorkflow(dir)
	if !w.Done || !reflect.DeepEqual(w.Phases, []string{"Port"}) || len(w.Agents) != 2 || w.Agents[1].State != "failed" || w.Agents[1].Error != "boom" {
		t.Errorf("ended = %+v", w)
	}
}

// The Workflow tool says where its run's folder is.
func TestWorkflowDir(t *testing.T) {
	out := "Workflow launched in background. Task ID: w1\nSummary: x\nTranscript dir: /p/s/subagents/workflows/wf_1\nScript file: /p/s/workflows/scripts/x.js\n"
	if got := WorkflowDir(out); got != "/p/s/subagents/workflows/wf_1" {
		t.Errorf("dir = %q", got)
	}
}
