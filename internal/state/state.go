// Package state is what agents remembers itself. It never lives inside a
// Claude config dir, so Claude Code updates cannot clobber it and it cannot
// corrupt Claude Code.
package state

import (
	"bufio"
	"bytes"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/uithread"
)

// Dir is rush's folder.
//
//uiblock:nowait Load asks it before the view starts: after that it's kept
func Dir() string {
	if d := os.Getenv("RUSH_HOME"); d != "" {
		return d
	}
	home, _ := os.UserHomeDir()
	if d, ok := dirs.Load(home); ok {
		return d.(string)
	}
	d := pick(filepath.Join(home, ".config", "rush"), filepath.Join(home, ".config", "agtop"), "config.json")
	dirs.Store(home, d)
	return d
}

// dirs are Dir's answers by home: it's asked for on every reading, and
// which folder is in use changes only when Migrate moves it.
var dirs sync.Map

// pick is rush's folder, or the one it had as agtop while that's the one
// in use (rush's has no mark in it yet). Both being folders, nothing is
// merged: what runs from agtop's keeps working.
func pick(rush, agtop, mark string) string {
	if _, err := os.Stat(filepath.Join(rush, mark)); err != nil {
		if _, err := os.Stat(filepath.Join(agtop, mark)); err == nil {
			return agtop
		}
	}
	return rush
}

// Key identifies a job across accounts.
func Key(account, id string) string { return account + "/" + id }

