// Package fleet folds every source (jobs, roster, pins, transcripts, the
// process table, plan usage) into one snapshot the UI renders.
package fleet

import (
	"bufio"
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/0xdeafcafe/rush/internal/advisor"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fswait"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/proc"
	"github.com/0xdeafcafe/rush/internal/state"
)

type Agent struct {
	agent.Job
	// Extra is the adapter's own record of it: Claude Code's job file
	// for its background sessions.
	Extra any
	Key   string
	// Acct is the profile it runs in: the agent's config folder.
	Acct        agent.Profile
	DisplayName string
	Pinned      bool
	Done        bool
	Group       string
	Worker      *Worker
	Repo        string
	// Root is the main checkout of Repo's repository: Repo itself, or for
	// a linked worktree the checkout it was made from.
	Root        string
	Branch      string
	Mem         uint64
	CPU         float64
	Procs       int
	Spend       Spend
	PRs         []agent.PR
	Interactive bool
	// Headless is an interactive-kind session that is really `claude -p`
	// driven by some other program: it can't be replied to at all.
	Headless bool
	// Advisor is rush's own advisor at work (its `claude -p`), not yours.
	Advisor  bool
	PID      int  // root of the process tree
	Checking bool // turn just ended; Claude Code has not classified it yet
	Subs     agent.SubagentStats
	// Subagents are Subs' own runs still working, for the Wall to give each
	// its own tile rather than only the count.
	Subagents []SubagentTile
	Seen      bool // the user has opened or answered this question already
	// Rush is a session rush runs itself, headless, through a host
	// process; its pane is the conversation rather than Claude Code's screen.
	Rush bool
	// Past is a conversation nothing has open, known from its transcript
	// alone: a message resumes it in rush mode.
	Past bool
	// Temp is how much disk its temp work takes, as last measured.
	Temp int64
	// Kind is the agent the session runs.
	Kind string
	// Profile is the profile a rush session was started under.
	Profile string
	// History is another agent's transcript, read through its adapter:
	// TranscriptPath is only ever Claude Code's.
	History string
	// Remote is a session running on its agent's own servers: Copilot's
	// coding agent on GitHub.
	Remote bool
	// Compaction is where its agent compacts its context, when its
	// settings or environment set a window smaller than the model's.
	Compaction agent.Compaction
}

// NeedsYou is a live agent asking something the user has not looked at yet.
func (a *Agent) NeedsYou() bool {
	return a.State == "blocked" && !a.Checking && a.PID != 0 && !a.Seen
}

// Waiting is a question the user has seen and left for later.
func (a *Agent) Waiting() bool {
	return a.State == "blocked" && !a.Checking && a.PID != 0 && a.Seen
}

// applyStatus trusts the session's live busy/idle flag over the job file,
// whose state Claude Code only re-summarises every 15-40s.
func (a *Agent) applyStatus(ss *agent.Session) {
	switch {
	case ss.Status == "busy" && a.State == "running":
		a.State = "working"
	case ss.Status == "busy" && a.State == "blocked":
		a.State, a.Needs, a.Detail = "working", "", ""
	case ss.Status == "busy" && a.State == "done" && len(a.Background) == 0:
		a.State, a.Detail = "working", ""
	case ss.Status == "idle" && a.State == "working" && !ss.StatusAt.IsZero() && ss.StatusAt.After(a.UpdatedAt):
		a.State, a.Checking, a.Needs = "blocked", true, ""
	}
}

// Nudge marks agents the user just sent something to as working until their
// own files catch up.
func (l *Loader) Nudge(key string) {
	l.inMu.Lock()
	defer l.inMu.Unlock()
	if l.in.nudged == nil {
		l.in.nudged = map[string]time.Time{}
	}
	l.in.nudged[key] = time.Now()
	l.in.stale = true
}

// JustFinished is a turn that ended moments ago; it lingers in Working so a
// finish is noticed rather than vanishing into history.
func (a *Agent) JustFinished(now time.Time) bool {
	return a.State == "done" && !a.Busy() && now.Sub(a.UpdatedAt) < 2*time.Minute
}

// Busy is a finished turn whose background work is still running in a live
// process; a job file can claim work long after its process has gone.
// Subagents still writing count too: the job file lists them late, and a
// terminal session never does. Their transcripts are proof enough on their
// own, as a background job's process isn't always known.
func (a *Agent) Busy() bool {
	return a.PID != 0 && a.Job.Busy() || !a.Live() && a.Subs.Direct+a.Subs.Nested > 0
}

// Age is what the native view prints on the right: time since last change.
func (a *Agent) Age(now time.Time) time.Duration {
	t := a.UpdatedAt
	if a.ModTime.After(t) {
		t = a.ModTime
	}
	return now.Sub(t)
}

// Elapsed is how long the agent has existed, frozen once it stops.
func (a *Agent) Elapsed(now time.Time) time.Duration {
	end := now
	if !a.Live() {
		end = a.UpdatedAt
		if !a.Spend.Last.IsZero() && a.Spend.Last.Before(end) {
			end = a.Spend.Last
		}
	}
	d := end.Sub(a.CreatedAt)
	if d < 0 {
		return 0
	}
	return d
}

// Spend is what an agent's transcripts say it cost.
type Spend = agent.Spend

// Worker is the process a background job runs in, under its agent's own
// service.
type Worker struct{ PID int }

type AccountView struct {
	state.Folder
	Usage usage.Reading // who it's signed in as, and its plan
	// Quota is the plan's limits, as Usage read them.
	Quota   usage.Quota
	Daemon  bool
	Live    int
	Agents  int
	Spend   float64
	Today   float64
	Current bool
}

type Role int

const (
	RoleOther Role = iota
	RoleDaemon
	RoleView
	RoleWorker
	RoleSpare
	RoleOrphan
)

type ProcRow struct {
	PID   int
	Role  Role
	Label string
	Cmd   string
	Mem   uint64
	CPU   float64
	Procs int
	Start time.Time
	Key   string // the agent a worker belongs to
}

type Machine struct {
	Rows     []ProcRow
	TotalMem uint64
	TotalCPU float64
	Spares   int
	SpareMem uint64
	// Orphans are processes whose session has ended; they run until the
	// user keeps or kills them.
	Orphans   int
	OrphanMem uint64
}

type Snapshot struct {
	At       time.Time
	Agents   []*Agent
	Accounts []AccountView
	Logins   []LoginView
	Machine  Machine
	Table    *proc.Table
}

