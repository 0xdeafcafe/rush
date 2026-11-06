package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

// A made-up Claude Code conversation, written as Claude Code keeps it: a
// transcript under ~/.claude/projects, and its subagents' beside it.

type conv struct {
	id     string // the session's uuid
	cwd    string
	branch string
	model  string
	// title is the session's name, as /rename would set it; empty uses the
	// first prompt.
	title string
	turns []turn
	// subs are subagent runs, each with a transcript of its own.
	subs []sub
}

type turn struct {
	prompt string
	at     time.Time
	steps  []step
	answer string
	// open is a turn still running: no answer.
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
// main conversation with the same tool use id.
type sub struct {
	id, toolUse, kind, desc string
	steps                   []step
	start                   time.Time
	done                    bool
	answer                  string
}

type line = map[string]any

// slug is the folder Claude Code keeps a project's transcripts in.
var slugRe = regexp.MustCompile(`[^A-Za-z0-9]`)

func slug(dir string) string { return slugRe.ReplaceAllString(dir, "-") }

// compact is a line as Claude Code writes it: no spaces, no HTML escapes,
// so rush's quick byte searches (`"type":"user"`) find it.
func compact(v any) string {
	var b bytes.Buffer
	e := json.NewEncoder(&b)
	e.SetEscapeHTML(false)
	if err := e.Encode(v); err != nil {
		panic(err)
	}
	return strings.TrimRight(b.String(), "\n")
}

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

// write puts the transcript, and its subagents', where Claude Code keeps
// them.
func (c *conv) write(projects string) error {
	dir := filepath.Join(projects, slug(c.cwd))
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	path := filepath.Join(dir, c.id+".jsonl")
	tr, last := c.lines("")
	if c.title != "" {
		// Written by hand: rush finds a title by the line's first bytes.
		tr = append(tr, fmt.Sprintf(`{"type":"custom-title","customTitle":%s,"sessionId":%q}`, compact(c.title), c.id))
	}
	if err := writeLines(path, tr); err != nil {
		return err
	}
	if err := os.Chtimes(path, last, last); err != nil {
		return err
	}
	for _, s := range c.subs {
		sd := filepath.Join(strings.TrimSuffix(path, ".jsonl"), "subagents")
		if err := os.MkdirAll(sd, 0o755); err != nil {
			return err
		}
		meta := compact(map[string]any{"toolUseId": s.toolUse, "agentType": s.kind, "description": s.desc, "spawnDepth": 1})
		if err := os.WriteFile(filepath.Join(sd, "agent-"+s.id+".meta.json"), []byte(meta), 0o644); err != nil {
			return err
		}
		sc := conv{id: c.id, cwd: c.cwd, branch: c.branch, model: c.model,
			turns: []turn{{prompt: s.desc, at: s.start, steps: s.steps, answer: s.answer, open: !s.done}}}
		str, slast := sc.lines(s.id)
		p := filepath.Join(sd, "agent-"+s.id+".jsonl")
		if err := writeLines(p, str); err != nil {
			return err
		}
		if err := os.Chtimes(p, slast, slast); err != nil {
			return err
		}
	}
	return nil
}

// lines writes the conversation, as a subagent's own when agentID is set,
// and says when its last line was written.
func (c *conv) lines(agentID string) (tr []string, last time.Time) {
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
		l["timestamp"] = at.UTC().Format("2006-01-02T15:04:05.000Z")
		l["sessionId"] = c.id
		l["cwd"] = c.cwd
		l["gitBranch"] = c.branch
		l["version"] = "2.1.280"
		l["userType"] = "external"
		l["isSidechain"] = agentID != ""
		if agentID != "" {
			l["agentId"] = agentID
		}
		tr = append(tr, compact(l))
		if at.After(last) {
			last = at
		}
	}
	for _, t := range c.turns {
		at := t.at
		put(at, line{"type": "user", "message": line{"role": "user", "content": t.prompt}})
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
			put(at, line{"type": "assistant", "message": msg, "requestId": id.next("req")})
			if s.tool == "" || s.running {
				continue
			}
			at = at.Add(max(s.took, 200*time.Millisecond))
			res := line{"type": "tool_result", "tool_use_id": useID, "content": s.result}
			if s.isErr {
				res["is_error"] = true
			}
			tl := line{"type": "user", "message": line{"role": "user", "content": []any{res}}}
			if s.extra != nil {
				tl["toolUseResult"] = s.extra
			}
			put(at, tl)
		}
		if t.open {
			continue
		}
		at = at.Add(6 * time.Second)
		ctx += 1200
		msg := line{"id": id.next("msg"), "type": "message", "role": "assistant", "model": c.model,
			"content": []any{line{"type": "text", "text": t.answer}}, "stop_reason": "end_turn", "usage": tokens(ctx, 1200, 400+len(t.answer)/3)}
		put(at, line{"type": "assistant", "message": msg, "requestId": id.next("req")})
	}
	return tr, last
}

func writeLines(path string, ls []string) error {
	return os.WriteFile(path, []byte(strings.Join(ls, "\n")+"\n"), 0o644)
}