type Config struct {
	// Twotter is the feed agents share: #twotter on|off, default off.
	Twotter bool `json:"twatter,omitzero"`
	// HideMinimap hides the conversation overview rail; narrow panes hide it automatically.
	HideMinimap bool `json:"hideMinimap,omitzero"`
	// Grid is the agents pinned as tiles beside the Session, by key, and
	// GridBelow whether they sit under it instead.
	Grid          []string `json:"grid,omitempty"`
	GridBelow     bool     `json:"gridBelow,omitzero"`      // old configs: every tile under
	GridBelowKeys []string `json:"gridBelowKeys,omitempty"` // the tiles in the row under the Session; the rest are beside it
	GridShare     float64  `json:"gridShare,omitzero"`      // how much of the pane the tiles take
	// PixelPictures draws pictures in coloured blocks even where the
	// terminal could show them sharp.
	PixelPictures bool `json:"pixelPictures,omitzero"`
	// CompactKind and CompactModel preselect the user's explicit #compact
	// choice. Empty keeps native harness compaction; never an automatic override.
	CompactKind  string `json:"compactKind,omitempty"`
	CompactModel string `json:"compactModel,omitempty"`
	// APIKeys are the providers whose API key rush keeps in the vault.
	APIKeys []string `json:"apiKeys,omitempty"`
	// Folders are the Claude config folders an older rush was given:
	// ~/.claude and ~/.claude-*. Everything is in ~/.claude now; the others
	// are only read to take their sign-ins and past sessions in, once, and
	// dropped after. ~/.claude's entry stays: its name is what its
	// sessions, names and hosted agents are kept under.
	Folders []Folder `json:"accounts,omitempty"`
	// FoldersImported is set once the older folders' sign-ins were taken
	// in as logins, so one you forget isn't taken in again.
	FoldersImported bool `json:"foldersImported,omitzero"`
	// Active named the folder new sessions started in, before accounts
	// became logins; cleared once logins are found.
	Active string `json:"active,omitempty"`
	// Logins are the Claude accounts ~/.claude can be signed in as; their
	// sign-ins are in the vault, not here.
	Logins []Login `json:"logins,omitempty"`
	// SignIns are the accounts of agents other than Claude Code that rush
	// keeps, several to an agent; their credentials are in the vault or
	// the agent's own keeping, not here.
	SignIns []SignIn `json:"signIns,omitempty"`
	// Using is the account an agent runs on where rush picks it rather
	// than the agent's home saying: a SignIn's ID, by kind.
	Using map[string]string `json:"using,omitempty"`
	// Profiles are the named lists of providers sessions run, with what
	// happens when they run out, that you made; every installed provider
	// is a profile of its own besides (Builtins). DefaultProfile names the
	// one a session gets when neither you nor a FolderRule picked one.
	// DefaultAgent, SwitchOnLimit and AgentOrder are still written from
	// it for older rushes.
	Profiles       []Profile    `json:"profiles,omitempty"`
	DefaultProfile string       `json:"defaultProfile,omitempty"`
	FolderRules    []FolderRule `json:"folderRules,omitempty"`
	// RunsIn is the harness each provider runs in, by provider, where
	// it isn't its usual one: "ollama": "pi" runs Ollama in Pi.
	RunsIn map[string]string `json:"runsIn,omitempty"`
	// Harnesses are the harnesses you use each provider in besides its
	// default, by provider.
	Harnesses map[string][]string `json:"harnesses,omitempty"`
	// BuiltinProfiles is set once an older config's profiles were fitted
	// to built-in ones (migrateProfiles).
	BuiltinProfiles bool `json:"builtinProfiles,omitzero"`
	// SwitchOnLimit is what rush does when the account in use is nearly out:
	// "" (or "account") switches to another account of the same agent, and
	// sessions carry on; "agent" does that, then starts new sessions on
	// the next agent in AgentOrder once every account of it is out; "off"
	// stays.
	SwitchOnLimit string `json:"switchOnLimit,omitempty"`
	// AgentOrder is the agents new sessions move on to, in turn, when
	// SwitchOnLimit is "agent". Installed agents not in it come after, by name.
	AgentOrder []string `json:"agentOrder,omitempty"`
	// StayOnAccount is SwitchOnLimit "off" as rushes before it read it:
	// kept in step with it.
	StayOnAccount bool   `json:"stayOnAccount,omitzero"`
	GroupBy       string `json:"groupBy"`
	// SplitBy splits each section of the list further: "project" (and
	// empty) by the repository agents work in, "none" not at all.
	SplitBy   string          `json:"splitBy,omitempty"`
	Folds     map[string]bool `json:"folds,omitempty"`
	Dispatch  Dispatch        `json:"dispatch"`
	Quiet     bool            `json:"quiet,omitzero"`
	DockLines int             `json:"dockLines,omitzero"`
	// SideWidth is the agent list's share of a split screen, 0.25 to 0.5;
	// zero means rush's own choice.
	SideWidth float64 `json:"sideWidth,omitzero"`
	// View is the layout you picked: "split" (Agents and the Session side
	// by side), "agent" (the Session alone) or "list" (Agents alone).
	// Empty asks, the first time rush opens.
	View string `json:"view,omitempty"`
	// ListOnly keeps a wide screen to the list alone: no Session beside
	// it until you open one.
	ListOnly bool `json:"listOnly,omitzero"`
	// ChatFull is how a Session opens from Agents alone: the whole screen
	// rather than beside the list. It follows how you last had one.
	ChatFull bool `json:"chatFull,omitzero"`
	// EnterOn is what enter does on an agent in the list: "rename" it, as
	// in the Finder, or "open" it. Empty asks, the first time.
	EnterOn string `json:"enterOn,omitempty"`
	SortBy  string `json:"sortBy,omitempty"`
	// NoHostRestart keeps a session's host on the rush it started with,
	// when a newer one is installed; by default it restarts on it
	// between turns.
	NoHostRestart bool `json:"noHostRestart,omitzero"`
	// StackAt is the share of the screen, in percent, at or under which
	// the list's rows take two lines; 0 is the default, negative only
	// when too narrow for one, 100 always. See StackPercent.
	StackAt int `json:"stackAt,omitzero"`
	// HideColumns are the agents list's figures you've turned off:
	// "running", "cpu", "ram", "tokens", "cost", "time".
	HideColumns []string `json:"hideColumns,omitempty"`
	Hibernate   struct {
		AfterMinutes int `json:"afterMinutes"`
	} `json:"hibernate"`
	// ActiveMinutes is how long a stopped agent stays in the list's Active
	// section before Today has it; 0 is the default (5), negative off.
	ActiveMinutes int `json:"activeMinutes,omitzero"`
	// JustLeftMinutes and JustLeftCount bound the list's Just left, what
	// left Idle most recently: 0 is the default (2 hours, 10), negative
	// minutes off.
	JustLeftMinutes int `json:"justLeftMinutes,omitzero"`
	JustLeftCount   int `json:"justLeftCount,omitzero"`
	// CleanupHours is how long an agent must have been done and untouched
	// before its worktree (clean and pushed) and temp work are removed on
	// their own; 0 is the default, and a negative number turns it off.
	CleanupHours int `json:"cleanupHours,omitzero"`
	// KeepOrphans leaves processes whose session ended running until you
	// end them; unset, they're ended once they've been orphaned a while.
	KeepOrphans bool `json:"keepOrphans,omitzero"`
	// TrimRunningTmp lets a running rush session clear its own old tmp
	// when the disk runs low: what's been untouched for three hours and is
	// open in no process. Off unless set.
	TrimRunningTmp bool `json:"trimRunningTmp,omitzero"`
	// KeepTranscriptsPlain turns off storing idle transcripts compressed.
	KeepTranscriptsPlain bool `json:"keepTranscriptsPlain,omitzero"`
	// SubLine is how see-through an agent's second line in the list is:
	// "" solid, else a transparency like "50%" ("dim" and "faint", from
	// before, are 50% and 75%).
	SubLine string `json:"subLine,omitempty"`
	// HideLogo drops the bottle from the header, which then takes three
	// rows instead of five.
	HideLogo bool `json:"hideLogo,omitzero"`
	// ColorBlind draws added and removed, done and failed in sky blue and
	// amber instead of green and red.
	ColorBlind bool `json:"colorBlind,omitzero"`
	// Theme is what rush's colours are made for: "dark" or "light", or,
	// empty, whatever the terminal says its background and text are.
	Theme string `json:"theme,omitempty"`
	// ShowWhitespace marks spaces and tabs in diffs, as · and →.
	ShowWhitespace bool `json:"showWhitespace,omitzero"`
	// CopyOnSelect is whether text dragged over goes to the clipboard as
	// the drag ends, as terminals do; off, it stays selected for cmd+c or
	// ctrl+c. Unset is on.
	CopyOnSelect *bool `json:"copyOnSelect,omitempty"`
	// SearchTranscriptsOnKey keeps ctrl+k to agent names and commands while
	// you type; ctrl+enter (or ctrl+j) then searches the transcripts.
	SearchTranscriptsOnKey bool `json:"searchTranscriptsOnKey,omitzero"`
	// EnterSendsCommand has enter send a message that ends in a /command
	// typed in full, rather than completing it first.
	EnterSendsCommand bool `json:"enterSendsCommand,omitzero"`
	// RowKey is the key that opens or closes a picked conversation row:
	// "enter", "space", or empty for either.
	RowKey string `json:"rowKey,omitempty"`
	// MenuBar keeps rush's menu bar icon running: usage, what's working,
	// and questions you can answer from their notification.
	MenuBar bool `json:"menuBar,omitzero"`
	// MenuBarAsked is set once rush has offered the menu bar icon.
	MenuBarAsked bool `json:"menuBarAsked,omitzero"`
	// Onboarding is how far a new user has got: the Getting started steps
	// they've done, which one-time tips have shown, and whether they've put
	// Getting started away.
	Onboarding Onboarding `json:"onboarding"`
	// Advisor lets rush's advisor look over your agents' figures now and
	// then, on Haiku, with Opus checking what it finds, to say what would
	// cut tokens or time. Off until you turn it on.
	Advisor bool `json:"advisor,omitzero"`
}

