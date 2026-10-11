package ui

import (
	"path/filepath"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// tailBytes is how much of a transcript's end a session opens on: a few
// turns, read in milliseconds. The rest is read after, in the background.
const tailBytes = 2 << 20

// wholeMsg brings a session's whole transcript, read after its end: a
// hosted one's as sess, with the first n of the host's lines kept since.
type wholeMsg struct {
	owner *hostConn
	key   string
	tail  *convo.Tail
	runs  agent.SubagentRuns // what the transcript says of its runs, read with it
	sess  *convo.Session
	n     int
}

// readWhole reads the whole transcript of a session opened on its end,
// and draws it once as warmed does. It gives up if the pane lets go first.
func (c *hostConn) readWhole(o convo.Options) tea.Cmd {
	key, path, closed, runs, exchangeID := c.key, c.tail.Path, &c.closed, c.newRuns(), c.exchangeID
	return func() tea.Msg {
		t := convo.NewTail(path)
		t.Stop = closed
		if runs != nil { // read once, for both
			t.Each = func(l []byte) { runs.Took(path, l) }
		}
		if _, err := t.Read(); err != nil || closed.Load() {
			return nil
		}
		if exchangeID != "" {
			t.Sess.RestoreExchanges(host.ReadExchanges(exchangeID))
		}
		warm(t.Sess, o)
		return wholeMsg{owner: c, key: key, tail: t, runs: runs}
	}
}

// subRuns are the runs as a follower of them takes them.
func subRuns(subs []convo.Subagent) []agent.SubagentRun {
	out := make([]agent.SubagentRun, len(subs))
	for i, sa := range subs {
		out[i] = agent.SubagentRun{ID: sa.ID, ToolUseID: sa.ToolUseID, Depth: max(1, sa.Depth), Type: sa.Type, Description: sa.Description, Path: sa.Path}
		if sa.Mod != 0 {
			out[i].Mod = time.Unix(0, sa.Mod)
		}
		if sa.Born != 0 {
			out[i].Born = time.Unix(0, sa.Born)
		}
	}
	return out
}

// warm draws a session read in the background once there, as warmed does.
func warm(s *convo.Session, o convo.Options) {
	s.Spawns()
	if o.Width > 0 {
		o.Now = time.Now()
		s.Tail(o, warmRows)
	}
}

func (m *Model) onWhole(msg wholeMsg) {
	c := m.host
	if c == nil || c.key != msg.key || !c.sess.Partial || msg.owner != nil && msg.owner != c {
		return
	}
	switch {
	case msg.sess != nil && c.keep:
		now := time.Now()
		for _, l := range c.since[msg.n:] {
			applyHostLine(msg.sess, l, &now)
		}
		c.since, c.keep = nil, false
		m.takeWhole(c, msg.sess)
	case msg.tail != nil && c.tail != nil:
		m.takeWhole(c, msg.tail.Sess)
		c.tail = msg.tail
		if msg.runs != nil && msg.tail.Path == c.path {
			c.subReader = msg.runs
		}
	}
}

// takeWhole puts the whole conversation where its end was. Refs name turns
// by number, and the end numbered its own from 1: what was opened, picked
// or scrolled to moves to the same turn, so nothing on screen jumps.
func (m *Model) takeWhole(c *hostConn, whole *convo.Session) {
	part := c.sess
	if c.sleeping {
		whole.Info = part.Info
	}
	shift := len(whole.Turns) - len(part.Turns)
	if n := len(part.Turns); n > 0 {
		last := part.Turns[n-1]
		for i := len(whole.Turns) - 1; i >= 0; i-- {
			if t := whole.Turns[i]; t.Start.Equal(last.Start) && t.Prompt == last.Prompt {
				shift = i - (n - 1)
				break
			}
		}
	}
	c.sel, c.top.ref = renumber(c.sel, shift), renumber(c.top.ref, shift)
	c.open, c.looks = renumbered(c.open, shift), renumbered(c.looks, shift)
	c.sess = whole
	c.paneKick = true // what was written while it was read
	// Agents rush hosted for it are looked for again in the whole of it:
	// the calls that started them may be further back than its end.
	c.hostedAt, c.hostNone = time.Time{}, nil
	m.followTail()
}

// renumber moves a ref of turn N ("t12", "t12:s:…") to turn N+shift.
func renumber(ref string, shift int) string {
	if shift == 0 || !strings.HasPrefix(ref, "t") {
		return ref
	}
	turn, rest, more := strings.Cut(ref, ":")
	n, err := strconv.Atoi(turn[1:])
	if err != nil {
		return ref
	}
	if more {
		rest = ":" + rest
	}
	return "t" + strconv.Itoa(n+shift) + rest
}

func renumbered[V any](m map[string]V, shift int) map[string]V {
	if shift == 0 || m == nil {
		return m
	}
	out := make(map[string]V, len(m))
	for k, v := range m {
		out[renumber(k, shift)] = v
	}
	return out
}

// agentHistoryTail is agentHistory read from only the end of the history
// when the adapter can, its session Partial when that left some out.
func agentHistoryTail(kind agent.Kind, s agent.Session) *convo.Session {
	a, _ := agent.Get(kind)
	if tr, ok := a.(agent.TailReader); ok {
		if evs, cut, err := tr.HistoryTail(s, tailBytes); err == nil {
			sess := convo.New()
			for _, ev := range evs {
				sess.Apply(ev, time.Time{})
			}
			sess.Partial = cut
			return sess
		}
	}
	return agentHistory(kind, s, time.Time{})
}

func agentHistoryTailBefore(kind agent.Kind, source agent.Session, before time.Time) *convo.Session {
	if reader, ok := agent.As[agent.HistoryTailBeforeReader](kind); ok {
		if events, partial, err := reader.HistoryTailBefore(source, tailBytes, before); err == nil {
			s := convo.New()
			for _, ev := range events {
				s.Apply(ev, time.Time{})
			}
			s.Partial = partial
			return s
		}
	}
	return agentHistory(kind, source, before)
}

// hostPre holds history from before the host's replay. Background stages
// prepare the recent history and replay; only their complete latest view is
// published. Older history can then be added without moving the viewport.
type hostPre struct {
	info       *host.Info
	path       string
	before     time.Time
	fork       string                // the transcript it forked from, read when its own has nothing yet
	other      func() *convo.Session // another agent's recent history, through its adapter
	otherWhole func() *convo.Session // full pre-replay history, read after the first frame
	at         int64                 // where its end was read from: -1 until it has been
}

// preFor is how a hosted session with this info and config reads what came
// before its replay, and the transcript it follows.
func preFor(info host.Info, infoErr error, cfg host.Config, cfgErr error, path string, acct agent.Profile) (*hostPre, string) {
	p := &hostPre{at: -1}
	if acct.Dir == "" && cfgErr == nil {
		// Live rows for other harnesses may only carry an account name.
		// Their saved host config owns the folder needed to find history.
		acct.Dir = cfg.Account.Dir
	}
	if infoErr == nil {
		p.info = &info
		if info.SessionID != "" && info.Cwd != "" {
			// The list may not have caught up with a rewind yet.
			if resolved := agent.TranscriptPath(agent.Kind(info.Kind), acct, info.Cwd, info.SessionID); resolved != "" {
				path = resolved
			}
		}
	}
	trimmed := infoErr == nil && !info.ReplayFrom.IsZero()
	resume := cfgErr == nil && (cfg.Resume || trimmed)
	if kind := agent.Kind(info.Kind); infoErr == nil && !agent.ReadsAsClaude(kind) {
		if resume {
			started := info.StartedAt
			if trimmed {
				started = info.ReplayFrom
			}
			s := agent.Session{ID: info.SessionID, Transcript: path, Profile: agent.Profile{Kind: kind, Dir: acct.Dir}}
			p.other = func() *convo.Session { return agentHistoryTailBefore(kind, s, started) }
			p.otherWhole = func() *convo.Session { return agentHistory(kind, s, started) }
		}
		return p, ""
	}
	if !resume {
		return p, path
	}
	p.path, p.before = path, time.Now()
	if infoErr == nil && !info.StartedAt.IsZero() {
		p.before = info.StartedAt
		for _, t := range []time.Time{info.RewoundAt, info.ReplayFrom} {
			if t.After(p.before) {
				p.before = t
			}
		}
	}
	if cfg.From != "" {
		p.fork = filepath.Join(filepath.Dir(path), cfg.From+".jsonl")
	}
	return p, path
}

// base is a session holding only the host's info, as a stage starts from.
func (p *hostPre) base() *convo.Session {
	s := convo.New()
	if p.info != nil {
		s.Apply(host.InfoEvent{Info: *p.info}, time.Now())
	}
	return s
}

// tail is the transcript's end before the replay, read from p.at, or
// from where its last tailBytes begin; p says where that was.
func (p *hostPre) tail() *convo.Session {
	if p.other != nil {
		return p.other()
	}
	if p.path == "" {
		return convo.New()
	}
	if p.at < 0 {
		p.at = convo.HistoryStart(p.path, p.before, tailBytes)
	}
	s := convo.HistoryFrom(p.path, p.before, p.at, nil)
	if len(s.Turns) == 0 && p.at == 0 && p.fork != "" {
		p.path, p.fork, p.at = p.fork, "", -1
		return p.tail()
	}
	return s
}

// preMsg brings a hosted session's transcript end, and where it was read.
type preMsg struct {
	owner *hostConn
	key   string
	sess  *convo.Session
	pre   hostPre
}

// readPre prepares recent history before replay, without exposing that older
// prefix as the current conversation or rendering it twice.
func (c *hostConn) readPre(o convo.Options) tea.Cmd {
	if c.pre == nil || c.pre.path == "" && c.pre.other == nil {
		return c.readReplay(o) // nothing before it, or another agent's, read whole there
	}
	key, pre := c.key, *c.pre
	return func() tea.Msg {
		s := pre.tail()
		if pre.info != nil {
			s.Apply(host.InfoEvent{Info: *pre.info}, time.Now())
		}
		return preMsg{owner: c, key: key, sess: s, pre: pre}
	}
}

func (m *Model) onPre(msg preMsg) tea.Cmd {
	c := m.host
	if c == nil || c.key != msg.key || c.client == nil || msg.owner != nil && msg.owner != c {
		return nil
	}
	*c.pre = msg.pre
	// Keep pre-replay history off screen: it ends before the latest messages.
	// Reuse this parsed session rather than reading and laying it out twice.
	return c.readReplayFrom(m.warmOpts(), msg.sess, nil)
}

// replayMsg brings a hosted session's transcript end with its host's
// replay taken in: whole says the replay came in whole; lines are its.
type replayMsg struct {
	key    string
	sess   *convo.Session
	whole  bool
	closed bool
	owner  *hostConn
	lines  [][]byte
}

// readReplay assembles history and replay off the UI thread. Only a complete
// replay is published, so the first visible conversation is its latest view.
func (c *hostConn) readReplay(o convo.Options) tea.Cmd {
	return c.readReplayFrom(o, nil, nil)
}

func (c *hostConn) readReplayFrom(o convo.Options, seed *convo.Session, kept [][]byte) tea.Cmd {
	key, lines := c.key, c.client.Lines
	var pre hostPre
	if c.pre != nil {
		pre = *c.pre
	}
	return func() tea.Msg {
		s := seed
		if s == nil {
			s = pre.base()
			if pre.at >= 0 || pre.other != nil {
				s = pre.tail()
			}
		}
		if pre.info != nil {
			s.Apply(host.InfoEvent{Info: *pre.info}, time.Now())
		}
		whole, closed := takeReplayState(lines, s, replayMost, &kept)
		if whole {
			warm(s, o)
		}
		return replayMsg{key: key, sess: s, whole: whole, closed: closed, owner: c, lines: kept}
	}
}

func (m *Model) onReplay(msg replayMsg) tea.Cmd {
	c := m.host
	if c == nil || c.key != msg.key || c.client == nil || msg.owner != nil && msg.owner != c {
		return nil
	}
	if msg.closed {
		m.flash("Connection closed while loading conversation", true)
		m.dropHost()
		return nil
	}
	if !msg.whole {
		// Continue off-thread without presenting an older prefix as the latest
		// conversation. The loading frame remains stable until the end marker.
		return c.readReplayFrom(m.warmOpts(), msg.sess, msg.lines)
	}
	c.sess, c.ready = msg.sess, true
	next := c.next()
	if !c.sess.Partial {
		return next
	}
	c.since, c.keep = msg.lines, true
	return tea.Batch(next, c.readWholeHost(m.warmOpts()))
}

// readWholeHost reads the whole of a hosted session's transcript before
// its replay, and the host's lines kept so far on top; onWhole adds those
// that came while it read.
func (c *hostConn) readWholeHost(o convo.Options) tea.Cmd {
	key, pre, since, closed := c.key, *c.pre, c.since, &c.closed
	return func() tea.Msg {
		var s *convo.Session
		if pre.otherWhole != nil {
			s = pre.otherWhole()
		} else {
			s = convo.HistoryFrom(pre.path, pre.before, 0, closed)
		}
		if closed.Load() {
			return nil
		}
		if pre.info != nil {
			s.Apply(host.InfoEvent{Info: *pre.info}, time.Now())
		}
		now := time.Now()
		for _, l := range since {
			applyHostLine(s, l, &now)
		}
		if pre.info != nil && pre.info.ID != "" {
			s.RestoreExchanges(host.ReadExchanges(pre.info.ID))
		}
		warm(s, o)
		return wholeMsg{owner: c, key: key, sess: s, n: len(since)}
	}
}
