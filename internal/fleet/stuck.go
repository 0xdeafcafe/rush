package fleet

import (
	"bytes"
	"encoding/json/jsontext"
	"io"
	"os"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// StuckAfter is how long an agent at work may go without writing anything
// before it shows as stuck; StuckSubAfter is the same for a subagent run,
// whose prompt cache lasts five minutes, so a longer wait buys nothing.
const (
	StuckAfter    = 30 * time.Minute
	StuckSubAfter = 5 * time.Minute
)

// Stuck is an agent at work that has written nothing, nor have its
// subagents, for StuckAfter: a hung command, or a wait that never ends.
func (a *Agent) Stuck(now time.Time) bool {
	if a.Past || a.Done || a.Checking || !(a.State == "working" || a.Busy()) {
		return false
	}
	return a.Quiet(now) >= StuckAfter
}

// Quiet is how long since it, or any subagent of it, last wrote.
func (a *Agent) Quiet(now time.Time) time.Duration {
	last := a.UpdatedAt
	if a.ModTime.After(last) {
		last = a.ModTime
	}
	for _, s := range a.Subagents {
		if s.Mod.After(last) {
			last = s.Mod
		}
	}
	return now.Sub(last)
}

// StuckSubs are its subagent runs quiet for StuckSubAfter, waiting on the
// model. One running a command writes nothing until the command ends, as a
// build, a test suite or a benchmark can take many minutes: it's stuck only
// after StuckAfter, as an agent is.
func (a *Agent) StuckSubs(now time.Time) []SubagentTile {
	var out []SubagentTile
	for _, s := range a.Subagents {
		if s.Mod.IsZero() || now.Sub(s.Mod) < StuckSubAfter {
			continue
		}
		if now.Sub(s.Mod) < StuckAfter && inTool(s.Path, s.Mod) {
			continue
		}
		out = append(out, s)
	}
	return out
}

// inToolSeen is what inTool last found for each transcript, as of when it
// was last written: a quiet one isn't read again.
var inToolSeen sync.Map // path -> inToolAt

type inToolAt struct {
	mod time.Time
	in  bool
}

// inTool is whether a subagent's transcript, last written at mod, has a
// tool call still without its result: it's running a command.
func inTool(path string, mod time.Time) bool {
	if path == "" {
		return false
	}
	if v, ok := inToolSeen.Load(path); ok && v.(inToolAt).mod.Equal(mod) {
		return v.(inToolAt).in
	}
	in := endsInTool(path)
	inToolSeen.Store(path, inToolAt{mod, in})
	return in
}

func endsInTool(path string) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return false
	}
	const window = 256 << 10
	off := max(0, st.Size()-window)
	b := make([]byte, st.Size()-off)
	if _, err := f.ReadAt(b, off); err != nil && err != io.EOF {
		return false
	}
	// Each call is its own line, and each result: one still open, of
	// several made at once, is a command still running.
	open := map[string]bool{}
	for _, ln := range bytes.Split(b, []byte{'\n'}) {
		if !bytes.Contains(ln, []byte(`"tool_use"`)) && !bytes.Contains(ln, []byte(`"tool_result"`)) {
			continue
		}
		var l struct {
			Message struct {
				Content jsontext.Value `json:"content"`
			} `json:"message"`
		}
		var blocks []struct {
			Type      string `json:"type"`
			ID        string `json:"id"`
			ToolUseID string `json:"tool_use_id"`
		}
		if jsonx.Unmarshal(ln, &l) != nil || jsonx.Unmarshal(l.Message.Content, &blocks) != nil {
			continue
		}
		for _, bl := range blocks {
			switch bl.Type {
			case "tool_use":
				open[bl.ID] = true
			case "tool_result":
				delete(open, bl.ToolUseID)
			}
		}
	}
	return len(open) > 0
}
