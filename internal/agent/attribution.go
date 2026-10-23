package agent

import "fmt"

// Attributor is an adapter whose agent can credit itself in the commits
// and pull requests it makes (a Co-Authored-By trailer, a "Generated
// with" line), and be told not to.
type Attributor interface {
	// Attribution is whether p's agent credits itself; set is false when
	// its settings don't say, and it does as it does by default.
	Attribution(p Profile) (on, set bool)
	SetAttribution(p Profile, on bool) error
}

// AttributionChange is one profile ApplyAttribution changed, or failed to.
type AttributionChange struct {
	Kind    Kind
	Profile string
	Err     error
}

func (c AttributionChange) String() string {
	if c.Err != nil {
		return fmt.Sprintf("%s (%s): %v", c.Kind, c.Profile, c.Err)
	}
	return fmt.Sprintf("%s (%s)", c.Kind, c.Profile)
}

// ApplyAttribution sets on in every profile of every agent that can say,
// where it isn't already: what changed, and what couldn't be.
func ApplyAttribution(on bool) []AttributionChange {
	var out []AttributionChange
	for _, a := range All() {
		at, ok := a.(Attributor)
		if !ok {
			continue
		}
		for _, p := range ProfilesOf(a) {
			if now, set := at.Attribution(p); set && now == on {
				continue
			}
			out = append(out, AttributionChange{Kind: a.Kind(), Profile: firstOf(p.Name, p.Dir), Err: at.SetAttribution(p, on)})
		}
	}
	return out
}

func firstOf(a, b string) string {
	if a != "" {
		return a
	}
	return b
}
