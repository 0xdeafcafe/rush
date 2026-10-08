package claude

import (
	"encoding/json/jsontext"
	"errors"
	"os"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// errNoTranscript is a session asked for its history that has no
// transcript to read it from.
var errNoTranscript = errors.New("claude: this session has no transcript")

// historyLine is what History needs of a transcript line.
type historyLine struct {
	Type          string         `json:"type"`
	IsMeta        bool           `json:"isMeta"`
	IsSidechain   bool           `json:"isSidechain"`
	Timestamp     time.Time      `json:"timestamp"`
	UUID          string         `json:"uuid"`
	Message       jsontext.Value `json:"message"`
	ToolUseResult jsontext.Value `json:"toolUseResult"`
}

// History reads s's transcript back as rush's own events: its messages,
// from before before when that's set. A subagent's lines and Claude Code's
// own notes are left out, as convo leaves them out of a conversation.
func (Adapter) History(s agent.Session, before time.Time) ([]event.Event, error) { //nolint:gocritic // HistoryReader takes the session by value
	if s.Transcript == "" {
		return nil, errNoTranscript
	}
	f, err := os.Open(s.Transcript)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var (
		out []event.Event
		n   headless.Neutral
		r   = jsonx.NewLineReader(f)
	)
	for {
		line, ok := r.Next()
		if !ok {
			break
		}
		var l historyLine
		if jsonx.Unmarshal(line, &l) != nil || l.IsMeta || l.IsSidechain {
			continue
		}
		if !before.IsZero() && !l.Timestamp.IsZero() && !l.Timestamp.Before(before) {
			break
		}
		if l.Type != "user" && l.Type != "assistant" {
			continue
		}
		ev, err := headless.DecodeMessage(l.Type, l.Message, l.ToolUseResult)
		if err != nil {
			continue
		}
		if m, ok := ev.(headless.Message); ok {
			m.UUID = l.UUID
			ev = m
		}
		out = append(out, n.Event(ev)...)
	}
	return out, r.Err()
}

var _ agent.HistoryReader = Adapter{}
