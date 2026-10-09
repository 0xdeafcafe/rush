package agent

import (
	"context"
	"encoding/json/jsontext"
	"os/exec"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// Kind names an agent: "claude", "codex", "copilot", "kimi".
type Kind string

// Profile is one config home of one agent: where its settings,
// transcripts and sessions live.
type Profile struct {
	Kind Kind   `json:"kind"`
	Name string `json:"name"`
	Dir  string `json:"dir"`
}

// Account is who pays for a session and whose limits it spends: a sign-in
// with a plan, or an API key. Any number of profiles and agents can use
// one.
type Account struct {
	Kind  Kind   // the adapter that knows how to sign it in
	ID    string // the agent's own id for it
	Key   string // unique across adapters, and where its Quota is kept: "claude:login:<uuid>"
	Name  string
	Email string
	Org   string
	Plan  string
}

// Adapter is one agent as rush knows it. The optional parts below are
// found with a type assertion; an adapter has only those it can do.
type Adapter interface {
	Kind() Kind
	Name() string // "Claude Code"
	// Features is what rush can do with it, feature by feature. A
	// feature it leaves out is No.
	Features() map[Feature]Support
	// Level is how far rush's support for it has been tried.
	Level() Level
	// Profiles are its config homes on this machine.
	Profiles() []Profile
}

// Homer is an agent with a home of its own, where it keeps everything
// when nothing says otherwise (~/.claude). Home reads nothing.
type Homer interface {
	Home() Profile
}

// Discoverer finds an agent's sessions outside rush: those running in a
// terminal or its own daemon, and past ones with a transcript.
type Discoverer interface {
	Live(p Profile) []Session
	Past(p Profile) []Session
}

// Driver runs a session headless, for rush to draw.
type Driver interface {
	Start(ctx context.Context, o StartOptions) (Conn, error)
}

// Orphans is a Driver that knows its own sessions' processes by their
// command line. A host that starts a session ends any process still running
// it for a host that's gone, so two never write one transcript.
type Orphans interface {
	RunsSession(args []string, sessionID string) bool
}

// StartOptions is how a Driver starts or resumes a session.
type StartOptions struct {
	Profile Profile
	// Home, when set, is the login home (Homed.Home) a session last ran
	// in: it starts there again rather than in the one in use, where it
	// still can, to keep its warm prompt cache.
	Home      string
	Dir       string
	SessionID string // resumed when Resume is set, else the new session's id if the agent takes one
	Resume    bool
	Fork      bool
	Model     string
	Effort    string
	Mode      string
	Env       []string
	Flags     []string // passed to the agent as they are
	Binary    string   // the agent's program, when it isn't on PATH by its usual name
	// Without is what the agent goes without, in its own words: for
	// Claude Code, tool rules taken out of its context.
	Without []string
	// Inbox is a command to run after each tool call, the call's hook
	// input on its stdin: what it prints is a message for the (sub)agent
	// that made the call. An agent that can take one says so (SubagentInbox).
	Inbox string
	// BashHook is a command to run before each shell call, the call's hook
	// input on its stdin; what it prints may rewrite the call. For the
	// gate: agents without hooks go without.
	BashHook string
	// APIKey, when set, pays for the session per token with the
	// provider's API key rather than out of the subscription signed in.
	APIKey string
	// TempDir is the session's own scratch folder, for the agent's
	// temporary files.
	TempDir string
	// Lean starts it without its non-essential network traffic, where it
	// can.
	Lean bool
	// Subagents is how many subagents it may run at once, where its
	// harness caps them; 0 leaves the harness's own.
	Subagents int
	// Tools are MCP servers rush serves in process for the session.
	Tools []ToolServer
	// Agents are subagents to offer it, by name, each defined in its
	// agent's own words; Prompt is added to its system prompt.
	Agents map[string]jsontext.Value
	Prompt string
	// Carry, for a new session, is another agent's conversation for it to
	// take as its own history, where it can (FeaturePort).
	Carry []Line
	// SkillRoots are folders of <name>/SKILL.md skills Claude Code has,
	// for an agent that isn't Claude Code to read as well.
	SkillRoots []string
	// Tap, when set, gets every line the agent writes, as it writes it, in
	// its own words: what rushes that don't read events are sent.
	Tap func(line []byte)
	// Lightly lets Events leave out what's only streamed text and tool
	// results: the caller has them from Tap.
	Lightly bool
}

// Conn is a running session.
type Conn interface {
	Events() <-chan event.Event
	Send(Input) error
	Answer(approvalID, optionID string) error
	Interrupt() error
	SetModel(model string) error
	SetMode(mode string) error
	Close() error
}

// Answerer is a Conn that takes answers to an event.Question: each Ask's
// chosen labels, keyed by the Ask's ID, or its Text when it has none.
type Answerer interface {
	AnswerQuestion(id string, answers map[string][]string) error
}

// Input is a message to a session.
type Input struct {
	Text   string
	Images []string // paths
}

// HistoryReader reads a session's transcript back as the events a live
// session sends.
type HistoryReader interface {
	History(s Session, before time.Time) ([]event.Event, error)
}

// HistoryFollower reads a session's history as it grows: each call of
// the func it returns reads only what's been written since, and gives all
// of it. Once stop is set a call gives up, with an error.
type HistoryFollower interface {
	FollowHistory(s Session, stop *atomic.Bool) func() ([]event.Event, error)
}

// TailReader reads only a session's end, from the first whole line of its
// transcript's last most bytes, for a long one to show at once; cut is
// whether that left some out.
type TailReader interface {
	HistoryTail(s Session, most int64) (evs []event.Event, cut bool, err error)
}

// HistoryTailBeforeReader reads a bounded tail ending before a timestamp.
// It preserves History's cutoff while avoiding decoding the whole prefix.
type HistoryTailBeforeReader interface {
	HistoryTailBefore(s Session, most int64, before time.Time) ([]event.Event, bool, error)
}

// QuotaSource reads an account's limits. p is the profile to read them
// through, for agents whose sign-in lives in the profile.
type QuotaSource interface {
	Quota(ctx context.Context, p Profile, a Account) (usage.Quota, error)
}

// SubagentInbox is an agent whose running subagents can be sent a message
// straight, with StartOptions.Inbox, rather than through the main session.
type SubagentInbox interface{ TakesSubagentMessages() }

// ResetSpender is an agent whose accounts earn limit resets: UseReset
// spends one (id, or the next when ""), on the account profile p is
// signed in to, and says what came of it.
type ResetSpender interface {
	UseReset(ctx context.Context, p Profile, id string) (string, error)
}

// PlanReader reads the plans of an agent's accounts, sharing each reading
// with every rush process through a file.
type PlanReader interface {
	// Plan is who p is signed in as, and its limits, as the agent itself
	// last saw them. It reads the disk only when that record changed.
	Plan(p Profile) usage.Reading
	// Readings are the readings shared through path, by key.
	Readings(path string) map[string]usage.Reading
	// RefreshPlan is p's reading, asked of the provider unless one shared
	// through path is fresh enough; offline never asks.
	RefreshPlan(path string, p Profile, offline bool) usage.Reading
}

// Accounts signs an agent's home in to one of several accounts. Which
// accounts there are, and what they're called, is rush's to keep; the
// adapter keeps their credentials.
type Accounts interface {
	// Current is who p is signed in as.
	Current(p Profile) (Account, error)
	// Switch signs p in as a, keeping the credential p holds now first.
	Switch(p Profile, a Account) error
	// SignIn signs in to an account in the terminal, apart from p: cmd
	// runs, then done keeps the credential and says whose it is.
	SignIn(p Profile) (cmd *exec.Cmd, done func() (Account, error), err error)
	// Forget drops the credential rush keeps for a.
	Forget(a Account) error
}

// SignInReader is an agent whose profile says which account it's signed
// in as: the account's own id, empty when none. It reads the disk.
type SignInReader interface {
	SignedInAs(p Profile) string
}

// Known is an Accounts whose agent keeps a list of its own sign-ins
// (Copilot's are gh's): rush lists them as they are.
type Known interface {
	Known() []Account
}

// AnyAccountQuota is a QuotaSource that reads any account's limits, not
// only those of the account a profile is signed in to.
type AnyAccountQuota interface {
	QuotaSource
	ReadsAnyAccount()
}

// Pricer prices a model's tokens.
type Pricer interface {
	Cost(model string, u usage.TokenUsage) (float64, bool)
}

// Commander lists the slash commands and skills a session can run.
type Commander interface {
	Commands(p Profile, cwd string) []Command
}

// Session is one conversation, running or past.
type Session struct {
	Kind       Kind
	Profile    Profile
	ID         string
	Name       string
	State      string // working, blocked, idle, done, stopped
	Detail     string // what it's doing, in words
	Needs      string // what it's waiting on you for
	Cwd        string
	Model      string
	Transcript string
	PID        int
	// Headless is a one-shot run another program asked for (codex exec,
	// claude -p): no one can reply to it, and one another agent's shell
	// ran is listed with that agent.
	Headless  bool
	Todos     []Todo
	Running   []Task
	CreatedAt time.Time
	UpdatedAt time.Time
	// Remote is a session running on the agent's own servers, not this
	// machine: Repo is the repository it works in, PRs what it opened.
	Remote bool
	Repo   string
	PRs    []PR
	// Extra is the adapter's own, for its own use: Claude's job file.
	Extra any
	// Job is a background job's row, as its agent keeps it: set on the
	// job, not on the session running it. JobID is the job a session
	// runs, if it runs one.
	Job   *Job
	JobID string
	// Interactive is a session opened in a terminal, not a job's.
	Interactive bool
	// Status is what its process last said it was doing (busy, idle,
	// shell), and StatusAt when; zero when it hasn't said.
	Status   string
	StatusAt time.Time
}

var (
	mu       sync.RWMutex
	adapters = map[Kind]Adapter{}
	sorted   []Adapter // adapters by kind, made again on Register
)

// Register makes an adapter known. Adapters call it from init.
func Register(a Adapter) {
	mu.Lock()
	defer mu.Unlock()
	adapters[a.Kind()] = a
	if n, ok := a.(Native); ok {
		native = n
	}
	sorted = make([]Adapter, 0, len(adapters))
	for _, a := range adapters {
		sorted = append(sorted, a)
	}
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].Kind() < sorted[j].Kind() })
	programs.Store(nil)
}

// Get is the adapter for kind, if one is registered.
func Get(k Kind) (Adapter, bool) {
	mu.RLock()
	defer mu.RUnlock()
	a, ok := adapters[k]
	return a, ok
}

// All is every registered adapter, by kind. It's asked for every process
// on every load, so it's kept sorted rather than sorted each time; the
// slice is shared, and mustn't be changed.
func All() []Adapter {
	mu.RLock()
	defer mu.RUnlock()
	return sorted[:len(sorted):len(sorted)]
}
