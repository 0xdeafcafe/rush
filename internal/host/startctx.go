package host

import (
	"os"
	"path/filepath"
	"slices"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// --- what the agent was given when it started, and what changed since
// the start before: Info.ContextChange ---

// startFile keeps the last start's usage.Start in the session's folder.
const startFile = "context-start.json"

// beginStart notes the prompt sections rush gives the start under way and
// reads the last start's, to compare once the agent says what it has.
// Called with mu held.
func (s *server) beginStart(sections []usage.Section) {
	s.lastStart = nil
	if b, err := os.ReadFile(filepath.Join(dir(s.cfg.ID), startFile)); err == nil {
		var was usage.Start
		if jsonx.Unmarshal(b, &was) == nil {
			s.lastStart = &was
		}
	}
	s.thisStart = &usage.Start{At: time.Now(), Sections: sections}
	s.info.ContextChange = nil
}

// startInit takes the tools and MCP servers the agent said it has. Until
// its first count it's taken again from each init, as servers still
// connecting at the first have their tools by the next. Called with mu
// held; the caller publishes.
func (s *server) startInit(e *event.Init) {
	st := s.thisStart
	if st == nil || st.Parts != nil || len(e.Tools) == 0 {
		return
	}
	tools := slices.Sorted(slices.Values(e.Tools))
	servers := make([]string, 0, len(e.MCP))
	for _, m := range e.MCP {
		servers = append(servers, m.Name)
	}
	slices.Sort(servers)
	if slices.Equal(tools, st.Tools) && slices.Equal(servers, st.Servers) {
		return
	}
	st.Tools, st.Servers = tools, servers
	s.noteStart()
}

// startCount takes the agent's first count by category. Called with mu
// held; it publishes.
func (s *server) startCount(u *usage.Context) {
	if st := s.thisStart; st != nil && st.Parts == nil && len(u.Parts) > 0 {
		st.Parts = usage.PartsOf(u)
		s.noteStart()
		s.publish()
	}
}

// noteStart keeps this start for the next to compare with, and says what
// changed from the last. Called with mu held.
func (s *server) noteStart() {
	if b, err := jsonx.Marshal(s.thisStart); err == nil {
		tmp := filepath.Join(dir(s.cfg.ID), startFile+".tmp")
		if os.WriteFile(tmp, b, 0o600) == nil {
			_ = os.Rename(tmp, filepath.Join(dir(s.cfg.ID), startFile))
		}
	}
	s.info.ContextChange = usage.Diff(s.lastStart, s.thisStart)
}
