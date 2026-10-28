package ui

import (
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// --- /rewind ---

// rewindSheet takes an agent back to before one of your messages, to try
// again with less in its context: down a rabbit hole, back out, down
// another. It's the same agent afterwards. The path it leaves is kept as
// a branch you can go back down, the code can stay as it is (the fix
// you got is kept, the context that got you there is not) or be put
// back, and a note of what was learned can come back with you.
type rewindSheet struct {
	agentKey, id, agent string
	acct                agent.Profile
	sid, cwd, path      string
	model               string

	turns    []convo.TurnStart // your messages, newest first
	branches []host.Branch     // paths left by earlier rewinds, newest first

	sel      int // into turns, then branches
	putBack  bool
	recap    bool
	previews map[string]*filesPreview // what putting the files back would do, by message
}

type filesPreview struct {
	loading bool
	r       agent.FileRewind
	err     error
}

// openRewindTo is alt+r on a turn in the history: the sheet, picked to go
// back to just after it, so it's the last thing the agent remembers.
func (m *Model) openRewindTo(c *hostConn, a *fleet.Agent, t *convo.Turn) tea.Cmd {
	var next *convo.Turn
	for _, u := range c.sess.Turns {
		if u.N > t.N && u.From == "" && (next == nil || u.N < next.N) {
			next = u
		}
	}
	if next == nil {
		m.flash("that's where "+a.DisplayName+" is now · pick an earlier turn, or /rewind", false)
		return nil
	}
	return m.openRewindAt(c, a, next.N, next.Prompt)
}

// openRewind reads where each of your messages starts in the conversation,
// then opens the /rewind sheet.
func (m *Model) openRewind(c *hostConn, a *fleet.Agent) tea.Cmd {
	return m.openRewindAt(c, a, 0, "")
}

// openRewindAt opens it with the message numbered n, whose text is prompt,
// picked (the transcript may number turns differently; the text decides).
func (m *Model) openRewindAt(c *hostConn, a *fleet.Agent, n int, prompt string) tea.Cmd {
	switch {
	case c.client == nil:
		m.flash("/rewind works in rush-mode sessions · /rush moves this one over, or /fork from an earlier turn", true)
		return nil
	case busy(a) || c.sess.Live() != nil:
		m.flash(a.DisplayName+" is working: stop it (esc) or let the turn end, then /rewind", true)
		return nil
	}
	f := &rewindSheet{
		agentKey: c.key, id: a.ID, agent: a.DisplayName, acct: a.Acct,
		sid:  firstNonEmpty(c.sess.Info.SessionID, a.SessionID),
		cwd:  firstNonEmpty(c.sess.Info.Cwd, a.Cwd),
		path: c.path, model: c.sess.Info.Model,
		previews: map[string]*filesPreview{},
	}
	type found struct {
		starts   []convo.TurnStart
		branches []host.Branch // newest first, those whose transcript is still there
	}
	path, id, acct, cwd := f.path, f.id, f.acct, f.cwd
	br, _ := agent.As[agent.Brancher](acct.Kind)
	return sheetDo(func() (found, error) {
		starts, err := convo.TurnStarts(path)
		if err != nil {
			return found{}, fmt.Errorf("couldn't read the conversation: %w", err)
		}
		cfg, _ := host.ReadConfig(id)
		v := found{starts: starts}
		for i := len(cfg.Branches) - 1; i >= 0; i-- {
			if b := cfg.Branches[i]; br != nil && statOK(br.TranscriptPath(acct, cwd, b.SessionID)) {
				v.branches = append(v.branches, b)
			}
		}
		return v, nil
	}, func(m *Model, v found, err error) tea.Cmd {
		if err != nil {
			m.fail(err.Error())
			return nil
		}
		for i := len(v.starts) - 1; i >= 0; i-- {
			if s := v.starts[i]; s.From == "" {
				f.turns = append(f.turns, s)
			}
		}
		f.branches = append(f.branches, v.branches...)
		if prompt != "" {
			gap := -1
			for i, t := range f.turns {
				if g := abs(t.N - n); t.Prompt == prompt && (gap < 0 || g < gap) {
					f.sel, gap = i, g
				}
			}
		}
		if len(f.turns) == 0 && len(f.branches) == 0 {
			m.flash("nothing to rewind yet: "+f.agent+" hasn't had a turn", true)
			return nil
		}
		m.sheet = f
		return nil
	})
}

func (f *rewindSheet) n() int { return len(f.turns) + len(f.branches) }

// turn is the message picked, or nil when a branch is.
func (f *rewindSheet) turn() *convo.TurnStart {
	if f.sel < len(f.turns) {
		return &f.turns[f.sel]
	}
	return nil
}

func (f *rewindSheet) branch() *host.Branch {
	if i := f.sel - len(f.turns); i >= 0 && i < len(f.branches) {
		return &f.branches[i]
	}
	return nil
}

// rewinder is the agent's Rewinder and Brancher: /rewind is offered only
// to agents that have both (FeatureRewind).
func (f *rewindSheet) rewinder() (agent.Rewinder, agent.Brancher, error) {
	rw, ok := agent.As[agent.Rewinder](f.acct.Kind)
	br, ok2 := agent.As[agent.Brancher](f.acct.Kind)
	if !ok || !ok2 {
		return nil, nil, errors.New(f.agent + " can't rewind")
	}
	return rw, br, nil
}

// preview asks, once per message, what putting the files back would do.
func (f *rewindSheet) preview() tea.Cmd {
	t := f.turn()
	if !f.putBack || t == nil || t.UUID == "" || f.previews[t.UUID] != nil {
		return nil
	}
	rw, _, err := f.rewinder()
	if err != nil {
		return nil
	}
	uuid, acct, cwd, sid, model := t.UUID, f.acct, f.cwd, f.sid, f.model
	p := &filesPreview{loading: true}
	f.previews[uuid] = p
	return sheetDo(func() (agent.FileRewind, error) {
		return rw.RewindFiles(acct, cwd, sid, model, uuid, true)
	}, func(_ *Model, r agent.FileRewind, err error) tea.Cmd {
		p.loading, p.r, p.err = false, r, err
		return nil
	})
}

func (f *rewindSheet) key(m *Model, k tea.KeyPressMsg, s string) tea.Cmd {
	switch s {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "up", "k", "shift+tab":
		f.sel = roundMove(f.sel, -1, f.n())
	case "down", "j", "tab":
		f.sel = roundMove(f.sel, 1, f.n())
	case "home", "g":
		f.sel = 0
	case "end", "G":
		f.sel = f.n() - 1
	case "c", "left", "right":
		if f.turn() != nil {
			f.putBack = !f.putBack
		}
	case "n", "space":
		if f.turn() != nil {
			f.recap = !f.recap
		}
	case "enter", "ctrl+s":
		return f.start(m)
	}
	return f.preview()
}

func (f *rewindSheet) body(m *Model, w, h int) []string {
	out := []string{
		sheetTitle("Rewind "+oneLine(f.agent), "go back to before one of your messages and try again with less in its context", w),
		"",
	}
	// What's under the list takes about 9 lines; the list gets the rest.
	room := max(3, h-len(out)-10)
	now := time.Now()
	row := func(i int, line string) {
		out = append(out, sheetRow(ansi.Truncate(line, w-2, "…"), i == f.sel, w))
	}
	// The list's items: the turns, then, when there are any, a heading and
	// the branches. Only the ones in view are drawn.
	items := len(f.turns)
	if len(f.branches) > 0 {
		items += 1 + len(f.branches)
	}
	draw := func(k int) {
		switch {
		case k < len(f.turns):
			t := f.turns[k]
			label := fmt.Sprintf("turn %d", t.N)
			if k == 0 {
				label += " · the last"
			}
			row(k, paint(cSub, fit(label, 20))+paint(cText, oneLine(t.Prompt)))
		case k == len(f.turns):
			out = append(out, "", dim("  or go back down a path you left"))
		default:
			j := k - len(f.turns) - 1
			b := f.branches[j]
			label := fmt.Sprintf("⑂ from turn %d", max(1, b.From))
			what := fmt.Sprintf("%d of your messages · left %s ago", b.Turns, age(now.Sub(b.Left)))
			if b.Last != "" {
				what += " · " + oneLine(b.Last)
			}
			row(len(f.turns)+j, paint(cSub, fit(label, 20))+paint(cText, what))
		}
	}
	// Keep the picked row in view: the list is short enough to window by
	// item, headings included.
	pos := f.sel
	if f.sel >= len(f.turns) {
		pos++ // the branches heading
	}
	from, to := window(items, pos, room)
	for k := from; k < to; k++ {
		draw(k)
	}
	if to < items {
		out = append(out, faint(fmt.Sprintf("  … %d more", items-to)))
	}
	out = append(out, "")

	if b := f.branch(); b != nil {
		out = append(out,
			dim(fmt.Sprintf("  Carries on down that path from where it was left: %d of your messages, apart from here since turn %d.", b.Turns, max(1, b.From))),
			dim("  This path is kept as a branch too. Files stay as they are now."),
			"", keys("↑↓", "pick", "enter", "switch", "esc", "cancel"))
		return out
	}
	t := f.turn()
	dropped := f.sel + 1
	if dropped == 1 {
		out = append(out, dim(fmt.Sprintf("  It forgets turn %d. ", t.N))+dim("Your message goes back in the box to change and send again."))
	} else {
		out = append(out, dim(fmt.Sprintf("  It forgets turns %d–%d (%d of your messages). ", t.N, f.turns[0].N, dropped))+dim("Your message goes back in the box."))
	}
	out = append(out, dim("  The path you leave is kept: /rewind again to go back down it."), "")

	opt := func(key, label, val string) {
		out = append(out, "  "+paint(cOrange, key)+" "+paint(cSub, fit(label, 11))+val)
	}
	if f.putBack {
		val := paint(cText, fmt.Sprintf("put back as they were before turn %d", t.N))
		if p := f.previews[t.UUID]; t.UUID == "" {
			val = paint(cRed, "can't: this message has no checkpoint")
		} else if p != nil {
			switch {
			case p.loading:
				val += faint(" · checking…")
			case p.err != nil || !p.r.CanRewind:
				why := firstNonEmpty(p.r.Error, errText(p.err), "no checkpoint")
				val = paint(cRed, "can't: "+oneLine(why))
			case len(p.r.Files) == 0:
				val += faint(" · nothing to change")
			default:
				val += faint(fmt.Sprintf(" · %d file%s ", len(p.r.Files), plural(len(p.r.Files)))) +
					paint(cGreen, fmt.Sprintf("+%d", p.r.Insertions)) + " " + paint(cRed, fmt.Sprintf("−%d", p.r.Deletions))
			}
		}
		opt("c", "Code", val)
		if p := f.previews[t.UUID]; p != nil && !p.loading && p.err == nil && len(p.r.Files) > 0 {
			var names []string
			for _, fl := range p.r.Files {
				names = append(names, tildify(fl))
			}
			out = append(out, faint("               "+ansi.Truncate(strings.Join(names, ", "), w-17, "…")))
		}
	} else {
		opt("c", "Code", paint(cText, "keep it as it is now")+faint(" · only the conversation goes back"))
	}
	if f.recap {
		opt("n", "Bring back", paint(cText, "a note of what was learned")+faint(" · Claude writes it first, one extra turn"))
	} else {
		opt("n", "Bring back", paint(cText, "nothing")+faint(" · or a note of what was tried and found"))
	}
	return append(out, "", keys("↑↓", "pick", "c", "code", "n", "note", "enter", "rewind", "esc", "cancel"))
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func plural(n int) string {
	if n == 1 {
		return ""
	}
	return "s"
}

type rewoundMsg struct {
	key, draft, text string
}

// start rewinds: a note of the path being left first (while it's all
// there), then the files, then a copy of the conversation cut before the
// picked message, which the agent's host switches to.
func (f *rewindSheet) start(m *Model) tea.Cmd {
	t, b := f.turn(), f.branch()
	if t != nil && f.putBack {
		if p := f.previews[t.UUID]; t.UUID == "" || p != nil && !p.loading && (p.err != nil || !p.r.CanRewind) {
			m.flash("can't put the code back from there · c keeps it as it is", true)
			return nil
		}
	}
	// The path being left, as a branch to come back to.
	left := host.Branch{Turns: len(f.turns)}
	if len(f.turns) > 0 {
		left.Last = f.turns[0].Prompt
	}
	m.sheet = nil
	key, id, agent := f.agentKey, f.id, f.agent
	if b != nil {
		left.From = b.From
		target := b.SessionID
		m.flash("switching "+agent+" to the path from turn "+fmt.Sprint(max(1, b.From))+"…", false)
		return func() tea.Msg {
			restarted, err := rewindHost(id, target, true, left)
			if err != nil {
				return doneMsg{err: err}
			}
			return rewoundMsg{key: key, text: "back down the path from turn " + fmt.Sprint(max(1, b.From)) + " · the one you left is kept" + restartNote(restarted)}
		}
	}
	turn, putBack, recap := *t, f.putBack, f.recap
	left.From = turn.N
	rw, br, err := f.rewinder()
	if err != nil {
		m.fail(err.Error())
		return nil
	}
	src, sid, acct, cwd, model := f.path, f.sid, f.acct, f.cwd, f.model
	if recap {
		m.flash("asking "+agent+" what it learned, then rewinding…", false)
	} else {
		m.flash("rewinding "+agent+"…", false)
	}
	return func() tea.Msg {
		draft := turn.Prompt
		if recap {
			note, err := rw.Recap(acct, cwd, sid, model, turn.Prompt)
			if err != nil {
				return doneMsg{err: err}
			}
			draft = "I tried this before and rewound. What we learned:\n\n" + note + "\n\n" + turn.Prompt
		}
		if putBack {
			if _, err := rw.RewindFiles(acct, cwd, sid, model, turn.UUID, false); err != nil {
				return doneMsg{err: fmt.Errorf("couldn't put the code back: %w", err)}
			}
		}
		// Before the first message there's nothing to resume: a fresh one.
		newID, _ := host.NewSessionID()
		resume := turn.Offset > 0 && turn.N > 1
		if resume {
			// With its file checkpoints, so the copy can put files back to
			// its earlier turns too.
			if err := br.Branch(acct, src, cwd, sid, newID, turn.Offset); err != nil {
				return doneMsg{err: fmt.Errorf("couldn't copy the conversation: %w", err)}
			}
		}
		restarted, err := rewindHost(id, newID, resume, left)
		if err != nil {
			return doneMsg{err: err}
		}
		text := fmt.Sprintf("rewound to before turn %d · the path you left is kept (/rewind)", turn.N)
		if putBack {
			text += " · code put back"
		}
		text += restartNote(restarted)
		return rewoundMsg{key: key, draft: draft, text: text}
	}
}

// clearInPlace is /clear: the agent carries on in a fresh conversation,
// named by its next message, and the one it leaves is kept as a path to
// go back down with /rewind.
func (m *Model) clearInPlace(c *hostConn, a *fleet.Agent) tea.Cmd {
	left := leftOf(c)
	key, id := c.key, a.ID
	m.flash("clearing "+a.DisplayName+"…", false)
	return func() tea.Msg {
		newID, _ := host.NewSessionID()
		restarted, err := rewindHost(id, newID, false, left)
		if err != nil {
			return doneMsg{err: err}
		}
		return rewoundMsg{key: key, text: "cleared · the conversation it had is kept (/rewind)" + restartNote(restarted)}
	}
}

// rewindHost tells an agent's host to carry on from sessionID. The host
// hangs up once it has, or says why it won't. A host from an older rush
// doesn't know how, so it's restarted on this one, already rewound.
func rewindHost(id, sessionID string, resume bool, left host.Branch) (restarted bool, err error) {
	if info, err := host.ReadInfo(id); err == nil && info.Proto < 1 {
		return true, host.RewindByRestart(id, sessionID, resume, left)
	}
	return false, rewindLive(id, sessionID, resume, left)
}

// leftOf is the conversation c leaves, as a path for /rewind.
func leftOf(c *hostConn) host.Branch {
	left := host.Branch{From: 1}
	for _, t := range c.sess.Turns {
		if t.Prompt != "" {
			left.Turns++
			left.Last = t.Prompt
		}
	}
	return left
}

func rewindLive(id, sessionID string, resume bool, left host.Branch) error {
	return hangUp(id, func(c *host.Client) error { return c.Rewind(sessionID, resume, left) })
}

// hangUp asks agent id's host to do something it hangs up after, and
// waits until it has, or says why it won't.
func hangUp(id string, do func(*host.Client) error) error {
	c, err := host.Dial(id)
	if err != nil {
		return err
	}
	defer c.Close()
	if err := do(c); err != nil {
		return err
	}
	timeout := time.After(15 * time.Second)
	for {
		select {
		case l, ok := <-c.Lines:
			if !ok {
				return nil
			}
			if ev, _ := host.Decode(l); ev != nil {
				if e, ok := ev.(host.ErrorEvent); ok {
					return errors.New(e.Error)
				}
			}
		case <-timeout:
			return errors.New("the agent's host didn't answer the rewind")
		}
	}
}

func restartNote(restarted bool) string {
	if restarted {
		return " · its host was from an older rush, so it restarted on this one"
	}
	return ""
}

// onRewound puts the message back in the box once the pane reconnects.
func (m *Model) onRewound(msg rewoundMsg) tea.Cmd {
	m.flash(msg.text, false)
	if msg.draft != "" {
		if m.rewound == nil {
			m.rewound = map[string]string{}
		}
		m.rewound[msg.key] = msg.draft
	}
	if m.host != nil && m.host.key == msg.key {
		// Let go now rather than wait to notice the host did.
		m.dropHost()
	}
	return nil
}

// statOK is whether path is there.
func statOK(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}
