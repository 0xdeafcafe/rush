package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
)

// A made-up Claude Code conversation, written both as Claude Code keeps
// it (its transcript) and as its host replays it to a view (stream-json).

type conv struct {
	id     string // the session's uuid
	cwd    string
	branch string
	model  string
	turns  []turn
	// subs are subagent runs, each with a transcript of its own.
	subs []sub
}

type turn struct {
	prompt string
	at     time.Time
	steps  []step
	answer string
	// open is a turn still running: no answer, no result.
	open bool
}

// step is one thing the model did: some words, a tool call, or both.
type step struct {
	text   string
	tool   string
	input  map[string]any
	result string
	isErr  bool
	extra  map[string]any // Claude Code's structured result (toolUseResult)
	took   time.Duration
	// running is a call without its result yet.
	running bool
	// in and out are the request's tokens; zero picks something plausible.
	in, out int
}

// sub is a subagent run: the Agent call that started it is a step of the
// main conversation with the same id.
type sub struct {
	id, toolUse, kind, desc string
	steps                   []step
	start                   time.Time
	done                    bool
	answer                  string
}

type line = map[string]any

// ids makes the conversation's ids, the same from run to run.
type ids struct {
	prefix string
	n      int
}

func (i *ids) next(kind string) string {
	i.n++
	return fmt.Sprintf("%s_%s%06d", kind, i.prefix, i.n)
}

// tokens is a request's tokens: most of the context read from the cache.
func tokens(ctx, in, out int) map[string]any {
	return map[string]any{"input_tokens": in, "output_tokens": out, "cache_read_input_tokens": ctx, "cache_creation_input_tokens": in * 3}
}

// write puts the transcript where Claude Code keeps it, and returns the
// host's replay of it.
func (c *conv) write(projects string) ([]string, error) {
	dir := filepath.Join(projects, claude.ProjectSlug(c.cwd))
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, err
	}
	path := filepath.Join(dir, c.id+".jsonl")
	tr, replay, last := c.lines()
	if err := writeLines(path, tr); err != nil {
		return nil, err
	}
	_ = os.Chtimes(path, last, last)
	for _, s := range c.subs {
		sd := filepath.Join(strings.TrimSuffix(path, ".jsonl"), "subagents")
		if err := os.MkdirAll(sd, 0o755); err != nil {
			return nil, err
		}
		meta := map[string]any{"toolUseId": s.toolUse, "agentType": s.kind, "description": s.desc, "spawnDepth": 1}
		b, _ := jsonx.Marshal(meta)
		if err := os.WriteFile(filepath.Join(sd, "agent-"+s.id+".meta.json"), b, 0o644); err != nil {
			return nil, err
		}
		sc := conv{id: c.id, cwd: c.cwd, branch: c.branch, model: c.model,
			turns: []turn{{prompt: s.desc, at: s.start, steps: s.steps, answer: s.answer, open: !s.done}}}
		str, _, slast := sc.linesAs(s.id)
		p := filepath.Join(sd, "agent-"+s.id+".jsonl")
		if err := writeLines(p, str); err != nil {
			return nil, err
		}
		_ = os.Chtimes(p, slast, slast)
	}
	return replay, nil
}

func (c *conv) lines() (tr, replay []string, last time.Time) {
	return c.linesAs("")
}

