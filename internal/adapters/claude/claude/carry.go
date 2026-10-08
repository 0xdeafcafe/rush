package claude

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"os"
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// WriteCarried writes lines, another agent's conversation, as the
// transcript of session sessionID in cwd, for Claude Code to resume as its
// own: your messages as user lines, its answers as synthetic assistant
// ones, each after the last.
func (a Account) WriteCarried(cwd, sessionID string, lines []agent.Line) error {
	p := a.TranscriptPath(cwd, sessionID)
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		return err
	}
	f, err := os.OpenFile(p, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600) // a start that died left one nothing resumed from
	if err != nil {
		return err
	}
	w := bufio.NewWriter(f)
	parent := any(nil)
	for _, l := range lines {
		at := l.At
		if at.IsZero() {
			at = time.Now()
		}
		id := uuid()
		line := map[string]any{"parentUuid": parent, "isSidechain": false, "userType": "external", "cwd": cwd,
			"sessionId": sessionID, "type": l.Role, "uuid": id, "timestamp": at.UTC().Format(time.RFC3339Nano)}
		if l.Role == "assistant" {
			line["message"] = map[string]any{"id": "msg_" + id, "type": "message", "role": "assistant", "model": "<synthetic>",
				"content": []map[string]any{{"type": "text", "text": l.Text}}, "stop_reason": "end_turn", "stop_sequence": nil,
				"usage": map[string]any{"input_tokens": 0, "output_tokens": 0}}
		} else {
			line["message"] = map[string]any{"role": "user", "content": l.Text}
		}
		b, _ := jsonx.Marshal(line)
		_, _ = w.Write(append(b, '\n'))
		parent = id
	}
	if err := w.Flush(); err != nil {
		f.Close()
		os.Remove(p)
		return err
	}
	return f.Close()
}

// uuid is a random (v4) UUID.
func uuid() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	b[6], b[8] = b[6]&0x0f|0x40, b[8]&0x3f|0x80
	h := hex.EncodeToString(b[:])
	return h[0:8] + "-" + h[8:12] + "-" + h[12:16] + "-" + h[16:20] + "-" + h[20:]
}
