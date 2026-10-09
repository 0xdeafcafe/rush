package ui

import (
	"cmp"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"unicode"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// An agent a session ran from its shell (claude -p, codex exec) wrote a
// session of its own. rush links it to the step that ran it by evidence:
// a session rush hosted for this one's shell, begun while the step ran;
// else, for one rush didn't host, a transcript begun then, asked what the
// command asks. It draws under the step's row, listed with the subagents.

// spawnPrefix marks a spawned agent among the subagent runs.
const spawnPrefix = "spawn:"

// spawnEvery is how often spawned agents are looked for, spawnGrace how
// long after its command ended a step may still have started one, and
// spawnSlop how far clocks may disagree on when one began.
const (
	spawnEvery = 2 * time.Second
	spawnGrace = 10 * time.Second
	spawnSlop  = 3 * time.Second
)

// spawnRun is a spawned agent's session, once found, and how it's read;
// until then, a step whose command reads as running one, looked for.
type spawnRun struct {
	step   string      // the step that ran it
	sp     convo.Spawn // who it is and what it was asked
	kind   agent.Kind
	path   string      // its transcript
	tail   *convo.Tail // Claude Code's, read as it grows
	hist   *history    // another agent's, read again when it changes
	sess   *convo.Session
	born   time.Time
	mod    time.Time // when what it wrote was last seen to change
	looked time.Time // when it was last looked for, while not found
	lost   bool      // looked for past its command's end, and not found
	fresh  bool      // it changed: its step is given it again
	hosted string    // the rush session it runs as, when rush hosts it
	live   bool      // a hosted one's host says it still runs
	drawn  bool      // as live as its step last drew it
}

// view is the spawned agent's conversation as a Tail, for the subagent
// views: Claude Code's own, or another agent's as last read.
func (r *spawnRun) view() *convo.Tail {
	if r.tail != nil {
		return r.tail
	}
	return &convo.Tail{Sess: r.sess}
}

// spawnFoundMsg brings the spawned agents looked for in the background:
// those found by their transcripts, by the step that ran each, and the
// sessions rush hosted for this one's shell.
type spawnFoundMsg struct {
	key    string
	found  map[string]agent.Session
	hosted []hostedRun
	live   map[string]bool // every hosted one's, by its rush session
	at     time.Time       // when the hosted ones were listed, if they were
}

// hostedRun is a session rush hosted for the session's shell.
type hostedRun struct {
	id   string
	sp   convo.Spawn
	s    agent.Session
	live bool
}

// spawnWant is a spawned agent to look for by its transcript.
type spawnWant struct {
	step       string
	sp         convo.Spawn
	dir        string
	parent     string // the session's own id, for a harness's subagent
	start, end time.Time
}

// refreshSpawns follows the agents the session's shell ran: those found
// are read as they grow and drawn under their rows; the rest are looked
// for in the background.
func (m *Model) refreshSpawns() tea.Cmd {
	c := m.host
	if c == nil || c.sess == nil || c.spawnLooking {
		return nil
	}
	if c.spawns == nil {
		c.spawns = map[string]*spawnRun{}
	}
	now := time.Now()
	// Those rush hosted are listed while a step may yet have started one
	// since the last look, or one of them works on.
	hosted := false
	var woke []string // hosted ones done: a message (agent_send) can set one working again
	if c.id != "" && now.Sub(c.hostedAt) >= spawnEvery {
		hosted = c.hostedAt.IsZero()
		for _, w := range c.sess.Windows(now, spawnGrace) {
			hosted = hosted || w.To.After(c.hostedAt)
		}
		for _, r := range c.spawns {
			hosted = hosted || r.live
			if r.hosted != "" && !r.live {
				woke = append(woke, r.hosted)
			}
		}
	}
	var want []spawnWant
	taken, known, has := map[string]bool{}, maps.Clone(c.hostNone), map[string]bool{}
	if known == nil {
		known = map[string]bool{}
	}
	for _, r := range c.spawns {
		if r.path != "" {
			taken[r.path], known[r.hosted], has[r.step] = true, true, true
		}
	}
	for _, st := range c.sess.Spawns() {
		r := c.spawns[st.ID]
		if r == nil {
			r = &spawnRun{step: st.ID}
			c.spawns[st.ID] = r
		}
		if has[st.ID] || r.lost || now.Sub(r.looked) < spawnEvery {
			continue // one found is read with the pane's transcripts: refreshSubs
		}
		// One whose command has ended is looked for until a while after,
		// and at least once: a past conversation's are looked for as it opens.
		// A command sent to the background returned at once; its agent
		// runs on, and may write its session well after.
		end := st.End
		if st.Status == convo.Running || end.IsZero() || c.sess.JobRunning(st.ID) {
			end = now
		} else if !r.looked.IsZero() && r.looked.After(end.Add(spawnGrace)) {
			r.lost = true
			continue
		}
		r.looked = now
		sp := c.spawnOf(st)
		want = append(want, spawnWant{step: st.ID, sp: sp, dir: m.spawnDir(c, sp), parent: c.sessionID(), start: st.Start, end: end})
	}
	if len(want) == 0 && !hosted && len(woke) == 0 {
		return nil
	}
	c.spawnLooking = true
	key, parent, list := c.key, c.id, &c.hostList
	var infos []host.Info // the fleet's reading's, when there's one: not every host listed again
	if m.snap != nil && now.Sub(m.snap.At) < 5*time.Second {
		infos = m.snap.Hosted
	}
	var mine []agent.Profile // the session's own profile, where its spawns likely wrote
	if a := m.agentByKey(c.key); a != nil {
		mine = append(mine, a.Acct)
	}
	own := c.path
	return func() tea.Msg {
		msg := spawnFoundMsg{key: key, found: map[string]agent.Session{}}
		if hosted {
			if infos == nil {
				infos = list.List()
			}
			msg.hosted, msg.live = hostedRuns(infos, parent, known)
			msg.at = now
			for _, h := range msg.hosted {
				taken[h.s.Transcript] = true
			}
		} else if len(woke) > 0 {
			// Only those already known: each one's info, not every host's.
			msg.live, msg.at = map[string]bool{}, now
			byID := make(map[string]host.Info, len(infos))
			for _, in := range infos {
				byID[in.ID] = in
			}
			for _, id := range woke {
				in, ok := byID[id]
				if !ok {
					var err error
					if in, err = host.ReadInfo(id); err != nil {
						continue
					}
				}
				msg.live[id] = hostedLive(in)
			}
		}
		for _, w := range want {
			if s, ok := findSpawn(w, mine, own, taken); ok {
				msg.found[w.step] = s
				taken[s.Transcript] = true
			}
		}
		return msg
	}
}

// hostedRuns are the sessions rush hosted for parent's shell not known yet,
// each with the session its agent writes once it has one, and whether
// each of them all still runs.
func hostedRuns(infos []host.Info, parent string, known map[string]bool) (out []hostedRun, live map[string]bool) {
	live = map[string]bool{}
	for _, in := range infos {
		if in.Meta["spawnedBy"] != parent {
			continue
		}
		live[in.ID] = hostedLive(in)
		if known[in.ID] || in.SessionID == "" || in.StartedAt.IsZero() {
			continue
		}
		cfg, err := host.ReadConfig(in.ID)
		if err != nil {
			continue
		}
		k := agent.Kind(firstNonEmpty(cfg.Kind, in.Kind, string(agent.LegacyKind)))
		// A host going to sleep forgets its first message; its request
		// exchange keeps it, the spawn_agent call that asked it.
		if cfg.Prompt == "" {
			for _, e := range host.ReadExchanges(in.ID) {
				if e.Phase == "request" {
					cfg.Prompt = e.Text
					break
				}
			}
		}
		s, ok := hostedSession(k, cfg, in)
		if !ok {
			continue
		}
		sp := convo.Spawn{Kind: k, Name: string(k), Prompt: cfg.Prompt, Model: firstNonEmpty(cfg.Model, in.Model), Dir: cfg.Cwd}
		if a, ok := agent.Get(k); ok {
			sp.Name = a.Name()
		}
		out = append(out, hostedRun{id: in.ID, sp: sp, s: s, live: live[in.ID]})
	}
	return out, live
}

// hostedLive is whether a hosted run works on: idle, its turn is done.
func hostedLive(in host.Info) bool { return in.State != "stopped" && in.State != "idle" }

// hostedSession is the session a hosted run's agent writes: where its
// agent keeps session in's, or else where its own list has it.
func hostedSession(k agent.Kind, cfg host.Config, in host.Info) (agent.Session, bool) {
	s := agent.Session{Kind: k, Profile: cfg.Account, ID: in.SessionID, Name: cfg.Prompt, Cwd: cfg.Cwd, CreatedAt: in.StartedAt}
	s.Transcript = agent.TranscriptPath(k, cfg.Account, cfg.Cwd, in.SessionID)
	if s.Transcript == "" && agent.ReadsAsClaude(k) {
		s.Transcript = agent.TranscriptPath(agent.LegacyKind, cfg.Account, cfg.Cwd, in.SessionID)
	}
	if s.Transcript != "" {
		return s, true
	}
	a, ok := agent.Get(k)
	d, isD := a.(agent.Discoverer)
	if !ok || !isD {
		return s, false
	}
	for _, p := range append([]agent.Profile{cfg.Account}, a.Profiles()...) {
		for _, f := range append(d.Live(p), d.Past(p)...) {
			if f.ID == in.SessionID && f.Transcript != "" {
				f.CreatedAt = in.StartedAt
				return f, true
			}
		}
	}
	return s, false
}

// claim is the step that ran a session of kind k, asked asked, begun at at:
// of the steps whose window holds it, a spawn_agent call that asked just
// that, else the shell step whose command names its program, else the
// latest to start before it; "" when no window holds it, or none names it
// while a subagent (whose shell is the session's too) works.
func claim(wins []convo.Window, k agent.Kind, asked string, at time.Time, subBusy bool) string {
	prog := agent.ProgramOf(k)
	best, named := "", false
	var from time.Time
	for _, w := range wins {
		if at.Before(w.From.Add(-spawnSlop)) || at.After(w.To) {
			continue
		}
		n := prog != "" && names(w.Command, prog)
		if w.Asked != "" {
			if strings.TrimSpace(w.Asked) != strings.TrimSpace(asked) {
				continue // it started another
			}
			n = true
		}
		if best == "" || n && !named || n == named && w.From.After(from) {
			best, named, from = w.Step, n, w.From
		}
	}
	if !named && subBusy {
		return ""
	}
	return best
}

// subAt is the step of c's own subagent at work at at, when there's one,
// and how many were: a run begun then with no step naming it is likely
// that subagent's (a relay to another agent is), whose shell is c's too.
func (c *hostConn) subAt(at time.Time) (step string, n int) {
	t := at.UnixNano()
	for _, sa := range c.subs {
		if !strings.HasPrefix(sa.ID, spawnPrefix) && sa.Born <= t+int64(spawnSlop) && t <= sa.Mod+int64(spawnGrace) {
			step, n = sa.ToolUseID, n+1
		}
	}
	return step, n
}

// names is whether cmd has prog as a word of its own: a path to it too.
func names(cmd, prog string) bool {
	return slices.Contains(strings.FieldsFunc(cmd, func(r rune) bool {
		return !unicode.IsLetter(r) && !unicode.IsDigit(r) && !strings.ContainsRune("-_.", r)
	}), prog)
}

// onSpawnFound takes in the spawned agents found, and reads each: a hosted
// one under the step whose window holds it, one no window holds under none.
func (m *Model) onSpawnFound(msg spawnFoundMsg) {
	c := m.host
	if c == nil || c.key != msg.key {
		return
	}
	c.spawnLooking = false
	if !msg.at.IsZero() {
		c.hostedAt = msg.at
	}
	for _, r := range c.spawns {
		if live, ok := msg.live[r.hosted]; ok && r.hosted != "" && live != r.live {
			r.live, r.fresh = live, true
		}
	}
	var wins []convo.Window
	if len(msg.hosted) > 0 {
		wins = c.sess.Windows(time.Now(), spawnGrace)
	}
	for _, h := range msg.hosted {
		// Found by its transcript already, it's hosted: what you type goes to it.
		if r := c.runAt(h.s.Transcript); r != nil {
			r.hosted, r.live, r.fresh = h.id, h.live, true
			continue
		}
		sub, subs := c.subAt(h.s.CreatedAt)
		step := claim(wins, h.sp.Kind, h.sp.Prompt, h.s.CreatedAt, subs > 0)
		if step == "" && subs == 1 && c.sess.Step(sub) != nil {
			step = sub // under the subagent that ran it
		}
		if step == "" {
			// Begun outside every window, it stays so: no later step holds it.
			if c.hostNone == nil {
				c.hostNone = map[string]bool{}
			}
			c.hostNone[h.id] = true
			continue
		}
		m.followSpawn(c, h.id, &spawnRun{step: step, sp: h.sp, hosted: h.id, live: h.live}, h.s)
	}
	for id, s := range msg.found {
		r, st := c.spawns[id], c.sess.Step(id)
		if r == nil || st == nil || r.path != "" || c.runAt(s.Transcript) != nil {
			continue
		}
		r.step, r.sp = id, c.spawnOf(st)
		m.followSpawn(c, id, r, s)
	}
}

// followSpawn reads r, found as session s, from now on.
func (m *Model) followSpawn(c *hostConn, id string, r *spawnRun, s agent.Session) {
	r.kind, r.path, r.born = s.Kind, s.Transcript, s.CreatedAt
	if agent.ReadsAsClaude(s.Kind) {
		r.tail = convo.NewTail(s.Transcript)
	} else {
		r.hist = &history{kind: s.Kind, s: s}
	}
	c.spawns[id] = r
	c.paneKick = true // read it now, in the background
	if m.loader != nil && len(s.ID) >= 8 && s.Profile.Name != "" {
		// Its row is listed with this session's from now on.
		m.loader.LinkSpawn(state.Key(s.Profile.Name, "i:"+s.ID[:8]), c.key)
	}
}

// runAt is the spawned agent found whose transcript is path, or nil.
func (c *hostConn) runAt(path string) *spawnRun {
	for _, r := range c.spawns {
		if r.path != "" && r.path == path {
			return r
		}
	}
	return nil
}

// takeSpawns takes in what the found agents the shell ran have written,
// read by refreshSubs (grew are the tails that took in something), and
// gives each step its agents again when any of them changed.
func (m *Model) takeSpawns(c *hostConn, grew map[*convo.Tail]bool, hists []spawnHist) {
	for _, sh := range hists {
		if sh.sess != nil {
			sh.r.sess = sh.sess
			sh.r.fresh = true
		}
	}
	again := map[string]bool{}
	for _, r := range c.spawns {
		if r.path == "" {
			continue
		}
		changed := r.fresh
		if r.tail != nil {
			changed = changed || r.sess == nil || grew[r.tail]
			r.sess = r.tail.Sess
		}
		r.fresh = false
		if changed {
			r.mod = time.Now()
		}
		live := c.runLive(r)
		if r.sess != nil && (changed || live != r.drawn || c.spawnsOn != c.sess) {
			again[r.step], r.drawn = true, live
		}
	}
	c.spawnsOn = c.sess
	for id := range again {
		if st := c.sess.Step(id); st != nil {
			c.sess.SetChildren(st, c.children(id))
		}
	}
}

// children are the agents step ran, found and read, as they began.
func (c *hostConn) children(step string) []convo.Child {
	var kids []convo.Child
	for id, r := range c.spawns {
		if r.step == step && r.path != "" && r.sess != nil {
			kids = append(kids, convo.Child{ID: id, Spawn: r.sp, Sess: r.sess, Live: c.runLive(r), Start: r.born})
		}
	}
	slices.SortFunc(kids, func(a, b convo.Child) int {
		return cmp.Or(a.Start.Compare(b.Start), strings.Compare(a.ID, b.ID))
	})
	return kids
}

// runLive is whether a spawned agent works on: a hosted one as its host
// says, else while the command that ran it, or its background shell, does.
func (c *hostConn) runLive(r *spawnRun) bool {
	if r.hosted != "" {
		return r.live
	}
	_, live := c.stepState(r.step)
	return live
}

// spawnDir is where a spawned agent ran: the folder its command went to,
// against the session's own.
func (m *Model) spawnDir(c *hostConn, sp convo.Spawn) string {
	base := firstNonEmpty(c.sess.Info.Cwd, c.sess.Cwd)
	d := sp.Dir
	if strings.HasPrefix(d, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			d = filepath.Join(home, d[2:])
		}
	}
	switch {
	case d == "":
		return base
	case filepath.IsAbs(d) || base == "":
		return filepath.Clean(d)
	}
	return filepath.Join(base, d)
}

