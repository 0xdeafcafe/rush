package acp

import (
	"bufio"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// codePast reads Kimi Code 2's state files. No agent process or model request
// is needed, and deleted sessions cannot linger through a stale index entry.
func (a Kimi) codePast(p agent.Profile) []agent.Session {
	paths, _ := filepath.Glob(filepath.Join(p.Dir, "sessions", "wd_*", "session_*", "state.json"))
	var out []agent.Session
	for _, path := range paths {
		var state struct {
			ID, Cwd, Title, LastPrompt string
			UpdatedAt                  int64
		}
		b, err := os.ReadFile(path)
		if err != nil || jsonx.Unmarshal(b, &state) != nil || state.ID == "" || state.Cwd == "" {
			continue
		}
		transcript := filepath.Join(filepath.Dir(path), "agents", "main", "wire.jsonl")
		st, err := os.Stat(transcript)
		if err != nil || st.Size() == 0 {
			continue
		}
		name := state.Title
		if name == "" {
			name = strings.TrimSpace(strings.SplitN(state.LastPrompt, "\n", 2)[0])
		}
		if name == "" {
			name = state.ID
		}
		at := st.ModTime()
		if state.UpdatedAt > 0 {
			at = time.UnixMilli(state.UpdatedAt)
		}
		out = append(out, agent.Session{Kind: a.ID, Profile: p, ID: state.ID, Name: name, Cwd: state.Cwd, Transcript: transcript, State: "done", UpdatedAt: at})
	}
	return out
}

// readKimiWireFile bounds tail reads without parsing earlier tool output.
// A zero byte budget means the whole transcript, filtered by before.
func readKimiWireFile(path string, most int64, before time.Time) ([]event.Event, bool, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, false, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return nil, false, err
	}
	var r io.Reader = f
	cut := most > 0 && st.Size() > most
	if cut {
		br := bufio.NewReader(io.NewSectionReader(f, st.Size()-most-1, most+1))
		for {
			_, err := br.ReadSlice('\n')
			if err == bufio.ErrBufferFull {
				continue
			}
			if err != nil && err != io.EOF {
				return nil, false, err
			}
			break
		}
		r = br
	}
	own := "main"
	if dir := filepath.Dir(path); filepath.Base(filepath.Dir(dir)) == "agents" {
		own = filepath.Base(dir) // agents/main, or a subagent's agents/agent-N
	}
	evs, err := readAgentWire(r, own, before)
	return evs, cut, err
}

func readKimiWire(r io.Reader, before time.Time) ([]event.Event, error) {
	return readAgentWire(r, "main", before)
}

