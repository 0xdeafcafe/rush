// Package convo turns a rush-mode session's event stream into turns and
// steps, and draws them in rush's own style: folding turns headed by your
// words, one row per tool call with its outcome on the right, and output
// that opens by itself when something failed.
//
// It has no terminal or Bubble Tea dependency, so it can be tested as plain
// data in, lines out.
package convo

import (
	"encoding/json/jsontext"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/agtools"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Status is where a step is.
type Status int

const (
	Running Status = iota
	OK
	Failed
	Waiting // an approval is pending
	Denied  // refused, by you or by a rule
	Lost    // its turn ended or Claude died before it reported back
)

// Step is one tool call.
type Step struct {
	ID   string
	Tool string
	// Kind is what the call does, whichever agent made it; Tool and Input
	// are in Claude Code's words.
	Kind     tool.Kind
	Input    jsontext.Value
	Status   Status
	Output   string         // the tool result as text
	Result   jsontext.Value // Claude Code's structured result (patches, stdout/stderr)
	Exit     int            // Bash exit code, -1 when unknown
	Start    time.Time
	End      time.Time
	Children []*Step // a subagent's own steps
	// Images are what the tool gave back as pictures: a Read of one, a
	// screenshot.
	// ponytail: the bytes stay for the session's life; let them go once
	// the thumbnail's made if screenshot-heavy sessions get too big.
	Images   []*event.ImageData
	Approval *Asking // what it waits on you for, while it does

	// call is the step's call as rush's own, read once as the step is
	// made: the agent's own when it spoke rush's events, else Claude
	// Code's words read.
	call *tool.Call
	// output is how the call came out, as rush's own: read once, as it
	// comes back.
	output *tool.Output
	parent *Step
	turn   *Turn // the turn whose steps hold it
	// A Bash chain's commands as seen running (chainrun.go), and the
	// furthest of them seen.
	parts map[int]*partRun
	at    int
	// The agent the command reads as running, as of an input spawnAt-1
	// long: a hint, until SetChildren says what it ran.
	spawn   *Spawn
	spawnAt int
	// run is the one agent it ran, found, whose session is child; fan is
	// several, each a step of Children. runLive is while any still works,
	// runEnd when the last stopped, ranVer how often they were set.
	run     *Spawn
	child   *Session
	fan     bool
	runLive bool
	runEnd  time.Time
	ranVer  int
	// unit is it as an item of its own, for the unit memo to keep its rows
	// while the subagent it's under works on.
	unit []*Item
	// toolSp is the agent a spawn_agent call starts, as of an input
	// toolAt-1 long.
	toolSp *Spawn
	toolAt int
	// agent is agentName's answer, as of a result agentFor long.
	agent    string
	agentFor int
}

// Asking is what a step waits on you for: leave to run, or answers to
// the questions it asks.
type Asking struct {
	ID       string
	Reason   string          // why the agent asks, when it says
	Path     string          // the file outside the workspace that made it ask, if one did
	Always   bool            // it can be allowed for good
	AlwaysAs string          // what allowing it for good says it does: "Always allow Bash(pnpm lint:*)"
	Question *event.Question // set when it asks you to choose
}

// flight is a light session's tool call waiting for its result.
type flight struct {
	tool  string
	start time.Time
	doing string // the call in words: "reading view.go"
}

// Kind of an item in a turn.
type Kind int

const (
	KText Kind = iota
	KThinking
	KStep
	KExchange  // a correlated message to or from another agent
	KInterject // you, sending mid-turn
	KCompact   // the conversation was compacted here; Text is the summary
	KNotice    // Claude Code telling you something; Level says how loudly
)

// grow appends streamed text. A builder keeps it linear: adding to the
// string itself would copy the whole answer on every piece.
func (it *Item) grow(x string) {
	if it.buf == nil {
		it.buf = &strings.Builder{}
		it.buf.WriteString(it.Text)
	}
	it.buf.WriteString(x)
	it.Text = it.buf.String()
}

// Item is one thing in a turn, in order.
type Item struct {
	Exchange *event.Exchange
	Kind     Kind
	Text     string
	Compact  *event.Compacted
	buf      *strings.Builder   // while it streams
	Level    string             // a notice's: info, warning, error
	Images   []string           // files sent with an interjection
	Pictures []*event.ImageData // embedded images from provider history
	userRef  string
	Step     *Step
	Answer   bool // the turn's final words, promoted when the turn ends
	// drawn is how it (and a run of steps from it) was last drawn in an
	// open turn: see drawer.memoized.
	drawn *unitDrawn
}

// Turn runs from your message to Claude's last word.
type Turn struct {
	N        int
	Prompt   string
	Items    []*Item
	Start    time.Time
	End      time.Time
	Live     bool
	Cost     float64
	Err      string // why it ended badly, if it did
	Stopped  bool   // you stopped it
	Model    string // the main agent's model for this turn
	Effort   string
	Images   []string           // paths or names of images sent with the prompt
	Pictures []*event.ImageData // embedded images from provider history
	// From is set when the turn wasn't started by you: a background task
	// reporting back, another session's message, a subagent's report.
	From string
	// Cause is what woke it when no message did: the background task that
	// had just finished, or the monitor that had just fired.
	Cause string
	// Command is the command of the shell task that woke it, when it runs
	// over a line; drawn as a command, not as a message.
	Command string
	wokeBy  *Job // the task that woke it, if one did
	// replied names the agents that task ran, once found: it's their reply.
	replied string
	// Streamed counts what Claude has written this turn as it streams
	// (text, thinking and tool input), for a live token estimate; Thinking
	// is when the thinking now under way began.
	Streamed int
	Thinking time.Time
	// Heard is when its agent last said anything, to tell a turn that has
	// gone quiet (a stream that stalled) from one that's working.
	Heard time.Time
	// Retry is the request to the model being tried again, from when it
	// was said, until the model starts answering.
	Retry   *event.Retry
	RetryAt time.Time

	steps map[string]*Step
	ver   int

	ref     string // "t13", made once
	waits   bool   // a step waits on you, as of waitVer-1
	waitVer int

	folded, foldedOf string // the prompt with its pastes folded, and the prompt
}

// Outcome is the first line of the turn's answer.
func (t *Turn) Outcome() string {
	for i := len(t.Items) - 1; i >= 0; i-- {
		if it := t.Items[i]; it.Kind == KText && it.Answer {
			return firstPlain(it.Text)
		}
	}
	return ""
}

// Answer is the turn's final words as Claude wrote them, markdown and all.
func (t *Turn) Answer() string {
	for i := len(t.Items) - 1; i >= 0; i-- {
		if it := t.Items[i]; it.Kind == KText && it.Answer {
			return it.Text
		}
	}
	return ""
}

// Steps counts every tool call in the turn, subagents' included.
func (t *Turn) Steps() int { return len(t.steps) }

func (t *Turn) touch() { t.ver++ }

// Task is one entry in the agent's task list.
type Task struct {
	ID      string
	Subject string
	Active  string // present-tense form, shown while in progress
	Status  string // pending, in_progress, completed
}

// Request is one call to the model, as its usage reports it.
type Request struct {
	ID    string
	At    time.Time
	Model string
	Agent string // "" for the main agent, else the subagent's type
	Run   string // the tool call that started a subagent; "" for the main agent
	Usage usage.TokenUsage
}

// ToolStat totals one tool's calls.
type ToolStat struct {
	Name   string
	Calls  int
	Failed int
	Time   time.Duration
}

// Session is everything known about one rush-mode session.
type Session struct {
	Turns     []*Turn
	Hooks     []Hook // what start hooks told the agent, shown ahead of the first turn
	exchanges map[string]sentEcho
	// Fast is whether the last render drew a running timer still showing
	// tenths: frames every 100ms keep it moving.
	Fast bool
	// Hex is whether the hex plugin is on: step output can be seen as bytes.
	Hex      bool
	Info     host.Info
	Commands []event.Command
	Tasks    []Task
	Model    string
	Cwd      string // where Claude Code says it's running
	Version  string // Claude Code's, from its init
	MCP      []event.MCPServer
	// Usage is what fills the context window, as the host last counted
	// it; nil until it has.
	Usage   *usage.Context
	NTools  int
	Context int // tokens in the context window after the last request
	// compacting is when the compaction under way began; compactRate is
	// how long the last one took per token it compacted, for an estimate.
	compacting  time.Time
	compactRate time.Duration
	Window      int // the context window's size, when the agent says it
	Limit       string
	Requests    []Request
	Tools       map[string]*ToolStat

	echoes     []sentEcho
	userIDs    map[string]bool
	streaming  *Item
	woke       *Job      // the background task that last ended or fired
	wokeAt     time.Time // when, or when the turn it came during ended
	spent      float64   // the process's cost total at its last result
	byID       map[string]*Step
	cache      map[*Turn]cached
	memo       map[memoKey][]Line
	memoOld    map[memoKey][]Line
	chains     map[string]string // chain labels drawn this render, by colour and command
	chainsOld  map[string]string
	rows       map[stepKey]string // steps' labels and summaries drawn this render
	rowsOld    map[stepKey]string
	rowsFor    string             // the folders rows were drawn against
	cards      map[stepKey][]card // steps' cards drawn this render
	cardsOld   map[stepKey][]card
	stale      bool          // the last render used some turns as drawn before: Stale
	deadline   time.Time     // when the render under way is out of budget
	drew       bool          // the render under way has drawn something afresh
	stepVer    int           // bumped whenever a step is added or changes
	asked      []*Step       // the steps asked for approval and maybe still waiting: Pending's
	changes    []*FileChange // Changes, as of changesVer
	parts      [][]Line      // the turns as last drawn, one entry per turn
	index      rowIndex      // each turn's rows, for RenderWindow, TurnRow and TurnAt
	older      olderEffort   // Effort of the turns before the newest
	searchHits []Hit         // the last search, for searchKey
	searchKey  string
	changesVer int
	baseList   []string
	baseFor    string
	reqIdx     map[string]int
	reqVer     int // bumped by every request recorded, for costMemo
	applied    int // bumped by every event applied or transcript line taken: Applied
	ovMemo     struct {
		key   overviewKey
		lines []Line
	}
	costMemo struct {
		ver, n int
		kind   string
		usd    float64
	}
	// light keeps only what a subagent's row shows (SubagentStats): no
	// tool inputs or outputs, no thinking, and only each turn's latest words.
	light    bool
	inFlight map[string]flight // a light session's tool calls still out
	done     []string          // a light session's latest calls back, in words, oldest first
	worktree string            // the worktree its calls last reached into, by name

	jobs []*Job // Claude Code's tasks, in the order they started
	// jobCall is jobs by the call that started them, built when asked and
	// dropped when a task is added or its call changes: steps ask per row.
	jobCall map[string][]*Job

	// nt reads the native agent's own events (agent.Native), as a
	// transcript or an older host has them, as rush's; started on first use.
	nt agent.Neutral
	// calls are a light session's tool calls, for how each came out.
	calls map[string]tool.Call

	// TaskStatus is what Claude Code last said about each background task
	// (completed, killed, …), keyed by task id: a subagent's agent id.
	TaskStatus map[string]string
	// First and Last are the times of the first and latest activity.
	First, Last time.Time
	// Partial is a session read from part way through its transcript
	// (NewTailFrom): what needs all of it says it's still counting.
	Partial bool
}

func New() *Session {
	return &Session{byID: map[string]*Step{}, cache: map[*Turn]cached{}, Tools: map[string]*ToolStat{},
		reqIdx: map[string]int{}, TaskStatus: map[string]string{}}
}

// Effort is the effort it runs at: the one rush set, else the one its
// transcript says the last turn ran at (a CLI default or settings.json).
// The turns before the newest are looked through once, not every frame.
func (s *Session) Effort() string {
	if s.Info.Effort != "" {
		return s.Info.Effort
	}
	n := len(s.Turns)
	if n == 0 {
		return ""
	}
	if e := s.Turns[n-1].Effort; e != "" {
		return e
	}
	if o := &s.older; o.n != n || o.first != s.Turns[0] {
		o.n, o.first, o.effort = n, s.Turns[0], ""
		for _, tn := range slices.Backward(s.Turns[:n-1]) {
			if tn.Effort != "" {
				o.effort = tn.Effort
				break
			}
		}
	}
	return s.older.effort
}

// olderEffort is the effort the turns before the newest last ran at, as
// counted over n turns from first.
type olderEffort struct {
	n      int
	first  *Turn
	effort string
}

// Live is the turn in progress, if any.
func (s *Session) Live() *Turn {
	if n := len(s.Turns); n > 0 && s.Turns[n-1].Live {
		return s.Turns[n-1]
	}
	return nil
}

// Pending lists the approvals waiting, oldest first.
// The pane asks several times a frame, so it looks only at the steps ever
// asked about, not every step of every turn.
func (s *Session) Pending() []*Step {
	var out []*Step
	kept := s.asked[:0]
	for _, st := range s.asked {
		if st.Approval == nil {
			continue // answered: it's asked again only by adding it again
		}
		kept = append(kept, st)
		// One in no turn is a subagent's whose run we never saw start.
		if st.turn == nil || st.turn.steps[st.ID] == st {
			out = append(out, st)
		}
	}
	clear(s.asked[len(kept):])
	s.asked = kept
	sortSteps(out)
	return out
}

func (s *Session) turnFor(now time.Time) *Turn {
	if t := s.Live(); t != nil {
		return t
	}
	// Output with no prompt to hold it: an agent waking for background
	// work, or a replay that starts mid-turn.
	t := &Turn{N: len(s.Turns) + 1, Live: true, Start: now, steps: map[string]*Step{}}
	if j := s.woke; j != nil && now.Sub(s.wokeAt) < wakeWindow {
		t.wokeBy = j
		s.wake(t)
	}
	s.woke = nil
	s.Turns = append(s.Turns, t)
	return t
}

// Apply folds one decoded host line (host.Decode's result) into the session.
// Applied counts the events applied, for a caller's own cache of what it
// worked out from them.
func (s *Session) Applied() int { return s.applied }

func (s *Session) Apply(ev any, now time.Time) {
	s.applied++
	if !now.IsZero() {
		if s.First.IsZero() || now.Before(s.First) {
			s.First = now
		}
		if now.After(s.Last) {
			s.Last = now
		}
	}
	switch ev := ev.(type) {
	case host.Sent:
		s.sent(ev, now, true)
	case host.InfoEvent:
		// A model switched mid-session comes only as the host's info: the
		// agent's init said the one it started on.
		if md := ev.Info.Model; md != "" && md != s.Info.Model {
			s.Model = md
		}
		if note := limitChange(s.Info, ev.Info); note != "" {
			s.notice(note, "ok", now)
		}
		s.Info = ev.Info
		s.syncJobs(ev.Info, now)
		// The host went idle with a turn still open: the agent died
		// mid-turn.
		if t := s.Live(); t != nil && ev.Info.State == "idle" && ev.Info.ClaudePID == 0 {
			s.endTurn(t, now)
			t.Err = programWord(ev.Info.Kind) + " exited mid-turn"
			if ev.Info.Error != "" {
				t.Err += ": " + firstLine(ev.Info.Error)
			}
		}
	case host.Commands:
		s.Commands = ev.Commands
	case host.Context:
		u := ev.Usage
		s.Usage = &u
	case host.Answered:
		s.settle(ev.ID)
	case event.Event:
		s.applyNeutral(ev, now)
	default:
		// The native agent's own, as a transcript or an older host has them.
		if s.nt == nil {
			n, ok := agent.NativeAgent()
			if !ok {
				return
			}
			s.nt = n.Neutral()
		}
		evs, _ := s.nt.Event(ev)
		for _, e := range evs {
			s.applyNeutral(e, now)
		}
	}
}

func (s *Session) endTurn(t *Turn, now time.Time) {
	s.stepVer++
	t.Live, t.End = false, now
	s.streaming = nil
	for _, st := range t.steps {
		if st.Approval != nil && st.parent != nil {
			continue // a background subagent's ask outlives the turn it landed in
		}
		if st.Status == Running || st.Status == Waiting {
			st.Status, st.Approval = Lost, nil
		}
	}
	// The last words become the answer.
	for i := len(t.Items) - 1; i >= 0; i-- {
		it := t.Items[i]
		if it.Kind == KText && strings.TrimSpace(it.Text) != "" {
			it.Answer = true
			break
		}
		if it.Kind == KStep {
			break
		}
	}
	t.touch()
}

// ask marks the step of call id as waiting on you.
func (s *Session) ask(id string, a *Asking) {
	st := s.byID[id]
	if st == nil {
		return
	}
	if !slices.Contains(s.asked, st) {
		s.asked = append(s.asked, st)
	}
	st.Approval, st.Status = a, Waiting
	s.touchStep(st)
}

func (s *Session) settle(requestID string) {
	for _, st := range s.byID {
		if st.Approval != nil && st.Approval.ID == requestID {
			st.Approval = nil
			if st.Status == Waiting {
				st.Status = Running
			}
			s.touchStep(st)
		}
	}
}

func (s *Session) touchStep(st *Step) {
	s.stepVer++
	if st.turn != nil && st.turn.steps[st.ID] == st {
		st.turn.touch()
	}
}

func (s *Session) message(m *event.Message, now time.Time) {
	if m.Role != "assistant" {
		// Tool results belong to the turn their call is in, however late
		// they arrive; they never open a turn of their own.
		s.results(m, now)
		return
	}
	parent := s.byID[m.Parent]
	// A subagent's message whose run we never saw start still isn't
	// the main agent's: it mustn't land in the turn or its numbers.
	sub := m.Parent != ""
	// Only the main agent opens a turn. A background subagent working on
	// after the turn that started it ended stays with that turn; one we
	// never saw start, with nothing running, is in no turn at all.
	t := s.Live()
	switch {
	case !sub:
		t = s.turnFor(now)
	case t == nil && parent != nil:
		t = parent.turn
	}
	if t != nil {
		defer t.touch()
	}
	if u := m.Tokens; u != nil {
		s.request(m, parent, now)
		if !sub {
			s.Context = int(u.Input + u.CacheRead + u.CacheWrite5m + u.CacheWrite1h + u.Output)
			if m.Model != "" {
				t.Model = m.Model
			}
		}
	}
	for _, p := range m.Parts {
		switch p.Kind {
		case event.Text:
			if !sub && strings.TrimSpace(p.Text) != "" {
				s.words(t, p.Text) // a subagent's words stay inside it
			}
		case event.Thinking:
			if !sub && !s.light && (len(t.Items) == 0 || t.Items[len(t.Items)-1].Kind != KThinking) {
				t.Items = append(t.Items, &Item{Kind: KThinking, Text: p.Text})
			}
		case event.ToolCall:
			if p.Call != nil {
				s.call(p.Call, parent, t, sub, now)
			}
		}
	}
}

// words are the main agent's, in turn t: what streamed in, made whole.
func (s *Session) words(t *Turn, text string) {
	switch {
	case s.light:
		// Only the latest words are ever shown.
		w := strings.Clone(firstPlain(text))
		if n := len(t.Items); n > 0 && t.Items[n-1].Kind == KText {
			t.Items[n-1].Text = w
		} else {
			t.Items = append(t.Items, &Item{Kind: KText, Text: w})
		}
	case s.streaming != nil:
		s.streaming.Text, s.streaming.buf = text, nil
		s.streaming = nil
	default:
		t.Items = append(t.Items, &Item{Kind: KText, Text: text})
	}
}

// call makes a step of a tool call, in turn t under parent.
func (s *Session) call(c *tool.Call, parent *Step, t *Turn, sub bool, now time.Time) {
	name, input := stepTool(c)
	if !sub {
		// A background subagent's call lands mid-stream too: it doesn't
		// end the main agent's words, or they'd be drawn twice.
		s.streaming = nil
	}
	s.tool(name).Calls++
	if wt := WorktreeIn(string(input)); wt != "" && !sub {
		s.worktree = wt
	}
	if s.light {
		// Counted and timed, never drawn: only calls still out are kept.
		if s.inFlight == nil {
			s.inFlight = map[string]flight{}
		}
		doing := nativeDoing(name, input)
		if in, ok := agtools.ReadSpawn(input); ok && agtools.IsSpawn(name) {
			doing = "asking " + in.Agent
		}
		s.inFlight[c.ID] = flight{name, now, doing}
		return
	}
	st := &Step{ID: c.ID, Tool: name, Input: input, Start: now, Exit: -1, parent: parent, turn: t}
	st.setCall(c)
	s.byID[c.ID] = st
	if t != nil {
		t.steps[c.ID] = st
	}
	s.stepVer++
	switch {
	case parent != nil:
		parent.Children = append(parent.Children, st)
	case !sub:
		t.Items = append(t.Items, &Item{Kind: KStep, Step: st})
	}
	s.tasksFromInput(st)
}

// limitChange is what to say when a limit that stopped the session lifts,
// or it moves to another login; empty when neither happened.
func limitChange(was, now host.Info) string {
	switch {
	case was.ID == "":
		return "" // the first info, not a change
	case was.Account != "" && now.Account != "" && now.Account != was.Account:
		return "now on " + now.Account + " · was " + was.Account
	case was.Limit != nil && now.Limit == nil:
		return "limit lifted · carries on"
	}
	return ""
}

// notice adds something Claude Code said to you (not the model) to the
// running turn, or the last one.
func (s *Session) notice(text, level string, now time.Time) {
	t := s.Live()
	if t == nil && len(s.Turns) > 0 {
		t = s.Turns[len(s.Turns)-1]
	}
	if t == nil {
		t = s.turnFor(now)
	}
	t.Items = append(t.Items, &Item{Kind: KNotice, Text: text, Level: level})
	t.touch()
}

// interrupted ends the running turn (or marks the last one) as stopped by
// you.
func (s *Session) interrupted(now time.Time) {
	t := s.Live()
	if t != nil {
		s.endTurn(t, now)
	} else if n := len(s.Turns); n > 0 {
		t = s.Turns[n-1]
	}
	if t != nil {
		t.Stopped, t.Err = true, ""
		t.touch()
	}
}

func (s *Session) results(m *event.Message, now time.Time) {
	for _, p := range m.Parts {
		if p.Kind == event.Text && strings.HasPrefix(strings.TrimSpace(p.Text), "[Request interrupted by user") {
			s.interrupted(now)
			return
		}
	}
	// Text Claude Code injects as a user message (a background task
	// finishing, another session's message) starts a turn of its own.
	for _, p := range m.Parts {
		if p.Kind == event.Text && strings.HasPrefix(strings.TrimSpace(p.Text), "<") {
			if from, text, ok := Injected(p.Text); ok && s.Live() == nil {
				raw, _ := jsonx.Marshal(p.Text)
				s.noteTask(raw)
				s.Apply(host.Sent{Text: text}, now)
				s.Turns[len(s.Turns)-1].From = from
			}
		}
	}
	var last *Step
	for _, p := range m.Parts {
		switch {
		case p.Kind == event.ToolResult && p.Output != nil:
			s.result(p.Output, now)
			last = s.byID[p.Output.CallID]
		case p.Kind == event.Image && p.Image != nil && last != nil:
			// An image a tool gave back, drawn under its step.
			last.Images = append(last.Images, p.Image)
			s.touchStep(last)
		}
	}
}

// result takes in how a call came out.
func (s *Session) result(o *tool.Output, now time.Time) {
	text := resultText(o)
	if f, ok := s.inFlight[o.CallID]; ok {
		delete(s.inFlight, o.CallID)
		if s.done = append(s.done, firstNonEmpty(f.doing, f.tool)); len(s.done) > 3 {
			s.done = s.done[1:]
		}
		ts := s.tool(f.tool)
		if o.IsError && !isRejection(text) {
			ts.Failed++
		}
		ts.Time += now.Sub(f.start)
		return
	}
	st := s.byID[o.CallID]
	if st == nil {
		return
	}
	// Claude Code's structured result is kept as it sent it; another
	// agent's is put as Claude's would be, and its own output kept.
	if c := st.Call(); claudes(&c) {
		st.Output, st.Result = text, slimResult(st.kind(), o.Raw)
		st.readOutput(o.IsError)
	} else {
		st.Output, st.Result = text, slimResult(st.kind(), claudeResult(o))
		own := *o
		own.Raw = nil // what's drawn is read from the rest
		st.output = &own
	}
	st.End = now
	st.Approval = nil
	switch {
	case st.Status == Denied:
	case o.IsError:
		st.Status = Failed
		if isRejection(text) {
			st.Status = Denied
		}
	default:
		st.Status = OK
	}
	if st.kind() == tool.Shell {
		st.Exit = exitCode(st)
	}
	s.touchStep(st)
	ts := s.tool(st.Tool)
	if st.Status == Failed {
		ts.Failed++
	}
	if !st.Start.IsZero() {
		ts.Time += st.End.Sub(st.Start)
	}
	s.tasksFromResult(st)
}

var exitRe = regexp.MustCompile(`(?m)^(?:Error: )?Exit code (\d+)`)

func exitCode(st *Step) int {
	if st.Status == OK {
		return 0 // "Exit code N" in a success's output is just output
	}
	if m := exitRe.FindStringSubmatch(st.Output); m != nil {
		n, _ := strconv.Atoi(m[1])
		return n
	}
	if st.Status == OK {
		return 0
	}
	return -1
}

// isRejection is a tool call you (or a rule) refused, as against one that
// failed on its own, like a file it had no permission to read.
func isRejection(text string) bool {
	t := strings.ToLower(text)
	for _, k := range []string{"user declined", "user rejected", "doesn't want to proceed", "tool use was rejected",
		"requires approval", "permission to use", "haven't granted", "has been denied", "auto mode classifier"} {
		if strings.Contains(t, k) {
			return true
		}
	}
	return false
}

// request records a model call. Claude Code sends one message per content
// block, all with the same id and usage, so a repeat replaces the last.
func (s *Session) request(m *event.Message, parent *Step, now time.Time) {
	agent := ""
	if parent != nil {
		agent = agentName(parent)
	}
	if m.Model == "<synthetic>" {
		return // Claude Code's own placeholder, not a model call
	}
	r := Request{ID: m.ID, At: now, Model: m.Model, Agent: agent, Run: m.Parent, Usage: *m.Tokens}
	s.reqVer++
	// One call arrives as several messages with the same id and usage; the
	// output count can grow between them, so keep the largest.
	if i, ok := s.reqIdx[m.ID]; ok && m.ID != "" {
		prev := s.Requests[i]
		r.At = prev.At
		r.Usage.Output = max(r.Usage.Output, prev.Usage.Output)
		s.Requests[i] = r
		return
	}
	if m.ID != "" {
		s.reqIdx[m.ID] = len(s.Requests)
	}
	s.Requests = append(s.Requests, r)
}

func (s *Session) tool(name string) *ToolStat {
	ts := s.Tools[name]
	if ts == nil {
		ts = &ToolStat{Name: name}
		s.Tools[name] = ts
	}
	return ts
}

// Tasks come from TodoWrite (the whole list each time) and from TaskCreate
// and TaskUpdate (one at a time; TaskCreate's id is only in its result).
func (s *Session) tasksFromInput(st *Step) {
	switch st.Tool {
	case "TodoWrite":
		var in struct {
			Todos []struct {
				Content    string `json:"content"`
				ActiveForm string `json:"activeForm"`
				Status     string `json:"status"`
			} `json:"todos"`
		}
		if jsonx.Unmarshal(st.Input, &in) != nil {
			return
		}
		s.Tasks = s.Tasks[:0]
		for i, td := range in.Todos {
			s.Tasks = append(s.Tasks, Task{ID: strconv.Itoa(i + 1), Subject: td.Content, Active: td.ActiveForm, Status: td.Status})
		}
	case "TaskUpdate":
		var in struct {
			ID         string `json:"taskId"`
			Status     string `json:"status"`
			Subject    string `json:"subject"`
			ActiveForm string `json:"activeForm"`
		}
		if jsonx.Unmarshal(st.Input, &in) != nil {
			return
		}
		for i := range s.Tasks {
			if s.Tasks[i].ID != in.ID {
				continue
			}
			if in.Status == "deleted" {
				s.Tasks = append(s.Tasks[:i], s.Tasks[i+1:]...)
				return
			}
			if in.Status != "" {
				s.Tasks[i].Status = in.Status
			}
			if in.Subject != "" {
				s.Tasks[i].Subject = in.Subject
			}
			if in.ActiveForm != "" {
				s.Tasks[i].Active = in.ActiveForm
			}
			return
		}
	}
}

var taskIDRe = regexp.MustCompile(`Task #(\w+) created`)

func (s *Session) tasksFromResult(st *Step) {
	if st.Tool != "TaskCreate" || st.Status != OK {
		return
	}
	var in struct {
		Subject    string `json:"subject"`
		ActiveForm string `json:"activeForm"`
	}
	_ = jsonx.Unmarshal(st.Input, &in)
	id := strconv.Itoa(len(s.Tasks) + 1)
	if m := taskIDRe.FindStringSubmatch(st.Output); m != nil {
		id = m[1]
	}
	s.Tasks = append(s.Tasks, Task{ID: id, Subject: in.Subject, Active: in.ActiveForm, Status: "pending"})
}

// Current is the task in progress and how far through the list the agent is.
func (s *Session) Current() (now *Task, done, total int) {
	for i := range s.Tasks {
		t := &s.Tasks[i]
		switch t.Status {
		case "completed":
			done++
		case "in_progress":
			if now == nil {
				now = t
			}
		}
	}
	return now, done, len(s.Tasks)
}

// bases are the folders paths are shown relative to, symlinks resolved.
func (s *Session) bases() []string {
	if s.baseList != nil && s.baseFor == s.Info.Cwd+"|"+s.Cwd {
		return s.baseList
	}
	var out []string
	known := true
	for _, b := range []string{s.Info.Cwd, s.Cwd} {
		if b == "" {
			continue
		}
		out = append(out, b)
		r, ok := realDir(b)
		if r != b {
			out = append(out, r)
		}
		known = known && ok
	}
	if known {
		s.baseList, s.baseFor = out, s.Info.Cwd+"|"+s.Cwd
	}
	return out
}

func sortSteps(xs []*Step) {
	for i := 1; i < len(xs); i++ {
		for j := i; j > 0 && xs[j].Start.Before(xs[j-1].Start); j-- {
			xs[j], xs[j-1] = xs[j-1], xs[j]
		}
	}
}

func firstLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	return strings.TrimSpace(s)
}