// Loader keeps the cheap caches between refreshes.
type Loader struct {
	// mu is held by Load, which runs off the UI's goroutine. What the UI
	// hands in waits in "in", under a lock of its own held only a moment,
	// so the UI never waits for a Load to finish; Load takes it in first.
	mu      sync.Mutex
	inMu    sync.Mutex
	in      inbox
	store   *state.Store
	args    map[int]argsEntry
	git     map[string]gitInfo
	roots   map[string]string // folder → main checkout, for mainCheckout
	prevTab *proc.Table
	spend   map[string]Spend
	nudged  map[string]time.Time
	subs    map[string]subsEntry
	quick   bool // this load leaves out what can wait: see LoadQuick
	fetched map[string]usage.Reading
	// burns are each login's last reading a minute or more old, to tell
	// how fast it's filling (usage.Follow).
	burns map[string]usage.Quota
	// UsagePath is the readings every rush process and session shares;
	// usageMod is its time when last read.
	UsagePath string
	usageMod  time.Time
	files     map[string]fileMemo
	// moved are transcripts found away from where their session started,
	// by session id: see transcriptOf.
	moved map[string]string
	// unfound are sessions whose transcript wasn't anywhere, and when it
	// was last looked for: looking globs every project folder, too much
	// for every reading.
	unfound map[string]time.Time
	hosts   host.Lister
	// compact is each row's Compaction as last worked out.
	compact map[string]compactEntry
	print   map[int]printEntry
	Temp    *TempSizes
	// pastRows are past conversations' rows as last made, and spendVer
	// counts each agent's spend updates, so an unchanged row is reused.
	pastRows map[string]pastRow
	idleRows map[string]idleHostedRow
	// pastKeys are the built-in agent's past conversations' row keys.
	pastKeys pastKeys
	// others are other agents' past sessions, by profile folder.
	others   map[string]othersListing
	spendVer map[string]int
	watching *watching
	// links are the sessions that ran agents from their shells: each
	// spawned agent's row key to its session's, kept across restarts, as
	// nothing else says so once it has finished.
	links      map[string]string
	linksDirty bool
	asked      map[string]bool // headless runs looked for in transcripts
	// skipPast leaves past conversations out: see SkipPast.
	skipPast atomic.Bool
}

// SkipPast leaves the conversations nothing has open out of each reading,
// or puts them back in the next one: finding them reads every transcript
// and the repository of each.
func (l *Loader) SkipPast(on bool) {
	if l.skipPast.Swap(on) == on || on {
		return
	}
	l.inMu.Lock()
	l.in.stale = true
	l.inMu.Unlock()
}

// printEntry remembers whether a pid runs claude -p, by its start time.
type printEntry struct {
	start time.Time
	print bool
}

// inbox is what was handed in since the last Load began.
type inbox struct {
	settle  bool
	stale   bool
	spend   map[string]Spend
	nudged  map[string]time.Time
	fetched []fetchedIn // in the order they came
	links   map[string]string
}

type fetchedIn struct {
	key string
	u   usage.Reading
}

// takeIn takes in what was handed in since the last Load; l.mu is held.
func (l *Loader) takeIn() {
	l.inMu.Lock()
	in := l.in
	l.in = inbox{}
	l.inMu.Unlock()
	for k, v := range in.spend {
		l.spend[k] = v
		l.spendVer[k]++
	}
	for k, t := range in.nudged {
		l.nudged[k] = t
	}
	for _, f := range in.fetched {
		l.setFetched(f.key, f.u)
	}
	for k, v := range in.links {
		l.link(k, v)
	}
	if in.stale {
		l.changedSince()
	}
	if in.settle && l.watching != nil {
		l.watching.settle = true
	}
}

// LinkSpawn records that the session with row key parent ran the agent
// with row key child from its shell: child is listed with it from then on.
func (l *Loader) LinkSpawn(child, parent string) {
	l.inMu.Lock()
	defer l.inMu.Unlock()
	if l.in.links == nil {
		l.in.links = map[string]string{}
	}
	l.in.links[child] = parent
	l.in.stale = true
}

func (l *Loader) link(child, parent string) {
	if old, ok := l.links[child]; child != parent && (!ok || old != parent) {
		if l.links == nil {
			l.links = map[string]string{}
		}
		l.links[child] = parent
		l.linksDirty = true
	}
}

// SetFetched stores a plan reading fetched for an account, kept under key
// (see ReadingKey).
func (l *Loader) SetFetched(key string, u usage.Reading) {
	l.inMu.Lock()
	defer l.inMu.Unlock()
	l.in.fetched = append(l.in.fetched, fetchedIn{key, u})
	l.in.stale = true
}

func (l *Loader) setFetched(key string, u usage.Reading) {
	if old, ok := l.fetched[key]; ok && u.FetchedAt.IsZero() {
		old.Problem = u.Problem // keep the last good numbers, note why they're not refreshing
		l.fetched[key] = old
		return
	}
	l.fetched[key] = u
}

// ReadingKey is where a reading r of profile p's account is kept: by the
// login it's signed in as, else by its folder.
func ReadingKey(p agent.Profile, r usage.Reading) string {
	if r.AccountID != "" {
		return state.Login{ID: r.AccountID}.UsageKey()
	}
	return p.Dir
}

// plans is how the logins agent's plans are read.
func plans() (agent.PlanReader, bool) {
	return agent.As[agent.PlanReader](agent.Kind(state.LoginsKind))
}

// syncUsage takes the readings shared through UsagePath that are newer
// than those it has: a session's, made as it runs, reach the header within
// a second rather than at the next fetch.
func (l *Loader) syncUsage() {
	st, err := os.Stat(l.UsagePath)
	if err != nil || st.ModTime().Equal(l.usageMod) {
		return
	}
	l.usageMod = st.ModTime()
	pr, ok := plans()
	if !ok {
		return
	}
	all := pr.Readings(l.UsagePath)
	for k := range all {
		if old, ok := l.fetched[k]; ok && !all[k].FetchedAt.After(old.FetchedAt) {
			continue
		}
		l.fetched[k] = all[k]
	}
}

type subsEntry struct {
	st   agent.SubagentStats
	at   time.Time
	dir  time.Time // the subagents folder's time when counted
	main int64     // the transcript's size when counted
	gone bool      // whether its process was known to be gone
	runs agent.SubagentRuns
}

// subagents counts an agent's subagent runs, and those still working.
// Whether one is working is read from the transcripts (SubagentRuns): its
// call without a result, or a background launch not yet reported done,
// however long the run itself has been quiet, unless gone says the
// session's process is known to have exited. It's counted again when a run
// started (the folder changed), when the transcript grew (a run ended or
// was woken), when some were working, or after 30s.
func (l *Loader) subagents(k agent.Kind, key, transcript string, gone bool, now time.Time) (agent.SubagentStats, []SubagentTile) {
	f, ok := agent.As[agent.RunFollower](k)
	if !ok {
		return agent.SubagentStats{}, nil
	}
	var dir time.Time
	if st, err := os.Stat(f.SubagentsDir(transcript)); err == nil {
		dir = st.ModTime()
	} else {
		return agent.SubagentStats{}, nil
	}
	var main int64
	if st, err := os.Stat(transcript); err == nil {
		main = st.Size()
	}
	e, ok := l.subs[key]
	if l.quick {
		if !ok || e.runs == nil {
			return agent.SubagentStats{}, nil
		}
		return e.st, subagentTiles(transcript, e.runs.Running())
	}
	working := e.st.Direct+e.st.Nested > 0
	if ok && e.runs != nil && e.dir.Equal(dir) && e.main == main && e.gone == gone && now.Sub(e.at) < 30*time.Second && (!working || now.Sub(e.at) < 3*time.Second) {
		return e.st, subagentTiles(transcript, e.runs.Running())
	}
	if e.runs == nil {
		e.runs = f.SubagentRuns()
	}
	e.runs.SetGone(gone)
	e.st, e.at, e.dir, e.main, e.gone = e.runs.Stats(transcript, now), now, dir, main, gone
	l.subs[key] = e
	return e.st, subagentTiles(transcript, e.runs.Running())
}

