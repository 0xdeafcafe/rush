package headless

import (
	"encoding/json/jsontext"
	"errors"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// ContextUsage is what fills the context window, by category, as Claude
// Code's /context counts it (the get_context_usage control request): its
// answer is rush's breakdown as it stands.
type ContextUsage = usage.Context

// AskContextUsage asks Claude Code what fills the context window; the
// answer is a ControlReply for the returned id (read it with
// ParseContextUsage). It counts from the last response and local estimates,
// so it costs nothing.
func (s *Session) AskContextUsage() (string, error) {
	return s.control(map[string]any{"subtype": "get_context_usage", "detail": "summary"})
}

// ParseContextUsage reads a reply to AskContextUsage.
func ParseContextUsage(reply ControlReply) (ContextUsage, error) {
	var u ContextUsage
	err := jsonx.Unmarshal(reply.Body, &u)
	return u, err
}

// Ask sends any control request (req carries its subtype) and returns its
// id; the answer is a ControlReply for it.
func (s *Session) Ask(req jsontext.Value) (string, error) {
	var m map[string]any
	if err := jsonx.Unmarshal(req, &m); err != nil {
		return "", err
	}
	if _, ok := m["subtype"].(string); !ok {
		return "", errors.New("a control request needs a subtype")
	}
	return s.control(m)
}