// CopiesOnSelect is whether a drag copies as it ends: see CopyOnSelect.
func (c Config) CopiesOnSelect() bool { return c.CopyOnSelect == nil || *c.CopyOnSelect }

// SignIn is an account of an agent other than Claude Code: who it is, and
// what rush calls it.
type SignIn struct {
	Kind  string `json:"kind"`
	ID    string `json:"id"` // the agent's own id for the account
	Name  string `json:"name"`
	Email string `json:"email,omitempty"`
	Plan  string `json:"plan,omitempty"`
}

// What SwitchOnLimit can be.
const (
	OnLimitAccount = ""      // another account of the same agent
	OnLimitAgent   = "agent" // then the next agent
	OnLimitOff     = "off"
)

// migrate brings a config written by an older rush up to date. It only
// adds and renames: nothing an older rush reads is taken away.
func (c *Config) migrate() {
	if c.StayOnAccount && c.SwitchOnLimit == "" {
		c.SwitchOnLimit = OnLimitOff
	}
	if c.GroupBy == "repo" || c.GroupBy == "folder" {
		// Grouping by repository became a split inside the status
		// sections: see SplitBy.
		c.GroupBy, c.SplitBy = "status", ""
	}
	if c.GroupBy == "account" {
		// Folders are gone; sessions group by the agent they run.
		c.GroupBy = "agent"
	}
	if c.Dispatch.Kind == "" {
		c.Dispatch.Kind = string(agent.Migrated("")) // migration: no kind was Claude Code
	}
	c.migrateProfiles()
	c.migrateKeyHarness()
}