// findSpawn is the session a spawned agent wrote: one of its agent's,
// begun while its command ran, asked what the command asked, and in the
// folder it ran in when that's known. own and taken are transcripts that
// are someone else's.
func findSpawn(w spawnWant, mine []agent.Profile, own string, taken map[string]bool) (agent.Session, bool) {
	from, to := w.start.Add(-3*time.Second), w.end.Add(3*time.Second)
	fits := func(s agent.Session) bool {
		if s.Transcript == "" || s.Transcript == own || taken[s.Transcript] {
			return false
		}
		if s.CreatedAt.Before(from) || s.CreatedAt.After(to) {
			return false
		}
		return samePrompt(w.sp.Prompt, s.Name)
	}
	a, ok := agent.Get(w.sp.Kind)
	if !ok {
		return agent.Session{}, false
	}
	if w.sp.Child != "" {
		f, ok := a.(agent.ChildFinder)
		if !ok {
			return agent.Session{}, false // its children keep no sessions rush can find
		}
		for _, p := range append(mine, a.Profiles()...) {
			if s, found := f.FindChild(p, w.parent, w.sp.Child, w.start); found && !taken[s.Transcript] {
				return s, true
			}
		}
		return agent.Session{}, false
	}
	if f, ok := a.(agent.SpawnFinder); ok {
		return f.FindSpawn(append(mine, a.Profiles()...), w.dir, w.start, fits)
	}
	d, ok := a.(agent.Discoverer)
	if !ok {
		return agent.Session{}, false
	}
	var best agent.Session
	for _, p := range a.Profiles() {
		list := d.Live(p)
		if time.Since(w.end) > time.Minute {
			list = append(list, d.Past(p)...)
		}
		for _, s := range list {
			if !fits(s) || s.Cwd != "" && w.dir != "" && !sameDir(s.Cwd, w.dir) {
				continue
			}
			if best.Transcript == "" || s.CreatedAt.Before(best.CreatedAt) {
				best = s
			}
		}
	}
	return best, best.Transcript != ""
}

