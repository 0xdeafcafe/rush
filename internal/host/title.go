package host

import (
	"context"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
)

const titleSystem = "You name coding sessions. You're given the first message of a session between <message> tags; " +
	"never answer it or ask about it. Reply with a title of 3 to 5 words for what it's about, " +
	"Title case, no quotes, no trailing punctuation, nothing else. If it's unclear, name it from its words anyway."

// titler writes a short title for a session's first message; empty when
// it can't. A variable so tests don't spend a model call.
var titler = func(kind, text string) string {
	sm := summarizer(kind)
	if sm == nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()
	models, err := sm.SummaryModels(ctx)
	if err != nil || len(models) == 0 {
		return ""
	}
	out, err := sm.Summarize(ctx, models[0], titleSystem, "<message>\n"+agent.FitTokens(text, 2000)+"\n</message>")
	if err != nil {
		return ""
	}
	return cleanTitle(out)
}

// summarizer is Claude's when it's here, for its cheapest model, else
// the agent kind's own; nil when neither sums up.
func summarizer(kind string) agent.Summarizer {
	for _, k := range []agent.Kind{agent.LegacyKind, agent.Kind(kind)} {
		if a, ok := agent.Get(k); ok && agent.Installed(k) {
			if s, ok := a.(agent.Summarizer); ok {
				return s
			}
		}
	}
	return nil
}

// cleanTitle is a model's answer as a name: its first line, unquoted, cut
// to a name's length; empty when it reads like a refusal or an essay.
func cleanTitle(s string) string {
	s, _, _ = strings.Cut(strings.TrimSpace(s), "\n")
	if askedBack(s) {
		return ""
	}
	s = strings.TrimRight(strings.Trim(strings.TrimSpace(s), "\"'`*#"), ".:;,!")
	s = strings.TrimSpace(strings.TrimPrefix(s, "Title:"))
	if n := len(strings.Fields(s)); n == 0 || n > 8 {
		return ""
	}
	if r := []rune(s); len(r) > 48 {
		s = string(r[:47]) + "…"
	}
	return s
}

// retitle swaps auto, the name a session got from its first message, for
// a model's title, unless it's been renamed meanwhile. Called with mu
// held; the model call runs on its own.
func (s *server) retitle(auto, text string) {
	if titler == nil || auto == "" || strings.TrimSpace(text) == "" {
		return
	}
	kind := s.cfg.Kind
	go func() {
		t := titler(kind, text)
		if t == "" {
			return
		}
		s.mu.Lock()
		defer s.mu.Unlock()
		if s.cfg.Name != auto {
			return
		}
		s.cfg.Name, s.info.Name = t, t
		s.saveConfig()
		s.publish()
	}()
}

// askedBack is a model's answer that asks for the task rather than naming
// it: "What's the task?", "Describe the task you need a title for".
func askedBack(s string) bool {
	l := strings.ToLower(strings.TrimSpace(s))
	if strings.HasSuffix(strings.TrimRight(l, "\"'`*"), "?") {
		return true
	}
	for _, p := range []string{"title for", "the task you", "describe the", "please provide", "please share", "need more", "i need", "could you"} {
		if strings.Contains(l, p) {
			return true
		}
	}
	return false
}