// SetSwitchOnLimit sets what rush does when an account is nearly out.
func (c *Config) SetSwitchOnLimit(v string) {
	c.SwitchOnLimit, c.StayOnAccount = v, v == OnLimitOff
}

// DefaultAgent is the agent new sessions run: its kind.
func (c Config) DefaultAgent() string {
	if c.Dispatch.Kind == "" {
		return LoginsKind // a config made here, not read: nothing's been chosen
	}
	return c.Dispatch.Kind
}

// NoteSignIn records an account of another agent, named name (or after
// its email) when it's new, and returns what it's called.
func (c *Config) NoteSignIn(s SignIn, name string) SignIn {
	for i, old := range c.SignIns {
		if old.Kind == s.Kind && old.ID == s.ID {
			s.Name = old.Name
			if s.Email == "" {
				s.Email = old.Email
			}
			if s.Plan == "" {
				s.Plan = old.Plan
			}
			c.SignIns[i] = s
			return s
		}
	}
	if name == "" {
		name, _, _ = strings.Cut(s.Email, "@")
	}
	if name == "" {
		name = s.ID
	}
	s.Name = name
	for n := 2; c.signInNamed(s.Kind, s.Name); n++ {
		s.Name = fmt.Sprintf("%s-%d", name, n)
	}
	c.SignIns = append(c.SignIns, s)
	return s
}

func (c Config) signInNamed(kind, name string) bool {
	for _, s := range c.SignIns {
		if s.Kind == kind && strings.EqualFold(s.Name, name) {
			return true
		}
	}
	return false
}

// SignInsOf are the accounts rush keeps for agent kind.
func (c Config) SignInsOf(kind string) []SignIn {
	var out []SignIn
	for _, s := range c.SignIns {
		if s.Kind == kind {
			out = append(out, s)
		}
	}
	return out
}

// ForgetSignIn drops an account of another agent.
func (c *Config) ForgetSignIn(kind, id string) {
	var keep []SignIn
	for _, s := range c.SignIns {
		if s.Kind != kind || s.ID != id {
			keep = append(keep, s)
		}
	}
	c.SignIns = keep
	if c.Using[kind] == id {
		delete(c.Using, kind)
	}
}

// Onboarding is what rush has taught you so far.
type Onboarding struct {
	Steps  []string `json:"steps,omitempty"`
	Tips   []string `json:"tips,omitempty"`
	Hidden bool     `json:"hidden,omitzero"`
}

// DefaultCleanup is how long done work waits before it's cleaned up.
const DefaultCleanup = 3 * time.Hour

// SetView keeps layout v ("split", "agent" or "list") for next time.
// Agents alone and the Session alone both leave the list without a Session
// beside it; the Session alone also opens every Session that way.
func (c *Config) SetView(v string) {
	c.View, c.ListOnly, c.ChatFull = v, v != "split", v == "agent"
}

// CleanupAfter is how long done work waits before it's cleaned up; zero
// means never.
func (c Config) CleanupAfter() time.Duration {
	switch {
	case c.CleanupHours < 0:
		return 0
	case c.CleanupHours > 0:
		return time.Duration(c.CleanupHours) * time.Hour
	}
	return DefaultCleanup
}