// linesAs writes the conversation, as a subagent's own when agentID is set.
func (c *conv) linesAs(agentID string) (tr, replay []string, last time.Time) {
	id := &ids{prefix: strings.ReplaceAll(c.id[:8], "-", "") + agentID}
	parent := ""
	ctx := 90_000
	put := func(at time.Time, l line) {
		l["uuid"] = id.next("u")
		if parent != "" {
			l["parentUuid"] = parent
		} else {
			l["parentUuid"] = nil
		}
		parent = l["uuid"].(string)
		l["timestamp"] = at.UTC().Format(time.RFC3339Nano)
		l["sessionId"] = c.id
		l["cwd"] = c.cwd
		l["gitBranch"] = c.branch
		l["version"] = "2.1.280"
		l["userType"] = "external"
		l["isSidechain"] = agentID != ""
		if agentID != "" {
			l["agentId"] = agentID
		}
		b, _ := jsonx.Marshal(l)
		tr = append(tr, string(b))
		if at.After(last) {
			last = at
		}
	}
	stamp := func(at time.Time) {
		b, _ := jsonx.Marshal(map[string]any{"type": "agtop_time", "t": at.UnixMilli()})
		replay = append(replay, string(b))
	}
	emit := func(l line) {
		b, _ := jsonx.Marshal(l)
		replay = append(replay, string(b))
	}
	if agentID == "" {
		emit(line{"type": "system", "subtype": "init", "session_id": c.id, "model": c.model, "cwd": c.cwd,
			"permissionMode": "auto", "claude_code_version": "2.1.280", "apiKeySource": "none",
			"tools": []string{"Bash", "Read", "Edit", "Write", "Grep", "Glob", "Agent", "TodoWrite", "WebFetch"}})
	}
	for ti, t := range c.turns {
		at := t.at
		stamp(at)
		emit(line{"type": "user", "message": line{"role": "user", "content": t.prompt}, "agtop_sent": true})
		put(at, line{"type": "user", "message": line{"role": "user", "content": t.prompt}})
		var cost float64
		for _, s := range t.steps {
			at = at.Add(4 * time.Second)
			in, out := s.in, s.out
			if in == 0 {
				in = 2500 + len(s.result)/3
			}
			if out == 0 {
				out = 1400 + len(s.text)/2
			}
			ctx += in
			var content []any
			if s.text != "" {
				content = append(content, line{"type": "text", "text": s.text})
			}
			useID := ""
			if s.tool != "" {
				useID = id.next("toolu")
				if s.tool == "Agent" {
					for i := range c.subs {
						if c.subs[i].desc == s.input["description"] {
							useID = c.subs[i].toolUse
						}
					}
				}
				content = append(content, line{"type": "tool_use", "id": useID, "name": s.tool, "input": s.input})
			}
			msg := line{"id": id.next("msg"), "type": "message", "role": "assistant", "model": c.model,
				"content": content, "stop_reason": "tool_use", "usage": tokens(ctx, in, out)}
			cost += claude.Cost(c.model, claude.TokenUsage{Input: int64(in), Output: int64(out), CacheRead: int64(ctx), CacheWrite5m: int64(in * 3)}, false)
			stamp(at)
			emit(line{"type": "assistant", "message": msg, "parent_tool_use_id": nil, "session_id": c.id})
			put(at, line{"type": "assistant", "message": msg, "requestId": id.next("req")})
			if s.tool == "" || s.running {
				continue
			}
			at = at.Add(max(s.took, 200*time.Millisecond))
			res := line{"type": "tool_result", "tool_use_id": useID, "content": s.result}
			if s.isErr {
				res["is_error"] = true
			}
			um := line{"role": "user", "content": []any{res}}
			ul := line{"type": "user", "message": um, "parent_tool_use_id": nil, "session_id": c.id}
			tl := line{"type": "user", "message": um}
			if s.extra != nil {
				ul["tool_use_result"] = s.extra
				tl["toolUseResult"] = s.extra
			}
			stamp(at)
			emit(ul)
			put(at, tl)
		}
		if t.open {
			continue
		}
		at = at.Add(6 * time.Second)
		ctx += 1200
		msg := line{"id": id.next("msg"), "type": "message", "role": "assistant", "model": c.model,
			"content": []any{line{"type": "text", "text": t.answer}}, "stop_reason": "end_turn", "usage": tokens(ctx, 1200, 400+len(t.answer)/3)}
		stamp(at)
		emit(line{"type": "assistant", "message": msg, "parent_tool_use_id": nil, "session_id": c.id})
		put(at, line{"type": "assistant", "message": msg, "requestId": id.next("req")})
		stamp(at)
		emit(line{"type": "result", "subtype": "success", "is_error": false, "result": t.answer, "session_id": c.id,
			"total_cost_usd": cost, "duration_ms": at.Sub(t.at).Milliseconds(), "num_turns": ti + 1,
			"usage": tokens(ctx, 1200, 400)})
	}
	return tr, replay, last
}

func writeLines(path string, ls []string) error {
	return os.WriteFile(path, []byte(strings.Join(ls, "\n")+"\n"), 0o644)
}
