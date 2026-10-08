package advisor

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A brief is a transcript cut down to what the advisor needs: one short
// line for each prompt, tool call and compaction, with what each tool sent
// back and how big the context was. A transcript line can be hundreds of
// kilobytes; a brief line is at most briefLine bytes, so Haiku can read a
// whole session for cents.
const (
	briefLine = 160
	briefHead = 100 // lines kept from a session's start
	briefTail = 400 // and from its end: about 16k tokens in all
)

// Briefs writes a brief of each transcript into dir, and returns their
// paths in the same order ("" where one couldn't be read).
func Briefs(dir string, transcripts []string) []string {
	_ = os.RemoveAll(dir)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return make([]string, len(transcripts))
	}
	out := make([]string, len(transcripts))
	for i, t := range transcripts {
		lines, err := brief(t)
		if err != nil {
			continue
		}
		p := filepath.Join(dir, fmt.Sprintf("%d-%s.txt", i+1, strings.TrimSuffix(filepath.Base(t), ".jsonl")))
		if os.WriteFile(p, []byte(strings.Join(lines, "\n")+"\n"), 0o600) == nil {
			out[i] = p
		}
	}
	return out
}

type briefEntry struct {
	Type      string    `json:"type"`
	Subtype   string    `json:"subtype"`
	Timestamp time.Time `json:"timestamp"`
	Message   *struct {
		Content jsontext.Value `json:"content"`
		Usage   *struct {
			In int64 `json:"input_tokens"`
			CR int64 `json:"cache_read_input_tokens"`
			CW int64 `json:"cache_creation_input_tokens"`
		} `json:"usage"`
	} `json:"message"`
	Compact *struct {
		Trigger string `json:"trigger"`
		Pre     int64  `json:"preTokens"`
	} `json:"compactMetadata"`
}

type briefBlock struct {
	Type      string         `json:"type"`
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Text      string         `json:"text"`
	Input     jsontext.Value `json:"input"`
	ToolUseID string         `json:"tool_use_id"`
	Content   jsontext.Value `json:"content"`
}

func brief(path string) ([]string, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var lines []string
	calls := map[string]int{} // tool_use id → its line, for the size of what came back
	var ctx int64
	r := bufio.NewReaderSize(f, 1<<16)
	for {
		raw, err := r.ReadBytes('\n')
		if len(raw) > 0 {
			lines = briefOf(raw, lines, calls, &ctx)
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
	}
	if n := len(lines); n > briefHead+briefTail {
		skipped := fmt.Sprintf("… %d lines skipped …", n-briefHead-briefTail)
		lines = append(append(lines[:briefHead:briefHead], skipped), lines[n-briefTail:]...)
	}
	return lines, nil
}

func briefOf(raw []byte, lines []string, calls map[string]int, ctx *int64) []string {
	// Only these lines say anything a brief keeps; the rest aren't decoded.
	if !bytes.Contains(raw, []byte(`"type":"assistant"`)) && !bytes.Contains(raw, []byte(`"type":"user"`)) && !bytes.Contains(raw, []byte(`"compact_boundary"`)) {
		return lines
	}
	var e briefEntry
	if jsonx.Unmarshal(raw, &e) != nil {
		return lines
	}
	at := e.Timestamp.Local().Format("15:04")
	add := func(s string) {
		s = strings.Join(strings.Fields(s), " ")
		if len(s) > briefLine {
			s = strings.ToValidUTF8(s[:briefLine], "") + "…" // not half a character
		}
		lines = append(lines, s)
	}
	if e.Subtype == "compact_boundary" && e.Compact != nil {
		add(fmt.Sprintf("%s — compacted (%s) at %s", at, e.Compact.Trigger, tokens(e.Compact.Pre)))
		return lines
	}
	if e.Message == nil {
		return lines
	}
	var blocks []briefBlock
	if jsonx.Unmarshal(e.Message.Content, &blocks) != nil {
		// A prompt written as a plain string.
		var s string
		if e.Type == "user" && jsonx.Unmarshal(e.Message.Content, &s) == nil && s != "" {
			add(fmt.Sprintf("%s you (ctx %s): %s", at, tokens(*ctx), s))
		}
		return lines
	}
	if u := e.Message.Usage; u != nil {
		*ctx = u.In + u.CR + u.CW
	}
	for _, b := range blocks {
		switch {
		case e.Type == "user" && b.Type == "text" && b.Text != "":
			add(fmt.Sprintf("%s you (ctx %s): %s", at, tokens(*ctx), b.Text))
		case b.Type == "tool_use":
			calls[b.ID] = len(lines)
			add(fmt.Sprintf("%s %s: %s", at, b.Name, toolInput(b.Input)))
		case b.Type == "tool_result":
			if i, ok := calls[b.ToolUseID]; ok && i < len(lines) {
				lines[i] += fmt.Sprintf(" → %s", size(len(b.Content)))
			}
		}
	}
	return lines
}

// toolInput is the part of a tool call's input that says what it did.
func toolInput(raw jsontext.Value) string {
	var in map[string]any
	if jsonx.Unmarshal(raw, &in) != nil {
		return ""
	}
	for _, k := range []string{"command", "file_path", "pattern", "description", "prompt", "url", "query"} {
		if v, ok := in[k].(string); ok && v != "" {
			if k == "description" || k == "prompt" {
				if t, ok := in["subagent_type"].(string); ok {
					v = t + ", " + v
				}
				if m, ok := in["model"].(string); ok {
					v = m + ", " + v
				}
			}
			return v
		}
	}
	return ""
}

func tokens(n int64) string {
	if n >= 1000 {
		return fmt.Sprintf("%dk", n/1000)
	}
	return fmt.Sprint(n)
}

func size(n int) string {
	switch {
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
	case n >= 1<<10:
		return fmt.Sprintf("%d KB", n>>10)
	}
	return fmt.Sprintf("%d B", n)
}