var mdMarks = strings.NewReplacer("**", "", "__", "", "`", "")

func stripMarkdown(s string) string {
	s = mdMarks.Replace(s)
	return strings.TrimLeft(s, "#> -*")
}

// firstPlain is firstLine(stripMarkdown(s)), reading only as far as the
// line it returns: the marks never span lines, so each line strips alone.
func firstPlain(s string) string {
	for first := true; s != ""; first = false {
		line, rest, _ := strings.Cut(s, "\n")
		l := mdMarks.Replace(line)
		if first {
			l = strings.TrimLeft(l, "#> -*")
		}
		if l = strings.TrimSpace(l); l != "" {
			return l
		}
		s = rest
	}
	return ""
}

// slimResult drops from a tool's result what nothing draws: the whole file
// an edit or write was made to, and a read file's text, which its Output
// already has. Else a long session holds a copy of every file it touched.
func slimResult(k tool.Kind, raw jsontext.Value) jsontext.Value {
	if len(raw) < 2<<10 {
		return raw
	}
	var drop func(map[string]jsontext.Value) bool
	switch k {
	case tool.Edit, tool.Write:
		drop = func(m map[string]jsontext.Value) bool {
			_, ok := m["originalFile"]
			delete(m, "originalFile")
			return ok
		}
	case tool.Read:
		drop = func(m map[string]jsontext.Value) bool {
			var f map[string]jsontext.Value
			if jsonx.Unmarshal(m["file"], &f) != nil {
				return false
			}
			_, text := f["content"]
			_, image := f["base64"]
			delete(f, "content")
			delete(f, "base64")
			b, err := jsonx.Marshal(f)
			if err != nil || !text && !image {
				return false
			}
			m["file"] = b
			return true
		}
	default:
		return raw
	}
	var m map[string]jsontext.Value
	if jsonx.Unmarshal(raw, &m) != nil || !drop(m) {
		return raw
	}
	b, err := jsonx.Marshal(m)
	if err != nil {
		return raw
	}
	return b
}

