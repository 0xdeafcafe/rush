package ui

import (
	"maps"
	"slices"
	"strings"
	"time"
	"unicode"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/keymap"
	"github.com/0xdeafcafe/rush/internal/netwatch"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// Plugins take part in the screen only through m.hooks, which never waits:
// see package hooks. Everything here reads the copy it holds, or hands it
// an event, or returns one of its tea.Cmds.

// hookSeen is what the last look at the fleet found, to tell what changed.
type hookSeen struct {
	states  map[string]string // each agent's state, and whether halted
	focus   string            // the Session in view
	offline bool
	input   string // the box being typed in, as it was
	inputAt string // whose box
	attach  uint64 // the connection it was told on
}

// startHooks connects this window to its plugins, in the background.
func (m *Model) startHooks() tea.Cmd {
	if m.hooks == nil {
		m.hooks = hooks.New()
	}
	m.hooks.Start()
	return tea.Batch(m.hooks.Next(), m.readBundled())
}

// readBundled reads which bundled plugins are on, off the UI.
func (m *Model) readBundled() tea.Cmd {
	return sheetDo(func() (map[string]bool, error) { return plugin.BundlesOn(), nil }, func(m *Model, on map[string]bool, _ error) tea.Cmd {
		m.bundledOn = on
		return nil
	})
}

// setBundled turns a bundled plugin on or off, off the UI, and has the
// broker start or stop it.
func (m *Model) setBundled(name string, on bool) tea.Cmd {
	return sheetDo(func() (map[string]bool, error) {
		if err := plugin.SetBundled(name, on); err != nil {
			return nil, err
		}
		return plugin.BundlesOn(), hooks.Reload()
	}, func(m *Model, st map[string]bool, err error) tea.Cmd {
		if st != nil {
			m.bundledOn = st
		}
		if d := m.dialog; d != nil && d.page == pagePlugins {
			d.pluginsRead = goPending(readPlugins)
		}
		if err != nil {
			m.flash(name+": "+err.Error(), true)
		}
		return nil
	})
}

// onHooks takes what the broker sent, and waits for the next.
func (m *Model) onHooks(msg tea.Msg) tea.Cmd {
	switch msg := msg.(type) {
	case hooks.StateMsg:
		// Commands came or went: the keymap has them.
		m.setKeys(m.keys.file)
		if n := m.hooks.Attaches(); n != m.hookSeen.attach {
			// A broker new to this window knows nothing of it: say again
			// what's here and which Session is open.
			m.hookSeen = hookSeen{attach: n}
			m.emitHooks()
		}
	case hooks.DoMsg:
		return tea.Batch(m.pluginDo(msg.UIDo), m.hooks.Next())
	}
	return m.hooks.Next()
}

// pluginDo does what a plugin asked, as far as rush lets it.
func (m *Model) pluginDo(d plugin.UIDo) tea.Cmd {
	switch d.Kind {
	case "notify":
		if d.Plugin == stashPlugin {
			// rush's own, in its words: no name before it.
			m.flash(m.stashSaid(plugin.CleanNotice(d.Text)), d.Tone == "bad")
			return nil
		}
		who := d.Plugin
		if d.Session != "" {
			if a := m.agentByKey(d.Session); a != nil {
				who += " · " + a.DisplayName
			}
		}
		m.flash(who+": "+plugin.CleanNotice(d.Text), d.Tone == "bad")
		return nil
	case "input.set":
		m.setBox(d)
		return nil
	case "pick":
		if d.Plugin == stashPlugin && d.Pick != nil {
			p := *d.Pick
			p.About = m.stashSaid(p.About)
			p.Empty = slices.Clone(p.Empty)
			for i := range p.Empty {
				p.Empty[i] = m.stashSaid(p.Empty[i])
			}
			d.Pick = &p
		}
		m.openPick(d.Plugin, d.Pick)
		return nil
	case "send":
		a := m.agentByKey(d.Session)
		switch {
		case a == nil, a.Interactive, a.Headless, a.Remote:
			return nil
		case m.skipsAsking(a):
			// Never on a plugin's word to one that acts without asking.
			m.flash(d.Plugin+" can't send to "+a.DisplayName+": it doesn't ask before acting", true)
			return nil
		case !a.Rush && (a.Live() || a.Busy()):
			return nil
		}
		cmd := m.replyTo(a, d.Text, d.Text)
		m.flash(d.Plugin+" → "+a.DisplayName+": "+plugin.CleanNotice(d.Text), false)
		return cmd
	}
	return nil
}

// skipsAsking is an agent running with permission to act without asking.
func (m *Model) skipsAsking(a *fleet.Agent) bool {
	mode := ""
	if c := m.host; c != nil && c.key == a.Key {
		mode = c.sess.Info.PermissionMode
	} else if a.Rush {
		mode = m.store.Config.Dispatch.Permission
	}
	return mode == "bypassPermissions" || mode == "auto"
}

// pluginActions are plugins' commands, for the keymap.
func (m *Model) pluginActions() []keymap.Action {
	if m.hooks == nil {
		return nil
	}
	var out []keymap.Action
	for _, p := range m.hooks.State().Plugins {
		if p.Name == stashPlugin {
			continue // rush's own stash and history keys run these
		}
		for _, c := range p.Commands {
			out = append(out, keymap.Action{ID: keymap.PluginID(p.Name, c.Name), Context: keymap.Any, Title: c.Description, Source: p.Name})
		}
	}
	return out
}

// pluginKeys are the keys plugins suggest for their commands.
func (m *Model) pluginKeys() map[string][]string {
	if m.hooks == nil {
		return nil
	}
	out := map[string][]string{}
	for _, p := range m.hooks.State().Plugins {
		for _, c := range p.Commands {
			if c.Key != "" {
				out[keymap.PluginID(p.Name, c.Name)] = []string{c.Key}
			}
		}
	}
	return out
}

// pluginHashCommands are plugins' commands as # commands: #haven.open.
func (m *Model) pluginHashCommands() []event.Command {
	if m.hooks == nil {
		return nil
	}
	var out []event.Command
	for _, p := range m.hooks.State().Plugins {
		if p.Name == stashPlugin {
			continue // #stash is rush's own
		}
		for _, c := range p.Commands {
			out = append(out, event.Command{Name: p.Name + "." + c.Name, Description: c.Description + " (" + p.Name + ")"})
		}
	}
	return out
}

// isPluginCommand is whether name is a plugin's command, as plugin.command.
func (m *Model) isPluginCommand(name string) bool {
	for _, c := range m.pluginHashCommands() {
		if c.Name == name {
			return true
		}
	}
	return false
}

// runPluginCommand runs name.command, as plugin:name.command, on a.
func (m *Model) runPluginCommand(id string, a *fleet.Agent) tea.Cmd {
	name, cmd, ok := strings.Cut(id, ".")
	if !ok || m.hooks == nil {
		return nil
	}
	m.flash(name+" "+cmd+"…", false)
	in, box, _ := m.boxState()
	return m.hooks.Command(name, cmd, m.uiSession(a), box, in, func(err error) tea.Msg {
		return sheetMsg{apply: func(m *Model) tea.Cmd {
			if err != nil {
				m.flash(name+" "+cmd+": "+err.Error(), true)
			}
			return nil
		}}
	})
}

// uiSession is what a plugin is told of an agent: what its row shows.
func (m *Model) uiSession(a *fleet.Agent) *plugin.UISession {
	if a == nil {
		return nil
	}
	return &plugin.UISession{ID: a.Key, SessionID: a.SessionID, Name: a.DisplayName, Agent: a.Kind,
		Cwd: a.Cwd, Repo: a.Repo, Branch: a.Branch, State: a.State, Hosted: a.Rush}
}

// haltKind sorts why an agent stopped, for a plugin.
func haltKind(a *fleet.Agent) *plugin.UIError {
	h := a.Spend.Halt
	if h == nil {
		return nil
	}
	e := &plugin.UIError{Kind: "other", Message: plugin.CleanNotice(h.Text)}
	switch {
	case h.Kind == "rate_limit":
		e.Kind = "limit"
	case h.Kind == "authentication_failed":
		e.Kind = "auth"
	case a.Offline():
		e.Kind = "offline"
	case a.Retryable():
		e.Kind = "retryable"
	}
	// rush continues its own sessions after these, and the rest for a day.
	e.Retrying = (e.Kind == "offline" || e.Kind == "retryable") && (a.Rush || a.Continues(time.Now()))
	return e
}

// emitHooks tells plugins what changed since it last looked: all from
// what's in memory, and Emit never waits.
func (m *Model) emitHooks() {
	h := m.hooks
	if h == nil || !h.Wants(plugin.EvTurnEnded) {
		return
	}
	s := &m.hookSeen
	if s.states == nil {
		// The first look is a baseline: nothing changed yet, but plugins
		// hear of each recent agent.
		s.states = map[string]string{}
		seen := 0
		for _, a := range m.snapAgents() {
			s.states[a.Key] = hookState(a)
			if seen < maxSeen && recent(a) {
				h.Emit(plugin.UIEvent{Kind: plugin.EvSessionSeen, Session: m.uiSession(a)})
				seen++
			}
		}
		s.offline = netwatch.Down()
		if s.offline {
			// A plugin just come can't know the network is already gone.
			h.Emit(plugin.UIEvent{Kind: plugin.EvNetworkDown})
		}
		return
	}
	for _, a := range m.snapAgents() {
		now, was := hookState(a), s.states[a.Key]
		s.states[a.Key] = now
		if was == "" && recent(a) {
			h.Emit(plugin.UIEvent{Kind: plugin.EvSessionSeen, Session: m.uiSession(a)})
		}
		if now != was && was != "" {
			m.emitChange(a, was, now)
		}
	}
	m.emitFocusAndNet()
}

// emitChange tells plugins an agent's state went from was to now.
func (m *Model) emitChange(a *fleet.Agent, was, now string) {
	h := m.hooks
	switch {
	case now == "halted":
		h.Emit(plugin.UIEvent{Kind: plugin.EvSessionStopped, Session: m.uiSession(a), Error: haltKind(a)})
	case now == "stopped":
		h.Emit(plugin.UIEvent{Kind: plugin.EvSessionStopped, Session: m.uiSession(a)})
	case now == "working" && was != "blocked":
		h.Emit(plugin.UIEvent{Kind: plugin.EvTurnStarted, Session: m.uiSession(a)})
	case was == "working" || was == "blocked":
		h.Emit(plugin.UIEvent{Kind: plugin.EvTurnEnded, Session: m.uiSession(a)})
	}
}

// emitFocusAndNet tells plugins the Session in view changed, or the
// network went or came back.
func (m *Model) emitFocusAndNet() {
	h, s := m.hooks, &m.hookSeen
	focus := ""
	if m.host != nil && (m.paneFocus || m.preview || m.full) {
		focus = m.host.key
	}
	if focus != s.focus {
		if a := m.agentByKey(s.focus); a != nil {
			h.Emit(plugin.UIEvent{Kind: plugin.EvSessionLeft, Session: m.uiSession(a)})
		}
		if a := m.agentByKey(focus); a != nil {
			h.Emit(plugin.UIEvent{Kind: plugin.EvSessionOpened, Session: m.uiSession(a)})
		}
		s.focus = focus
	}
	if off := netwatch.Down(); off != s.offline {
		s.offline = off
		k := plugin.EvNetworkUp
		if off {
			k = plugin.EvNetworkDown
		}
		h.Emit(plugin.UIEvent{Kind: k})
	}
}

// maxSeen is how many agents plugins hear of at once, well inside the
// queue events wait in.
const maxSeen = 150

// recent is an agent worth telling plugins of: open, or at work today.
func recent(a *fleet.Agent) bool {
	return !a.Past && !a.Done && (a.Live() || a.Busy() || time.Since(a.UpdatedAt) < 24*time.Hour)
}

// hookState is an agent's state as events tell it.
func hookState(a *fleet.Agent) string {
	if a.Halted() {
		return "halted"
	}
	if a.Busy() {
		return "working"
	}
	return a.State
}

// boxNow is the message box keys go to, and whose it is: a Session's by its
// agent, the Prompt's as "".
func (m *Model) boxNow() (string, string, bool) {
	if c := m.host; c != nil && m.paneFocus {
		return string(c.input), c.key, true
	}
	if m.mode == modeList && m.inKind == inPrompt && m.dialog == nil && m.sheet == nil {
		return string(m.input), "", true
	}
	return "", "", false
}

// emitInput tells plugins with "input" what's in the box, when it changed.
func (m *Model) emitInput() {
	if m.hooks == nil || !m.hooks.Wants(plugin.EvInputChanged) {
		return
	}
	text, who, ok := m.boxNow()
	if !ok || text == m.hookSeen.input && who == m.hookSeen.inputAt {
		return
	}
	// Sent or cleared said so themselves, and left input empty: an empty
	// box here was emptied by hand, and is said as one.
	m.hookSeen.input, m.hookSeen.inputAt = text, who
	b, _, _ := m.boxState()
	m.hooks.Emit(plugin.UIEvent{Kind: plugin.EvInputChanged, Session: m.uiSession(m.agentByKey(who)), Text: text, Box: b})
}

// boxState is the box keys go to, whole, and whose it is.
func (m *Model) boxState() (*plugin.Box, string, bool) {
	if _, who, ok := m.boxNow(); !ok {
		return nil, "", false
	} else if who != "" {
		c := m.host
		return &plugin.Box{Text: string(c.input), Cursor: max(0, len(c.input)-c.back), Pastes: maps.Clone(c.pastes.text), Images: maps.Clone(c.imgs.Path)}, who, true
	}
	return &plugin.Box{Text: string(m.input), Cursor: max(0, len(m.input)-m.back), Pastes: maps.Clone(m.pastes.text), Images: maps.Clone(m.imgs.Path)}, "", true
}

// setBox is a plugin setting a message box: a session's, when it's the one
// open, or the Prompt's, when it has the keys. With If, only a box still
// holding that text is set, so nothing typed meanwhile is lost.
func (m *Model) setBox(d plugin.UIDo) {
	b := d.Box
	if b == nil {
		b = &plugin.Box{Text: d.Text}
	}
	text := []rune(b.Text)
	back := len(text) - min(len(text), max(0, b.Cursor))
	if d.Box == nil {
		back = 0 // text alone: the cursor at its end, as before
	}
	if c := m.host; c != nil && c.key == d.Session && d.Session != "" {
		if d.If != nil && string(c.input) != *d.If {
			return
		}
		c.undo.save(c.input, c.back, false)
		c.input, c.back, c.anchor = text, back, 0
		c.pastes = pastes{text: maps.Clone(b.Pastes)}
		c.imgs = imageRefs{Path: maps.Clone(b.Images)}
		for n := range b.Pastes {
			c.pastes.n = max(c.pastes.n, n)
		}
		for n := range b.Images {
			c.imgs.N = max(c.imgs.N, n)
		}
		return
	}
	if d.Session != "" || m.inKind != inPrompt {
		return
	}
	if d.If != nil && string(m.input) != *d.If {
		return
	}
	m.input, m.back, m.anchor = text, back, 0
	m.pastes = pastes{text: maps.Clone(b.Pastes)}
	for n := range b.Pastes {
		m.pastes.n = max(m.pastes.n, n)
	}
	m.imgs = imageRefs{Path: maps.Clone(b.Images)}
	for n := range b.Images {
		m.imgs.N = max(m.imgs.N, n)
	}
}

// boxNote is what plugins put on the edge of a message box: a session's by
// its agent, the Prompt's by "".
func (m *Model) boxNote(key string) string {
	if m.hooks == nil {
		return ""
	}
	var parts []string
	for _, n := range m.hooks.State().Notes[key] {
		parts = append(parts, toned(firstNonEmpty(n.Tone, "dim"), n.Text))
	}
	return strings.Join(parts, dim(" · "))
}

// emitBox tells plugins with "input" that a box was sent or cleared.
func (m *Model) emitBox(kind, key, text string) {
	if m.hooks == nil || text == "" {
		return
	}
	m.hookSeen.input, m.hookSeen.inputAt = "", key
	m.hooks.Emit(plugin.UIEvent{Kind: kind, Session: m.uiSession(m.agentByKey(key)), Text: text})
}

// interceptedMsg is the plugins' say on a message about to go, from a
// Session's box (key) or, with prompt, the Prompt's. was is the box as it
// was asked about, req what was asked, and before the question r answers,
// if it answers one.
type interceptedMsg struct {
	key, was    string
	now, prompt bool
	req         plugin.Intercept
	before      *plugin.InterceptResult
	r           plugin.InterceptResult
}

// asksShown are the kinds of ask this window shows beside one with
// choices.
var asksShown = []string{plugin.AskInput}

// interceptSend asks plugins about what's in c's box before it goes, in
// the background; the box stays as it is until they answer.
func (m *Model) interceptSend(c *hostConn, now bool) tea.Cmd {
	c.intercepting = true
	req := plugin.Intercept{Hook: "before-send", Box: c.key, Session: m.uiSession(m.agentByKey(c.key)), Text: c.pastes.out(c.input, false), Asks: asksShown}
	at := interceptedMsg{key: c.key, was: string(c.input), now: now, req: req}
	return m.hooks.Intercept(req, func(r plugin.InterceptResult) tea.Msg { at.r = r; return at })
}

// interceptPrompt is interceptSend for the Prompt: a message to a, or with
// a nil a, one that starts a session.
func (m *Model) interceptPrompt(a *fleet.Agent) tea.Cmd {
	m.promptIntercepting = true
	req := plugin.Intercept{Hook: "before-send", Session: m.uiSession(a), Text: m.pastes.out(m.input, false), Asks: asksShown}
	at := interceptedMsg{was: string(m.input), prompt: true, req: req}
	return m.hooks.Intercept(req, func(r plugin.InterceptResult) tea.Msg { at.r = r; return at })
}

// wantsPromptIntercept is whether the Prompt's text goes past plugins
// before it's sent: a message, not a # or / command.
func (m *Model) wantsPromptIntercept(text string) bool {
	if m.hooks == nil || m.promptIntercepted || !m.hooks.Intercepts() {
		return false
	}
	t := strings.TrimSpace(text)
	return t != "" && !isHashCmd(t) && !strings.HasPrefix(t, "/")
}

// sendBox is a message box a plugin's say is about: a Session's or the
// Prompt's, how to mark it asked about, and how to send it once they've
// had their say.
type sendBox struct {
	input  *[]rune
	pastes *pastes
	asking *bool
	send   func() tea.Cmd
}

// interceptBox is the box msg is about, if it's still on the screen.
func (m *Model) interceptBox(msg interceptedMsg) (sendBox, bool) {
	if msg.prompt {
		return sendBox{&m.input, &m.pastes, &m.promptIntercepting, func() tea.Cmd {
			m.promptIntercepted = true
			defer func() { m.promptIntercepted = false }()
			return m.submit()
		}}, true
	}
	c := m.host
	if c == nil || c.key != msg.key {
		return sendBox{}, false // the Session was left meanwhile: the box went with it
	}
	return sendBox{&c.input, &c.pastes, &c.intercepting, func() tea.Cmd {
		c.intercepted = true
		defer func() { c.intercepted = false }()
		return m.sendPane(c, msg.now)
	}}, true
}

// rewrite puts r's message in the box: changed in place when its
// replacements and what it appends make the whole of it, so pastes stay
// chips; else as the text it is.
func (b sendBox) rewrite(r plugin.InterceptResult) {
	in := string(*b.input)
	ps := pastes{n: b.pastes.n, text: maps.Clone(b.pastes.text)}
	if ps.text == nil {
		ps.text = map[int]string{}
	}
	for _, p := range r.Replace {
		if p.Old == "" {
			continue
		}
		in = strings.ReplaceAll(in, p.Old, p.New)
		for k, v := range ps.text {
			ps.text[k] = strings.ReplaceAll(v, p.Old, p.New)
		}
	}
	in = strings.TrimRightFunc(in, unicode.IsSpace) + r.Append
	if len(r.Replace) > 0 || r.Append != "" {
		if ps.out([]rune(in), false) == r.Text {
			*b.input, *b.pastes = []rune(in), ps
			return
		}
	}
	*b.input, *b.pastes = []rune(r.Text), pastes{}
}

// onIntercepted sends the message, changed or not, says why it wasn't, or
// asks the question a plugin has about it.
func (m *Model) onIntercepted(msg interceptedMsg) tea.Cmd {
	b, ok := m.interceptBox(msg)
	if !ok {
		return nil
	}
	*b.asking = false
	if string(*b.input) != msg.was {
		m.flash("the message changed while plugins looked at it: send it again", true)
		return nil
	}
	r := msg.r
	if r.Plugin == "" && msg.before != nil {
		r.Plugin = msg.before.Plugin
	}
	switch r.Action {
	case "block":
		why := r.Reason
		if why == "" {
			why = "held back"
		}
		m.flash(r.Plugin+": "+plugin.CleanNotice(why), true)
		return nil
	case "ask":
		// What changed before the question is in the box while it's asked,
		// so a secret already dealt with is out of it, whatever you answer.
		if r.Text != "" && r.Text != b.pastes.out(*b.input, false) {
			b.rewrite(r)
			m.emitInput()
		}
		msg.was = string(*b.input)
		m.askFor(msg, b, r)
		return nil
	case "rewrite":
		b.rewrite(r)
		if c := m.host; !msg.prompt && c != nil {
			c.back = 0
		}
		m.emitInput()
		if r.Plugin != "" && msg.before == nil {
			m.flash(r.Plugin+" changed the message", false)
		}
	}
	return b.send()
}

// askFor shows a plugin's question about the message in b. A key that
// answers it goes back to the plugin with the message as it stands, and
// what the plugin then says is about the box as it is now; ctrl+c, or esc
// when no answer takes it, leaves the box as it is. A question with an
// input is a line to type, which enter hands back.
func (m *Model) askFor(msg interceptedMsg, b sendBox, r plugin.InterceptResult) {
	answer := func(key, value string) tea.Cmd {
		*b.asking = true
		req := msg.req
		req.Text = r.Text
		next := interceptedMsg{key: msg.key, was: msg.was, now: msg.now, prompt: msg.prompt, req: req, before: &r}
		return m.hooks.Answer(plugin.InterceptAnswer{Intercept: req, Plugin: r.Plugin, ID: r.ID, Key: key, Value: value},
			func(res plugin.InterceptResult) tea.Msg { next.r = res; return next })
	}
	q := &confirmation{question: r.Question, detail: r.Detail, only: true}
	if in := r.Input; in != nil {
		buf := []rune(in.Value)
		q.line = &confirmLine{buf: buf, pos: len(buf), err: in.Error, enterText: in.Enter,
			submit: func(v string) tea.Cmd { return answer(plugin.KeyEnter, v) }}
	}
	for _, ch := range r.Choices {
		key := ch.Key
		if ch.Enter {
			q.enterIs = key
		}
		if ch.Esc {
			q.escIs = key
		}
		q.more = append(q.more, confirmChoice{key: key, text: ch.Label, do: func() tea.Cmd { return answer(key, "") }})
	}
	m.confirm = q
}

// wantsIntercept is whether c's box goes past plugins before it's sent:
// a message, not a # or / command, and not an edit of one queued.
func (m *Model) wantsIntercept(c *hostConn, now bool) bool {
	if m.hooks == nil || c.intercepted || c.editQ > 0 || !m.hooks.Intercepts() {
		return false
	}
	t := strings.TrimSpace(string(c.input))
	if t == "" || isHashCmd(t) || strings.HasPrefix(t, "/") || strings.HasSuffix(string(c.input), "\\") && !now {
		return false
	}
	return true
}

// pluginOverview is what plugins add to a Session's overview.
func (m *Model) pluginOverview(key string) []convo.Line {
	if m.hooks == nil {
		return nil
	}
	secs := m.hooks.State().Sections[key]
	var out []convo.Line
	for _, s := range secs {
		out = append(out, convo.Line{Text: ""}, convo.Line{Text: "  " + paint(cSub, s.Title) + dim("  · "+s.Plugin)})
		for _, l := range s.Lines {
			out = append(out, convo.Line{Text: "    " + toned(l.Tone, l.Text)})
		}
	}
	return out
}

// pluginStatus is what plugins put on an agent's row, short.
func (m *Model) pluginStatus(key string) string {
	if m.hooks == nil {
		return ""
	}
	var parts []string
	for _, s := range m.hooks.State().Statuses[key] {
		parts = append(parts, toned(s.Tone, s.Text))
	}
	return strings.Join(parts, " ")
}

func toned(tone, s string) string {
	switch tone {
	case "dim":
		return dim(s)
	case "good":
		return paint(cGreen, s)
	case "warn":
		return paint(cYellow, s)
	case "bad":
		return paint(cRed, s)
	case "accent":
		return paint(cBlue, s)
	}
	return s
}

func (m *Model) snapAgents() []*fleet.Agent {
	if m.snap == nil {
		return nil
	}
	return m.snap.Agents
}