// Dispatch is how new sessions start: which coding agent, model, effort and
// permission mode. Empty means Claude Code's own default.
type Dispatch struct {
	// Kind is the agent new sessions run: empty is Claude Code.
	Kind       string `json:"kind,omitempty"`
	Agent      string `json:"agent,omitempty"`
	Model      string `json:"model,omitempty"`
	Effort     string `json:"effort,omitempty"`
	Permission string `json:"permission,omitempty"`
	// RunIn is where new Claude sessions run: "" for rush mode (rush's own
	// host, headless) or "daemon" for Claude Code's background service.
	RunIn string `json:"runIn,omitempty"`
	// OnLimit is what rush-mode sessions do when a usage limit stops them:
	// "" asks once per session (opt-in), "auto" continues at the reset,
	// "off" waits for you.
	OnLimit string `json:"onLimit,omitempty"`
	// Lean starts rush-mode sessions without Claude Code's non-essential
	// network traffic: ready in about half the time, but without DesignSync,
	// Projects, plugin downloads or live preview.
	Lean bool `json:"lean,omitzero"`
	// RestMinutes is how long an idle rush-mode session keeps Claude Code
	// running before stopping it (a message starts it again); 0 is the
	// default, as soon as it's done.
	RestMinutes int `json:"restMinutes,omitzero"`
	// Starts are what new sessions of the agents other than the built-in
	// one start with, by kind; the built-in's are Model, Effort and
	// Permission above, where older rushes read them.
	Starts map[string]Start `json:"starts,omitempty"`
}

// Start is the model, effort and permission mode a new session starts
// with; empty is the agent's own default.
type Start struct {
	Model  string `json:"model,omitempty"`
	Effort string `json:"effort,omitempty"`
	Mode   string `json:"mode,omitempty"`
	// RestMinutes is how long its idle sessions keep the agent running,
	// over Dispatch.RestMinutes; 0 leaves that to decide.
	RestMinutes int `json:"restMinutes,omitzero"`
}

// StartFor is what a new session of agent kind starts with.
func (d Dispatch) StartFor(kind string) Start {
	if kind == LoginsKind {
		return Start{Model: d.Model, Effort: d.Effort, Mode: d.Permission}
	}
	return d.Starts[kind]
}

// SetStartFor sets what new sessions of agent kind start with.
func (d *Dispatch) SetStartFor(kind string, s Start) {
	if kind == LoginsKind {
		d.Model, d.Effort, d.Permission = s.Model, s.Effort, s.Mode
		return
	}
	if d.Starts == nil {
		d.Starts = map[string]Start{}
	}
	if s == (Start{}) {
		delete(d.Starts, kind)
		return
	}
	d.Starts[kind] = s
}

// DefaultRest is how long an idle rush-mode session keeps Claude Code
// running when RestMinutes isn't set: a moment after it's done, with
// nothing left in the background. An idle Claude Code holds 150-200 MB;
// starting it again takes about a second, and the prompt cache (an hour)
// isn't lost.
const DefaultRest = 3 * time.Second

// RestFor is how long an idle session of profile and agent kind keeps its
// agent running: the profile's RestMinutes, else the kind's, else Rest's.
// ok is false when neither sets one.
func (c Config) RestFor(profile, kind string) (d time.Duration, ok bool) {
	if p, found := c.ProfileNamed(profile); found && p.RestMinutes > 0 {
		return time.Duration(p.RestMinutes) * time.Minute, true
	}
	if m := c.Dispatch.Starts[kind].RestMinutes; m > 0 {
		return time.Duration(m) * time.Minute, true
	}
	return c.Dispatch.Rest(), false
}

// Rest is how long an idle rush-mode session keeps Claude Code running.
func (d Dispatch) Rest() time.Duration {
	if d.RestMinutes > 0 {
		return time.Duration(d.RestMinutes) * time.Minute
	}
	return DefaultRest
}

func (d Dispatch) Flags() []string {
	var f []string
	for _, p := range [][2]string{{"--agent", d.Agent}, {"--model", d.Model}, {"--effort", d.Effort}, {"--permission-mode", d.Permission}} {
		if p[1] != "" {
			f = append(f, p[0], p[1])
		}
	}
	return f
}

// OldFolders are the folders besides ~/.claude an older rush was given,
// whose sign-ins and past sessions are still to be taken in.
func (c Config) OldFolders() []Folder { //nolint:gocritic // Config goes by value, as everywhere in state
	var out []Folder
	root := home().ConfigDir
	for _, a := range c.Folders {
		if a.ConfigDir != "" && a.ConfigDir != root {
			out = append(out, a)
		}
	}
	return out
}

// RootFolder is ~/.claude's entry in Folders, if it has one: what's kept
// when the older folders are dropped.
func (c Config) RootFolder() []Folder { //nolint:gocritic // Config goes by value, as everywhere in state
	root := home().ConfigDir
	for _, a := range c.Folders {
		if a.ConfigDir == root {
			return []Folder{a}
		}
	}
	return nil
}

