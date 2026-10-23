package copilot

import (
	"path/filepath"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
)

// Copilot CLI adds a Co-authored-by trailer to its commits unless its
// settings.json says includeCoAuthoredBy false.

// Attribution is whether p's Copilot CLI credits itself.
func (Adapter) Attribution(p agent.Profile) (on, set bool) {
	s, err := settingsfile.Load(filepath.Join(p.Dir, "settings.json"))
	if err != nil {
		return false, false
	}
	set = s.Get("includeCoAuthoredBy", &on)
	return on, set
}

// SetAttribution says whether p's Copilot CLI credits itself: on leaves it
// to its default, which does.
func (Adapter) SetAttribution(p agent.Profile, on bool) error {
	s, err := settingsfile.Load(filepath.Join(p.Dir, "settings.json"))
	if err != nil {
		return err
	}
	if on {
		_ = s.Set("includeCoAuthoredBy", nil)
	} else {
		_ = s.Set("includeCoAuthoredBy", false)
	}
	return s.Save()
}

var _ agent.Attributor = Adapter{}
