package host

import (
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// guide hands a message to the turn under way without stopping it, where
// the agent takes one (FeatureGuide): Claude Code reads it at its next
// step. Idle, or where it can't, it's sent as any message is.
func (s *server) guide(text string, images []string) error {
	return s.guideExchange(text, images, nil, "")
}

// guideExchange is guide for another agent's message, and with the ev of
// what sent it, its cause.
func (s *server) guideExchange(text string, images []string, exchange *event.Exchange, cause string) error {
	s.mu.Lock()
	k := agent.Kind(s.cfg.Kind)
	if k == "" {
		k = agent.LegacyKind // Claude Code, as older sessions say it
	}
	ok := s.conn != nil && s.info.State == "working" && agent.Supports(k, agent.FeatureGuide)
	s.mu.Unlock()
	if !ok {
		return s.sendExchange(text, images, false, exchange, cause)
	}
	var pics []string // looked at before taking the lock: they can be megabytes
	for _, p := range images {
		pic, err := readImage(p, TempDir(s.cfg.ID))
		if err != nil {
			return err
		}
		pics = append(pics, pic)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.event("steer", map[string]any{"text": clip(text), "cause": cause})
	return s.deliverExchange(text, images, pics, exchange, cause)
}
