package ui

import (
	"cmp"
	"errors"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/state"
)

// #new starts an agent on any route once, from the box:
//
//	#new [harness][@provider[:account]] [model] [effort] [task…]
//
// Each part is optional, the rest coming from the route's defaults; a
// word is taken only when it names one exactly, and the first that
// doesn't starts the task. With no task it sets the next session, as
// #use does. Nothing it does changes a default. #with is the same,
// without a task. The design: docs/providers-harnesses.md.

// newStart is #new's words, read: the start, the task, and what to know.
type newStart struct {
	o    startOver
	task string
	warn []string
}

// newCommand runs #new (or #with) with arg.
func (m *Model) newCommand(arg string) tea.Cmd {
	n, err := m.parseNew(arg)
	if err != nil {
		m.flash(err.Error(), true)
		return nil
	}
	if n.task == "" {
		m.startOver = &n.o
		m.flash(strings.Join(append([]string{"the next session starts as " + strings.Join(m.startWords(n.o), " · ")}, n.warn...), " · "), false)
		return nil
	}
	return m.startAs(n.o, n.task)
}

// startAs starts a session on o with msg in the folder new ones start in,
// once o's account (when it names one) is switched to.
func (m *Model) startAs(o startOver, msg string) tea.Cmd {
	m.startOver = &o
	dir := m.startDir()
	sw := m.accountSwitch(agent.Kind(o.kind), o.account)
	if sw == nil {
		return m.startHosted(msg, dir)
	}
	return tea.Sequence(sw, func() tea.Msg {
		return applyMsg(func(m *Model) tea.Cmd {
			m.startOver = &o
			return m.startHosted(msg, dir)
		})
	})
}

// newPreview is what #new or #with being typed starts, for the box's
// border: the start, or why it can't. Empty for any other text.
func (m *Model) newPreview(text string) string {
	name, arg, _ := strings.Cut(strings.TrimPrefix(text, "#"), " ")
	if !strings.HasPrefix(text, "#") || name != "new" && name != "with" {
		return ""
	}
	n, err := m.parseNew(arg)
	if err != nil {
		return paint(cRed, ansi.Truncate("✗ "+err.Error(), max(24, m.w-40), "…")) // the border drops what doesn't fit
	}
	when := " · next session"
	if n.task != "" {
		when = " · once, defaults untouched"
	}
	out := m.setupChip(n.o) + dim(when)
	for _, w := range n.warn {
		out += paint(cYellow, " · "+w)
	}
	return out
}

// errNoRoute is a #new that names nothing it knows.
var errNoRoute = errors.New("#new [harness][@provider[:account]] [model] [effort] task")

// parseNew reads #new's argument.
func (m *Model) parseNew(arg string) (newStart, error) {
	m.readCatalogs()
	rest := strings.TrimSpace(arg)
	next := func() string {
		if f := strings.Fields(rest); len(f) > 0 {
			return f[0]
		}
		return ""
	}
	eat := func() { rest = strings.TrimSpace(strings.TrimPrefix(rest, next())) }
	var n newStart
	o, picked, err := m.newTarget(next())
	switch {
	case err != nil:
		return n, err
	case picked:
		eat()
	default:
		o = m.nextStart(m.startDir())
		if w := next(); w != "" && !slices.ContainsFunc(m.models(o.kind), func(c agent.Choice) bool { return strings.EqualFold(c.ID, w) }) {
			if mo, ok := m.modelRoute(w); ok {
				o = mo // a model alone picks the provider that serves it
				eat()
			}
		}
	}
	for range 2 {
		w := next()
		switch {
		case w == "":
		case o.model != w && slices.ContainsFunc(m.models(o.kind), func(c agent.Choice) bool { return strings.EqualFold(c.ID, w) }):
			o.model = w
			eat()
			continue
		case slices.ContainsFunc(effortsOf(o.kind), func(c agent.Choice) bool { return strings.EqualFold(c.ID, w) }):
			o.effort = strings.ToLower(w)
			eat()
			continue
		}
		break
	}
	if len(rest) > 1 && rest[0] == '"' && rest[len(rest)-1] == '"' {
		rest = rest[1 : len(rest)-1]
	}
	n.o, n.task = o, rest
	k := agent.Kind(o.kind)
	if _, warn, _ := agent.Compat(startID(o), agent.HarnessOf(k)); warn != "" {
		n.warn = append(n.warn, warn)
	}
	if o.account != "" && o.account != m.accountOf(k) {
		h := agent.HarnessLabel(k)
		n.warn = append(n.warn, "swaps "+h+"'s sign-in to "+o.account+" for every "+h+" session")
	}
	return n, nil
}