// The wire journal records accepted prompts, context messages and loop events.
// Only context messages and loop events are displayed: replaying prompt.accepted
// as well would show every user message twice. Internal context injections are
// explicitly marked so they do not become messages attributed to the user.
// Only own's records are read: main's, or a subagent's (agents/agent-N).
func readAgentWire(r io.Reader, own string, before time.Time) ([]event.Event, error) {
	scan := bufio.NewScanner(r)
	scan.Buffer(make([]byte, 64<<10), 32<<20)
	partial := false // the last line has no newline yet: still being written
	scan.Split(func(data []byte, atEOF bool) (int, []byte, error) {
		adv, tok, err := bufio.ScanLines(data, atEOF)
		partial = atEOF && adv == len(data) && len(data) > 0 && data[len(data)-1] != '\n'
		return adv, tok, err
	})
	var out []event.Event
	var tokens usage.TokenUsage
	model := ""
	line := 0
	for scan.Scan() {
		line++
		var rec struct {
			Type, AgentID, ModelAlias, Reason string
			Time, DurationMs                  int64
			TokensBefore, TokensAfter         int
			Message                           struct {
				ID, Role string
				Content  jsontext.Value
				Origin   struct{ Kind string }
			}
			Event struct {
				Type, UUID, ToolCallID, Name string
				Args                         jsontext.Value
				Part                         struct{ Type, Text, Think string }
				Result                       struct {
					Output  jsontext.Value
					IsError bool
				}
			}
			Usage struct{ InputOther, Output, InputCacheRead, InputCacheCreation int64 }
			Error struct{ Message string }
			Info  struct {
				TaskID, Description, Status, Kind, SubagentType, ParentToolCallID string
				Detached                                                          bool
			}
		}
		if err := jsonx.Unmarshal(scan.Bytes(), &rec); err != nil {
			if partial {
				continue // a live session's last line is often half-written
			}
			return nil, fmt.Errorf("kimi wire line %d: %w", line, err)
		}
		if rec.AgentID != "" && rec.AgentID != own {
			continue
		}
		if !before.IsZero() && rec.Time > 0 && !time.UnixMilli(rec.Time).Before(before) {
			continue
		}
		switch rec.Type {
		case "profile.bind", "config.update":
			if rec.ModelAlias != "" {
				model = rec.ModelAlias
			}
		case "context.append_message":
			m := rec.Message
			if m.Role != "user" {
				continue
			}
			parts := ContentParts(m.Content)
			if len(parts) > 0 {
				out = append(out, event.Message{ID: m.ID, Role: "user", Parts: parts, Injected: m.Origin.Kind != "" && m.Origin.Kind != "user"})
			}
		case "context.append_loop_event":
			e := rec.Event
			msg := event.Message{ID: e.UUID, Role: "assistant", Model: model}
			switch e.Type {
			case "content.part":
				if e.Part.Type == "text" {
					msg.Parts = []event.Part{{Kind: event.Text, Text: e.Part.Text}}
				}
				if e.Part.Type == "think" {
					msg.Parts = []event.Part{{Kind: event.Thinking, Text: e.Part.Think}}
				}
			case "tool.call":
				c := kimiCall(e.ToolCallID, e.Name, e.Args)
				if c.Kind == tool.Subagent {
					c.Input.Child = c.ID // its run, found by Kimi.FindChild
				}
				msg.Parts = []event.Part{{Kind: event.ToolCall, Call: &c}}
			case "tool.result":
				var body string
				if jsonx.Unmarshal(e.Result.Output, &body) != nil {
					body = string(e.Result.Output)
				}
				msg.Role = "user"
				msg.Parts = []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: e.ToolCallID, Text: body, IsError: e.Result.IsError}}}
			}
			if len(msg.Parts) > 0 {
				out = append(out, msg)
			}
		case "usage.record":
			u := rec.Usage
			tokens.Add(usage.TokenUsage{Input: u.InputOther, Output: u.Output, CacheRead: u.InputCacheRead, CacheWrite5m: u.InputCacheCreation})
		case "turn.ended":
			reason := "done"
			switch rec.Reason {
			case "cancelled":
				reason = "interrupted"
			case "failed":
				reason = "error"
			}
			out = append(out, event.TurnEnd{Reason: reason, Err: rec.Error.Message, Tokens: tokens, Duration: time.Duration(rec.DurationMs) * time.Millisecond, Turns: 1})
			tokens = usage.TokenUsage{}
		case "task.started", "task.terminated":
			// Work beside the turn: a background shell ("process") or subagent.
			t := rec.Info
			if rec.Type == "task.terminated" {
				out = append(out, event.TaskDone{ID: t.TaskID, CallID: t.ParentToolCallID, Status: t.Status})
				continue
			}
			kind := event.ShellTask
			if t.Kind == "agent" {
				kind = event.SubagentTask
			}
			out = append(out, event.TaskStarted{ID: t.TaskID, CallID: t.ParentToolCallID, Kind: kind, Label: t.Description, Agent: t.SubagentType, Background: t.Detached})
		case "context.apply_compaction":
			out = append(out, event.Compacted{Before: rec.TokensBefore, After: rec.TokensAfter})
		}
	}
	return EndHistoryTurns(out), scan.Err()
}

// Current Kimi shares these tool inputs with Claude; older Kimi spellings
// are kept for migrated sessions. Unknown tools retain their raw input.
var kimiToolKinds = map[string]tool.Kind{
	"Bash": tool.Shell, "Shell": tool.Shell,
	"Read": tool.Read, "ReadFile": tool.Read,
	"Write": tool.Write, "WriteFile": tool.Write,
	"Edit": tool.Edit, "StrReplaceFile": tool.Edit,
	"Grep": tool.Search, "Glob": tool.Glob,
	"Agent": tool.Subagent, "Task": tool.Subagent,
	"AskUserQuestion": tool.Question,
	"EnterPlanMode":   tool.PlanMode, "ExitPlanMode": tool.PlanMode,
	"FetchURL": tool.Fetch, "WebSearch": tool.WebSearch,
	// The older Python Kimi's, and Vibe's.
	"SearchWeb": tool.WebSearch, "Think": tool.Think,
	"bash": tool.Shell, "read_file": tool.Read, "write_file": tool.Write, "grep": tool.Search,
	"web_fetch": tool.Fetch, "web_search": tool.WebSearch, "task": tool.Subagent,
}

func kimiCall(id, name string, raw jsontext.Value) tool.Call {
	kind := kimiToolKinds[name]
	return tool.Call{ID: id, Name: name, Kind: kind, Raw: raw, Input: readInput(kind, raw, nil, nil)}
}