// Call is the step's call as rush's own: the one its agent made, or its
// input read from Claude Code's words, with the kind its agent gave it.
// Under is the subagent call st was made under; nil for the main agent's.
func (st *Step) Under() *Step { return st.parent }

func (st *Step) Call() tool.Call {
	if st.call != nil {
		return *st.call
	}
	return st.readCall()
}

// read reads the step's call once, as it's made, so drawing it each
// frame doesn't decode its input again.
func (st *Step) read() {
	c := st.readCall()
	st.call = &c
}

// setCall keeps a copy of the call its agent made, in rush's own words.
func (st *Step) setCall(c *tool.Call) {
	cp := *c
	st.Kind, st.call = cp.Kind, &cp
}

func (st *Step) readCall() tool.Call {
	c := nativeCall(st.ID, st.Tool, st.Input)
	c.Kind = st.kind()
	return c
}

// readOutput reads how the step's call came out, once, as it comes back.
func (st *Step) readOutput(isError bool) {
	o := nativeOutput(st.Call(), st.Output, isError, st.Result)
	st.output = &o
}

// out is how the step's call came out, as rush's own: its streams, its
// exit, the hunks it changed, the lines it read. Empty until it's back.
func (st *Step) out() *tool.Output {
	if st.output == nil {
		o := nativeOutput(st.Call(), st.Output, st.Status == Failed, st.Result)
		return &o
	}
	return st.output
}

// in is what the step's call works on, as rush's own: the path, command,
// pattern and the rest, whichever agent's words its input is in.
func (st *Step) in() *tool.Input {
	if st.call == nil {
		c := st.readCall()
		return &c.Input
	}
	return &st.call.Input
}

// kind is what the step's call does: the kind its agent gave it, or else
// what the native agent's tool of its name does.
func (st *Step) kind() tool.Kind {
	if st.Kind != tool.Other {
		return st.Kind
	}
	return kindOf(st.Tool)
}

// programWord is what the session's agent is called when its process
// dies: its program's name ("claude", "codex"), else its own name.
func programWord(kind string) string {
	k := agent.Kind(kind)
	if k == "" {
		k = agent.LegacyKind
	}
	if p := agent.ProgramOf(k); p != "" {
		return p
	}
	if a, ok := agent.Get(k); ok {
		return a.Name()
	}
	return "the agent"
}