// newTarget reads #new's first word as a route: harness@provider[:account],
// harness alone, @provider alone, or a name #use took. picked is false
// when it names none, and it's the task's.
func (m *Model) newTarget(w string) (o startOver, picked bool, err error) {
	if w == "" {
		return o, false, nil
	}
	hw, pw, at := strings.Cut(w, "@")
	h := agent.HarnessByWord(hw)
	if !at {
		if h != "" {
			o, err = m.routeStart("", h, "")
			return o, true, err
		}
		if o, err := pickSetup(m.setups(), w, effortsOf); err == nil {
			return o, true, nil
		}
		return o, false, nil
	}
	if hw != "" && h == "" {
		return o, true, errors.New("no harness called " + hw + " · " + strings.Join(harnessWords(), ", "))
	}
	pw, acct, _ := strings.Cut(pw, ":")
	id := ""
	switch pw {
	case "":
	case "sub", "api":
		if h == "" {
			return o, true, errors.New("@" + pw + " is the harness's own provider: name the harness, codex@" + pw)
		}
		id = agent.ProviderOf(h)
		if pw == "api" && agent.Split(id) {
			id = agent.KeyOf(id)
		}
	default:
		if id = agent.ProviderByWord(pw); id == "" {
			return o, true, errors.New("no provider called " + pw + " · " + strings.Join(providerWords(), ", "))
		}
	}
	o, err = m.routeStart(id, h, acct)
	return o, true, err
}

// routeStart is a start on provider id in harness h, either one empty for
// its default: id's default harness, or h on the next session's provider
// when h runs it, else the first whose ★ is h, else any h runs.
func (m *Model) routeStart(id string, h agent.Kind, acct string) (startOver, error) {
	if id != "" && h == "" {
		h = agent.HarnessOf(m.provKind(id))
	}
	ids := []string{id}
	if id == "" {
		ids = append([]string{startID(m.nextStart(m.startDir()))}, agent.ProviderIDs()...)
		elsewhere := func(id string) int { return bool2int(agent.HarnessOf(m.provKind(id)) != h) }
		slices.SortStableFunc(ids[1:], func(a, b string) int { return cmp.Compare(elsewhere(a), elsewhere(b)) })
	}
	var first error
	for _, id := range ids {
		k, _, why := agent.Compat(id, h)
		if why == "" && paysByKey(id) && !m.store.Config.HasAPIKey(provOf(id)) {
			why = "no " + agent.ProviderLabel(provOf(id)) + " API key yet: $ adds one in Settings › Providers"
		}
		if why != "" {
			first = cmp.Or(first, errors.New(why+" · try "+m.routesInto(id, h)))
			continue
		}
		o := m.startOn(id, string(k))
		if acct == "" {
			return o, nil
		}
		if o.billing == state.BillingKey {
			return o, errors.New(provLabel(id) + " pays with its API key: it has no accounts")
		}
		names := m.accountNames(k)
		i := slices.IndexFunc(names, func(n string) bool { return nameWord(n) == nameWord(acct) })
		if i < 0 {
			return o, errors.New("no " + agent.HarnessLabel(k) + " account called " + acct + " · " + strings.Join(names, ", "))
		}
		o.account = names[i]
		return o, nil
	}
	return startOver{}, cmp.Or(first, errors.New(agent.HarnessLabel(h)+" runs no provider here"))
}

// routesInto are the routes worth trying instead of id in h, as words:
// h on another provider, or id in another harness.
func (m *Model) routesInto(id string, h agent.Kind) string {
	var out []string
	for _, r := range m.routeRows() {
		if r.why == "" && (r.id == id || r.harness == h) && len(out) < 2 {
			out = append(out, r.word())
		}
	}
	if len(out) == 0 {
		return "#new alone for the list"
	}
	return strings.Join(out, " or ")
}

// modelRoute is the start of the first route that serves model, the next
// session's provider first.
func (m *Model) modelRoute(model string) (startOver, bool) {
	rows := m.routeRows()
	now := startID(m.nextStart(m.startDir()))
	slices.SortStableFunc(rows, func(a, b routeRow) int {
		return cmp.Compare(bool2int(a.id != now), bool2int(b.id != now))
	})
	for _, r := range rows {
		if r.why == "" && slices.ContainsFunc(m.models(string(r.kind)), func(c agent.Choice) bool { return strings.EqualFold(c.ID, model) }) {
			o := m.startOn(r.id, string(r.kind))
			o.model = model
			return o, true
		}
	}
	return startOver{}, false
}

func bool2int(b bool) int {
	if b {
		return 1
	}
	return 0
}

