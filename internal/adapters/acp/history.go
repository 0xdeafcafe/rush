package acp

import (
	"bufio"
	"encoding/base64"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// ContentParts reads the text and inline images shared by provider transcripts.
// Remote image URLs remain visible as text; reading history never fetches them.
func ContentParts(raw jsontext.Value) []event.Part {
	var text string
	if jsonx.Unmarshal(raw, &text) == nil {
		if text == "" {
			return nil
		}
		return []event.Part{{Kind: event.Text, Text: text}}
	}
	var blocks []struct {
		Type     string `json:"type"`
		Text     string `json:"text"`
		ImageURL struct {
			URL string `json:"url"`
		} `json:"image_url"`
		InlineData *struct {
			MimeType string `json:"mimeType"`
			Data     string `json:"data"`
		} `json:"inlineData"`
	}
	if jsonx.Unmarshal(raw, &blocks) != nil {
		return nil
	}
	var parts []event.Part
	for _, b := range blocks {
		if b.Text != "" {
			parts = append(parts, event.Part{Kind: event.Text, Text: b.Text})
		}
		if b.InlineData != nil {
			if data, err := base64.StdEncoding.DecodeString(b.InlineData.Data); err == nil {
				parts = append(parts, event.Part{Kind: event.Image, Image: &event.ImageData{MediaType: b.InlineData.MimeType, Data: data}})
			}
		}
		if url := b.ImageURL.URL; url != "" {
			header, data, ok := strings.Cut(url, ",")
			if ok && strings.HasPrefix(header, "data:") && strings.HasSuffix(header, ";base64") {
				if decoded, err := base64.StdEncoding.DecodeString(data); err == nil {
					parts = append(parts, event.Part{Kind: event.Image, Image: &event.ImageData{MediaType: strings.TrimSuffix(strings.TrimPrefix(header, "data:"), ";base64"), Data: decoded}})
				}
			} else {
				parts = append(parts, event.Part{Kind: event.Text, Text: "[image: " + url + "]"})
			}
		}
	}
	return parts
}

// ReadMessages reads Kimi's context and Vibe's message log without starting an
// agent. Checkpoint/system records aren't conversation messages.
func ReadMessages(path string) ([]event.Event, error) { return ReadMessagesBefore(path, time.Time{}) }

func ReadMessagesBefore(path string, before time.Time) ([]event.Event, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	return readMessages(f, path, before)
}

// ReadMessagesTail seeks only within an append-only message log. Context files
// contain complete independent records, so unknown tool parents remain honest.
func ReadMessagesTail(path string, most int64) ([]event.Event, bool, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, false, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return nil, false, err
	}
	if st.Size() <= max(1, most) {
		evs, err := readMessages(f, path, time.Time{})
		return evs, false, err
	}
	r := bufio.NewReader(io.NewSectionReader(f, st.Size()-max(1, most)-1, max(1, most)+1))
	// Skip an incomplete first line without allocating its potentially huge body.
	for {
		_, err := r.ReadSlice('\n')
		if err == bufio.ErrBufferFull {
			continue
		}
		if err != nil && err != io.EOF {
			return nil, false, err
		}
		break
	}
	evs, err := readMessages(r, path, time.Time{})
	return evs, true, err
}

