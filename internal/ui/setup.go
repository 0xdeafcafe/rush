package ui

import (
	"cmp"
	"fmt"
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A setup is what a session runs as: whose models and how they're paid
// for, the harness that runs them, the account, the model and the
// effort. rush shows one as Provider (Harness) · account (OpenAI (Codex) · alex),
// and /agent takes it as <provider>-<harness>:<account> (openai-codex:alex),
// an effort after it if you like (openai-codex:alex:high).

// harnessWord is the harness agent k runs in, as a setup's name starts:
// claudecode, codex, pi.
func harnessWord(k agent.Kind) string {
	h := agent.HarnessOf(k)
	if h == loginsKind {
		return "claudecode" // its program's name, not its models'
	}
	return string(h)
}

// setupWord is agent k as one word to type, as agent.Label says it:
// openai-codex, ollama-pi, opencode.
func setupWord(k agent.Kind) string {
	p, h := nameWord(agent.ProviderLabel(agent.ProviderOf(k))), harnessWord(k)
	if p == h {
		return p
	}
	return p + "-" + h
}

// setupName is what /agent calls a setup: <provider>-<harness>, then
// :<account>, or :key for its provider's API key; openai-codex:alex.
func setupName(k agent.Kind, billing, account string) string {
	w := setupWord(k)
	switch {
	case billing == state.BillingKey || agent.KeyOnly(k):
		return w + ":key"
	case account != "" && agent.HarnessOf(k) == k:
		return w + ":" + nameWord(account)
	}
	return w
}

// setupLabel is a setup as it's shown: OpenAI (Codex) · alex, Anthropic
// (Pi) · API key.
func setupLabel(k agent.Kind, billing, account string) string {
	switch {
	case billing == state.BillingKey || agent.KeyOnly(k):
		return agent.Label(k) + " · API key"
	case account != "" && agent.HarnessOf(k) == k:
		return agent.Label(k) + " · " + account
	}
	return agent.Label(k)
}

// oldSetupName is a setup's name before provider words: <harness>:<account>,
// pi:ollama. /agent still takes it.
func oldSetupName(k agent.Kind, billing, account string) string {
	w, p := harnessWord(k), agent.ProviderOf(k)
	switch {
	case agent.HarnessOf(k) != k:
		if agent.KeyOnly(k) && agent.Split(p) {
			p = agent.KeyOf(p)
		}
		return w + ":" + p
	case billing == state.BillingKey:
		return w + ":key"
	case account != "":
		return w + ":" + nameWord(account)
	}
	return w
}

// nameWord is a name as one word, to type: "Alex Work" is alex-work.
func nameWord(s string) string { return strings.ToLower(strings.Join(strings.Fields(s), "-")) }

// setup is one way a session can run here: its name, the other names
// /agent takes for it (claude:alex for claudecode:alex), and the start.
type setup struct {
	name  string
	alias []string
	o     startOver
}

// setups are every way a session can run here: each harness on the
// account in use, on each of its accounts and on its provider's key, and
// each provider in another's harness.
func (m *Model) setups() []setup {
	var out []setup
	rows := m.accountRows()
	for _, r := range m.startRoutes() {
		p, key := agent.Billed(r.id)
		for _, k := range r.kinds {
			kk := agent.Kind(k)
			o := m.startDefaults(k)
			o.billing = billingOf(r.id, kk)
			if key || agent.HarnessOf(kk) != kk {
				out = append(out, setup{setupName(kk, o.billing, ""), []string{r.id, k, oldSetupName(kk, o.billing, "")}, o})
				continue
			}
			out = append(out, setup{setupName(kk, "", ""), []string{p, oldSetupName(kk, "", "")}, o})
			for _, a := range accountsOf(rows, kk) {
				o.account = a.name()
				out = append(out, setup{setupName(kk, "", o.account), []string{p + ":" + nameWord(o.account), oldSetupName(kk, "", o.account)}, o})
			}
		}
	}
	return out
}

// pickSetup is /agent's argument as one of ss: a setup's name, else
// another name for one, then an effort of efforts(kind) if you like.
func pickSetup(ss []setup, arg string, efforts func(kind string) []agent.Choice) (startOver, error) {
	arg = strings.ToLower(strings.TrimSpace(arg))
	find := func(name string) (startOver, bool) {
		for _, s := range ss {
			if s.name == name {
				return s.o, true
			}
		}
		for _, s := range ss {
			if slices.Contains(s.alias, name) {
				return s.o, true
			}
		}
		return startOver{}, false
	}
	if o, ok := find(arg); ok {
		return o, nil
	}
	i := strings.LastIndex(arg, ":")
	o, ok := startOver{}, false
	if i > 0 {
		o, ok = find(arg[:i])
	}
	if !ok {
		return startOver{}, fmt.Errorf("nothing here called %s · #agent <harness>:<account>[:<effort>]", arg)
	}
	name, effort := arg[:i], arg[i+1:]
	var ids []string
	for _, e := range efforts(o.kind) {
		ids = append(ids, e.ID)
	}
	switch {
	case len(ids) == 0:
		return startOver{}, fmt.Errorf("%s takes no effort", name)
	case !slices.Contains(ids, effort):
		return startOver{}, fmt.Errorf("%s takes effort %s", name, strings.Join(ids, ", "))
	}
	o.effort = effort
	return o, nil
}

// effortsOf are the efforts agent kind starts with.
func effortsOf(kind string) []agent.Choice {
	ch, _ := agent.ChoicesOf(agent.Kind(kind))
	return ch.Efforts
}

// setupArgs are the completions for #agent's or #profile's argument in
// text, now's marked; nil for any other text.
func (m *Model) setupArgs(text string, back int, now func() startOver) []event.Command {
	name, q, ok := strings.Cut(strings.TrimPrefix(text, "#"), " ")
	if back != 0 || !ok || !strings.HasPrefix(text, "#") || strings.ContainsAny(q, " \n") {
		return nil
	}
	q = strings.ToLower(q)
	var first, rest []event.Command
	add := func(arg, note string, now bool) {
		c := event.Command{Name: name + " " + arg, Description: note}
		if now {
			c.Description = strings.TrimPrefix(c.Description+" · now", " · ")
		}
		switch {
		case strings.HasPrefix(strings.ToLower(arg), q):
			first = append(first, c)
		case strings.Contains(strings.ToLower(arg), q):
			rest = append(rest, c)
		}
	}
	switch name {
	case "agent":
		cur := m.startName(now())
		for _, s := range m.setups() {
			add(s.name, m.setupNote(s.o), s.name == cur)
			if strings.HasPrefix(q, s.name+":") {
				for _, e := range effortsOf(s.o.kind) {
					add(s.name+":"+e.ID, e.ID+" effort · "+e.Note, false)
				}
			}
		}
	case "profile":
		cur := now().profile
		for _, p := range m.store.Config.Profiles {
			add(p.Name, m.profileWords(p), p.Name == cur)
		}
	default:
		return nil
	}
	return append(first, rest...)
}

// setupNote says whose models a setup runs, and how they're paid for:
// OpenAI · subscription, Ollama in Pi.
func (m *Model) setupNote(o startOver) string {
	return provLabel(startID(o)) + runsInWords(agent.Kind(o.kind))
}

// startName is what o is called: the profile of yours it starts under,
// else its setup, on the account its provider is signed in as when it
// names none.
func (m *Model) startName(o startOver) string {
	if o.profile != "" {
		return o.profile
	}
	k := agent.Kind(o.kind)
	return setupLabel(k, o.billing, cmp.Or(o.account, m.accountOf(k)))
}

// startWords are o in words: its name, model and effort.
func (m *Model) startWords(o startOver) []string {
	model := "default model"
	if o.model != "" {
		model = modelWord(o.kind, o.model)
	}
	words := []string{m.startName(o), model}
	if o.effort != "" {
		words = append(words, o.effort+" effort")
	}
	return words
}

// setupChip is o as a chip: its provider's glyph, its name, model and
// effort.
func (m *Model) setupChip(o startOver) string {
	l := lookOf(agent.Kind(o.kind))
	w := m.startWords(o)
	s := paint(l.colour(), l.glyph) + " " + paint(cText+bold, w[0]) + dim(" · "+strings.Join(w[1:], " · "))
	return barChip + " " + strings.ReplaceAll(s, reset, reset+barChip) + " " + reset
}

// accountNames are the accounts agent k can start on, by name.
func (m *Model) accountNames(k agent.Kind) []string {
	var out []string
	for _, r := range accountsOf(m.accountRows(), k) {
		out = append(out, r.name())
	}
	return out
}

// accountName is the name of agent k's account id, or "" when rush keeps
// no such account.
func (m *Model) accountName(k agent.Kind, id string) string {
	if id == "" {
		return ""
	}
	for _, r := range accountsOf(m.accountRows(), k) {
		if r.id() == id {
			return r.name()
		}
	}
	return ""
}

// accountSwitch signs agent k's provider in as the account called name,
// for every session of it, unless it already is.
func (m *Model) accountSwitch(k agent.Kind, name string) tea.Cmd {
	if name == "" {
		return nil
	}
	for _, r := range accountsOf(m.accountRows(), k) {
		if r.current || nameWord(r.name()) != nameWord(name) {
			continue
		}
		if r.login == nil {
			return m.switchAccount(r.acct, "")
		}
		if i := m.loginIndex(r.login.ID); i >= 0 {
			return m.switchLogin(m.store.Config.Logins[i], "")
		}
	}
	return nil
}

// sessionStart is what session c runs as now, as a start.
func (m *Model) sessionStart(c *hostConn) startOver {
	k := sessionAgent(c)
	model, known := argNow(c, "model", argOptions(c, "model"))
	if !known {
		model = argRunning(c, "model")
	}
	effort, picked := c.picked["effort"]
	if !picked {
		effort = c.sess.Effort()
	}
	o := startOver{kind: string(k), account: m.accountOf(k), model: model, effort: effort, mode: c.sess.Info.PermissionMode, modeSet: true}
	if agent.KeyOnly(k) || c.sess.Info.Billing == string(usage.Metered) {
		o.billing, o.account = state.BillingKey, ""
	}
	if a := m.agentByKey(c.key); a != nil {
		if p := m.sessionProfile(a); !p.Builtin {
			o.profile = p.Name
		}
	}
	return o
}

// inPlace is whether a session running as from can switch to to without
// a new session: the same agent, paid the same way.
func inPlace(from, to startOver) bool {
	return from.kind == to.kind && from.billing == to.billing &&
		(agent.ProviderOf(agent.Kind(to.kind)) != "ollama" || from.model == to.model)
}

// switchSession switches session c to o: its model, effort and account in
// place when it stays on the same agent, paid the same way; else its
// conversation is handed on to a new session that starts as o.
func (m *Model) switchSession(c *hostConn, o startOver) tea.Cmd {
	return m.switchSessionMessage(c, o, "")
}

func (m *Model) switchSessionMessage(c *hostConn, o startOver, message string) tea.Cmd {
	a := m.agentByKey(c.key)
	if a == nil {
		return nil
	}
	from := m.sessionStart(c)
	if !inPlace(from, o) {
		if !agent.Supports(agent.Kind(o.kind), agent.FeatureHandoffIn) {
			m.flash(m.startName(o)+" can't take a conversation on from another", true)
			return nil
		}
		if a.Rush && c.sess != nil && !handoffQuiet(c.sess.Info) {
			m.flash("switch harness between turns · finish or stop this turn and send or clear queued messages first", true)
			draft := string(c.input)
			if draft == "" {
				draft = message
			}
			if draft != "" {
				return func() tea.Msg {
					return applyMsg(func(*Model) tea.Cmd {
						if len(c.input) == 0 {
							c.input = []rune(draft)
							c.back = 0
						}
						return nil
					})
				}
			}
			return nil
		}
		return m.handOverMessage(a, o, message)
	}
	if c.client == nil && c.sleeping {
		return m.wakeHostThen(c, func(m *Model, next *hostConn) tea.Cmd { return m.switchSessionMessage(next, o, message) })
	}
	if c.client == nil {
		m.flash("switching works in rush-mode sessions · /rush moves this one over", true)
		return nil
	}
	var cmds []tea.Cmd
	var said []string
	if o.modeSet && o.mode != from.mode && !permissionKnown(sessionAgent(c), c, o.mode) {
		m.flash("This harness does not offer permission mode "+o.mode, true)
		return nil
	}
	if o.modeSet && o.mode != from.mode {
		cmds = append(cmds, m.setPermission(c, o.mode))
		said = append(said, "permission change requested")
	}
	if o.model != from.model {
		cmds = append(cmds, m.setArg(c, "model", o.model))
		said = append(said, cmp.Or(modelWord(o.kind, o.model), "its default model"))
	}
	if o.effort != from.effort {
		cmds = append(cmds, m.setArg(c, "effort", o.effort))
		said = append(said, cmp.Or(o.effort, "its default")+" effort from its next start")
	}
	if cmd := m.accountSwitch(agent.Kind(o.kind), o.account); cmd != nil {
		cmds = append(cmds, cmd)
		said = append(said, o.account+" for every "+agent.HarnessLabel(agent.Kind(o.kind))+" session, this one's too")
	}
	if message != "" {
		// Sequence preserves model/account changes before sending the instruction.
		cmds = append(cmds, hostCmd(func() error { return c.client.Send(message) }))
	}
	if len(said) == 0 {
		m.flash("already "+m.startName(o), false)
		return tea.Sequence(cmds...)
	}
	m.flash(m.startName(o)+" · "+strings.Join(said, " · "), false)
	return tea.Sequence(cmds...)
}

// handOver carries a's conversation on in a new session that starts as
// o, in the same folder. An agent that takes it whole (FeaturePort) has
// it as its own history and waits for your next message, and a rush
// session a stops and goes to Done: a swap. Else it starts on a summary
// of it, and a is left as it is.
func (m *Model) handOver(a *fleet.Agent, o startOver) tea.Cmd {
	return m.handOverMessage(a, o, "")
}

func (m *Model) handOverMessage(a *fleet.Agent, o startOver, message string) tea.Cmd {
	whole := agent.Supports(agent.Kind(o.kind), agent.FeaturePort)
	cfg := m.configAs(o, a.Cwd)
	cfg.Name, cfg.Profile = a.DisplayName+" · on "+m.startName(o), o.profile
	if whole {
		cfg.Name = a.DisplayName
	}
	if err := cfg.UseAgent(o.kind); err != nil {
		m.flash(err.Error(), true)
		return nil
	}
	conv := m.conversationLater(a)
	retire, oldID := "", a.ID
	if whole && a.Rush {
		retire = a.Key
	}
	m.flash("handing "+a.DisplayName+" to "+m.startName(o)+"…", false)
	return tea.Sequence(m.accountSwitch(agent.Kind(o.kind), o.account), func() tea.Msg {
		var sourceStamp time.Time
		if retire != "" {
			info, err := host.ReadInfo(oldID)
			if err != nil {
				return doneMsg{err: err}
			}
			if !handoffQuiet(info) {
				return doneMsg{err: fmt.Errorf("source session is busy or has queued messages; switch after it finishes")}
			}
			sourceStamp = info.UpdatedAt
		}
		c := conv()
		if whole {
			cfg.Carry = c.History
			if len(cfg.Carry) == 0 {
				// There is no history to confirm; keep the empty source available.
				retire = ""
			}
			cfg.SystemPrompt = strings.TrimSpace(cfg.SystemPrompt + "\n\n" + agent.Carried(c))
		} else {
			in := agent.Handoff(c)
			cfg.Prompt, cfg.Images = in.Text, in.Images
		}
		if message != "" {
			if whole {
				cfg.Prompt = message
			} else {
				cfg.Prompt += "\n\nThe user's next instruction:\n" + message
			}
		}
		hc, err := host.Spawn(cfg)
		if err != nil {
			return doneMsg{err: err}
		}
		if retire != "" {
			if err := waitCarried(hc.ID); err != nil {
				return doneMsg{err: fmt.Errorf("destination %s did not confirm the handoff; source kept available: %w", hc.ID, err)}
			}
			// Another client may have sent the source work while the new
			// harness was starting. Leave it available when that happened.
			info, err := host.ReadInfo(oldID)
			if err != nil || !handoffQuiet(info) || !info.UpdatedAt.Equal(sourceStamp) {
				retire = ""
			} else if old, err := host.Dial(oldID); err == nil {
				if err := old.Stop(); err != nil {
					retire = ""
				}
				old.Close()
			} else {
				retire = ""
			}
		}
		return hostStartedMsg{id: hc.ID, name: cfg.Name, retire: retire}
	})
}

// useSetup is #agent and #profile with arg: in the Prompt (c nil), what
// the next session starts as, started at once with msg when there is
// one; in session c, a switch of it.
func (m *Model) useSetup(c *hostConn, name, arg, msg string) tea.Cmd {
	if arg == "" {
		switch {
		case c != nil:
			return m.openSwitchSheet(c)
		case name == "agent":
			return m.openComposer()
		}
		m.usePickedProfile("")
		return nil
	}
	var o startOver
	if name == "profile" {
		p, ok := m.store.Config.ProfileNamed(arg)
		inst := p.Installed()
		switch {
		case !ok:
			m.flash("no profile named "+arg, true)
			return nil
		case len(inst) == 0:
			m.flash("none of "+p.Name+"'s providers runs here", true)
			return nil
		case c == nil:
			// The Prompt goes under it as #profile does, its folder's
			// profile and the room its accounts have deciding the rest.
			m.startOver = nil
			m.usePickedProfile(arg)
			if msg != "" {
				return m.startHosted(msg, m.startDir())
			}
			return nil
		}
		o = m.profileSetup(p, inst[0])
	} else {
		var err error
		if o, err = pickSetup(m.setups(), arg, effortsOf); err != nil {
			m.flash(err.Error(), true)
			return nil
		}
	}
	if c != nil {
		return m.switchSessionMessage(c, o, msg)
	}
	m.startOver = &o
	if msg != "" {
		return m.startHosted(msg, m.startDir())
	}
	m.flash("the next session starts as "+m.startWith(m.startDir(), true), false)
	return nil
}

// setupCommand is whether text is #agent or #profile, and if so runs it:
// its first word after the name is the setup, the rest a message, taken
// from tagged, as a session is sent it. Typed the old way, /agent, it
// still runs, and says what it's called now.
func (m *Model) setupCommand(c *hostConn, text, tagged string) (tea.Cmd, bool) {
	f := strings.Fields(text)
	if len(f) == 0 || !slices.Contains([]string{"#agent", "#profile", "/agent", "/profile"}, strings.ToLower(f[0])) {
		return nil, false
	}
	arg, msg := "", ""
	if len(f) > 1 {
		arg = f[1]
		msg = strings.TrimSpace(afterWords(tagged, 2))
	}
	name, at := strings.ToLower(f[0][1:]), m.statusAt
	cmd := m.useSetup(c, name, arg, msg)
	if f[0][0] == '/' && m.statusAt == at {
		m.flash("rush's own commands start with #: #"+name, false)
	}
	return cmd, true
}

// afterWords is s after its first n words.
func afterWords(s string, n int) string {
	for range n {
		s = strings.TrimLeft(s, " \t\n")
		i := strings.IndexAny(s, " \t\n")
		if i < 0 {
			return ""
		}
		s = s[i:]
	}
	return s
}

// clickLabel is whether x, y is on the agent label in session c's header,
// which opens the sheet to switch it.
func (m *Model) clickLabel(c *hostConn, x, y int) bool {
	lx := m.paneX() + c.label[0]
	return !m.zenFull() && c.label[1] > 0 && y == m.paneTop+paneMetaRow && x >= lx && x < lx+c.label[1]
}