// routeRow is one provider in one harness, as #new and the sheet list it.
type routeRow struct {
	id        string
	harness   agent.Kind
	kind      agent.Kind // the agent that runs it; "" when nothing does
	def       bool       // the provider's default harness
	warn, why string
}

// word is the route as #new takes it: codex@openai-sub.
func (r routeRow) word() string { return agent.HarnessWord(r.harness) + "@" + agent.ProviderWord(r.id) }

// routeRows are every route of every installed provider, those that can
// run first, each provider's default before its others.
func (m *Model) routeRows() []routeRow {
	var out []routeRow
	for _, id := range agent.ProviderIDs() {
		if !agent.ProviderInstalled(provOf(id)) {
			continue
		}
		def := agent.HarnessOf(m.provKind(id))
		for _, h := range agent.HarnessKinds() {
			k, warn, why := agent.Compat(id, h)
			if k == "" {
				continue
			}
			if why == "" && paysByKey(id) && !m.store.Config.HasAPIKey(provOf(id)) {
				why = "no API key yet: $ in Settings › Providers"
			}
			out = append(out, routeRow{id: id, harness: h, kind: k, def: h == def, warn: warn, why: why})
		}
	}
	slices.SortStableFunc(out, func(a, b routeRow) int {
		return cmp.Or(cmp.Compare(bool2int(a.why != ""), bool2int(b.why != "")), cmp.Compare(bool2int(!a.def), bool2int(!b.def)))
	})
	return out
}

// newArgs are the completions for #new's (or #with's) words in q, the
// word being typed: routes first, then the route's models and efforts.
func (m *Model) newArgs(name, q string) []event.Command {
	m.readCatalogs()
	words := strings.Fields(q)
	typing := ""
	if len(words) > 0 && !strings.HasSuffix(q, " ") {
		typing, words = strings.ToLower(words[len(words)-1]), words[:len(words)-1]
	}
	lead := strings.TrimSpace(name + " " + strings.Join(words, " "))
	var first, rest []event.Command
	add := func(arg, note string) {
		c := event.Command{Name: lead + " " + arg, Description: note, ArgumentHint: "[next]"}
		switch {
		case strings.HasPrefix(strings.ToLower(arg), typing):
			first = append(first, c)
		case typing != "" && strings.Contains(strings.ToLower(arg+" "+note), typing):
			rest = append(rest, c)
		}
	}
	if len(words) == 0 {
		for _, r := range m.routeRows() {
			note := glyph(r.kind) + " " + agent.HarnessLabel(r.harness) + " · " + provLabel(r.id)
			switch {
			case r.why != "":
				note += "  ✗ " + r.why
			case r.def:
				note += "  ★ " + agent.ProviderWord(r.id) + "'s default"
			}
			if r.warn != "" {
				note += "  · " + r.warn
			}
			add(r.word(), note)
		}
		return append(first, rest...)
	}
	n, err := m.parseNew(strings.Join(words, " "))
	if err != nil || n.task != "" {
		return nil // past the route's words: the task is being typed
	}
	k, def := n.o.kind, m.startOn(startID(n.o), n.o.kind)
	if n.o.model == "" || n.o.model == def.model {
		for _, c := range m.models(k) {
			note := c.ID + "  " + oneLine(c.Note)
			if c.ID == def.model {
				note += "  ★ default here"
			}
			add(c.ID, note)
		}
	}
	if n.o.effort == "" || n.o.effort == def.effort {
		for _, e := range effortsOf(k) {
			add(e.ID, e.ID+" effort · "+e.Note)
		}
	}
	return append(first, rest...)
}

// readCatalogs reads every installed agent's models once, off the UI, for
// #new's words; a frame after they land has them.
func (m *Model) readCatalogs() {
	a := &m.accts
	if a.catalogs == nil {
		a.catalogs = goPending(readSettingsModels)
		return
	}
	if found, ok := a.catalogs.take(); ok {
		if m.listed == nil {
			m.listed = map[string][]agent.Choice{}
		}
		for k, v := range found {
			if len(m.listed[k]) == 0 {
				m.listed[k] = v
			}
		}
	}
}

// provOf is the provider id is, whichever way it's paid.
func provOf(id string) string {
	p, _ := agent.Billed(id)
	return p
}

func harnessWords() []string {
	var out []string
	for _, h := range agent.HarnessKinds() {
		out = append(out, agent.HarnessWord(h))
	}
	return out
}

func providerWords() []string {
	var out []string
	for _, id := range agent.ProviderIDs() {
		if agent.ProviderInstalled(provOf(id)) {
			out = append(out, agent.ProviderWord(id))
		}
	}
	return out
}