// ActiveAccount is where every session runs: ~/.claude, signed in as
// whichever login is in use.
func (c Config) ActiveAccount() Folder { //nolint:gocritic // Config goes by value, as everywhere in state
	root := home()
	for _, a := range c.Folders {
		if a.ConfigDir == root.ConfigDir && a.Name != "" {
			root.Name = a.Name
		}
	}
	return root
}

// SwitchAt is how full, in percent, the login in use may get before rush
// switches to another.
const SwitchAt = 95.0

// Login is the saved login with id.
func (c Config) Login(id string) (Login, bool) { //nolint:gocritic // Config goes by value, as everywhere in state
	for _, l := range c.Logins {
		if l.ID == id {
			return l, true
		}
	}
	return Login{}, false
}

// NoteLogin records who a login is, adding it named after name (or its
// email) when it's new; it reports whether anything changed.
func (c *Config) NoteLogin(l Login, name string) bool {
	for i, old := range c.Logins {
		if old.ID == l.ID {
			if old.Email == l.Email && old.Org == l.Org && string(old.Profile) == string(l.Profile) {
				return false
			}
			l.Name = old.Name
			c.Logins[i] = l
			return true
		}
	}
	if name == "" {
		name, _, _ = strings.Cut(l.Email, "@")
	}
	if name == "" {
		name = "account"
	}
	l.Name = name
	for n := 2; c.loginNamed(l.Name); n++ {
		l.Name = fmt.Sprintf("%s-%d", name, n)
	}
	c.Logins = append(c.Logins, l)
	return true
}

func (c Config) loginNamed(name string) bool {
	for _, l := range c.Logins {
		if l.Name == name {
			return true
		}
	}
	return false
}

type Overlay struct {
	Done   map[string]time.Time `json:"done,omitempty"`
	Names  map[string]string    `json:"names,omitempty"`
	Groups map[string]string    `json:"groups,omitempty"`
	Moved  map[string]string    `json:"moved,omitempty"`
	Seen   map[string]time.Time `json:"seen,omitempty"`
	// Hidden are agents put away with ctrl+x: no reading lists them. Their
	// transcripts are untouched, and the key stays here to find them by.
	Hidden map[string]time.Time `json:"hidden,omitempty"`
}

type Store struct {
	mu      sync.Mutex
	Config  Config
	Overlay Overlay
	copied  copied
	env     []envSet // the settings the environment set: see applyEnv
}

// copied is the config as Copy last made it, and as JSON: while the
// config is unchanged, Copy hands out that config again rather than
// decoding a fresh one on every load.
type copied struct {
	buf  bytes.Buffer
	json []byte
	cfg  Config
}

func Load() *Store {
	s := &Store{}
	loadJSON(filepath.Join(Dir(), "config.json"), &s.Config)
	loadJSON(filepath.Join(Dir(), "state.json"), &s.Overlay)
	s.Config.migrate()
	s.env = applyEnv(&s.Config, lookupEnv)
	if s.Overlay.Done == nil {
		s.Overlay.Done = map[string]time.Time{}
	}
	if s.Overlay.Names == nil {
		s.Overlay.Names = map[string]string{}
	}
	if s.Overlay.Groups == nil {
		s.Overlay.Groups = map[string]string{}
	}
	if s.Overlay.Moved == nil {
		s.Overlay.Moved = map[string]string{}
	}
	if s.Overlay.Seen == nil {
		s.Overlay.Seen = map[string]time.Time{}
	}
	return s
}

// Copy is the config and overlay as they are now, sharing nothing with s:
// for reading, and only reading, off the UI's goroutine while the UI goes
// on changing s. Copies made while the config is unchanged share theirs.
func (s *Store) Copy() *Store {
	c := &Store{Overlay: s.Overlay.clone()}
	// The config is read only, off the UI's goroutine, so copies can share
	// one while it's the same.
	cp := &s.copied
	cp.buf.Reset()
	if jsonx.MarshalWrite(&cp.buf, s.Config) != nil {
		return c
	}
	if cp.json == nil || !bytes.Equal(cp.buf.Bytes(), cp.json) {
		cp.cfg = Config{}
		if jsonx.Unmarshal(cp.buf.Bytes(), &cp.cfg) != nil {
			cp.json = nil
			return c
		}
		cp.json = bytes.Clone(cp.buf.Bytes())
	}
	c.Config = cp.cfg
	return c
}

