package headless

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// What a workflow leaves on disk, for one rush didn't hear run: the
// session's own, before rush was there, or one it ran before a restart.
// Claude Code keeps each run's agents beside the session's transcript, in
// subagents/workflows/<run>/: a journal of each agent started and what it
// gave back, and each agent's transcript. Once the run ends, it writes the
// whole of its progress to workflows/<run>.json.

// Workflow is a workflow run's phases and agents, as its files have them.
type Workflow struct {
	Phases []string
	Agents []event.WorkflowAgent
	Done   bool // read from what it wrote as it ended: it won't change
}

// workflowDirRe finds the run's folder in what the Workflow tool gave back.
var workflowDirRe = regexp.MustCompile(`(?m)^Transcript dir: (.+)$`)

// WorkflowDir is the folder of the run the Workflow tool started, from
// what it gave back; "" when it doesn't say.
func WorkflowDir(output string) string {
	if m := workflowDirRe.FindStringSubmatch(output); m != nil {
		return strings.TrimSpace(m[1])
	}
	return ""
}

// ReadWorkflow reads the run whose folder is dir: what it wrote as it
// ended, else its journal so far.
func ReadWorkflow(dir string) Workflow {
	run := filepath.Base(dir)
	session := filepath.Dir(filepath.Dir(filepath.Dir(dir))) // <session>/subagents/workflows/<run>
	if b, err := os.ReadFile(filepath.Join(session, "workflows", run+".json")); err == nil {
		var r struct {
			Phases []struct {
				Title string `json:"title"`
			} `json:"phases"`
			Progress []WorkflowEntry `json:"workflowProgress"`
		}
		if jsonx.Unmarshal(b, &r) == nil && len(r.Progress) > 0 {
			phases, agents := workflowProgress(r.Progress)
			if len(phases) == 0 {
				for _, p := range r.Phases {
					phases = append(phases, p.Title)
				}
			}
			return Workflow{Phases: phases, Agents: agents, Done: true}
		}
	}
	w := readJournal(filepath.Join(dir, "journal.jsonl"))
	// The phases it will go through are in its script's meta.
	if scripts, _ := filepath.Glob(filepath.Join(session, "workflows", "scripts", "*"+run+".js")); len(scripts) > 0 {
		if planned := scriptPhases(scripts[0]); len(planned) >= len(w.Phases) {
			w.Phases = planned
		}
	}
	return w
}

// phasesRe finds a workflow script's meta phases, and each one's title.
var (
	phasesRe = regexp.MustCompile(`(?s)phases\s*:\s*\[(.*?)\]\s*,?\s*\}`)
	titleRe  = regexp.MustCompile(`title\s*:\s*(?:'([^']*)'|"([^"]*)"|` + "`([^`]*)`" + `)`)
)

// scriptPhases are the phases a workflow's script says it has, in order.
func scriptPhases(path string) []string {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	if i := bytes.Index(b, []byte("export const meta")); i >= 0 {
		b = b[i:]
	}
	m := phasesRe.FindSubmatch(b)
	if m == nil {
		return nil
	}
	var out []string
	for _, t := range titleRe.FindAllSubmatch(m[1], -1) {
		out = append(out, string(t[1])+string(t[2])+string(t[3]))
	}
	return out
}

// readJournal is a run's agents from its journal: each one started, and
// what each gave back. An agent is known there by its id, and numbered in
// the order it started.
func readJournal(path string) Workflow {
	f, err := os.Open(path)
	if err != nil {
		return Workflow{}
	}
	defer f.Close()
	var w Workflow
	at := map[string]int{}
	sc := bufio.NewScanner(f)
	sc.Buffer(nil, 16<<20) // a result can be long
	for sc.Scan() {
		var l struct {
			Type    string         `json:"type"`
			AgentID string         `json:"agentId"`
			Label   string         `json:"label"`
			Phase   string         `json:"phase"`
			Result  jsontext.Value `json:"result"`
		}
		if jsonx.Unmarshal(bytes.TrimSpace(sc.Bytes()), &l) != nil || l.AgentID == "" {
			continue
		}
		switch l.Type {
		case "started":
			if l.Phase != "" && !slices.Contains(w.Phases, l.Phase) {
				w.Phases = append(w.Phases, l.Phase)
			}
			i, ok := at[l.AgentID]
			if !ok {
				i, at[l.AgentID] = len(w.Agents)+1, len(w.Agents)+1
			}
			w.Agents = event.MergeAgents(w.Agents, event.WorkflowAgent{Index: i, ID: l.AgentID, Label: l.Label, Phase: l.Phase, State: "running"})
		case "result":
			if i, ok := at[l.AgentID]; ok {
				w.Agents = event.MergeAgents(w.Agents, event.WorkflowAgent{Index: i, State: "done", Result: resultText(l.Result)})
			}
		}
	}
	return w
}

// resultText is the start of what an agent gave back: its words, or the
// summary of what it gave back as an object.
func resultText(v jsontext.Value) string {
	var s string
	if jsonx.Unmarshal(v, &s) != nil {
		var o struct {
			Summary string `json:"summary"`
		}
		_ = jsonx.Unmarshal(v, &o)
		s = o.Summary
	}
	if len(s) > 400 {
		s = strings.ToValidUTF8(s[:400], "")
	}
	return s
}
