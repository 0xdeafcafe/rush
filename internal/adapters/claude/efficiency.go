package claude

import (
	"bytes"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/efficiency"
)

// Claude Code is efficiency's Source: its transcripts, read line by line
// into efficiency's figures, and its setup, for savers to be found in.

var (
	_ efficiency.Source = Adapter{}
	_ agent.FastPricer  = Adapter{}
)

// Transcripts lists p's transcripts: sessions' and their subagents'.
func (Adapter) Transcripts(p agent.Profile) []string {
	dir := Account(p).ProjectsDir()
	main, _ := filepath.Glob(filepath.Join(dir, "*", "*.jsonl"))
	subs, _ := filepath.Glob(filepath.Join(dir, "*", "*", "subagents", "*.jsonl"))
	return append(main, subs...)
}

// TranscriptsDir is p's projects folder.
func (Adapter) TranscriptsDir(p agent.Profile) string { return Account(p).ProjectsDir() }

// CostFast is what model's tokens cost in fast mode.
func (Adapter) CostFast(model string, u usage.TokenUsage) (float64, bool) {
	if _, ok := claude.PriceFor(model); !ok {
		return 0, false
	}
	return claude.Cost(model, u, true), true
}

// Setup reads p's settings, plugins and MCP servers.
func (Adapter) Setup(p agent.Profile) efficiency.Setup {
	a := Account(p)
	s := efficiency.Setup{
		Enabled: map[string]bool{}, Plugins: map[string]time.Time{}, MCP: map[string]bool{},
		Backup: []string{filepath.Join(a.ConfigDir, "settings.json"), filepath.Join(a.ConfigDir, "CLAUDE.md"), a.StatePath()},
		Env:    a.Env(),
	}
	s.Settings, _ = claude.LoadSettings(a)
	if s.Settings == nil {
		s.Settings, _ = claude.LoadSettingsFile(os.DevNull)
	}
	var hooks map[string][]struct {
		Hooks []struct {
			Command string `json:"command"`
		} `json:"hooks"`
	}
	s.Settings.Get("hooks", &hooks)
	for _, groups := range hooks {
		for _, g := range groups {
			for _, h := range g.Hooks {
				s.Hooks = append(s.Hooks, h.Command)
			}
		}
	}
	s.Settings.Get("enabledPlugins", &s.Enabled)
	var inst struct {
		Plugins map[string][]struct {
			Scope       string    `json:"scope"`
			InstalledAt time.Time `json:"installedAt"`
		} `json:"plugins"`
	}
	if b, err := os.ReadFile(filepath.Join(a.ConfigDir, "plugins", "installed_plugins.json")); err == nil {
		_ = jsonx.Unmarshal(b, &inst)
	}
	for id, list := range inst.Plugins {
		for _, pl := range list {
			if pl.Scope == "user" || s.Plugins[id].IsZero() {
				s.Plugins[id] = pl.InstalledAt
			}
		}
	}
	var st struct {
		MCP map[string]jsontext.Value `json:"mcpServers"`
	}
	if b, err := os.ReadFile(a.StatePath()); err == nil {
		_ = jsonx.Unmarshal(b, &st)
	}
	for name := range st.MCP {
		s.MCP[name] = true
	}
	return s
}

type rawLine struct {
	Type      string    `json:"type"`
	Subtype   string    `json:"subtype"`
	Timestamp time.Time `json:"timestamp"`
	Cwd       string    `json:"cwd"`
	SessionID string    `json:"sessionId"`
	Message   struct {
		ID      string         `json:"id"`
		Model   string         `json:"model"`
		Content jsontext.Value `json:"content"`
		Usage   *struct {
			Input       int64  `json:"input_tokens"`
			Output      int64  `json:"output_tokens"`
			CacheRead   int64  `json:"cache_read_input_tokens"`
			CacheCreate int64  `json:"cache_creation_input_tokens"`
			Speed       string `json:"speed"`
			Details     *struct {
				Thinking int64 `json:"thinking_tokens"`
			} `json:"output_tokens_details"`
			CacheBreakup *struct {
				M5 int64 `json:"ephemeral_5m_input_tokens"`
				H1 int64 `json:"ephemeral_1h_input_tokens"`
			} `json:"cache_creation"`
		} `json:"usage"`
	} `json:"message"`
	Attachment *struct {
		Type      string `json:"type"`
		HookEvent string `json:"hookEvent"`
		Command   string `json:"command"`
		Skills    []struct {
			Name string `json:"name"`
		} `json:"skills"`
	} `json:"attachment"`
	Compact *struct {
		Trigger string `json:"trigger"`
		Pre     int64  `json:"preTokens"`
		Post    int64  `json:"postTokens"`
	} `json:"compactMetadata"`
	Content jsontext.Value `json:"content"` // a system line's
}

type block struct {
	Type      string         `json:"type"`
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Input     jsontext.Value `json:"input"`
	ToolUseID string         `json:"tool_use_id"`
	Content   jsontext.Value `json:"content"`
}