// samePrompt is whether a session's first words are what the command
// asked: the start of it, as agents clip and fold what they keep. A prompt
// not known (fed from a file) matches anything.
func samePrompt(asked, got string) bool {
	norm := func(s string) string { return strings.Join(strings.Fields(s), " ") }
	a, g := norm(asked), norm(got)
	if a == "" {
		return true
	}
	n := min(len(a), len(g), 60)
	return n > 0 && a[:n] == g[:n]
}

func sameDir(a, b string) bool {
	if a == b {
		return true
	}
	ra, err1 := filepath.EvalSymlinks(a)
	rb, err2 := filepath.EvalSymlinks(b)
	return err1 == nil && err2 == nil && ra == rb
}

// spawnJob is whether j is the shell of a spawned agent rush has found:
// it's listed with the subagents, not as a background command.
func (c *hostConn) spawnJob(j *convo.Job) bool {
	for _, r := range c.spawns {
		if r.step == j.ToolUseID && r.path != "" {
			return true
		}
	}
	return false
}

// jobKind is what task j is, as the pane shows it: a found spawned
// agent's shell is a subagent.
func (c *hostConn) jobKind(j *convo.Job) string {
	if c.spawnJob(j) {
		return "subagent"
	}
	return c.sess.JobKind(j)
}

