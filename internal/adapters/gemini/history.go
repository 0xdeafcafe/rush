package gemini

import (
	"bufio"
	"encoding/json/jsontext"
	"maps"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/acp"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

type savedMessage struct {
	Injected  bool           `json:"injected"`
	ID        string         `json:"id"`
	Type      string         `json:"type"`
	Timestamp time.Time      `json:"timestamp"`
	Model     string         `json:"model"`
	Content   jsontext.Value `json:"content"`
	Thoughts  []struct {
		Subject     string `json:"subject"`
		Description string `json:"description"`
	} `json:"thoughts"`
	Tokens *struct {
		Input    int64 `json:"input"`
		Output   int64 `json:"output"`
		Cached   int64 `json:"cached"`
		Thoughts int64 `json:"thoughts"`
	} `json:"tokens"`
	Tools []struct {
		ID     string         `json:"id"`
		Name   string         `json:"name"`
		Args   jsontext.Value `json:"args"`
		Result jsontext.Value `json:"result"`
		Status string         `json:"status"`
	} `json:"toolCalls"`
}
type savedConversation struct {
	ID          string         `json:"sessionId"`
	Summary     string         `json:"summary"`
	Start       time.Time      `json:"startTime"`
	Updated     time.Time      `json:"lastUpdated"`
	Directories []string       `json:"directories"`
	Messages    []savedMessage `json:"messages"`
}

// readConversation handles both legacy JSON snapshots and append-only JSONL,
// including updated messages, rewinds and compacted replacement histories.
func readConversation(path string) (savedConversation, error) {
	var c savedConversation
	f, err := os.Open(path)
	if err != nil {
		return c, err
	}
	defer f.Close()
	if strings.HasSuffix(path, ".json") {
		err = jsonx.Decode(f, &c)
		return c, err
	}
	scan := bufio.NewScanner(f)
	scan.Buffer(make([]byte, 64<<10), 64<<20)
	indexes := map[string]int{}
	rebuild := func() {
		clear(indexes)
		for i, m := range c.Messages {
			indexes[m.ID] = i
		}
	}
	for scan.Scan() {
		var record struct {
			savedMessage
			Rewind  string         `json:"$rewindTo"`
			Set     jsontext.Value `json:"$set"`
			Session string         `json:"sessionId"`
		}
		if jsonx.Unmarshal(scan.Bytes(), &record) != nil {
			continue
		}
		switch {
		case record.Rewind != "":
			if i, ok := indexes[record.Rewind]; ok {
				c.Messages = c.Messages[:i]
			} else {
				c.Messages = nil
			}
			rebuild()
		case record.ID != "":
			m := record.savedMessage
			if i, ok := indexes[m.ID]; ok {
				c.Messages[i] = m
			} else {
				indexes[m.ID] = len(c.Messages)
				c.Messages = append(c.Messages, m)
			}
		case len(record.Set) > 0:
			// Decode against the existing metadata, while detecting an explicit empty replacement.
			var patch map[string]jsontext.Value
			if jsonx.Unmarshal(record.Set, &patch) != nil {
				continue
			}
			if v, ok := patch["summary"]; ok {
				_ = jsonx.Unmarshal(v, &c.Summary)
			}
			if v, ok := patch["lastUpdated"]; ok {
				_ = jsonx.Unmarshal(v, &c.Updated)
			}
			if v, ok := patch["directories"]; ok {
				_ = jsonx.Unmarshal(v, &c.Directories)
			}
			if v, ok := patch["messages"]; ok {
				_ = jsonx.Unmarshal(v, &c.Messages)
				rebuild()
			}
		case record.Session != "":
			_ = jsonx.Unmarshal(scan.Bytes(), &c)
			rebuild()
		}
	}
	return c, scan.Err()
}

// The fleet polls saved sessions repeatedly. Keep only metadata between polls,
// invalidating on file changes, rather than decoding all conversation images again.
var discoveryCache = struct {
	sync.Mutex
	rows map[string]cachedSession
}{rows: make(map[string]cachedSession)}

type cachedSession struct {
	size int64
	mod  time.Time
	row  agent.Session
}

func (Adapter) Live(agent.Profile) []agent.Session { return nil }
func (a Adapter) Past(p agent.Profile) []agent.Session {
	if p.Dir == "" {
		h, _ := os.UserHomeDir()
		p.Dir = filepath.Join(h, ".gemini")
	}
	paths, _ := filepath.Glob(filepath.Join(p.Dir, "tmp", "*", "chats", "session-*.json*"))
	var out []agent.Session
	seen := map[string]int{}
	for _, path := range paths {
		if !strings.HasSuffix(path, ".json") && !strings.HasSuffix(path, ".jsonl") {
			continue
		}
		stat, err := os.Stat(path)
		if err != nil {
			continue
		}
		discoveryCache.Lock()
		cached, hit := discoveryCache.rows[path]
		discoveryCache.Unlock()
		if hit && cached.size == stat.Size() && cached.mod.Equal(stat.ModTime()) {
			row := cached.row
			row.Profile = p
			if i, ok := seen[row.ID]; ok {
				if strings.HasSuffix(path, ".jsonl") {
					out[i] = row
				}
			} else {
				seen[row.ID] = len(out)
				out = append(out, row)
			}
			continue
		}
		c, err := readConversation(path)
		if err != nil || c.ID == "" {
			continue
		}
		cwd := ""
		if len(c.Directories) > 0 {
			cwd = c.Directories[0]
		} else {
			b, _ := os.ReadFile(filepath.Join(filepath.Dir(filepath.Dir(path)), ".project_root"))
			cwd = strings.TrimSpace(string(b))
		}
		name := c.Summary
		model := ""
		for _, m := range c.Messages {
			if name == "" && m.Type == "user" {
				for _, p := range acp.ContentParts(m.Content) {
					if p.Kind == event.Text {
						name = p.Text
						break
					}
				}
			}
			if m.Model != "" {
				model = m.Model
			}
		}
		row := agent.Session{Kind: Kind, Profile: p, ID: c.ID, Name: name, Cwd: cwd, Model: model, Transcript: path, State: "done", CreatedAt: c.Start, UpdatedAt: c.Updated}
		discoveryCache.Lock()
		if len(discoveryCache.rows) >= 2048 {
			clear(discoveryCache.rows)
		}
		discoveryCache.rows[path] = cachedSession{size: stat.Size(), mod: stat.ModTime(), row: row}
		discoveryCache.Unlock()
		if i, ok := seen[c.ID]; ok {
			if strings.HasSuffix(path, ".jsonl") {
				out[i] = row
			}
		} else {
			seen[c.ID] = len(out)
			out = append(out, row)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}
func (Adapter) History(s agent.Session, before time.Time) ([]event.Event, error) {
	c, err := readConversation(s.Transcript)
	if err != nil {
		return nil, err
	}
	var out []event.Event
	for _, m := range c.Messages {
		if !before.IsZero() && !m.Timestamp.IsZero() && !m.Timestamp.Before(before) {
			continue
		}
		role := m.Type
		if role == "gemini" {
			role = "assistant"
		}
		if role != "user" && role != "assistant" {
			continue
		}
		msg := event.Message{Role: role, ID: m.ID, Model: m.Model, Injected: m.Injected}
		for _, t := range m.Thoughts {
			msg.Parts = append(msg.Parts, event.Part{Kind: event.Thinking, Text: strings.TrimSpace(t.Subject + "\n" + t.Description)})
		}
		msg.Parts = append(msg.Parts, acp.ContentParts(m.Content)...)
		if t := m.Tokens; t != nil {
			msg.Tokens = &usage.TokenUsage{Input: t.Input, Output: t.Output, CacheRead: t.Cached, Reasoning: t.Thoughts}
		}
		var results []event.Part
		for _, t := range m.Tools {
			msg.Parts = append(msg.Parts, event.Part{Kind: event.ToolCall, Call: &tool.Call{ID: t.ID, Name: t.Name, Raw: t.Args}})
			if len(t.Result) > 0 && string(t.Result) != "null" {
				results = append(results, event.Part{Kind: event.ToolResult, Output: &tool.Output{CallID: t.ID, Text: string(t.Result), IsError: t.Status == "error"}})
			}
		}
		if len(msg.Parts) > 0 {
			out = append(out, msg)
		}
		if len(results) > 0 {
			out = append(out, event.Message{Role: "user", Parts: results})
		}
	}
	return acp.EndHistoryTurns(out), nil
}
func (a Adapter) Features() map[agent.Feature]agent.Support {
	out := maps.Clone(a.Agent.Features())
	out[agent.FeatureSignIn] = agent.Yes.With("native terminal login; no account swapping")
	out[agent.FeatureHistory] = agent.Yes.With("saved local conversations")
	return out
}

var _ agent.Discoverer = Adapter{}
var _ agent.HistoryReader = Adapter{}