// clone is o sharing nothing with it: its maps hold only strings and
// times, so copying them copies it all.
func (o Overlay) clone() Overlay {
	return Overlay{
		Done: maps.Clone(o.Done), Names: maps.Clone(o.Names),
		Groups: maps.Clone(o.Groups), Moved: maps.Clone(o.Moved),
		Seen: maps.Clone(o.Seen), Hidden: maps.Clone(o.Hidden),
	}
}

// Hide puts the agent with key away for good; call SaveOverlay after.
func (s *Store) Hide(key string) {
	if s.Overlay.Hidden == nil {
		s.Overlay.Hidden = map[string]time.Time{}
	}
	s.Overlay.Hidden[key] = time.Now()
}

// SaveOverlay writes state.json. With WriteBehind on, the writer encodes
// it too, from a clone: the view saves it on every seen mark and rename,
// and encoding it is dearer than copying its maps.
//
//uiblock:nowait the view turns WriteBehind on: its goroutine writes
func (s *Store) SaveOverlay() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	path := filepath.Join(Dir(), "state.json")
	if !behindOn() {
		return writeJSON(path, s.Overlay)
	}
	o := s.Overlay.clone()
	queueWrite(path, func() error { return writeJSON(path, o) })
	return nil
}

//uiblock:nowait the view turns WriteBehind on: its goroutine writes
func (s *Store) SaveConfig() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return save(filepath.Join(Dir(), "config.json"), forSaving(s.Config, s.env))
}

// KeepBefore copies config.json aside as config.json.<name>, once, before
// a change an older rush wouldn't make: the copy is never overwritten.
// With WriteBehind on, the writer copies it, before any save after.
//
//uiblock:nowait the view turns WriteBehind on: its goroutine copies
func KeepBefore(name string) {
	if behindOn() {
		queueWrite("keep:"+name, func() error { keepBefore(name); return nil })
		return
	}
	keepBefore(name)
}

func keepBefore(name string) {
	path := filepath.Join(Dir(), "config.json")
	aside := path + "." + name
	if _, err := os.Stat(aside); err == nil {
		return
	}
	if b, err := os.ReadFile(path); err == nil {
		_ = os.WriteFile(aside, b, 0o600)
	}
}

func readJSON(path string, v any) {
	if b, err := os.ReadFile(path); err == nil {
		_ = jsonx.Unmarshal(b, v)
	}
}

// loadJSON reads one of rush's own files, falling back to the copy of it
// last read whole when it can't be: starting from nothing would save
// nothing over it, and your settings, accounts and done marks with it. The
// unreadable one is kept aside as .broken.
func loadJSON(path string, v any) {
	b, err := os.ReadFile(path)
	if err != nil {
		readJSON(path+".bak", v)
		return
	}
	if jsonx.Valid(b) {
		noteConfig(path, b)
		_ = jsonx.Unmarshal(b, v)
		_ = os.WriteFile(path+".bak", b, 0o600)
		return
	}
	_ = os.WriteFile(path+".broken", b, 0o600)
	readJSON(path+".bak", v)
}

func writeJSON(path string, v any) error {
	b, err := jsonx.MarshalIndent(v)
	if err != nil {
		return err
	}
	return writeBytes(path, b)
}

func writeBytes(path string, b []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	// A temp file of its own, so two rushes saving at once can't write
	// into each other's and leave half of one behind.
	f, err := os.CreateTemp(filepath.Dir(path), filepath.Base(path)+".*.tmp")
	if err != nil {
		return err
	}
	_, err = f.Write(b)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Chmod(f.Name(), 0o600)
	}
	if err != nil {
		_ = os.Remove(f.Name())
		return err
	}
	if err := os.Rename(f.Name(), path); err != nil {
		return err
	}
	noteConfig(path, b)
	return nil
}

// seenConfig is config.json as this rush last read or wrote it, so a
// change made by anything else shows.
var seenConfig struct {
	sync.Mutex
	b []byte
}

func noteConfig(path string, b []byte) {
	if path != filepath.Join(Dir(), "config.json") {
		return
	}
	seenConfig.Lock()
	seenConfig.b = b
	seenConfig.Unlock()
}