func readMessages(r io.Reader, transcript string, before time.Time) ([]event.Event, error) {
	scan := bufio.NewScanner(r)
	scan.Buffer(make([]byte, 64<<10), 32<<20)
	var out []event.Event
	line := 0
	for scan.Scan() {
		line++
		var m struct {
			Timestamp jsontext.Value `json:"timestamp"`
			Role      string         `json:"role"`
			ID        string         `json:"message_id"`
			Content   jsontext.Value `json:"content"`
			Reasoning jsontext.Value `json:"reasoning_content"`
			ToolID    string         `json:"tool_call_id"`
			Injected  bool           `json:"injected"`
			Images    []struct {
				Path      string `json:"path"`
				Data      string `json:"data"`
				MediaType string `json:"mime_type"`
				Source    struct {
					Kind string `json:"kind"`
					Path string `json:"path"`
					Data string `json:"data"`
				} `json:"source"`
			} `json:"images"`
			Calls []struct {
				ID       string `json:"id"`
				Function struct {
					Name      string `json:"name"`
					Arguments string `json:"arguments"`
				} `json:"function"`
			} `json:"tool_calls"`
		}
		if err := jsonx.Unmarshal(scan.Bytes(), &m); err != nil {
			return nil, fmt.Errorf("transcript line %d: %w", line, err)
		}
		if !before.IsZero() && len(m.Timestamp) > 0 {
			var stamp string
			_ = jsonx.Unmarshal(m.Timestamp, &stamp)
			if at, err := time.Parse(time.RFC3339Nano, stamp); err == nil && !at.Before(before) {
				continue
			}
		}
		if m.Role != "user" && m.Role != "assistant" && m.Role != "tool" {
			continue
		}
		msg := event.Message{Role: m.Role, ID: m.ID, Injected: m.Injected}
		for _, p := range ContentParts(m.Reasoning) {
			p.Kind = event.Thinking
			msg.Parts = append(msg.Parts, p)
		}
		parts := ContentParts(m.Content)
		if m.Role == "tool" {
			msg.Role = "user"
			var b strings.Builder
			for _, p := range parts {
				b.WriteString(p.Text)
			}
			msg.Parts = append(msg.Parts, event.Part{Kind: event.ToolResult, Output: &tool.Output{CallID: m.ToolID, Text: b.String()}})
		} else {
			msg.Parts = append(msg.Parts, parts...)
		}
		for _, image := range m.Images {
			path, data := image.Source.Path, image.Source.Data
			if path == "" {
				path = image.Path
			}
			if data == "" {
				data = image.Data
			}
			im := &event.ImageData{MediaType: image.MediaType}
			if path != "" {
				if !filepath.IsAbs(path) {
					path = filepath.Join(filepath.Dir(transcript), path)
				}
				im.Path = path
			}
			if data != "" {
				im.Data, _ = base64.StdEncoding.DecodeString(data)
			}
			if im.Path != "" || len(im.Data) > 0 {
				msg.Parts = append(msg.Parts, event.Part{Kind: event.Image, Image: im})
			}
		}
		for _, c := range m.Calls {
			call := kimiCall(c.ID, c.Function.Name, jsontext.Value(c.Function.Arguments))
			if call.Kind == tool.Subagent {
				call.Input.Child = call.ID // Vibe's child session, linked by the call
			}
			msg.Parts = append(msg.Parts, event.Part{Kind: event.ToolCall, Call: &call})
		}
		if len(msg.Parts) > 0 {
			out = append(out, msg)
		}
	}
	return EndHistoryTurns(out), scan.Err()
}

// EndHistoryTurns supplies the boundaries absent from message-only snapshots.
// A regular user message starts a new historical turn; tool results and messages
// explicitly marked injected stay in the current turn. The final snapshot turn
// is closed so viewing a saved conversation never leaves a working spinner.
func EndHistoryTurns(events []event.Event) []event.Event {
	out := make([]event.Event, 0, len(events)+1)
	open := false
	for _, ev := range events {
		switch m := ev.(type) {
		case event.Message:
			prompt := m.Role == "user" && !m.Injected && m.Parent == "" && len(m.Parts) > 0
			for _, p := range m.Parts {
				if p.Kind != event.Text && p.Kind != event.Image {
					prompt = false
					break
				}
			}
			if prompt && open {
				out = append(out, event.TurnEnd{Reason: "done", Turns: 1})
				open = false
			}
			if len(m.Parts) > 0 {
				open = true
			}
		case event.TurnEnd:
			open = false
		}
		out = append(out, ev)
	}
	if open {
		out = append(out, event.TurnEnd{Reason: "done", Turns: 1})
	}
	return out
}