// spawnSubs are the spawned agents found, as subagent runs, as they began.
func (c *hostConn) spawnSubs() []convo.Subagent {
	var out []convo.Subagent
	for id, r := range c.spawns {
		if r.path == "" {
			continue
		}
		sa := convo.Subagent{ID: spawnPrefix + id, Type: r.sp.Name, Description: firstNonEmpty(c.spawnTitle(r.step), oneLineUI(r.sp.Prompt), r.sp.From),
			Model: r.sp.Model, ToolUseID: r.step, Path: r.path, Born: r.born.UnixNano()}
		if !r.mod.IsZero() {
			sa.Mod = r.mod.UnixNano() // noticed as it was read, not asked of the disk
		}
		if r.tail != nil {
			sa.Size = r.tail.Size()
		}
		out = append(out, sa)
	}
	slices.SortFunc(out, func(a, b convo.Subagent) int { return cmp.Or(cmp.Compare(a.Born, b.Born), strings.Compare(a.ID, b.ID)) })
	return out
}

// spawnTitle is what the call that ran step's one agent says it's for,
// its title; a call that ran several leaves each to what it was asked.
func (c *hostConn) spawnTitle(step string) string {
	st := c.sess.Step(step)
	if st == nil || len(c.children(step)) > 1 {
		return ""
	}
	call := st.Call()
	return oneLineUI(firstNonEmpty(call.Input.Description, call.Title))
}

