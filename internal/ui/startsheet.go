package ui

import (
	"cmp"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The start sheet picks what the next session starts as, for that one
// session: a profile of yours, or the provider, harness, account, model
// and effort. Settings' defaults are untouched. Opened on a session, the
// same sheet switches it (switchSession).

// startOver is what a session starts as, over what its folder's profile
// and Settings say.
type startOver struct {
	kind, model, effort string
	mode                string
	modeSet             bool
	billing             string // state.BillingKey pays with the provider's API key; "" as it's signed in
	account             string // the account's name; "" is the one in use
	profile             string // the profile of yours it is; "" none
}

// route is a provider id (claude, claude-key, ollama) and the agents here
// that run it.
type route struct {
	id    string
	kinds []string
}

// routes are the valid ways to start here, of kinds (the agents that run
// here): a split provider's subscription in its own harness only, its API
// key in any that speaks its API, and anything paid by key only once the
// key is kept (hasKey).
func routes(kinds []string, hasKey func(p string) bool) []route {
	var out []route
	var seen []string
	for _, k := range kinds {
		p := agent.ProviderOf(agent.Kind(k))
		if slices.Contains(seen, p) {
			continue
		}
		seen = append(seen, p)
		ids := []string{p}
		if agent.Split(p) {
			ids = append(ids, agent.KeyOf(p))
		}
		for _, id := range ids {
			_, key := agent.Billed(id)
			var ks []string
			for _, r := range agent.RunsFor(id) {
				if slices.Contains(kinds, string(r)) && (!key && !agent.KeyOnly(r) || hasKey(p)) {
					ks = append(ks, string(r))
				}
			}
			if len(ks) > 0 {
				out = append(out, route{id, ks})
			}
		}
	}
	return out
}

// billingOf is how agent k is paid for as provider id: by key when id is
// a key, or k runs only on one.
func billingOf(id string, k agent.Kind) string {
	if _, key := agent.Billed(id); key || agent.KeyOnly(k) {
		return state.BillingKey
	}
	return ""
}

// startID is the provider o is paid through: a split provider's key one
// when it goes by the key.
func startID(o startOver) string {
	p := agent.ProviderOf(agent.Kind(o.kind))
	if o.billing == state.BillingKey && agent.Split(p) {
		return agent.KeyOf(p)
	}
	return p
}

// startRoutes are the ways a session can start here now.
func (m *Model) startRoutes() []route {
	var kinds []string
	for _, a := range agent.All() {
		if _, ok := a.(agent.Driver); ok && agent.CurrentKind(a.Kind()) == a.Kind() && agent.Installed(a.Kind()) && agent.Runs(a.Kind()) {
			kinds = append(kinds, string(a.Kind()))
		}
	}
	return routes(kinds, m.store.Config.HasAPIKey)
}

// nextStart is what the next session in dir starts as: one picked on the
// sheet, else its folder's agent with what Settings says it starts with,
// and what its profile says over that.
func (m *Model) nextStart(dir string) startOver {
	if m.startOver != nil {
		return *m.startOver
	}
	return m.byProfile(dir)
}

// byProfile is what a new session in dir starts as by its profile,
// whatever was picked for the next one.
func (m *Model) byProfile(dir string) startOver {
	k := m.startKindIn(dir)
	if p := m.startProfile(dir); len(p.Providers) > 0 && agent.ProviderOf(agent.Kind(k)) == p.Providers[0] {
		o := m.profileSetup(p, k)
		o.account = "" // the one in use: a profile's account is where it starts while that has room
		return o
	}
	return m.startDefaults(k)
}

// startDefaults is agent k as Settings says it starts, paid as it is
// unless startOn says otherwise.
func (m *Model) startDefaults(k string) startOver { return m.startOn("", k) }

// startOn is agent k on provider id as Settings says that route starts:
// id's key pays when id is one; empty is k's own way.
func (m *Model) startOn(id, k string) startOver {
	billing := billingOf(id, agent.Kind(k))
	if id == "" {
		id = startID(startOver{kind: k, billing: billing})
	}
	st := m.store.Config.Dispatch.StartOn(id, agent.Kind(k))
	return startOver{kind: k, model: st.Model, effort: st.Effort, mode: st.Mode, modeSet: true, billing: billing}
}

// profileSetup is profile p as a start on agent k (its first provider
// here): its model, effort, billing and account over what Settings
// starts k with.
func (m *Model) profileSetup(p state.Profile, k string) startOver {
	o := m.startDefaults(k)
	o.model, o.effort = cmp.Or(p.Model, o.model), cmp.Or(p.Effort, o.effort)
	if p.Billing != "" {
		o.billing = p.Billing // with no key kept yet, starting says where to add one
	}
	o.account = m.accountName(agent.Kind(k), p.Account)
	if !p.Builtin {
		o.profile = p.Name
	}
	return o
}

// The start sheet: a profile row, then three linked columns (provider,
// harness, model) and the account, effort and permissions rows. Each
// column shows every choice, the ones that can't go with the others
// faint, with why; picking one of those moves the other column to one
// that can. Its last line is the #new that starts the same.

// startSheet picks a startOver. row is the part in focus, as startRows
// names them; 1, 2 and 4 are the columns.
type startSheet struct {
	applyOnly bool // permission controls never submit a retained draft
	routes    []route
	o         startOver
	row       int
	conn      string // the session it switches; "" for the next one
	chose     string // the provider picked, kept while harnesses that can't run it go by
}

// startRows are the sheet's parts.
var startRows = []string{"Preset", "Provider", "Harness", "Account", "Model", "Effort", "Permissions"}

// tabOrder is the order tab goes through them: the columns left to right.
var tabOrder = []int{0, 1, 2, 4, 3, 5, 6}

// openStartSheet opens the sheet on what the next session would start as.
func (m *Model) openStartSheet() tea.Cmd {
	return m.openSetupSheet(nil, m.nextStart(m.startDir()))
}

// openSwitchSheet opens the sheet on what session c runs as, to switch it.
func (m *Model) openSwitchSheet(c *hostConn) tea.Cmd {
	return m.openSetupSheet(c, m.sessionStart(c))
}

func (m *Model) openSetupSheet(c *hostConn, o startOver) tea.Cmd {
	rs := m.startRoutes()
	if c != nil {
		found := false
		for i := range rs {
			if rs[i].id == startID(o) {
				found = true
				if !slices.Contains(rs[i].kinds, o.kind) {
					rs[i].kinds = append(rs[i].kinds, o.kind)
				}
			}
		}
		if !found {
			rs = append(rs, route{id: startID(o), kinds: []string{o.kind}})
		}
	}
	if len(rs) == 0 {
		m.flash("rush can't run any harness here yet", true)
		return nil
	}
	s := &startSheet{routes: rs, o: o, row: 1}
	if c != nil {
		s.conn = c.key
	}
	if o.profile != "" {
		s.row = 0
	}
	if r := s.at(startID(o)); r == nil || !slices.Contains(r.kinds, o.kind) {
		s.o = m.startOn(rs[0].id, rs[0].kinds[0])
	}
	m.sheet = s
	return tea.Batch(m.loadModels(s.o.kind), m.loadAllModels())
}

// loadAllModels reads every installed agent's models off the UI, for the
// sheet's other routes.
func (m *Model) loadAllModels() tea.Cmd {
	return sheetDo(func() (map[string][]agent.Choice, error) { return readSettingsModels(), nil },
		func(m *Model, found map[string][]agent.Choice, _ error) tea.Cmd {
			if m.listed == nil {
				m.listed = map[string][]agent.Choice{}
			}
			for k, v := range found {
				if len(m.listed[k]) == 0 {
					m.listed[k] = v
				}
			}
			return nil
		})
}

// at is the route of provider id, or nil.
func (s *startSheet) at(id string) *route {
	for i := range s.routes {
		if s.routes[i].id == id {
			return &s.routes[i]
		}
	}
	return nil
}

// loadModels reads the models agent k offers from its home (Codex's
// cache), off the UI, for the sheet; nil when its adapter lists them
// itself or reads none.
func (m *Model) loadModels(k string) tea.Cmd {
	ad, ok := agent.Get(agent.Kind(k))
	ml, lists := ad.(agent.ModelLister)
	if !ok || !lists {
		return nil
	}
	return sheetDo(func() ([]agent.Choice, error) {
		if ps := ad.Profiles(); len(ps) > 0 { // off the UI, so it may read
			return ml.ListModels(ps[0]), nil
		}
		return nil, nil
	}, func(m *Model, v []agent.Choice, _ error) tea.Cmd {
		if m.listed == nil {
			m.listed = map[string][]agent.Choice{}
		}
		m.listed[k] = v
		return nil
	})
}

// models are what agent k can start on: its adapter's list, else the one
// read from its home.
func (m *Model) models(k string) []agent.Choice {
	ch, _ := agent.ChoicesOf(agent.Kind(k))
	out := append([]agent.Choice(nil), m.listed[k]...)
	for _, c := range ch.Models {
		if !slices.ContainsFunc(out, func(v agent.Choice) bool { return v.ID == c.ID }) {
			out = append(out, c)
		}
	}
	return out
}

// sheetProviders are the provider column: every provider with a route
// that runs here, and the one picked.
func (s *startSheet) sheetProviders(m *Model) []string {
	var out []string
	for _, r := range m.routeRows() {
		if r.why == "" && !slices.Contains(out, r.id) {
			out = append(out, r.id)
		}
	}
	if id := startID(s.o); !slices.Contains(out, id) {
		out = append(out, id)
	}
	return out
}

// sheetHarnesses are the harness column: every harness with a route that
// runs here, and the one picked.
func (s *startSheet) sheetHarnesses(m *Model) []agent.Kind {
	var out []agent.Kind
	rows := m.routeRows() // once: each is every provider by every harness
	for _, h := range agent.HarnessKinds() {
		if slices.ContainsFunc(rows, func(r routeRow) bool { return r.harness == h && r.why == "" }) {
			out = append(out, h)
		}
	}
	if h := agent.HarnessOf(agent.Kind(s.o.kind)); !slices.Contains(out, h) {
		out = append(out, h)
	}
	return out
}

// fits is whether provider id runs in harness h here, and why not.
func (m *Model) fits(id string, h agent.Kind) (agent.Kind, string, string) {
	k, warn, why := agent.Compat(id, h)
	if why == "" && paysByKey(id) && !m.store.Config.HasAPIKey(provOf(id)) {
		why = "no API key yet"
	}
	return k, warn, why
}

// choices are row's values, as the startOver field it sets holds them.
func (s *startSheet) choices(m *Model, row int) []string {
	k := agent.Kind(s.o.kind)
	switch row {
	case 0:
		out := []string{""}
		for _, p := range m.store.Config.Profiles {
			out = append(out, p.Name)
		}
		return out
	case 1:
		return s.sheetProviders(m)
	case 2:
		var out []string
		for _, h := range s.sheetHarnesses(m) {
			out = append(out, string(h))
		}
		return out
	case 3:
		if s.o.billing == state.BillingKey {
			return []string{""}
		}
		return append([]string{""}, m.accountNames(k)...)
	case 6:
		var c *hostConn
		if s.conn != "" {
			c = m.sheetConn(s.conn)
			if c != nil && string(sessionAgent(c)) != s.o.kind {
				c = nil
			}
		}
		var out []string
		if c == nil {
			out = append(out, "")
		}
		for _, v := range permissionChoices(k, c) {
			out = append(out, v.ID)
		}
		if !slices.Contains(out, s.o.mode) {
			out = append(out, s.o.mode)
		}
		return out
	}
	list, now := m.models(s.o.kind), s.o.model
	if row == 5 {
		ch, _ := agent.ChoicesOf(k)
		list, now = ch.Efforts, s.o.effort
	}
	out := []string{""}
	for _, c := range list {
		out = append(out, c.ID)
	}
	if !slices.Contains(out, now) {
		out = append(out, now) // one Settings names that the list doesn't
	}
	return out
}

// now is row's value in o.
func (s *startSheet) now(row int) string {
	return []string{s.o.profile, startID(s.o), string(agent.HarnessOf(agent.Kind(s.o.kind))), s.o.account, s.o.model, s.o.effort, s.o.mode}[row]
}

// word is row's value v in words.
func (s *startSheet) word(m *Model, row int, v string) string {
	k := agent.Kind(s.o.kind)
	switch row {
	case 0:
		return cmp.Or(v, "none: as below")
	case 1:
		return provLabel(v)
	case 2:
		return agent.HarnessLabel(agent.Kind(v))
	case 3:
		switch in := m.accountOf(k); {
		case s.o.billing == state.BillingKey:
			return "its API key"
		case v == "" && in != "":
			return in + ", in use"
		case v == "":
			return "its own sign-in"
		case v == in:
			return v + ", in use"
		}
		return v + ", switched to for every " + agent.HarnessLabel(k) + " session"
	case 6:
		if v != "" {
			return v
		}
		if s.conn != "" {
			return "not reported by harness"
		}
		return "harness default"
	case 4:
		if v == "" {
			return "its default model"
		}
		return modelWord(s.o.kind, v)
	}
	return cmp.Or(v, "its default effort")
}

func (s *startSheet) width(m *Model) int { return max(60, min(112, m.w-6)) }

func (s *startSheet) body(m *Model, w, h int) []string {
	title, about := "Start as", "the next session only; Settings stay as they are"
	if s.conn != "" {
		title, about = "Switch to", "in place in the same harness, else the conversation is handed over"
	}
	field := func(row int) string {
		v := s.word(m, row, s.now(row))
		line := dim(startRows[row]+" ") + faint("‹ ") + paint(cText+bold, v) + faint(" ›")
		if len(s.choices(m, row)) < 2 {
			line = dim(startRows[row]+" ") + paint(cText, v)
		}
		return line
	}
	var bottom []string
	if all := field(3) + "    " + field(5) + "    " + field(6); cellw.String(all)+2 <= w {
		on := map[bool]string{true: paint(cOrange, "▍"), false: " "}
		bottom = append(bottom, " "+on[s.row == 3]+field(3)+"   "+on[s.row == 5]+field(5)+"   "+on[s.row == 6]+field(6))
	} else {
		for _, row := range []int{3, 5, 6} {
			bottom = append(bottom, sheetRow(field(row), s.row == row, w))
		}
	}
	if s.row == 6 {
		var c *hostConn
		if s.conn != "" {
			c = m.sheetConn(s.conn)
			if c != nil && string(sessionAgent(c)) != s.o.kind {
				c = nil
			}
		}
		choices := permissionChoices(agent.Kind(s.o.kind), c)
		note := "Permissions are per session; pending approvals and queued messages stay unchanged."
		if len(choices) == 0 {
			note = "This harness has not advertised selectable permission modes."
		}
		for _, v := range choices {
			if v.ID == s.o.mode && v.Note != "" {
				note = v.Note
			}
		}
		bottom = append(bottom, wrap(paint(cText, note), w)...)
	}
	_, warn, _ := agent.Compat(startID(s.o), agent.HarnessOf(agent.Kind(s.o.kind)))
	chip := "  " + m.setupChip(s.o)
	if warn != "" {
		chip += "  " + paint(cYellow, "⚠ "+warn)
	}
	said := []string{"", chip, "  " + dim("same as  ") + paint(cText, m.newWords(s.o))}
	do := "use it"
	switch {
	case s.row == 6 || s.applyOnly:
		do = "Apply"
	case s.conn != "":
		do = "switch"
	case len(m.input) > 0:
		do = "start it"
	}
	footer := keysControls(w, "←→", "column or value", "↑↓", "choose", "tab", "next part", "enter", do, "esc", "cancel")
	out := []string{sheetTitle(title, about, w)}
	room := 1 << 20
	if h > 0 {
		room = h - len(out) - len(bottom) - len(footer)
	}
	if room >= len(said)+6 {
		bottom, room = append(bottom, said...), room-len(said)
	}
	if room >= 6 { // a profile row, and columns three choices tall
		out = append(out, "", sheetRow(field(0), s.row == 0, w), "")
		out = append(out, s.columns(m, w, min(8, room-4))...)
		out = append(out, "")
	}
	out = append(append(out, bottom...), footer...)
	if h > 0 && len(out) > h {
		out = append(out[:max(0, h-len(footer))], footer...)
	}
	for i := range out {
		out[i] = fit(out[i], w)
	}
	return out
}

// columns draws the provider, harness and model columns, n lines tall.
func (s *startSheet) columns(m *Model, w, n int) []string {
	pw, hw := (w-4)*35/100, (w-4)*37/100
	mw := w - 4 - pw - hw
	id, h := startID(s.o), agent.HarnessOf(agent.Kind(s.o.kind))
	head := func(row int, name string, cw int) string {
		if row == s.row {
			return paint(cOrange+bold, fit(strings.ToUpper(name), cw))
		}
		return dim(fit(strings.ToUpper(name), cw))
	}
	cell := func(row int, on bool, text string, cw int) string {
		mark := "  "
		if on {
			mark = paint(cOrange, "▸ ")
			if row == s.row {
				text = paint(cBright+bold, ansi.Strip(text))
			}
		}
		return fit(mark+text, cw)
	}
	var provs, harns, mods []string
	for _, p := range s.sheetProviders(m) {
		k, _, why := m.fits(p, h)
		pk := m.provKind(p)
		// ★ and the glyph of the harness it starts in by default.
		text := glyph(agent.Kind(provOf(p))) + " " + paint(cText, fit(provLabel(p), pw-10)) + " " + dim("★") + glyph(agent.HarnessOf(pk))
		if why != "" || k == "" {
			text = faint(ansi.Strip(text))
		}
		provs = append(provs, cell(1, p == id, text, pw))
	}
	pk := agent.HarnessOf(m.provKind(id))
	for _, hk := range s.sheetHarnesses(m) {
		k, warn, why := m.fits(id, hk)
		text := glyph(hk) + " " + paint(cText, fit(agent.HarnessLabel(hk), 13))
		switch {
		case why != "":
			text = faint(ansi.Strip(glyph(hk)+" "+fit(agent.HarnessLabel(hk), 13)) + " – " + why)
		case k == "":
		case warn != "":
			text += " " + paint(cYellow, "⚠ on your plan")
		case hk == pk:
			text += " " + paint(cOrange, "★")
		}
		harns = append(harns, cell(2, hk == h, text, hw))
	}
	def := m.startOn(id, s.o.kind).model
	for _, v := range s.choices(m, 4) {
		text := paint(cText, fit(s.word(m, 4, v), mw-8))
		if v != "" && v == def {
			text += " " + paint(cOrange, "★")
		}
		mods = append(mods, cell(4, v == s.o.model, text, mw))
	}
	at := func(list []string, now int) []string {
		from, to := window(len(list), now, n)
		return list[from:to]
	}
	provs = at(provs, slices.Index(s.sheetProviders(m), id))
	harns = at(harns, slices.Index(s.sheetHarnesses(m), h))
	mods = at(mods, slices.Index(s.choices(m, 4), s.o.model))
	out := []string{"  " + head(1, "Provider", pw) + head(2, "Harness", hw) + head(4, "Model", mw)}
	for i := range max(len(provs), len(harns), len(mods)) {
		line := "  "
		for _, col := range []struct {
			list []string
			w    int
		}{{provs, pw}, {harns, hw}, {mods, mw}} {
			if i < len(col.list) {
				line += col.list[i]
			} else {
				line += strings.Repeat(" ", col.w)
			}
		}
		out = append(out, line)
	}
	return out
}

// newWords are o as #new takes it: codex@openai-sub:alex gpt-6-astra high.
func (m *Model) newWords(o startOver) string {
	k := agent.Kind(o.kind)
	out := "#new " + agent.HarnessWord(agent.HarnessOf(k)) + "@" + agent.ProviderWord(startID(o))
	if o.account != "" {
		out += ":" + nameWord(o.account)
	}
	for _, v := range []string{o.model, o.effort} {
		if v != "" {
			out += " " + v
		}
	}
	return out
}

func (s *startSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	column := s.row == 1 || s.row == 2 || s.row == 4
	move := func(d int) tea.Cmd {
		vals := s.choices(m, s.row)
		i := max(0, slices.Index(vals, s.now(s.row)))
		return s.set(m, vals[roundMove(i, d, len(vals))])
	}
	switch k {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "tab", "shift+tab":
		d := 1
		if k == "shift+tab" {
			d = -1
		}
		s.row = tabOrder[roundMove(slices.Index(tabOrder, s.row), d, len(tabOrder))]
	case "up", "down":
		d := 1
		if k == "up" {
			d = -1
		}
		switch {
		case column:
			return move(d)
		case s.row == 0 && d > 0:
			s.row = 1
		case s.row != 0 && d < 0:
			s.row = 4 // from the rows below, back into the columns
		case s.row != 0:
			s.row = []int{3: 5, 5: 6, 6: 3}[s.row]
		}
	case "left", "right":
		d := 1
		if k == "left" {
			d = -1
		}
		if column {
			cols := []int{1, 2, 4}
			i := slices.Index(cols, s.row) + d
			if i >= 0 && i < len(cols) {
				s.row = cols[i]
			}
			return nil
		}
		return move(d)
	case "enter":
		m.sheet = nil
		o := s.o
		if s.conn != "" {
			if c := m.sheetConn(s.conn); c != nil {
				return m.switchSession(c, o)
			}
			return nil
		}
		if len(m.input) > 0 && s.row != 6 && !s.applyOnly {
			msg := string(m.input)
			m.input, m.back = nil, 0
			return m.startAs(o, msg)
		}
		m.startOver = &o
		m.flash("the next session starts as "+m.startWith(m.startDir(), true), false)
	}
	return nil
}

// set puts v in the part in focus. A provider keeps the harness when it
// runs there, else goes to its default one; a harness keeps the provider
// when it runs it, else takes the first that it runs. Anything but a
// profile makes it a setup of its own.
func (s *startSheet) set(m *Model, v string) tea.Cmd {
	switch s.row {
	case 0:
		p, ok := m.store.Config.ProfileNamed(v)
		if !ok || v == "" {
			s.o.profile = ""
			return nil
		}
		inst := p.Installed()
		if len(inst) == 0 {
			m.flash("none of "+v+"'s providers runs here", true)
			return nil
		}
		s.o, s.chose = m.profileSetup(p, inst[0]), ""
		return m.loadModels(s.o.kind)
	case 1:
		k, _, why := m.fits(v, agent.HarnessOf(agent.Kind(s.o.kind)))
		if why != "" || k == "" {
			k = m.provKind(v)
		}
		s.o, s.chose = m.startOn(v, string(k)), v
		return m.loadModels(string(k))
	case 2:
		h := agent.Kind(v)
		id := cmp.Or(s.chose, startID(s.o))
		s.chose = id
		k, _, why := m.fits(id, h)
		if why != "" || k == "" {
			for _, r := range m.routeRows() {
				if r.harness == h && r.why == "" {
					id, k = r.id, r.kind
					break
				}
			}
		}
		s.o = m.startOn(id, string(k))
		return m.loadModels(string(k))
	case 3:
		s.o.account = v
	case 4:
		s.o.model = v
	case 5:
		s.o.effort = v
	case 6:
		s.o.mode, s.o.modeSet = v, true
	}
	s.o.profile = ""
	return nil
}
