package acp

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

var _ agent.ChildFinder = Kimi{}

// agentIDRe finds the subagent an Agent call's result says ran it.
var agentIDRe = regexp.MustCompile(`(?m)^agent_id: (\S+)$`)

// FindChild is the subagent parent's Agent call child started: its own
// wire, agents/agent-N/wire.jsonl beside the session's main one. Main's
// wire names it in a task.started or the call's result; a subagent still
// running in the foreground has neither yet, so it's the one asked what
// the call asked.
func (a Kimi) FindChild(p agent.Profile, parent, child string, start time.Time) (agent.Session, bool) {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	if _, id, ok := strings.Cut(child, ":"); ok {
		child = id // live, ACP's call id is the wire's after the turn: "0:tool_…"
	}
	if parent == "" || child == "" || filepath.Base(parent) != parent {
		return agent.Session{}, false
	}
	dirs, _ := filepath.Glob(filepath.Join(p.Dir, "sessions", "*", parent, "agents"))
	for _, dir := range dirs {
		id, prompt := kimiChildOf(filepath.Join(dir, "main", "wire.jsonl"), child)
		file := ""
		if id != "" && filepath.Base(id) == id {
			file = filepath.Join(dir, id, "wire.jsonl")
		} else if prompt != "" {
			file = kimiChildAsked(dir, prompt, start)
		}
		if file == "" {
			continue
		}
		if st, err := os.Stat(file); err == nil {
			id = filepath.Base(filepath.Dir(file))
			return agent.Session{Kind: a.ID, Profile: p, ID: id, Transcript: file, State: "done", CreatedAt: kimiWireCreated(file, st.ModTime()), UpdatedAt: st.ModTime()}, true
		}
	}
	return agent.Session{}, false
}

// kimiChildOf reads main's wire for what it says of the Agent call child:
// the subagent it started, once said, and the prompt it was given.
func kimiChildOf(main, child string) (id, prompt string) {
	f, err := os.Open(main)
	if err != nil {
		return "", ""
	}
	defer f.Close()
	scan := bufio.NewScanner(f)
	scan.Buffer(make([]byte, 64<<10), 32<<20)
	needle := []byte(child)
	for scan.Scan() {
		if !bytes.Contains(scan.Bytes(), needle) {
			continue
		}
		var rec struct {
			Type  string
			Event struct {
				Type, ToolCallID string
				Args             struct{ Prompt string }
				Result           struct{ Output jsontext.Value }
			}
			Info struct{ AgentID, ParentToolCallID string }
		}
		if jsonx.Unmarshal(scan.Bytes(), &rec) != nil {
			continue
		}
		switch {
		case rec.Type == "task.started" && rec.Info.ParentToolCallID == child && rec.Info.AgentID != "":
			return rec.Info.AgentID, prompt
		case rec.Event.ToolCallID != child:
		case rec.Event.Type == "tool.call":
			prompt = rec.Event.Args.Prompt
		case rec.Event.Type == "tool.result":
			var out string
			_ = jsonx.Unmarshal(rec.Event.Result.Output, &out)
			if m := agentIDRe.FindStringSubmatch(out); m != nil {
				return m[1], prompt
			}
		}
	}
	return "", prompt
}

// kimiChildAsked is the subagent wire in dir begun after start whose first
// message carries prompt (Kimi puts its git context before it).
func kimiChildAsked(dir, prompt string, start time.Time) string {
	files, _ := filepath.Glob(filepath.Join(dir, "agent-*", "wire.jsonl"))
	prompt = strings.TrimSpace(prompt)
	for _, file := range files {
		if st, err := os.Stat(file); err != nil || st.ModTime().Before(start.Add(-3*time.Second)) {
			continue
		}
		if strings.Contains(kimiWireFirstAsk(file), prompt) {
			return file
		}
	}
	return ""
}

// kimiWireFirstAsk is the text of a wire's first context message.
func kimiWireFirstAsk(file string) string {
	f, err := os.Open(file)
	if err != nil {
		return ""
	}
	defer f.Close()
	scan := bufio.NewScanner(f)
	scan.Buffer(make([]byte, 64<<10), 32<<20)
	for n := 0; n < 50 && scan.Scan(); n++ {
		if !bytes.Contains(scan.Bytes(), []byte(`"context.append_message"`)) {
			continue
		}
		var rec struct {
			Message struct{ Content jsontext.Value }
		}
		if jsonx.Unmarshal(scan.Bytes(), &rec) != nil {
			return ""
		}
		var b strings.Builder
		for _, p := range ContentParts(rec.Message.Content) {
			b.WriteString(p.Text)
		}
		return b.String()
	}
	return ""
}

// kimiWireCreated is when a wire's metadata says it began, else at.
func kimiWireCreated(file string, at time.Time) time.Time {
	f, err := os.Open(file)
	if err != nil {
		return at
	}
	defer f.Close()
	line, _ := bufio.NewReader(f).ReadSlice('\n')
	var m struct {
		CreatedAt int64 `json:"created_at"`
	}
	if jsonx.Unmarshal(line, &m) == nil && m.CreatedAt > 0 {
		return time.UnixMilli(m.CreatedAt)
	}
	return at
}