// subOfStep is whether sa is the run the conversation's step id draws: a
// subagent's call, or one of the agents a command ran (step/run when it
// ran several, each a row of its own).
func (c *hostConn) subOfStep(sa convo.Subagent, id string) bool {
	run, ok := strings.CutPrefix(sa.ID, spawnPrefix)
	if step, kid, fan := strings.Cut(id, "/"); ok && fan {
		return sa.ToolUseID == step && run == kid
	}
	return sa.ToolUseID == id && (!ok || len(c.children(id)) <= 1)
}

// spawnState is how a spawned agent stands: a hosted one as its host
// says; else as the command that ran it, or its background shell, does.
func (c *hostConn) spawnState(sa convo.Subagent) (status string, live bool) {
	if r := c.spawns[strings.TrimPrefix(sa.ID, spawnPrefix)]; r != nil && r.hosted != "" {
		return "", r.live
	}
	return c.stepState(sa.ToolUseID)
}

// stepState is how the command step id ran stands: still running, or its
// background shell; else how it ended.
func (c *hostConn) stepState(id string) (status string, live bool) {
	for _, j := range c.sess.Jobs() {
		if j.ToolUseID == id {
			if j.Running() {
				return "", true
			}
			if j.Status == "failed" || j.Status == "stopped" {
				return j.Status, false
			}
		}
	}
	st := c.sess.Step(id)
	switch {
	case st == nil:
		return "", false
	case st.Status == convo.Running:
		return "", true
	case st.Status == convo.Failed:
		return "failed", false
	case st.Status == convo.Lost:
		return "stopped", false
	}
	return "", false
}