// SubagentTile is one of an agent's subagent runs still working, as the
// Wall draws it: its own tile, beside its parent's.
type SubagentTile struct {
	ID, Type, Description string
	Path                  string    // its own transcript
	Worktree              string    // its checkout, when not its session's
	ToolUseID             string    // the parent's call waiting on it
	Mod                   time.Time // when its transcript was last written
}

// subagentTiles are the runs a SubagentRuns calls still working, as tiles:
// each one's own transcript sits flat beside the others, whatever its
// depth, under the session's own.
func subagentTiles(transcript string, runs []agent.SubagentRun) []SubagentTile {
	if len(runs) == 0 {
		return nil
	}
	out := make([]SubagentTile, 0, len(runs))
	for _, r := range runs {
		t := SubagentTile{ID: r.ID, Type: r.Type, Description: r.Description, Path: r.Path, ToolUseID: r.ToolUseID, Mod: r.Mod}
		t.Worktree, _ = SubWorktree(t.Path, TranscriptCwd(transcript))
		out = append(out, t)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// isPrint reports whether pid is claude -p, reading its arguments once.
func (l *Loader) isPrint(tab *proc.Table, pid int) bool {
	var start time.Time
	if tab != nil {
		if p := tab.Procs[pid]; p != nil {
			start = p.Start
		}
	}
	if e, ok := l.print[pid]; ok && !start.IsZero() && e.start.Equal(start) {
		return e.print
	}
	v := isPrint(proc.Args(pid))
	if !start.IsZero() {
		l.print[pid] = printEntry{start: start, print: v}
	}
	return v
}

type argsEntry struct {
	start time.Time
	cmd   string
}

type gitInfo struct {
	repo, branch string
	at           time.Time
}

func NewLoader(s *state.Store) *Loader {
	return &Loader{
		store: s,
		args:  map[int]argsEntry{}, git: map[string]gitInfo{}, roots: map[string]string{},
		spend: map[string]Spend{}, nudged: map[string]time.Time{}, subs: map[string]subsEntry{}, fetched: map[string]usage.Reading{},
		files: map[string]fileMemo{}, moved: map[string]string{}, unfound: map[string]time.Time{}, pastRows: map[string]pastRow{}, spendVer: map[string]int{}, print: map[int]printEntry{},
		Temp: NewTempSizes(), UsagePath: filepath.Join(state.Dir(), "usage.json"), links: loadLinks(),
	}
}

// SetSpend receives cost totals from the background scanner.
func (l *Loader) SetSpend(m map[string]Spend) {
	if len(m) == 0 {
		return
	}
	l.inMu.Lock()
	defer l.inMu.Unlock()
	if l.in.spend == nil {
		l.in.spend = make(map[string]Spend, len(m))
	}
	for k, v := range m {
		l.in.spend[k] = v
	}
	l.in.stale = true
}

func (l *Loader) Load(sampleProcs bool) *Snapshot {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.load(sampleProcs)
}

// LoadQuick is Load without reading subagent runs, which means reading
// every recent session's whole transcript: rush's first list, drawn
// before anything else. The next Load is a whole one, and it counts them.
func (l *Loader) LoadQuick() *Snapshot {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.quick = true
	snap := l.load(true)
	l.quick = false
	if l.watching != nil {
		l.watching.stale = true
	}
	return snap
}

// LoadFrom is Load reading the config and overlay from s, a copy the UI
// made (state.Store.Copy), rather than the store it goes on changing.
func (l *Loader) LoadFrom(s *state.Store, sampleProcs bool) *Snapshot {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.store = s
	return l.load(sampleProcs)
}

func (l *Loader) load(sampleProcs bool) *Snapshot { //nolint:gocognit,gocyclo,maintidx // Load's body as it was, moved under the lock
	// Nothing is read as the Loader is made, on the UI goroutine: the
	// first load reads what's kept and starts watching.
	l.Temp.Load()
	if wt := l.watching; wt != nil && wt.w == nil {
		wt.w = fswait.NewWatcher()
	}
	l.takeIn()
	now := time.Now()
	if sampleProcs {
		if snap, ok := l.reuse(now); ok {
			return snap
		}
	}
	l.began()
	skipPast := l.skipPast.Load()
	snap := &Snapshot{At: now}
	cfg := l.store.Config
	ov := l.store.Overlay
	active := cfg.ActiveAccount()

	var tab *proc.Table
	if sampleProcs {
		tab = proc.Snapshot(l.prevTab)
	}

	seen := map[string]bool{}
	hosted := l.hosts.List()
	for _, info := range hosted {
		if info.Lost {
			go func() { _ = host.Revive(info) }() // its next reading shows it working again
		}
	}
	if len(l.idleRows) > 0 {
		present := make(map[string]bool, len(hosted))
		for _, info := range hosted {
			present[info.ID] = true
		}
		for id := range l.idleRows {
			if !present[id] {
				delete(l.idleRows, id)
			}
		}
	}
	// Claude Code processes rush's own hosts run: they register as
	// sessions too, but they're the rush agents, not agents of their own.
	ours := map[string]bool{}
	oursPID := map[int]bool{}
	for _, info := range hosted {
		ours[info.SessionID] = true
		if info.ClaudePID != 0 {
			oursPID[info.ClaudePID] = true
		}
	}
	// Processes of sessions a row stands for: a claude -p run under one of
	// them is its spawn, counted with its subagents rather than a row.
	parents := map[int]bool{}
	for _, info := range hosted {
		parents[info.ClaudePID], parents[info.HostPID] = true, true
	}
	var spawned []spawn
	// Conversations a row already stands for: the rest are past ones.
	claimed := map[string]bool{}
	for id := range ours {
		claimed[id] = true
	}
	l.branches(hosted, claimed)
	l.syncUsage()
	for _, p := range []agent.Profile{active.Profile()} { // ~/.claude: every session runs there
		var workers, pins map[string]int
		var prs map[string]agent.PR
		if k, ok := agent.As[agent.JobKeeper](p.Kind); ok {
			workers, pins, prs = k.Workers(p), k.Pins(p), k.LinkedPRs(p)
		}
		av := AccountView{Folder: active, Current: true}
		if j, ok := agent.As[agent.Joiner](p.Kind); ok {
			av.Daemon = j.ServiceUp(p)
		}
		av.Usage = l.readUsage(p)
		av.Quota = av.Usage.Quota(ReadingKey(p, av.Usage))
		// Its sessions, as its adapter finds them: background jobs, then
		// every session whose process is alive.
		var jobs, sessions []agent.Session
		live := liveOf(p)
		for i := range live {
			if live[i].Job != nil {
				jobs = append(jobs, live[i])
			} else {
				sessions = append(sessions, live[i])
			}
		}
		byJob := map[string]agent.Session{}
		for _, ss := range sessions {
			parents[ss.PID] = true
		}
		for _, ss := range sessions {
			claimed[ss.ID] = true
			if ss.JobID != "" {
				byJob[ss.JobID] = ss
			}
		}
		for i := range jobs {
			j := jobs[i].Job
			id := j.ID
			key := state.Key(p.Name, id)
			seen[key] = true
			claimed[j.SessionID] = true
			a := &Agent{Job: *j, Extra: jobs[i].Extra, Key: key, Acct: p, Kind: string(p.Kind), DisplayName: j.Name}
			if ss, ok := byJob[id]; ok {
				a.applyStatus(&ss)
			}
			if a.State == "running" { // only a busy session makes it work
				a.State = "done"
			}
			if t, ok := l.nudged[key]; ok {
				switch {
				case now.Sub(t) > 20*time.Second || j.UpdatedAt.After(t) && j.State == "working":
					delete(l.nudged, key)
				case !a.Live():
					a.State, a.Needs, a.Checking, a.Detail = "working", "", false, ""
				}
			}
			if n := ov.Names[key]; n != "" {
				a.DisplayName = n
			}
			a.Pinned = pins[id] > 0
			_, a.Done = ov.Done[key]
			a.Group = ov.Groups[key]
			if t, ok := ov.Seen[key]; ok && !j.UpdatedAt.After(t) {
				a.Seen = true
			}
			if pid, ok := workers[id]; ok {
				a.Worker = &Worker{PID: pid}
			}
			// A job's worktree, not its cwd, is where it actually runs:
			// Claude Code doesn't always move cwd to match once a job's
			// given a worktree.
			a.Cwd = jobCwd(*j)
			a.Repo, a.Branch = l.gitFor(a.Cwd, now)
			if a.Branch == "" && j.WorktreeBranch != "" {
				// Claude Code's own record of the branch it made the
				// worktree on, for before the checkout exists to read;
				// once it does, what's actually checked out there wins,
				// since a job's worktree can be re-pointed at another
				// branch after Claude Code first recorded it.
				a.Branch = j.WorktreeBranch
			}
			a.Spend = l.spend[key]
			if j.TranscriptPath != "" && (a.Live() || a.PID != 0 || now.Sub(j.UpdatedAt) < 24*time.Hour) {
				// Its process isn't always known, so it's never taken as gone.
				a.Subs, a.Subagents = l.subagents(p.Kind, key, j.TranscriptPath, false, now)
			}
			for _, u := range a.Spend.PRs {
				// Only PRs Claude Code linked to a session; a URL merely
				// mentioned in a transcript is not this agent's PR.
				if pr, ok := prs[u]; ok {
					pr.URL = u
					a.PRs = append(a.PRs, pr)
				}
			}
			if a.Worker != nil {
				a.PID = a.Worker.PID
				// A roster left behind by a crashed daemon names pids that are
				// gone or reused; only a live claude process counts.
				if tab != nil && !isProgramPID(tab, a.PID, a.Acct.Kind) {
					a.PID, a.Worker = 0, nil
				}
			}
			if tab != nil && a.Live() && a.PID == 0 {
				a.State, a.Detail = "stopped", "lost its process"
			}
			l.sample(tab, a)
			if a.Live() {
				av.Live++
			}
			av.Agents++
			av.Spend += a.Spend.Cost
			av.Today += a.Spend.Today
			snap.Agents = append(snap.Agents, a)
		}
		for _, ss := range sessions {
			if !ss.Interactive || len(ss.ID) < 8 || tab != nil && !isProgramPID(tab, ss.PID, p.Kind) || ours[ss.ID] || oursPID[ss.PID] {
				continue
			}
			key := state.Key(p.Name, "i:"+ss.ID[:8])
			seen[key] = true
			st := "idle"
			if ss.Status == "busy" || ss.Status == "shell" {
				st = "working"
			}
			j := agent.Job{
				ID: ss.ID[:8], Account: p.Name, Name: ss.Name, State: st, Cwd: ss.Cwd,
				SessionID: ss.ID, CreatedAt: ss.CreatedAt, UpdatedAt: ss.UpdatedAt,
				TranscriptPath: l.transcriptOf(p, ss.Cwd, ss.ID),
			}
			headless := l.isPrint(tab, ss.PID)
			if headless && spawnOf(tab, ss.PID, parents) != 0 {
				spawned = append(spawned, spawn{key, ss.PID})
				continue
			}
			if st == "idle" {
				j.Detail = "open in a terminal"
				if headless {
					j.Detail = "run by another program"
				}
			}
			a := &Agent{Job: j, Key: key, Acct: p, Kind: string(p.Kind), DisplayName: ss.Name, Interactive: true, Headless: headless, PID: ss.PID}
			if ss.Cwd == advisor.Dir() {
				a.Advisor, a.DisplayName, a.Detail = true, "✦ advisor", "looking over your sessions for savings"
				if tab != nil && tab.Procs[ss.PID] != nil && strings.Contains(l.cmdline(tab.Procs[ss.PID]), "--model opus") {
					a.Detail = "checking a saving it found"
				}
			}
			if n := ov.Names[key]; n != "" {
				a.DisplayName = n
			}
			_, a.Done = ov.Done[key]
			a.Group = ov.Groups[key]
			a.Spend = l.spend[key]
			// ss.Cwd is only ever where the session started; its own
			// transcript's cwd (never a subagent's) says where it last
			// actually worked.
			dir := ss.Cwd
			if a.Spend.Dir != "" {
				dir = a.Spend.Dir
			}
			a.Cwd = dir
			a.Repo, a.Branch = l.gitFor(dir, now)
			a.Subs, a.Subagents = l.subagents(p.Kind, key, j.TranscriptPath, false, now) // listed only while its process runs
			l.sample(tab, a)
			if a.Live() {
				av.Live++
			}
			av.Agents++
			av.Spend += a.Spend.Cost
			av.Today += a.Spend.Today
			snap.Agents = append(snap.Agents, a)
		}
		for _, info := range hosted {
			// Every Claude session runs in ~/.claude, whatever name its
			// folder had when it started.
			if info.Kind != string(p.Kind) {
				continue // listed under its own profile, below
			}
			a := l.hostedAgent(p, info, tab, now)
			if a.Live() {
				av.Live++
			}
			av.Agents++
			av.Spend += a.Spend.Cost
			av.Today += a.Spend.Today
			snap.Agents = append(snap.Agents, a)
		}
		if skipPast {
			snap.Accounts = append(snap.Accounts, av)
			continue
		}
		for _, a := range l.pastAgents(p, claimed, seen, now) {
			av.Agents++
			av.Spend += a.Spend.Cost
			av.Today += a.Spend.Today
			snap.Agents = append(snap.Agents, a)
		}
		snap.Accounts = append(snap.Accounts, av)
	}
	// Other agents' sessions belong to no Claude folder: they're listed
	// under their own profile's name.
	for _, info := range hosted {
		if info.Kind != string(active.Profile().Kind) {
			snap.Agents = append(snap.Agents, l.hostedAgent(agent.Profile{Kind: active.Profile().Kind, Name: info.Account}, info, tab, now))
		}
	}
	snap.Agents = append(snap.Agents, l.otherAgents(active.Profile().Kind, claimed, seen, now, skipPast)...)
	// Rows kept from one reading to the next (past conversations) are
	// the loader's: the snapshot gets its own, which the UI may change
	// while the next reading is made.
	for i, a := range snap.Agents {
		c := *a
		snap.Agents[i] = &c
	}
	spawned = l.hostedSpawns(hosted, snap.Agents, spawned)
	snap.Agents = l.foldSpawns(tab, snap.Agents, spawned, parents)
	if len(ov.Hidden) > 0 { // every route above ends here: one filter for all of them
		snap.Agents = slices.DeleteFunc(snap.Agents, func(a *Agent) bool { _, h := ov.Hidden[a.Key]; return h })
	}
	if len(snap.Accounts) > 0 {
		snap.Logins = l.logins(cfg, snap.Accounts[0], now)
	}
	if tab != nil {
		snap.Machine = l.machine(tab, snap)
		l.prevTab = tab
		snap.Table = tab
	}
	l.Temp.Save()
	l.saveLinks()
	listed := make(map[string]bool, len(snap.Agents))
	for _, a := range snap.Agents {
		a.Temp = l.Temp.Bytes(a.Key)
		if a.Repo != "" {
			a.Root = firstNonEmpty(mainCheckout(a.Repo, l.roots), a.Repo)
		}
		listed[a.Key] = true
	}
	l.compactions(snap.Agents, hosted, now)
	for k := range l.subs {
		if !listed[k] {
			delete(l.subs, k) // an agent no longer listed: its runs' reader goes
		}
	}
	sort.SliceStable(snap.Agents, func(i, j int) bool {
		return snap.Agents[i].Age(now) < snap.Agents[j].Age(now)
	})
	if sampleProcs {
		l.read(snap, hosted)
	}
	return snap
}

// hosted turns a rush-mode session's info into an agent row.
// discoverer is how agent k's sessions are found.
func discoverer(k agent.Kind) (agent.Discoverer, bool) {
	a, ok := agent.Get(k)
	if !ok {
		return nil, false
	}
	d, ok := a.(agent.Discoverer)
	return d, ok
}

// liveOf is the running sessions in profile p, as its agent finds them.
func liveOf(p agent.Profile) []agent.Session {
	if d, ok := discoverer(p.Kind); ok {
		return d.Live(p)
	}
	return nil
}

// isProgram is whether a process called comm (a name, or a path) is agent
// k's program.
func isProgram(k agent.Kind, comm string) bool {
	p := agent.ProgramOf(k)
	return p != "" && filepath.Base(comm) == p
}

// hostedAgent is a rush session's row, with what you've set on it.
func (l *Loader) hostedAgent(p agent.Profile, info host.Info, tab *proc.Table, now time.Time) *Agent { //nolint:gocritic // host.Info goes by value, as the hosts list it
	ov := l.store.Overlay
	a := l.hostedBase(p, info, tab, now)
	if n := ov.Names[a.Key]; n != "" {
		a.DisplayName = n
	}
	_, a.Done = ov.Done[a.Key]
	a.Group = ov.Groups[a.Key]
	if t, ok := ov.Seen[a.Key]; ok && !info.UpdatedAt.After(t) {
		a.Seen = true
	}
	a.Spend = l.spend[a.Key]
	// The host follows runtime cwd changes. An asynchronous transcript scan
	// may still describe its old checkout; never let that move this row.
	if a.Cwd == "" && a.Spend.Dir != "" {
		a.Cwd = a.Spend.Dir
		a.Repo, a.Branch = l.gitFor(a.Cwd, now)
	}
	// A transcript is priced call by call, subagents and all; the host's
	// own figure is only for agents that leave none. (Older hosts summed
	// Claude Code's running totals, so the one they saved can be far out.)
	if !a.Spend.Ready && a.Spend.Cost < info.CostUSD {
		a.Spend.Cost = info.CostUSD
	}
	return a
}

// transcriptOf is where a session's conversation is now: under the
// folder it started in, or, once entering a worktree has moved it,
// wherever it was found, remembered so it's looked for only once.
func (l *Loader) transcriptOf(pr agent.Profile, cwd, sid string) string {
	if p, ok := l.moved[sid]; ok {
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	at := agent.TranscriptPath(pr.Kind, pr, cwd, sid)
	if _, err := os.Stat(at); err == nil {
		delete(l.moved, sid)
		delete(l.unfound, sid)
		return at
	}
	if t, ok := l.unfound[sid]; ok && time.Since(t) < 10*time.Second {
		return at
	}
	t, ok := agent.As[agent.Transcripts](pr.Kind)
	if !ok {
		return at
	}
	p := t.FindTranscript(pr, cwd, sid)
	if p != at {
		l.moved[sid] = p
		delete(l.unfound, sid)
	} else {
		delete(l.moved, sid)
		l.unfound[sid] = time.Now()
	}
	return p
}

func (l *Loader) hosted(p agent.Profile, info host.Info, tab *proc.Table, now time.Time) *Agent { //nolint:gocyclo,gocritic // one case per thing a host can say
	st := info.State
	switch st {
	case "idle", "starting":
		st = "done"
	case "":
		st = "stopped"
	}
	name := info.Name
	if name == "" {
		name = info.Detail
	}
	if name == "" {
		name = "rush session " + info.ID
	}
	j := agent.Job{
		ID: info.ID, Account: p.Name, Name: name, State: st, Detail: info.Detail, Needs: info.Needs,
		Cwd: info.Cwd, SessionID: info.SessionID, CreatedAt: info.StartedAt, UpdatedAt: info.UpdatedAt,
	}
	if !info.IdleSince.IsZero() {
		j.UpdatedAt = info.IdleSince // a restart while it waits isn't a finish
	}
	if info.Kind == string(p.Kind) {
		j.TranscriptPath = l.transcriptOf(p, info.Cwd, info.SessionID)
	}
	// What it runs in the background, as Claude Code's own background
	// sessions record theirs, so the list says so alike.
	for _, t := range info.Background {
		kind := "shell"
		switch t.Type {
		case "local_agent", "remote_agent", "in_process_teammate":
			kind = "agent"
			j.Subagents++
		case "monitor_mcp", "monitor_ws":
			kind = "monitor"
		case "local_bash":
		default:
			kind = "task"
		}
		j.Running = append(j.Running, agent.Task{Kind: kind, Label: t.Label, StartedAt: t.StartedAt})
		j.Background = append(j.Background, kind+"\x00"+t.Label)
		j.InFlight++
	}
	switch {
	case info.Limit != nil:
		j.Detail = "usage limit"
		if !info.Limit.ResetsAt.IsZero() {
			j.Detail += " · resets " + info.Limit.ResetsAt.Local().Format("15:04")
		}
		if info.Limit.Ask {
			st, j.State, j.Needs = "blocked", "blocked", "continue when the limit resets?"
		}
	case info.Retry != nil && info.Retry.GaveUp:
		j.Detail = "API error · " + info.Retry.Why
	case info.Retry != nil && info.Retry.Offline:
		j.Detail = "offline · continues when the network is back"
	case info.Retry != nil && info.Retry.Proof:
		j.Detail = "API error · continues once the connection holds"
	case info.Retry != nil:
		j.Detail = fmt.Sprintf("API error · retry %d of %d", info.Retry.Attempt, info.Retry.Max)
	case info.Error != "" && st == "done":
		j.Detail = "stopped mid-turn · your next message resumes it"
	case info.Lost:
		j.Detail = "its host went away mid-turn · bringing it back"
	}
	a := &Agent{Job: j, Key: state.Key(p.Name, "a:"+info.ID), Acct: agent.Profile{Kind: agent.Kind(info.Kind), Name: p.Name, Dir: p.Dir}, DisplayName: name, Rush: true, Kind: info.Kind, Profile: info.Profile}
	// A sleeping host has gone; its pid is the one it had. Trusted on the
	// loads that don't sample processes, it pulled the row into Active
	// every other refresh.
	if info.State != "stopped" && !info.Sleeping && info.HostPID > 0 && (tab == nil || tab.Procs[info.HostPID] != nil) {
		a.PID = info.HostPID
	}
	a.Repo, a.Branch = l.gitFor(info.Cwd, now)
	l.sample(tab, a)
	return a
}

func (l *Loader) sample(tab *proc.Table, a *Agent) {
	if tab == nil || a.PID == 0 {
		return
	}
	tab.Fill(l.prevTab, []int{a.PID})
	a.Mem, a.CPU, a.Procs = tab.Sum(a.PID, nil)
}

// readUsage is p's plan: the agent's own record of it, or rush's
// reading when that's newer.
func (l *Loader) readUsage(p agent.Profile) usage.Reading {
	var r usage.Reading
	if pr, ok := plans(); ok {
		r = pr.Plan(p)
	}
	return l.freshest(p.Dir, r)
}

// freshest prefers rush's own fetch when it is newer than the agent's
// cache. Windows that have reset since either reading are dropped.
func (l *Loader) freshest(dir string, cached usage.Reading) usage.Reading {
	f, ok := l.fetched[dir]
	if cached.AccountID != "" {
		// Readings are kept by the login it's signed in as.
		g, gok := l.fetched[state.Login{ID: cached.AccountID}.UsageKey()]
		if gok && (!ok || g.FetchedAt.After(f.FetchedAt) || f.AccountID != cached.AccountID) {
			f, ok = g, true
			f.AccountID = cached.AccountID
		}
	}
	if !ok {
		return cached.Since(time.Now())
	}
	// A reading made before the folder was signed in as another account is
	// that account's, not this one's.
	other := f.AccountID != "" && cached.AccountID != "" && f.AccountID != cached.AccountID
	if f.FetchedAt.After(cached.FetchedAt) && !other {
		f.AccountID, f.Email, f.Org, f.Plan = cached.AccountID, cached.Email, cached.Org, cached.Plan
		f.Role, f.Billing, f.OrgType, f.Extra = cached.Role, cached.Billing, cached.OrgType, cached.Extra
		return f.Since(time.Now())
	}
	if !other {
		cached.Problem = f.Problem
	}
	return cached.Since(time.Now())
}

// gitFor finds the repository and branch for a folder by reading .git
// directly; no git subprocess.
func (l *Loader) gitFor(dir string, now time.Time) (string, string) {
	if dir == "" {
		return "", ""
	}
	if g, ok := l.git[dir]; ok && now.Sub(g.at) < 30*time.Second {
		return g.repo, g.branch
	}
	g := gitInfo{at: now}
	g.repo, g.branch = gitAt(dir)
	l.git[dir] = g
	return g.repo, g.branch
}

// gitAt is the checkout dir is in, and its branch, read directly.
func gitAt(dir string) (repo, branch string) {
	for d := dir; d != "/" && d != "."; d = filepath.Dir(d) {
		p := filepath.Join(d, ".git")
		st, err := os.Stat(p)
		if err != nil {
			continue
		}
		gitDir := p
		if !st.IsDir() {
			b, _ := os.ReadFile(p)
			s := strings.TrimSpace(strings.TrimPrefix(string(b), "gitdir:"))
			if !filepath.IsAbs(s) {
				s = filepath.Join(d, s)
			}
			gitDir = s
		}
		repo = d
		if b, err := os.ReadFile(filepath.Join(gitDir, "HEAD")); err == nil {
			h := strings.TrimSpace(string(b))
			branch = strings.TrimPrefix(h, "ref: refs/heads/")
			if len(branch) == 40 {
				branch = branch[:8]
			}
		}
		break
	}
	return repo, branch
}

func (l *Loader) cmdline(p *proc.Proc) string {
	if e, ok := l.args[p.PID]; ok && e.start.Equal(p.Start) {
		return e.cmd
	}
	c := proc.CommandLine(p.PID)
	if c == "" {
		c = p.Comm
	}
	l.args[p.PID] = argsEntry{start: p.Start, cmd: c}
	return c
}

func (l *Loader) machine(tab *proc.Table, snap *Snapshot) Machine {
	home := l.store.Config.ActiveAccount().Profile().Kind // whose daemon, workers and spares these are
	var m Machine
	workerOf := make(map[int]*Agent, len(snap.Agents))
	agentOf := make(map[int]*Agent, len(snap.Agents)) // every agent's own root process
	for _, a := range snap.Agents {
		if a.Worker != nil {
			workerOf[a.Worker.PID] = a
		}
		if a.PID != 0 && tab.Procs[a.PID] != nil {
			agentOf[a.PID] = a
		}
	}
	for pid := range l.args {
		if _, ok := tab.Procs[pid]; !ok {
			delete(l.args, pid)
		}
	}
	for pid := range l.print {
		if _, ok := tab.Procs[pid]; !ok {
			delete(l.print, pid)
		}
	}
	for pid, p := range tab.Procs {
		var row ProcRow
		switch {
		case agentOf[pid] != nil && workerOf[pid] == nil:
			// A rush session's host, or a claude in a terminal: the agent.
			a := agentOf[pid]
			row = ProcRow{PID: pid, Cmd: l.cmdline(p), Start: p.Start, Role: RoleWorker, Label: a.DisplayName, Key: a.Key}
		case isProgram(home, p.Comm):
			cmd := l.cmdline(p)
			row = ProcRow{PID: pid, Cmd: cmd, Start: p.Start}
			switch {
			case strings.Contains(cmd, " daemon run"):
				row.Role, row.Label = RoleDaemon, "daemon"
			case strings.Contains(cmd, "--bg-pty-host"):
				if a := workerOf[pid]; a != nil {
					row.Role, row.Label, row.Key = RoleWorker, a.DisplayName, a.Key
				} else {
					row.Role, row.Label = RoleSpare, "spare (pre-warmed, unclaimed)"
				}
			case strings.Contains(cmd, "--bg-spare"):
				continue // counted inside its pty host
			case strings.HasSuffix(strings.TrimSpace(cmd), " agents") || strings.Contains(cmd, " agents "):
				row.Role, row.Label = RoleView, "claude agents (native view)"
			default:
				if hasAgentAncestor(tab, p) || underAgent(tab, p, agentOf) || !isProgram(home, strings.Fields(cmd + " x")[0]) {
					continue
				}
				row.Role, row.Label = RoleOther, "claude (interactive)"
			}
		case p.PPID == 1:
			cmd := l.cmdline(p)
			if !strings.Contains(cmd, "/.claude/") {
				continue
			}
			row = ProcRow{PID: pid, Role: RoleOrphan, Cmd: ShellCmd(cmd), Start: p.Start, Label: orphanLabel(cmd)}
		default:
			continue
		}
		m.Rows = append(m.Rows, row)
	}
	roots := make(map[int]bool, len(m.Rows))
	for _, r := range m.Rows {
		roots[r.PID] = true
	}
	for i := range m.Rows {
		r := &m.Rows[i]
		tab.Fill(l.prevTab, []int{r.PID})
		r.Mem, r.CPU, r.Procs = tab.Sum(r.PID, roots)
		m.TotalMem += r.Mem
		m.TotalCPU += r.CPU
		if r.Role == RoleSpare {
			m.Spares++
			m.SpareMem += r.Mem
		}
		if r.Role == RoleOrphan {
			m.Orphans++
			m.OrphanMem += r.Mem
		}
	}
	sort.Slice(m.Rows, func(i, j int) bool {
		if m.Rows[i].Role != m.Rows[j].Role {
			return m.Rows[i].Role < m.Rows[j].Role
		}
		return m.Rows[i].Mem > m.Rows[j].Mem
	})
	return m
}

// isProgramPID is whether process pid is agent k's program.
func isProgramPID(tab *proc.Table, pid int, k agent.Kind) bool {
	p := tab.Procs[pid]
	return p != nil && isProgram(k, p.Comm)
}

// hasAgentAncestor is whether p runs under an agent's program.
func hasAgentAncestor(tab *proc.Table, p *proc.Proc) bool {
	for i, pid := 0, p.PPID; i < 64 && pid > 1; i++ {
		q := tab.Procs[pid]
		if q == nil {
			return false
		}
		if agent.IsProgram(q.Comm) {
			return true
		}
		pid = q.PPID
	}
	return false
}

// underAgent reports whether p runs inside one of the agents' trees.
func underAgent(tab *proc.Table, p *proc.Proc, agentOf map[int]*Agent) bool {
	for i, pid := 0, p.PPID; i < 64 && pid > 1; i++ {
		if agentOf[pid] != nil {
			return true
		}
		q := tab.Procs[pid]
		if q == nil {
			return false
		}
		pid = q.PPID
	}
	return false
}

func orphanLabel(cmd string) string {
	i := strings.Index(cmd, "/.claude/jobs/")
	if i >= 0 && len(cmd) >= i+22 {
		return "job " + cmd[i+14:i+22]
	}
	return "ended session"
}

// ShellCmd is what a Bash-tool shell was asked to run: the command inside
// Claude Code's eval '…' wrapper, without the snapshot sourcing around it.
func ShellCmd(cmd string) string {
	i := strings.Index(cmd, "eval '")
	if i < 0 {
		return cmd
	}
	rest := cmd[i+6:]
	var b strings.Builder
	for len(rest) > 0 {
		if strings.HasPrefix(rest, `'"'"'`) {
			b.WriteByte('\'')
			rest = rest[5:]
			continue
		}
		if rest[0] == '\'' {
			return b.String()
		}
		b.WriteByte(rest[0])
		rest = rest[1:]
	}
	return cmd
}

// isPrint reports whether a claude command line runs it non-interactively.
func isPrint(args []string) bool {
	for _, a := range args {
		if a == "-p" || a == "--print" || strings.HasPrefix(a, "--output-format") {
			return true
		}
	}
	return false
}

// Where says where an interactive-kind agent is being driven from.
func (a *Agent) Where() string {
	if a.Headless && a.Kind == state.LoginsKind {
		return "run by another program (claude -p)"
	}
	if a.Headless {
		return "run by another program"
	}
	if a.Remote {
		return "on GitHub"
	}
	return "open in a terminal"
}

// spawnOf is the nearest process above pid that is one of parents: the
// session whose shell ran it. Zero when none is.
func spawnOf(tab *proc.Table, pid int, parents map[int]bool) int {
	if tab == nil || tab.Procs[pid] == nil || parents[tab.Procs[pid].PPID] {
		// Run straight from a session's process is no shell's: a host's
		// own Claude Code, before the host says which it is.
		return 0
	}
	for i, p := 0, tab.Procs[pid]; p != nil && i < 12; i++ {
		if p.PPID <= 1 {
			return 0
		}
		if parents[p.PPID] && p.PPID != pid {
			return p.PPID
		}
		p = tab.Procs[p.PPID]
	}
	return 0
}

// spawn is an agent a session's shell ran, known by its process: its
// row's key and its pid.
type spawn struct {
	key string
	pid int
}

// foldSpawns takes the agents sessions' shells ran out of the list, each
// counted with the subagents of the session that ran it. One running is
// found by its process being under that session's; one finished, by that
// having been seen before. spawned are those left out of the list already.
func (l *Loader) foldSpawns(tab *proc.Table, agents []*Agent, spawned []spawn, parents map[int]bool) []*Agent {
	byKey := make(map[string]*Agent, len(agents))
	byPID := map[int]*Agent{}
	pids := map[*Agent]int{}
	for _, a := range agents {
		byKey[a.Key] = a
		pid := a.PID
		// Another agent's session says no process: a codex exec quiet for
		// a while is listed as past while it still runs.
		if pid == 0 && (a.Interactive && !a.Past || a.Headless) && !a.Remote && !a.Rush {
			pid = procOf(tab, agent.Kind(a.Kind), a.CreatedAt)
		}
		if pid != 0 {
			pids[a], byPID[pid] = pid, a
			parents[pid] = true
		}
	}
	for _, a := range agents {
		if pid := pids[a]; a.Headless && pid != 0 && spawnOf(tab, pid, parents) != 0 {
			spawned = append(spawned, spawn{a.Key, pid})
		}
	}
	l.lookForAskers(agents)
	gone := map[string]*Agent{} // by key, the session each ran under
	for _, sp := range spawned {
		if p := ranBy(tab, sp.pid, byPID); p != nil {
			p.Subs.Direct++
			p.Subs.Spawned++
			gone[sp.key] = p
			l.link(sp.key, p.Key)
		}
	}
	out := agents[:0]
	for _, a := range agents {
		if p := gone[a.Key]; p != nil {
			p.addSpend(a)
			continue
		}
		// ponytail: one level only; a spawn's own spawns fold into it, and
		// out of sight once it has folded too.
		// One running was found by its process above: this one has ended.
		if p := byKey[l.links[a.Key]]; p != nil && p != a && gone[p.Key] == nil {
			p.Subs.Spawned++
			p.addSpend(a)
			continue
		}
		out = append(out, a)
	}
	return out
}

// hostedSpawns links the rush sessions an agent's shell ran (rush spawn,
// through its stand-in on PATH) to the session that ran them, when that's
// a rush session too; one run by another is found by its process, while
// it runs.
func (l *Loader) hostedSpawns(hosted []host.Info, agents []*Agent, spawned []spawn) []spawn {
	byID := map[string]*Agent{}
	for _, a := range agents {
		if a.Rush {
			byID[a.ID] = a
		}
	}
	for i := range hosted {
		by, ok := hosted[i].Meta["spawnedBy"]
		a := byID[hosted[i].ID]
		if !ok || a == nil {
			continue
		}
		if p := byID[by]; p != nil {
			l.link(a.Key, p.Key)
		} else if a.PID != 0 {
			spawned = append(spawned, spawn{a.Key, a.PID})
		}
	}
	return spawned
}

// addSpend counts what a run it folds in cost with its own.
func (a *Agent) addSpend(run *Agent) {
	a.Spend.Cost += run.Spend.Cost
	a.Spend.Today += run.Spend.Today
}

// askFor is how far back a finished headless run is looked for in the
// sessions that may have run it, and askedBefore how long before it began
// its command may have been written.
const (
	askFor      = 30 * 24 * time.Hour
	askedBefore = 10 * time.Minute
)

// asking is a headless run not seen running, and the sessions that may
// have run it: those working when it began, less other runs like it.
type asking struct {
	key, needle string
	from, to    time.Time
	keys, paths []string
}

// lookForAskers looks, in the background, for the sessions that ran the
// headless runs not seen running, by their prompts in those sessions'
// transcripts shortly before they began: each is linked (LinkSpawn) when
// found, and folded by the next load. Each run is looked for once.
// ponytail: matches the prompt's opening words, so a prompt fed from a
// file isn't found; the UI links those when their session is opened.
func (l *Loader) lookForAskers(agents []*Agent) {
	if l.asked == nil {
		l.asked = map[string]bool{}
	}
	var want []asking
	for _, a := range agents {
		needle := promptNeedle(a.Name)
		if !a.Headless || a.Remote || l.asked[a.Key] || needle == "" ||
			a.CreatedAt.IsZero() || time.Since(a.CreatedAt) > askFor {
			continue
		}
		if _, ok := l.links[a.Key]; ok {
			continue // linked, or looked for before
		}
		l.asked[a.Key] = true
		w := asking{key: a.Key, needle: needle, from: a.CreatedAt.Add(-askedBefore), to: a.CreatedAt.Add(5 * time.Second)}
		for _, p := range agents {
			path := firstNonEmpty(p.TranscriptPath, p.History)
			if p != a && (!p.Headless || p.Kind != a.Kind) && !strings.HasPrefix(p.Name, needle) && path != "" && !p.CreatedAt.After(w.to) && !p.UpdatedAt.Before(w.from) {
				w.keys, w.paths = append(w.keys, p.Key), append(w.paths, path)
			}
		}
		want = append(want, w)
	}
	if len(want) == 0 {
		return
	}
	go func() {
		for i := range want {
			w := &want[i]
			parent := "" // none: not looked for again
			for j, path := range w.paths {
				if wrote(path, []byte(w.needle), w.from, w.to) {
					parent = w.keys[j]
					break
				}
			}
			l.LinkSpawn(w.key, parent)
		}
	}()
}

// promptNeedle is the opening of a prompt as a transcript would hold it,
// up to anything JSON would escape; empty when that's too short to tell by.
func promptNeedle(prompt string) string {
	n := strings.IndexAny(prompt, `"\<>&…`)
	if n < 0 {
		n = len(prompt)
	}
	n = min(n, 48)
	if n < 20 {
		return ""
	}
	return prompt[:n]
}

var tsMark = []byte(`"timestamp":"`)

// wrote is whether a transcript has a line holding needle stamped between
// from and to.
func wrote(path string, needle []byte, from, to time.Time) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 1<<20), 64<<20)
	for sc.Scan() {
		b := sc.Bytes()
		if !bytes.Contains(b, needle) {
			continue
		}
		_, rest, ok := bytes.Cut(b, tsMark)
		if !ok {
			continue
		}
		ts, _, ok := bytes.Cut(rest, []byte(`"`))
		if !ok {
			continue
		}
		if t, err := time.Parse(time.RFC3339Nano, string(ts)); err == nil && !t.Before(from) && !t.After(to) {
			return true
		}
	}
	return false
}

