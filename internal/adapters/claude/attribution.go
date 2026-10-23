package claude

import (
	"path/filepath"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
)

// Claude Code credits itself in commits and pull requests unless its
// settings' attribution says otherwise: an empty commit and pr text adds
// nothing. includeCoAuthoredBy is the older word for the commit's, still
// read.

// Attribution is whether p's Claude Code credits itself.
func (Adapter) Attribution(p agent.Profile) (on, set bool) { return AttributionIn(p.Dir) }

// SetAttribution says whether p's Claude Code credits itself.
func (Adapter) SetAttribution(p agent.Profile, on bool) error { return SetAttributionIn(p.Dir, on) }

// AttributionIn is whether Claude Code with its config in dir credits
// itself, and whether its settings say.
func AttributionIn(dir string) (on, set bool) {
	s, err := settingsfile.Load(filepath.Join(dir, "settings.json"))
	if err != nil {
		return false, false
	}
	var commit, pr string
	hasCommit, hasPR := s.Get("attribution.commit", &commit), s.Get("attribution.pr", &pr)
	if hasCommit || hasPR {
		return commit != "" || pr != "", true
	}
	var co bool
	if s.Get("includeCoAuthoredBy", &co) {
		return co, true
	}
	return false, false
}

// SetAttributionIn says in dir's settings whether Claude Code credits
// itself: off empties its texts, on removes them so it adds its own.
func SetAttributionIn(dir string, on bool) error {
	s, err := settingsfile.Load(filepath.Join(dir, "settings.json"))
	if err != nil {
		return err
	}
	if on {
		_ = s.Set("attribution.commit", nil)
		_ = s.Set("attribution.pr", nil)
		_ = s.Set("includeCoAuthoredBy", nil)
	} else {
		_ = s.Set("attribution.commit", "")
		_ = s.Set("attribution.pr", "")
		_ = s.Set("includeCoAuthoredBy", false)
	}
	return s.Save()
}

var _ agent.Attributor = Adapter{}