// watchedHost is the rush session of the spawned agent the pane shows,
// when rush hosts it: what you send goes to it rather than the session.
func (m *Model) watchedHost(c *hostConn) string {
	if !m.watchingSub(c) {
		return ""
	}
	if r := c.spawnRunFor(c.subOpen); r != nil {
		return r.hosted
	}
	return ""
}

// spawnRunFor is the spawned agent a subagent run id stands for, or nil.
func (c *hostConn) spawnRunFor(id string) *spawnRun {
	if !strings.HasPrefix(id, spawnPrefix) || c.spawns == nil {
		return nil
	}
	if r := c.spawns[strings.TrimPrefix(id, spawnPrefix)]; r != nil && r.path != "" && r.sess != nil {
		return r
	}
	return nil
}

// spawnOf is the agent step st started: a harness's own subagent is of
// the session's agent.
func (c *hostConn) spawnOf(st *convo.Step) convo.Spawn {
	sp, _ := st.Spawn()
	if sp.Kind == "" {
		sp.Kind = c.kindOf()
	}
	return sp
}

// sessionID is the id the session's agent knows it by.
func (c *hostConn) sessionID() string {
	if c.hist != nil {
		return firstNonEmpty(c.sess.Info.SessionID, c.hist.s.ID)
	}
	return c.sess.Info.SessionID
}

// subKind is the agent run id is: a spawned one's own, else the session's.
func (c *hostConn) subKind(id string) agent.Kind {
	if r := c.spawnRunFor(id); r != nil && r.kind != "" {
		return r.kind
	}
	return c.kindOf()
}

func oneLineUI(s string) string { return strings.Join(strings.Fields(s), " ") }
