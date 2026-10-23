package state

import (
	"maps"
	"slices"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// Settings › Config: which harnesses are on, which models each provider
// may call in each, and what happens at a usage limit. Settings ›
// Harnesses: the effort and permissions every harness starts with, and
// what one changes. See docs/providers-harnesses.md.

// What AtLimit can be.
const (
	AtLimitAccount = ""
	AtLimitWait    = "wait"
	AtLimitNext    = "next"
)

// ModeApproveAll is the permissions Every and ByHarness can name for
// every harness alike: whichever mode of its own approves every tool.
const ModeApproveAll = "approve-all"

// HarnessOn is whether harness h is switched on.
func (c *Config) HarnessOn(h agent.Kind) bool { return !slices.Contains(c.HarnessesOff, string(h)) }

// SetHarnessOn switches harness h on or off.
func (c *Config) SetHarnessOn(h agent.Kind, on bool) {
	c.HarnessesOff = slices.DeleteFunc(c.HarnessesOff, func(v string) bool { return v == string(h) })
	if !on {
		c.HarnessesOff = append(c.HarnessesOff, string(h))
	}
	c.SyncLegacy()
}

// CellKey is the cell of provider id in harness h: "ollama@pi".
func CellKey(id string, h agent.Kind) string { return id + "@" + string(h) }

// CellModelsOf is what provider id may call in harness h; nil is all.
func (c *Config) CellModelsOf(id string, h agent.Kind) []string { return c.CellModels[CellKey(id, h)] }

// SetCellModels sets what provider id may call in harness h; none is all.
func (c *Config) SetCellModels(id string, h agent.Kind, models []string) {
	if len(models) == 0 {
		delete(c.CellModels, CellKey(id, h))
		return
	}
	if c.CellModels == nil {
		c.CellModels = map[string][]string{}
	}
	c.CellModels[CellKey(id, h)] = models
}

// Calls is whether provider id may call model in harness h; "" is its
// default model, which any cell may.
func (c *Config) Calls(id string, h agent.Kind, model string) bool {
	ms := c.CellModelsOf(id, h)
	return model == "" || len(ms) == 0 || slices.Contains(ms, model)
}

// StarHarness is the harness auto runs provider id in when it's on: the
// one you picked, else its own.
func (c *Config) StarHarness(id string) agent.Kind {
	if h := c.RunsIn[id]; h != "" {
		return agent.Kind(h)
	}
	if def := agent.RunsFor(id); len(def) > 0 {
		return agent.HarnessOf(def[0])
	}
	return ""
}

// CellOn is whether provider id may run in harness h: its ★ or one you
// use it in, the harness on, and the two able to go together.
func (c *Config) CellOn(id string, h agent.Kind) bool {
	if !c.HarnessOn(h) {
		return false
	}
	if k, _, why := agent.Compat(id, h); k == "" || why != "" {
		return false
	}
	return c.StarHarness(id) == h || c.Uses(id, string(h))
}

// HarnessFor is the harness auto runs provider id in: its ★, else, when
// that's switched off, its first cell that's on; its ★ when none is.
func (c *Config) HarnessFor(id string) agent.Kind {
	star := c.StarHarness(id)
	if c.HarnessOn(star) {
		return star
	}
	for _, h := range c.Harnesses[id] {
		if c.CellOn(id, agent.Kind(h)) {
			return agent.Kind(h)
		}
	}
	return star
}

// harnessesNow is RunsIn as auto reads it: a provider whose ★ harness is
// off goes to HarnessFor's.
func (c *Config) harnessesNow() map[string]string {
	if len(c.HarnessesOff) == 0 {
		return c.RunsIn
	}
	out := maps.Clone(c.RunsIn)
	if out == nil {
		out = map[string]string{}
	}
	for _, id := range agent.ProviderIDs() {
		if h := c.HarnessFor(id); h != c.StarHarness(id) {
			out[id] = string(h)
		}
	}
	return out
}

// ready is whether provider id can start here: installed, its key kept
// when it's paid by one, and its harness on.
func (c *Config) ready(id string) bool {
	pr, key := agent.Billed(id)
	if !agent.ProviderInstalled(pr) {
		return false
	}
	if (key || !agent.Split(pr) && agent.KeyEnv(pr) != "") && !c.HasAPIKey(pr) {
		return false
	}
	return c.HarnessOn(c.HarnessFor(id))
}

// HarnessStart is what harness h changes from Every.
func (d Dispatch) HarnessStart(h agent.Kind) Start { return d.ByHarness[string(h)] }

// SetHarnessStart sets what harness h changes from Every; empty is none.
func (d *Dispatch) SetHarnessStart(h agent.Kind, s Start) {
	s.Model = ""
	if s == (Start{}) {
		delete(d.ByHarness, string(h))
		return
	}
	if d.ByHarness == nil {
		d.ByHarness = map[string]Start{}
	}
	d.ByHarness[string(h)] = s
}

// Resolved is what a new session on provider id, run by agent k, starts
// with: the route's model; the effort and permissions its harness
// changes, else Every's, each only where k offers it.
func (d Dispatch) Resolved(id string, k agent.Kind) Start {
	route := d.StartOn(id, k)
	h := d.HarnessStart(agent.HarnessOf(k))
	out := Start{Model: route.Model, Effort: h.Effort, Mode: h.Mode}
	if out.Effort == "" {
		out.Effort = d.Every.Effort
	}
	if out.Mode == "" {
		out.Mode = d.Every.Mode
	}
	if !d.HarnessStarts {
		if out.Effort == "" {
			out.Effort = route.Effort
		}
		if out.Mode == "" {
			out.Mode = route.Mode
		}
	}
	ch, _ := agent.ChoicesOf(k)
	out.Effort = offered(ch.Efforts, out.Effort)
	if out.Mode == ModeApproveAll {
		out.Mode = ApproveAllMode(ch.Modes)
	} else {
		out.Mode = offered(ch.Modes, out.Mode)
	}
	return out
}

// ResolvedFor is Resolved for agent kind on its own provider.
func (d Dispatch) ResolvedFor(kind string) Start {
	return d.Resolved(agent.ProviderOf(agent.Kind(kind)), agent.Kind(kind))
}

// offered is v when list has it, or lists nothing to check against.
func offered(list []agent.Choice, v string) string {
	if v == "" || len(list) == 0 || slices.ContainsFunc(list, func(c agent.Choice) bool { return c.ID == v }) {
		return v
	}
	return ""
}

// ApproveAllMode is the mode of modes that approves every tool, or "".
func ApproveAllMode(modes []agent.Choice) string {
	for _, v := range modes {
		switch v.ID {
		case "bypassPermissions", "full-access", "yolo", "auto-approve", "always-proceed":
			return v.ID
		}
	}
	return ""
}

// moveHarnessStarts moves each route's own effort and permissions onto
// its harness, the first of a harness's routes to name one winning, once.
// The routes keep theirs for older rushes.
func (d *Dispatch) moveHarnessStarts() { // migration: effort and permissions by harness
	if d.HarnessStarts {
		return
	}
	d.HarnessStarts = true
	move := func(k agent.Kind, s Start) {
		h := agent.HarnessOf(k)
		hs := d.HarnessStart(h)
		hs.Effort, hs.Mode = cmpOr(hs.Effort, s.Effort), cmpOr(hs.Mode, s.Mode)
		d.SetHarnessStart(h, hs)
	}
	move(agent.Kind(LoginsKind), Start{Effort: d.Effort, Mode: d.Permission})
	keys := make([]string, 0, len(d.Starts))
	for k := range d.Starts {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	for _, k := range keys {
		if pr, key := agent.Billed(k); key {
			k = pr
		}
		move(agent.Kind(k), d.Starts[k])
	}
}

func cmpOr(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

// moveProfilesIn moves the default profile you made into what replaced
// profiles, once: its first provider the default, its harness that
// provider's ★, its model that route's, its effort its harness's, and
// what it did at a limit AtLimit. OldProfiles and OldFolderRules stay,
// unread, for older rushes.
func (c *Config) moveProfilesIn() { // migration: profiles replaced by Settings › Config
	if c.ProfilesMovedIn {
		return
	}
	c.ProfilesMovedIn = true
	i := slices.IndexFunc(c.OldProfiles, func(p Profile) bool { return strings.EqualFold(p.Name, c.DefaultProfile) })
	if i < 0 {
		if _, ok := c.ProfileNamed(c.DefaultProfile); !ok {
			c.DefaultProfile = ""
		}
		return
	}
	p := c.OldProfiles[i]
	if len(p.Providers) == 0 {
		c.DefaultProfile = ""
		return
	}
	first := p.Providers[0]
	id := p.ID(first)
	c.DefaultProfile = id
	if h := p.RunsIn[first]; h != "" {
		if c.RunsIn == nil {
			c.RunsIn = map[string]string{}
		}
		c.RunsIn[id] = h
	}
	p.runsIn = c.RunsIn
	k := agent.Kind(p.KindOf(first))
	if p.Model != "" {
		st := c.Dispatch.StartOn(id, k)
		st.Model = p.Model
		c.Dispatch.SetStartOn(id, k, st)
	}
	if p.Effort != "" {
		hs := c.Dispatch.HarnessStart(agent.HarnessOf(k))
		if hs.Effort == "" {
			hs.Effort = p.Effort
			c.Dispatch.SetHarnessStart(agent.HarnessOf(k), hs)
		}
	}
	switch {
	case p.Limit() == LimitWait:
		c.AtLimit = AtLimitWait
	case p.Mixes() || p.Limit() == LimitHandoff:
		c.AtLimit = AtLimitNext
	}
	c.SyncLegacy()
}