// ranBy is the agent whose process is nearest above pid's, or nil.
func ranBy(tab *proc.Table, pid int, byPID map[int]*Agent) *Agent {
	if tab == nil {
		return nil
	}
	for i, p := 0, tab.Procs[pid]; p != nil && i < 12; i, p = i+1, tab.Procs[p.PPID] {
		if a := byPID[p.PPID]; a != nil {
			return a
		}
	}
	return nil
}

// procOf is the process of kind k's that started nearest to at, within
// seconds of it: for a session its agent lists no process for.
// ponytail: by start time alone, so two runs of one agent begun within
// seconds of each other may swap; match their folders too if that shows.
func procOf(tab *proc.Table, k agent.Kind, at time.Time) int {
	if tab == nil || at.IsZero() {
		return 0
	}
	best, gap := 0, 10*time.Second
	for pid, p := range tab.Procs {
		if d := p.Start.Sub(at).Abs(); d < gap && isProgram(k, p.Comm) {
			best, gap = pid, d
		}
	}
	return best
}

func linksPath() string { return state.CachePath("spawns.json") }

// loadLinks reads which sessions ran which agents.
func loadLinks() map[string]string {
	m := map[string]string{}
	if b, err := os.ReadFile(linksPath()); err == nil {
		_ = jsonx.Unmarshal(b, &m)
	}
	return m
}

// saveLinks writes the links if they changed.
// ponytail: never pruned; an entry per spawned agent, a few dozen bytes.
func (l *Loader) saveLinks() {
	if !l.linksDirty {
		return
	}
	if b, err := jsonx.Marshal(l.links); err == nil {
		_ = os.MkdirAll(filepath.Dir(linksPath()), 0o700)
		if os.WriteFile(linksPath()+".tmp", b, 0o600) == nil {
			_ = os.Rename(linksPath()+".tmp", linksPath())
		}
	}
	l.linksDirty = false
}

// jobCwd keeps a nested working folder when it belongs to the recorded worktree.
func jobCwd(j agent.Job) string {
	if j.WorktreePath == "" {
		return j.Cwd
	}
	if j.Cwd != "" {
		rel, err := filepath.Rel(j.WorktreePath, j.Cwd)
		if err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return j.Cwd
		}
	}
	return j.WorktreePath
}
