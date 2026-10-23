package ui

import (
	"cmp"
	"slices"
	"strings"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// recentSetups are up to 3 setups the fleet's agents ran, newest first,
// each once, cur left out.
func (m *Model) recentSetups(cur startOver) []startOver {
	if m.snap == nil {
		return nil
	}
	if m.newestOf != m.snap { // sorted once a reading, not once a frame
		m.newest = append(m.newest[:0], m.snap.Agents...)
		slices.SortStableFunc(m.newest, func(a, b *fleet.Agent) int { return b.CreatedAt.Compare(a.CreatedAt) })
		m.newestOf = m.snap
	}
	seen := []string{strings.Join(m.startWords(cur), " · ")}
	var out []startOver
	// Most of a fleet runs a few setups: each is weighed once a frame, not
	// once an agent.
	var tried [][2]string
	for _, a := range m.newest {
		if len(out) == 3 {
			break
		}
		if a.Advisor {
			continue
		}
		k := cmp.Or(a.Kind, string(a.Acct.Kind))
		if k == "" {
			continue
		}
		var o startOver
		if c := m.host; c != nil && c.key == a.Key && c.sess != nil {
			o = m.sessionStart(c)
		} else if km := [2]string{k, a.Spend.Model}; slices.Contains(tried, km) {
			continue
		} else {
			tried = append(tried, km)
			o = m.startDefaults(k)
			if a.Spend.Model != "" {
				o.model = a.Spend.Model
			}
		}
		if w := strings.Join(m.startWords(o), " · "); !slices.Contains(seen, w) {
			seen, out = append(seen, w), append(out, o)
		}
	}
	return out
}