// The lines worth decoding carry one of these; the rest (most of the bytes:
// snapshots, progress, queue operations) are skipped unread.
var markers = [][]byte{
	[]byte(`"type":"assistant"`),
	[]byte(`"tool_result"`),
	[]byte(`"hook_success"`),
	[]byte(`"compact_boundary"`),
	[]byte(`"invoked_skills"`),
	[]byte(`<command-name>`),
}

func interesting(b []byte) bool {
	for _, m := range markers {
		if bytes.Contains(b, m) {
			return true
		}
	}
	return false
}

// ReadLine reads one transcript line into f.
func (Adapter) ReadLine(f *efficiency.File, b []byte) {
	if !interesting(b) {
		return
	}
	var l rawLine
	if jsonx.Unmarshal(b, &l) != nil {
		return
	}
	at := l.Timestamp
	if at.IsZero() {
		return
	}
	f.Saw(at, l.SessionID, l.Cwd)
	switch l.Type {
	case "assistant":
		assistant(f, &l, at)
	case "user":
		user(f, &l, at)
	case "attachment":
		a := l.Attachment
		if a == nil {
			return
		}
		switch a.Type {
		case "hook_success":
			if a.Command != "" {
				f.Hook(a.Command, at)
			}
		case "invoked_skills":
			for _, s := range a.Skills {
				f.Skill(s.Name, at)
			}
		}
	case "system":
		if l.Subtype == "compact_boundary" && l.Compact != nil {
			f.Compacted(efficiency.Compact{At: at, Auto: l.Compact.Trigger == "auto", Pre: l.Compact.Pre, Post: l.Compact.Post})
		}
		var s string
		if jsonx.Unmarshal(l.Content, &s) == nil {
			slash(f, s, at)
		}
	}
}

func slash(f *efficiency.File, s string, at time.Time) {
	_, rest, ok := strings.Cut(s, "<command-name>")
	if !ok {
		return
	}
	name, _, ok := strings.Cut(rest, "</command-name>")
	if ok && name != "" && len(name) < 64 {
		f.Command(name, at)
	}
}

func assistant(f *efficiency.File, l *rawLine, at time.Time) {
	m := &l.Message
	if u := m.Usage; u != nil && m.Model != "" && m.Model != "<synthetic>" {
		tu := usage.TokenUsage{Input: u.Input, Output: u.Output, CacheRead: u.CacheRead}
		if cb := u.CacheBreakup; cb != nil && cb.M5+cb.H1 > 0 {
			tu.CacheWrite5m, tu.CacheWrite1h = cb.M5, cb.H1
		} else {
			tu.CacheWrite5m = u.CacheCreate
		}
		var think int64
		if u.Details != nil {
			think = u.Details.Thinking
		}
		f.Request(m.ID, m.Model, tu, think, u.Speed == "fast", at)
	}
	if !bytes.Contains(m.Content, []byte(`"tool_use"`)) {
		return
	}
	var blocks []block
	if jsonx.Unmarshal(m.Content, &blocks) != nil {
		return
	}
	for _, bl := range blocks {
		if bl.Type != "tool_use" {
			continue
		}
		switch {
		case bl.Name == "Skill":
			var in struct {
				Skill string `json:"skill"`
			}
			if jsonx.Unmarshal(bl.Input, &in) == nil && in.Skill != "" {
				f.Skill(in.Skill, at)
			}
		case strings.HasPrefix(bl.Name, "mcp__"):
			server, _, _ := strings.Cut(strings.TrimPrefix(bl.Name, "mcp__"), "__")
			f.MCP(server, at)
		}
		c := claude.Call(bl.ID, bl.Name, bl.Input)
		f.Call(bl.ID, claude.KindOf(bl.Name), bl.Name, &c.Input, at)
	}
}

func user(f *efficiency.File, l *rawLine, at time.Time) {
	c := l.Message.Content
	if len(c) > 0 && c[0] == '"' {
		var s string
		if jsonx.Unmarshal(c, &s) == nil {
			slash(f, s, at)
		}
		return
	}
	if !bytes.Contains(c, []byte(`"tool_result"`)) {
		return
	}
	var blocks []block
	if jsonx.Unmarshal(c, &blocks) != nil {
		return
	}
	for _, bl := range blocks {
		if bl.Type == "tool_result" {
			f.Result(bl.ToolUseID, resultSize(bl.Content), at)
		}
	}
}

// resultSize is how much text a tool result gave back; images count as
// nothing, being tokens of another kind.
func resultSize(raw jsontext.Value) int64 {
	if len(raw) == 0 {
		return 0
	}
	if raw[0] == '"' {
		return int64(len(raw) - 2)
	}
	var parts []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if jsonx.Unmarshal(raw, &parts) != nil {
		return int64(len(raw))
	}
	var n int64
	for _, p := range parts {
		n += int64(len(p.Text))
	}
	return n
}
