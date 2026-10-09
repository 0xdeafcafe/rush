package state

import (
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// A profile is a named list of providers (where the model comes from:
// Claude, Codex, Ollama…) plus what to do when they run out. Every
// installed provider is a profile of its own, built in and never stored;
// the ones you make pick and group providers. Accounts within a provider
// rotate as they always have. Each session gets one profile: the one
// picked for it, else the one its folder's rule names, else the default.
//
// A provider runs in a harness (agent.Harnesses): Ollama in Claude Code,
// Pi or Codex. Config.RunsIn picks it for each provider, and a profile's
// own RunsIn picks it for that profile alone. Anthropic and OpenAI are
// two providers each (agent.Split): "claude" is Anthropic's subscription,
// "claude-key" its API key, and each has its own profile.

// Profile is a named list of providers and a policy.
type Profile struct {
	Name string `json:"name"`
	// Providers are the providers, in the order new sessions try them.
	// Before harnesses these were agents' kinds, and still read as such:
	// "ollama" is Ollama's provider as well as Ollama in Claude Code.
	Providers []string `json:"providers,omitempty"`
	// RunsIn is the harness a provider runs in under this profile, by
	// provider, where it isn't the one Config.RunsIn gives it.
	RunsIn map[string]string `json:"runsIn,omitempty"`
	// Mix is where new sessions go once every account of the first
	// provider is nearly out: "" or "stay" waits on it, "mix" moves on to
	// the next provider that has room.
	Mix string `json:"mix,omitempty"`
	// OnLimit is what a running conversation does when a usage limit
	// stops it: "wait" for the reset, "" or "account" moves to another
	// account of the same provider, and "handoff" does that, then hands
	// the conversation to the next provider when none has room.
	OnLimit string `json:"onLimit,omitempty"`
	// Billing is how the first provider is paid for: "key" with its API
	// key, "" as it's signed in.
	Billing string `json:"billing,omitempty"`
	// Account is the first provider's account new sessions start on, by
	// ID, while it has room; empty is whichever has the most.
	Account string `json:"account,omitempty"`
	// Model and Effort are what its sessions start with, over what
	// Settings gives the provider in its harness.
	Model  string `json:"model,omitempty"`
	Effort string `json:"effort,omitempty"`
	// RestMinutes is how long its idle sessions keep the agent running,
	// over the agent's and Dispatch's; 0 leaves those to decide.
	RestMinutes int `json:"restMinutes,omitzero"`
	// Builtin is a provider's own profile, made here rather than stored.
	Builtin bool `json:"-"`
	// runsIn is Config.RunsIn, for the providers RunsIn leaves out.
	runsIn map[string]string
}

// What Profile.Mix and Profile.OnLimit can be.
const (
	MixStay = "stay"
	MixMix  = "mix"

	LimitWait    = "wait"
	LimitAccount = "account"
	LimitHandoff = "handoff"

	BillingKey = "key"
)

// DefaultProfileName is what the profile made from an older config is
// called.
const DefaultProfileName = "Default" // migration: built-in profiles replaced it

// LoginsKind is the agent whose accounts are Config.Logins, and whose
// Start is kept in Dispatch's own fields, where older rushes read it.
const LoginsKind = string(agent.LegacyKind)

// FolderRule gives every session started in Path, or a folder inside it,
// the profile named Profile. Path may start with ~.
type FolderRule struct {
	Path    string `json:"path"`
	Profile string `json:"profile"`
}

// Mixes is whether new sessions move on to the next provider once the
// first is out.
func (p Profile) Mixes() bool { return p.Mix == MixMix }

// Limit is what a running conversation does at a usage limit.
func (p Profile) Limit() string {
	if p.OnLimit == "" {
		return LimitAccount
	}
	return p.OnLimit
}

// Harness is the harness provider runs in under this profile: its own
// choice, else the config's; empty for the provider's default.
func (p Profile) Harness(provider string) agent.Kind {
	if h := p.RunsIn[provider]; h != "" {
		return agent.Kind(h)
	}
	return agent.Kind(p.runsIn[p.ID(provider)])
}

// ID is provider as this profile pays for it: its key one when that's
// the first provider and the profile pays with the key.
func (p Profile) ID(provider string) string {
	if p.Billing == BillingKey && len(p.Providers) > 0 && p.Providers[0] == provider && agent.Split(provider) {
		return agent.KeyOf(provider)
	}
	return provider
}

// KindOf is the agent that runs provider under this profile.
func (p Profile) KindOf(provider string) string {
	if k, ok := agent.KindFor(p.ID(provider), p.Harness(provider)); ok {
		return string(k)
	}
	return provider // no adapter for it: stays as named, and isn't installed
}

// Kinds are the agents that run the profile's providers, in order.
func (p Profile) Kinds() []string {
	var out []string
	for _, pr := range p.Providers {
		if k := p.KindOf(pr); !slices.Contains(out, k) {
			out = append(out, k)
		}
	}
	return out
}

// Has is whether the profile runs agent kind, or lists it as a provider.
func (p Profile) Has(kind string) bool {
	return slices.Contains(p.Providers, kind) || slices.Contains(p.Kinds(), kind)
}

// Installed are the agents of the profile's providers rush can run
// sessions of here, in order.
func (p Profile) Installed() []string {
	var out []string
	for _, k := range p.Kinds() {
		if agent.Runs(agent.Kind(k)) {
			out = append(out, k)
		}
	}
	return out
}

// ProfileNamed is the profile called name, ignoring case: one you made,
// else a provider's own ("ollama"), else an agent's, which is its
// provider in that harness ("ollama-pi" is Ollama in Pi).
func (c Config) ProfileNamed(name string) (Profile, bool) {
	for _, p := range c.Profiles {
		if strings.EqualFold(p.Name, name) {
			p.runsIn = c.RunsIn
			return p, true
		}
	}
	k := agent.Kind(strings.ToLower(name))
	if name == "" {
		return Profile{}, false
	}
	if agent.IsProvider(string(k)) {
		return c.builtin(string(k)), true
	}
	if _, key := agent.Billed(string(k)); key {
		return c.builtin(string(k)), true
	}
	if _, ok := agent.Get(k); ok {
		p := c.builtin(agent.ProviderOf(k))
		p.Name, p.RunsIn = string(k), map[string]string{p.Providers[0]: string(agent.HarnessOf(k))}
		if agent.KeyOnly(k) {
			p.Billing = BillingKey
		}
		return p, true
	}
	return Profile{}, false
}

// builtin is provider id's own profile: it alone, in the harness the
// config gives it, moving to another of its accounts at a limit.
func (c *Config) builtin(id string) Profile {
	p := Profile{Name: id, Providers: []string{id}, Builtin: true, runsIn: c.RunsIn}
	if pr, key := agent.Billed(id); key {
		p.Providers, p.Billing = []string{pr}, BillingKey
	}
	return p
}

// IDs are the installed providers, each a split one's key after it.
func IDs() []string {
	var out []string
	for _, pr := range agent.Providers() {
		if !agent.ProviderInstalled(pr) {
			continue
		}
		out = append(out, pr)
		if agent.Split(pr) {
			out = append(out, agent.KeyOf(pr))
		}
	}
	return out
}

// Builtins are the installed providers' own profiles, by name, less any
// you made of the same name, which stands in for it.
func (c *Config) Builtins() []Profile {
	var out []Profile
	for _, id := range IDs() {
		if slices.ContainsFunc(c.Profiles, func(p Profile) bool { return strings.EqualFold(p.Name, id) }) {
			continue
		}
		out = append(out, c.builtin(id))
	}
	return out
}

// AllProfiles are the providers' own profiles, then yours.
func (c *Config) AllProfiles() []Profile {
	out := c.Builtins()
	for _, p := range c.Profiles {
		p.runsIn = c.RunsIn
		out = append(out, p)
	}
	return out
}

// Default is the profile a session gets when nothing else says: the one
// you made the default, else the default agent's provider's own.
func (c Config) Default() Profile {
	if p, ok := c.ProfileNamed(c.DefaultProfile); ok {
		return p
	}
	if p, ok := c.ProfileNamed(agent.ProviderOf(agent.Kind(c.DefaultAgent()))); ok {
		return p
	}
	return c.builtin(LoginsKind)
}

// ProfileFor is the profile a session in cwd gets: explicit when it names
// one, else the longest folder rule cwd is in, else the default.
func (c Config) ProfileFor(cwd, explicit string) Profile {
	if explicit != "" {
		if p, ok := c.ProfileNamed(explicit); ok {
			return p
		}
	}
	if r, ok := c.RuleFor(cwd); ok {
		if p, ok := c.ProfileNamed(r.Profile); ok {
			return p
		}
	}
	return c.Default()
}

// RuleFor is the folder rule for cwd: the one with the longest path cwd
// is in.
func (c Config) RuleFor(cwd string) (FolderRule, bool) {
	if cwd == "" {
		return FolderRule{}, false
	}
	cwd = filepath.Clean(ExpandHome(cwd))
	best, n := FolderRule{}, -1
	for _, r := range c.FolderRules {
		root := filepath.Clean(ExpandHome(r.Path))
		if r.Path == "" || !within(cwd, root) || len(root) <= n {
			continue
		}
		best, n = r, len(root)
	}
	return best, n >= 0
}

// within is whether dir is root or a folder inside it.
func within(dir, root string) bool {
	if dir == root || root == string(filepath.Separator) {
		return true
	}
	return strings.HasPrefix(dir, root+string(filepath.Separator))
}

// ExpandHome turns a leading ~ into the home folder.
func ExpandHome(p string) string {
	if p != "~" && !strings.HasPrefix(p, "~/") {
		return p
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return p
	}
	return filepath.Join(home, strings.TrimPrefix(p, "~"))
}

// Seat is one account of a provider as the picker sees it.
type Seat struct {
	ID, Name string
	Current  bool    // the one the provider is signed in as
	Out      bool    // a fresh reading says it's nearly out
	Used     float64 // its tightest window, in percent
}

// Room is what rush knows of each provider's accounts, by kind. A
// provider with none listed is taken to have room: its own sign-in, whose
// limits rush can't read or hasn't yet.
type Room map[string][]Seat

// Pick is where a session starts: a provider, and the account of it with
// room. Wait is set when every account of every provider the profile
// allows is nearly out: the session starts there anyway and waits.
type Pick struct {
	Kind    string
	Account Seat
	Wait    bool
}

// seat is the account of a provider to start on: the one in use while it
// has room, else the one with the most room. ok is false when all are out.
func (r Room) seat(kind string) (Seat, bool) {
	seats := r[kind]
	if len(seats) == 0 {
		return Seat{}, true
	}
	var best *Seat
	for i, s := range seats {
		if s.Out {
			continue
		}
		if s.Current {
			return s, true
		}
		if best == nil || s.Used < best.Used {
			best = &seats[i]
		}
	}
	if best == nil {
		for _, s := range seats {
			if s.Current {
				return s, false
			}
		}
		return seats[0], false
	}
	return *best, true
}

// Pick is the provider and account a new session starts on, given what
// room each has: the first installed provider while any of its accounts
// has room; then, when the profile mixes, the next that has. ok is false
// when none of its providers is installed.
func (p Profile) Pick(room Room) (Pick, bool) {
	inst := p.Installed()
	if len(inst) == 0 {
		return Pick{}, false
	}
	first, ok := room.seat(inst[0])
	if i := slices.IndexFunc(room[inst[0]], func(s Seat) bool { return s.ID == p.Account && !s.Out }); p.Account != "" && i >= 0 {
		first, ok = room[inst[0]][i], true
	}
	if ok {
		return Pick{Kind: inst[0], Account: first}, true
	}
	if p.Mixes() {
		if next, ok := p.Next(inst[0], room); ok {
			return next, true
		}
	}
	return Pick{Kind: inst[0], Account: first, Wait: true}, true
}

// Next is the first installed provider after kind with room, for a
// conversation handed on from it. Providers before kind aren't tried: they
// were out, or the profile prefers kind to them.
func (p Profile) Next(kind string, room Room) (Pick, bool) {
	inst := p.Installed()
	at := slices.Index(inst, kind)
	for _, k := range inst[at+1:] {
		if s, ok := room.seat(k); ok {
			return Pick{Kind: k, Account: s}, true
		}
	}
	return Pick{}, false
}

// PickFor is the account of provider kind to use under this profile: for
// work that has to run on that provider (Claude Code's advisor, say). ok
// is false when the profile doesn't list it, it isn't installed, or every
// account of it is nearly out.
func (p Profile) PickFor(kind string, room Room) (Pick, bool) {
	if !p.Has(kind) || !agent.Runs(agent.Kind(kind)) {
		return Pick{}, false
	}
	s, ok := room.seat(kind)
	return Pick{Kind: kind, Account: s, Wait: !ok}, ok
}

// legacyProfile is the profile an older config meant: its default agent,
// then its order, with its choice of what to do when nearly out.
func (c Config) legacyProfile() Profile {
	p := Profile{Name: DefaultProfileName, Providers: []string{c.DefaultAgent()}}
	for _, k := range c.AgentOrder {
		if !slices.Contains(p.Providers, k) {
			p.Providers = append(p.Providers, k)
		}
	}
	switch c.SwitchOnLimit {
	case OnLimitAgent:
		p.Mix = MixMix
		// An older rush moved on through every installed agent, those not
		// in its order after, by name.
		for _, k := range agent.Providers() {
			if !slices.Contains(p.Providers, k) {
				p.Providers = append(p.Providers, k)
			}
		}
	case OnLimitOff:
		p.OnLimit = LimitWait
	}
	return p
}

// plain is whether p does no more than its first provider's own profile:
// it never moves on to another, and switches account at a limit.
func (p Profile) plain() bool {
	return !p.Mixes() && p.Limit() == LimitAccount && len(p.RunsIn) == 0
}

// migrateProfiles keeps what an older config meant, once. A config from
// before profiles, or with the Default profile rush made from one, needs
// no profile of its own when that did only what its first provider's
// built-in one does: its default is that one, and its folders get it.
// Otherwise the Default profile stays (or is made) for what it does.
func (c *Config) migrateProfiles() { // migration: built-in profiles replaced Default
	if c.BuiltinProfiles {
		return
	}
	c.BuiltinProfiles = true
	if len(c.Profiles) == 0 {
		if p := c.legacyProfile(); !p.plain() {
			c.Profiles, c.DefaultProfile = []Profile{p}, p.Name
			return
		}
		c.DefaultProfile = agent.ProviderOf(agent.Kind(c.DefaultAgent()))
		return
	}
	if len(c.Profiles) != 1 || c.Profiles[0].Name != DefaultProfileName || !c.Profiles[0].plain() || len(c.Profiles[0].Providers) == 0 {
		return
	}
	to := agent.ProviderOf(agent.Kind(c.Profiles[0].Providers[0]))
	for i, r := range c.FolderRules {
		if strings.EqualFold(r.Profile, DefaultProfileName) {
			c.FolderRules[i].Profile = to
		}
	}
	c.Profiles, c.DefaultProfile = nil, to
}

// SyncLegacy writes the default profile back into the fields older rushes
// read: the default agent, the order, and what happens when nearly out.
// Call it after changing profiles.
func (c *Config) SyncLegacy() {
	p := c.Default()
	if ks := p.Kinds(); len(ks) > 0 {
		c.Dispatch.Kind = ks[0]
		c.AgentOrder = ks
	}
	switch {
	case p.Limit() == LimitWait:
		c.SetSwitchOnLimit(OnLimitOff)
	case p.Mixes():
		c.SetSwitchOnLimit(OnLimitAgent)
	default:
		c.SetSwitchOnLimit(OnLimitAccount)
	}
}

// SetProfile adds p, or replaces the profile called old (which may be
// p's own name), keeping rules and the default pointing at it. A
// provider's own profile, changed, becomes one of yours.
func (c *Config) SetProfile(old string, p Profile) {
	p.Builtin, p.runsIn = false, nil
	i := slices.IndexFunc(c.Profiles, func(q Profile) bool { return strings.EqualFold(q.Name, old) })
	if i < 0 {
		c.Profiles = append(c.Profiles, p)
	} else {
		c.Profiles[i] = p
	}
	if old != "" && old != p.Name {
		if strings.EqualFold(c.DefaultProfile, old) {
			c.DefaultProfile = p.Name
		}
		for j, r := range c.FolderRules {
			if strings.EqualFold(r.Profile, old) {
				c.FolderRules[j].Profile = p.Name
			}
		}
	}
	c.SyncLegacy()
}

// DeleteProfile drops the profile you made called name, and the folder
// rules that name it; a provider's own can't go. When it was the default,
// the default is its first provider's own again.
func (c *Config) DeleteProfile(name string) bool {
	i := slices.IndexFunc(c.Profiles, func(p Profile) bool { return strings.EqualFold(p.Name, name) })
	if i < 0 {
		return false
	}
	gone := c.Profiles[i]
	c.Profiles = slices.Delete(c.Profiles, i, i+1)
	c.FolderRules = slices.DeleteFunc(c.FolderRules, func(r FolderRule) bool { return strings.EqualFold(r.Profile, name) })
	if strings.EqualFold(c.DefaultProfile, name) {
		c.DefaultProfile = ""
		if len(gone.Providers) > 0 {
			c.DefaultProfile = agent.ProviderOf(agent.Kind(gone.Providers[0]))
		}
	}
	c.SyncLegacy()
	return true
}

// SetDefaultProfile makes the profile called name the default.
func (c *Config) SetDefaultProfile(name string) bool {
	p, ok := c.ProfileNamed(name)
	if ok {
		c.DefaultProfile = p.Name
		c.SyncLegacy()
	}
	return ok
}

// SetDefaultProvider makes agent kind's own profile the default, so new
// sessions run it: "ollama-pi" is Ollama's, in Pi.
func (c *Config) SetDefaultProvider(kind string) { c.SetDefaultProfile(kind) }

// SetRunsIn gives provider id the harness it runs in wherever a profile
// doesn't pick one; empty is its default.
func (c *Config) SetRunsIn(id, harness string) {
	if def := agent.RunsFor(id); len(def) > 0 && string(agent.HarnessOf(def[0])) == harness {
		harness = ""
	}
	if harness == "" {
		delete(c.RunsIn, id)
	} else {
		if c.RunsIn == nil {
			c.RunsIn = map[string]string{}
		}
		c.RunsIn[id] = harness
	}
	c.SyncLegacy()
}

// Uses is whether you use provider id in harness, besides its default.
func (c Config) Uses(id, harness string) bool {
	return slices.Contains(c.Harnesses[id], harness)
}

// SetUses says whether you use provider id in harness.
func (c *Config) SetUses(id, harness string, on bool) {
	hs := slices.DeleteFunc(slices.Clone(c.Harnesses[id]), func(h string) bool { return h == harness })
	if on {
		hs = append(hs, harness)
	}
	if len(hs) == 0 {
		delete(c.Harnesses, id)
		return
	}
	if c.Harnesses == nil {
		c.Harnesses = map[string][]string{}
	}
	c.Harnesses[id] = hs
}

// migrateKeyHarness keeps a split provider set to run in a harness only
// its API key can (Anthropic in Pi) doing so: that's its key's harness
// now, and the key what the default and folders that named it get.
func (c *Config) migrateKeyHarness() { // migration: providers split by billing
	for pr, h := range c.RunsIn {
		if !agent.Split(pr) || h == pr {
			continue
		}
		key := agent.KeyOf(pr)
		c.RunsIn[key] = h
		delete(c.RunsIn, pr)
		if strings.EqualFold(c.DefaultProfile, pr) {
			c.DefaultProfile = key
		}
		for i, r := range c.FolderRules {
			if strings.EqualFold(r.Profile, pr) {
				c.FolderRules[i].Profile = key
			}
		}
	}
}

// SetRule gives folder path the profile called name; an empty name drops
// the folder's rule.
func (c *Config) SetRule(path, name string) {
	clean := func(p string) string { return filepath.Clean(ExpandHome(p)) }
	c.FolderRules = slices.DeleteFunc(c.FolderRules, func(r FolderRule) bool { return clean(r.Path) == clean(path) })
	if name != "" {
		c.FolderRules = append(c.FolderRules, FolderRule{Path: path, Profile: name})
	}
}