// ConfigOnDisk is config.json when something other than this rush has
// changed it since it last read or wrote it, for Reload. It reads the
// file, so it's for off the UI's goroutine.
//
// ponytail: an outside edit landing while a write-behind save is queued
// is lost to that save; a merge would need a base to diff against.
func ConfigOnDisk() ([]byte, bool) {
	uithread.Forbid("state.ConfigOnDisk")
	b, err := os.ReadFile(filepath.Join(Dir(), "config.json"))
	if err != nil || !jsonx.Valid(b) {
		return nil, false
	}
	seenConfig.Lock()
	defer seenConfig.Unlock()
	return b, !bytes.Equal(b, seenConfig.b)
}

// Reload takes config b, as ConfigOnDisk read it, in place of s's own.
func (s *Store) Reload(b []byte) error {
	var c Config
	if err := jsonx.Unmarshal(b, &c); err != nil {
		return err
	}
	c.migrate()
	env := applyEnv(&c, lookupEnv)
	s.mu.Lock()
	s.Config, s.env = c, env
	s.mu.Unlock()
	noteConfig(filepath.Join(Dir(), "config.json"), b)
	return nil
}

// CostCache persists transcript totals so a restart does not rescan gigabytes.
const costCacheVersion = 5 // 5: the folder each transcript last worked in, cd and writes included

// T is what the scan keeps of one transcript.
type CostCache[T any] struct {
	mu      sync.Mutex
	Version int           `json:"version"`
	Files   map[string]*T `json:"files"`
	dirty   bool
}

func cacheDir() string {
	if d := os.Getenv("RUSH_CACHE"); d != "" {
		return d
	}
	d, err := os.UserCacheDir()
	if err != nil {
		return Dir()
	}
	return pick(filepath.Join(d, "rush"), filepath.Join(d, "agtop"), "")
}

// CachePath is a file in rush's cache folder: what can be worked out
// again, but is kept so a restart needn't.
func CachePath(name string) string { return filepath.Join(cacheDir(), name) }

func LoadCostCache[T any]() *CostCache[T] {
	c := &CostCache[T]{Files: map[string]*T{}}
	readJSON(filepath.Join(cacheDir(), "costs.json"), c)
	if c.Version != costCacheVersion {
		c.Files, c.Version = nil, costCacheVersion
	}
	if c.Files == nil {
		c.Files = map[string]*T{}
	}
	// Transcripts their agent has since deleted (it keeps them 30 days by
	// default) needn't be remembered.
	for p := range c.Files {
		if _, err := os.Stat(p); os.IsNotExist(err) {
			delete(c.Files, p)
			c.dirty = true
		}
	}
	return c
}

func (c *CostCache[T]) Get(path string) *T {
	c.mu.Lock()
	defer c.mu.Unlock()
	t := c.Files[path]
	if t == nil {
		t = new(T)
		c.Files[path] = t
	}
	return t
}

func (c *CostCache[T]) MarkDirty() {
	c.mu.Lock()
	c.dirty = true
	c.mu.Unlock()
}

func (c *CostCache[T]) Save() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if !c.dirty {
		return nil
	}
	c.dirty = false
	// Compact: it's a cache nobody reads, and indenting made it a third bigger.
	// It's written as it's made: it runs to megabytes, saved every 30s.
	path := filepath.Join(cacheDir(), "costs.json")
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	f, err := os.OpenFile(path+".tmp", os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	w := bufio.NewWriterSize(f, 64<<10)
	err = jsonx.MarshalWrite(w, c)
	if err == nil {
		err = w.Flush()
	}
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		_ = os.Remove(path + ".tmp")
		return err
	}
	return os.Rename(path+".tmp", path)
}

// WriteJSON writes v to path as indented JSON, all at once: a reader sees
// the old file or the new one, never half of either.
func WriteJSON(path string, v any) error { return writeJSON(path, v) }

// Shows is whether the agents list shows column col.
func (c Config) Shows(col string) bool { return !slices.Contains(c.HideColumns, col) } //nolint:gocritic // Config goes by value, as everywhere in state

// StackPercent is StackAt with its default: 42, and 0 when turned off.
func (c *Config) StackPercent() int {
	switch {
	case c.StackAt < 0:
		return 0
	case c.StackAt == 0:
		return 42
	}
	return c.StackAt
}
