package usage

import (
	"slices"
	"time"
)

// Start is what a session was given when its agent last started: the
// tools and MCP servers it said it had, the parts of the prompt rush
// added, and, once it has counted, the agent's own totals by category.
// The agent's own base prompt isn't in it: rush never sees that text.
type Start struct {
	At       time.Time `json:"at"`
	Tools    []string  `json:"tools,omitempty"`
	Servers  []string  `json:"servers,omitempty"`
	Sections []Section `json:"sections,omitempty"`
	// Parts are the agent's totals by category at its first count, the
	// conversation and free space left out; nil until it has counted.
	Parts map[string]int `json:"parts,omitempty"`
}

// Section is one part of the prompt rush adds, by name, and its size
// estimated at CharsPerToken.
type Section struct {
	Name   string `json:"name"`
	Tokens int    `json:"tokens"`
}

// CharsPerToken is how many characters make a token, roughly, for text
// rush adds that no agent has counted.
const CharsPerToken = 3.5

// Estimate is text's size in tokens at CharsPerToken.
func Estimate(text string) int { return int(float64(len(text))/CharsPerToken + 0.5) }

// PartsOf are a count's categories as a Start keeps them: what's used,
// less the conversation, which grows by itself between starts.
func PartsOf(u *Context) map[string]int {
	out := map[string]int{}
	for _, p := range u.Parts {
		if p.Deferred || p.Kind == "deferred" || p.Kind == "buffer" || p.Kind == "free" || p.Name == "Messages" {
			continue
		}
		out[p.Name] += p.Tokens
	}
	return out
}

// Change is what differs between two starts of a session.
type Change struct {
	At             time.Time `json:"at"`
	ToolsAdded     []string  `json:"toolsAdded,omitempty"`
	ToolsRemoved   []string  `json:"toolsRemoved,omitempty"`
	ServersAdded   []string  `json:"serversAdded,omitempty"`
	ServersRemoved []string  `json:"serversRemoved,omitempty"`
	// Sections are rush's prompt sections added (Was 0, New set), removed
	// (New 0) or changed in size, estimated.
	Sections []Delta `json:"sections,omitempty"`
	// Parts are the agent's own categories that changed, as it counted
	// them; only when both starts were counted.
	Parts []Delta `json:"parts,omitempty"`
}

// Delta is one thing's size before and after, in tokens; Gone or Added
// say it wasn't there on one side.
type Delta struct {
	Name  string `json:"name"`
	Was   int    `json:"was"`
	Now   int    `json:"now"`
	Added bool   `json:"added,omitzero"`
	Gone  bool   `json:"gone,omitzero"`
}

// Change is Now less Was.
func (d Delta) Change() int { return d.Now - d.Was }

// Empty is whether nothing changed.
func (c *Change) Empty() bool {
	return c == nil || len(c.ToolsAdded)+len(c.ToolsRemoved)+len(c.ServersAdded)+len(c.ServersRemoved)+len(c.Sections)+len(c.Parts) == 0
}

// Diff is what changed from was to now; nil when nothing did. Tools are
// only compared when both starts said theirs, and categories when both
// were counted, so a start the agent hadn't yet described isn't read as
// everything removed.
func Diff(was, now *Start) *Change {
	if was == nil || now == nil {
		return nil
	}
	c := &Change{At: now.At}
	if len(was.Tools) > 0 && len(now.Tools) > 0 {
		c.ToolsAdded, c.ToolsRemoved = setDiff(was.Tools, now.Tools)
		c.ServersAdded, c.ServersRemoved = setDiff(was.Servers, now.Servers)
	}
	ws := map[string]int{}
	for _, s := range was.Sections {
		ws[s.Name] += s.Tokens
	}
	ns := map[string]int{}
	for _, s := range now.Sections {
		ns[s.Name] += s.Tokens
	}
	c.Sections = deltas(ws, ns, true)
	if was.Parts != nil && now.Parts != nil {
		c.Parts = deltas(was.Parts, now.Parts, false)
	}
	if c.Empty() {
		return nil
	}
	return c
}

// setDiff is what's in now and not was, and in was and not now, sorted.
func setDiff(was, now []string) (added, removed []string) {
	for _, n := range now {
		if !slices.Contains(was, n) && !slices.Contains(added, n) {
			added = append(added, n)
		}
	}
	for _, w := range was {
		if !slices.Contains(now, w) && !slices.Contains(removed, w) {
			removed = append(removed, w)
		}
	}
	slices.Sort(added)
	slices.Sort(removed)
	return added, removed
}

// deltas are the names whose size changed, added or went, sorted by how
// much they moved, most first. A part that's 0 on one side and absent on
// the other hasn't changed; keepZero keeps a section with no text as
// added or gone, while a category at 0 counts as absent.
func deltas(was, now map[string]int, keepZero bool) []Delta {
	var out []Delta
	for n, t := range now {
		w, ok := was[n]
		switch {
		case !ok && (t > 0 || keepZero):
			out = append(out, Delta{Name: n, Now: t, Added: true})
		case ok && w != t:
			out = append(out, Delta{Name: n, Was: w, Now: t})
		}
	}
	for n, w := range was {
		if _, ok := now[n]; !ok && (w > 0 || keepZero) {
			out = append(out, Delta{Name: n, Was: w, Gone: true})
		}
	}
	slices.SortFunc(out, func(a, b Delta) int {
		if d := abs(b.Change()) - abs(a.Change()); d != 0 {
			return d
		}
		if a.Name < b.Name {
			return -1
		}
		return 1
	})
	return out
}

func abs(n int) int {
	if n < 0 {
		return -n
	}
	return n
}
