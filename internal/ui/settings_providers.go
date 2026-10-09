package ui

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Providers is a list and, beside it, everything about the line picked:
// each installed provider in the default profile's order, Anthropic's and
// OpenAI's subscriptions apart from their API keys. A provider shows its
// accounts or key, the harnesses it runs in (★ where new sessions start),
// what they start with, what it does at a limit, and under all of it what
// rush can do with it and its models. Profiles is the same shell over the
// profiles you made and the folders that pick one.

func providersPage() page {
	return page{
		name: "Providers",
		pre: func(m *Model, s string) (tea.Cmd, bool) {
			d := m.dialog
			if d.inside && (s == "esc" || s == "q" || s == "backspace") {
				d.inside, d.cursor = false, d.pick
				d.catalogScroll = 0
				return nil, true
			}
			return m.catalogKey(s, d.inside)
		},
		body: (*Model).providersBody,
		key:  (*Model).providersKey,
		rows: func(m *Model) int {
			if m.dialog.inside {
				return len(flat(m.provForm(m.provPicked())))
			}
			return len(m.provItems())
		},
	}

}

func profilesPage() page {
	p := providersPage()
	p.name = "Presets"
	return p
}

// provItem is a line of Providers' list: one of these is set.
type provItem struct {
	provider string // an installed provider, by id (state.IDs)
	profile  string // a profile of yours
	folder   string // a folder rule's path
	add      string // "profile" or "folder": its group's + line
}

// provItems are the lines of the page showing: Providers' or Profiles'.
func (m *Model) provItems() []provItem {
	if m.dialog != nil && m.dialog.page == pageProfiles {
		return m.profileItems()
	}
	var out []provItem
	for _, ad := range m.agentOrder() {
		pr := agent.ProviderOf(ad.Kind())
		ids := []string{pr}
		if agent.Split(pr) {
			ids = append(ids, agent.KeyOf(pr))
		}
		for _, id := range ids {
			if it := (provItem{provider: id}); !slices.Contains(out, it) {
				out = append(out, it)
			}
		}
	}
	return out
}

// profileItems are Profiles' lines: your profiles, then the folders that
// pick one.
func (m *Model) profileItems() []provItem {
	var out []provItem
	cfg := m.store.Config
	for _, p := range cfg.Profiles {
		if !ownProfile(p.Name) {
			out = append(out, provItem{profile: p.Name})
		}
	}
	out = append(out, provItem{add: "profile"})
	for _, r := range cfg.FolderRules {
		out = append(out, provItem{folder: r.Path})
	}
	return append(out, provItem{add: "folder"})
}

// provPicked is the list's line shown: the cursor's, or the one gone into.
func (m *Model) provPicked() provItem {
	d, items := m.dialog, m.provItems()
	i := d.cursor
	if d.inside {
		i = d.pick
	}
	return items[max(0, min(i, len(items)-1))]
}

// openItem goes into it on Providers, when it's there.
func (m *Model) openItem(it provItem) {
	if i := slices.Index(m.provItems(), it); i >= 0 {
		d := m.dialog
		d.pick, d.cursor, d.inside = i, 0, true
	}
}

// provKind is the agent provider id runs as: in its own harness, or the
// one chosen as its default.
func (m *Model) provKind(id string) agent.Kind {
	p, ok := m.store.Config.ProfileNamed(id)
	if !ok || len(p.Providers) == 0 {
		return agent.Kind(id)
	}
	return agent.Kind(p.KindOf(p.Providers[0]))
}

// provLabel is provider id as you'd call it: whose models, and how
// they're paid for where that's worth saying (Anthropic · subscription,
// DeepSeek · API key, GitHub Copilot).
func provLabel(id string) string {
	if b := billWord(id); b != "" {
		return provName(id) + " · " + b
	}
	return provName(id)
}

// provName is whose models provider id serves: GitHub Copilot, where
// the company isn't the agent's name, else the one that is.
func provName(id string) string {
	p, _ := agent.Billed(id)
	name := agent.ProviderLabel(p)
	if !agent.Split(p) && agent.KeyEnv(p) == "" && name != agentName(p) {
		if strings.HasPrefix(agentName(p), name+" ") {
			return agentName(p)
		}
		return name + " " + agentName(p)
	}
	return name
}

// billWord is how provider id is paid for, where it's one of two ways or
// only by key.
func billWord(id string) string {
	p, _ := agent.Billed(id)
	switch {
	case paysByKey(id):
		return "API key"
	case agent.Split(p):
		return "subscription"
	}
	return ""
}

// paysByKey is whether provider id is paid for with an API key alone.
func paysByKey(id string) bool {
	p, key := agent.Billed(id)
	return key || !agent.Split(p) && agent.KeyEnv(p) != ""
}

// spentToday is what provider id's sessions spent today, in every
// harness it runs in: a split provider's own program's are its
// subscription's.
func (m *Model) spentToday(id string) float64 {
	p, key := agent.Billed(id)
	sum := 0.0
	for _, k := range agent.RunsFor(id) {
		if !key || k != agent.Kind(p) {
			sum += m.useOf(k).spentToday
		}
	}
	return sum
}

// provUse is how an agent is being used: the account it's on, and what
// its sessions run and cost.
type provUse struct {
	inUse          acctRow // the account in use, or the agent itself
	accts, out     int     // accounts kept, and how many are nearly out
	running, today int     // sessions running, and active today
	spentToday     float64
	spentWeek      float64 // by sessions active in the last 7 days
}

func (m *Model) useOf(k agent.Kind) provUse {
	var u provUse
	for _, r := range m.accountRows() {
		switch {
		case r.kind != k:
		case r.head:
			if u.inUse.kind == "" {
				u.inUse = r
			}
		default:
			u.accts++
			if nearlyOut(r.q) {
				u.out++
			}
			if r.current {
				u.inUse = r
			}
		}
	}
	now := m.snap.At
	for _, a := range m.snap.Agents {
		if a.Kind != string(k) {
			continue
		}
		if a.Live() {
			u.running++
		}
		if a.Spend.Today > 0 || sameDay(a.Spend.Last, now) {
			u.today++
		}
		u.spentToday += a.Spend.Today
		if now.Sub(a.Spend.Last) < 7*24*time.Hour {
			u.spentWeek += a.Spend.Cost
		}
	}
	return u
}

func sameDay(a, b time.Time) bool {
	ay, am, ad := a.Local().Date()
	by, bm, bd := b.Local().Date()
	return ay == by && am == bm && ad == bd
}

func (m *Model) providersKey(s string) tea.Cmd {
	d := m.dialog
	if d.inside {
		secs := m.provForm(m.provPicked())
		if s == "left" || s == "h" {
			// Back out, as esc: going back never changes a setting; → and space do.
			d.inside, d.cursor = false, d.pick
			return nil
		}
		return m.formKey(secs, s)
	}
	items, it := m.provItems(), m.provPicked()
	if n := int(s[0] - '0'); len(s) == 1 && n >= 1 && n <= 9 {
		if n <= len(items) && items[n-1].provider != "" {
			d.cursor = n - 1
		}
		return nil
	}
	cfg := &m.store.Config
	switch s {
	case "enter", "right", "l":
		switch it.add {
		case "profile":
			m.newProfile()
		case "folder":
			m.addFolder("")
		default:
			d.pick, d.cursor, d.inside = d.cursor, 0, true
			if it.provider != "" {
				return m.loadModels(string(m.provKind(it.provider))) // for its Models, off the UI
			}
		}
	case "n":
		if d.page != pageProfiles {
			m.setSettingsPage(pageProfiles)
		}
		m.newProfile()
	case "*", "space":
		if name := it.provider + it.profile; name != "" {
			m.makeDefaultProfile(name)
		}
	case "$":
		if it.provider != "" && paysByKey(it.provider) {
			pr, _ := agent.Billed(it.provider)
			return m.askAPIKey(pr)
		}
	case "a":
		switch {
		case it.provider != "" && !paysByKey(it.provider):
			return m.addAccount(m.provKind(it.provider))
		case it.provider == "":
			m.addFolder(it.profile)
		}
	case "r":
		if it.profile != "" {
			m.renameProfile(it.profile)
			return nil
		}
		m.flash("reading every provider's limits again…", false)
		return tea.Batch(m.fetchUsage(), m.fetchQuotas())
	case "x", "d":
		switch {
		case it.profile != "":
			m.confirmThen("Delete the preset "+it.profile+"? Its folders go back to the default.", func() tea.Cmd {
				cfg.DeleteProfile(it.profile)
				_ = m.store.SaveConfig()
				return nil
			})
		case it.folder != "":
			cfg.SetRule(it.folder, "")
			_ = m.store.SaveConfig()
		case it.provider != "":
			if p, _ := cfg.ProfileNamed(it.provider); !p.Builtin {
				cfg.DeleteProfile(it.provider)
				_ = m.store.SaveConfig()
				m.flash(provLabel(it.provider)+"'s own preset is as it was", false)
			}
		}
	}
	return nil
}

// listKeys are the keys of the list's line it.
func (m *Model) listKeys(it provItem) []string {
	switch {
	case it.provider != "":
		keys := []string{"enter", "open", "1-9", "provider", "*", "make default", "a", "add account", "r", "read limits", "n", "new preset"}
		if paysByKey(it.provider) {
			keys = append(keys[:6], "$", "API key", "n", "new preset")
		}
		if p, _ := m.store.Config.ProfileNamed(it.provider); !p.Builtin {
			keys = append(keys, "x", "put its preset back")
		}
		return keys
	case it.profile != "":
		return []string{"enter", "open", "*", "make default", "r", "rename", "d", "delete", "n", "new preset"}
	case it.folder != "":
		return []string{"enter", "open", "x", "remove"}
	}
	return []string{"enter", "add"}
}

func (m *Model) providersBody(w int) []string {
	d := m.dialog
	items := m.provItems()
	var catalog []section
	for _, it := range items {
		group, label := "Providers", provLabel(it.provider)
		if it.provider != "" {
			pr, _ := agent.Billed(it.provider)
			label = glyph(agent.Kind(pr)) + " " + label
		}
		switch {
		case it.profile != "":
			group, label = "Presets", m.profileMark(it.profile)+it.profile
		case it.folder != "":
			group, label = "Folders", tildify(it.folder)
		case it.add == "profile":
			group, label = "Presets", "+ New preset"
		case it.add == "folder":
			group, label = "Folders", "+ Add folder"
		}
		if len(catalog) == 0 || catalog[len(catalog)-1].title != group {
			catalog = append(catalog, section{title: group})
		}
		catalog[len(catalog)-1].rows = append(catalog[len(catalog)-1].rows, setting{label: label})
	}
	picked := d.cursor
	if d.inside {
		picked = d.pick
	}
	picked = max(0, min(picked, len(items)-1))
	it := items[picked]
	return m.catalogBody(catalog, picked, d.inside, true, w, func(dw int) ([]string, int) {
		secs := m.provForm(it)
		cur := -1
		if d.inside {
			cur = d.cursor
		}
		head := m.provHead(it, dw)
		rows := append(head, m.formRows(secs, cur, dw)...)
		cursor := m.frameCursor(rows)
		indices := formLineIndices(secs, len(head), len(rows))
		if d.inside {
			rows = append(rows, m.about(rowAt(secs, cur), dw)...)
		}
		if it.provider != "" {
			rows = append(rows, m.canDo(it.provider, dw)...)
		}
		for len(indices) < len(rows) {
			indices = append(indices, -1)
		}
		d.catalogGeometry.allDetails = indices
		return rows, cursor
	})
}

// provTotals is every provider's spend and sessions summed, and which
// provider new sessions run for now, when it isn't the first.
func (m *Model) provTotals() string {
	var u provUse
	for _, ad := range m.agentOrder() {
		v := m.useOf(ad.Kind())
		u.running += v.running
		u.today += v.today
		u.spentToday += v.spentToday
		u.spentWeek += v.spentWeek
	}
	s := paint(cText, money(u.spentToday)) + dim(" today") + faint(" · ") +
		paint(cText, money(u.spentWeek)) + dim(" this week") + faint(" · ") +
		paint(cText, fmt.Sprint(u.running)) + dim(" running") + faint(" · ") +
		paint(cText, fmt.Sprint(u.today)) + dim(" sessions today")
	if sp := m.accts.spill; sp != "" {
		s += faint("   ·   ") + paint(cYellow, "→ ") + dim("new sessions run ") + paint(cText, agentName(sp)) + dim(" for now: the first's accounts are nearly out")
	}
	return s
}

// provHead is what's above the form of the line picked, w wide.
func (m *Model) provHead(it provItem, w int) []string {
	cfg := &m.store.Config
	para := func(s string) []string {
		var out []string
		for _, l := range wrap(s, w) {
			out = append(out, dim(l))
		}
		return out
	}
	switch {
	case it.provider != "":
		return m.providerHead(it.provider, w)
	case it.profile != "":
		if p, ok := cfg.ProfileNamed(it.profile); ok {
			return m.profileHead(p)
		}
	case it.folder != "":
		return para("Sessions started in " + tildify(state.ExpandHome(it.folder)) + ", or a folder inside it, get this preset unless you pick another for them. The longest folder that matches wins.")
	case it.add == "profile":
		return append(para("A preset groups providers for some of your work: Claude then Codex for one client, say, or Ollama in Pi for another, and chooses what happens when they run low. Every provider is a preset of its own already."),
			append([]string{""}, para("A new session gets the preset you pick for it (#preset name), else its folder's, else the default ★. enter names a new one; it starts with the default's providers, for you to change.")...)...)
	case it.add == "folder":
		return para("Gives a folder a preset, so every session started in it gets that one: a client's repositories on its own account's agent, say. enter takes the selected session's folder, or one you type, and gives it the default; open it to give it another.")
	}
	return nil
}

// provForm is the form of the line picked.
func (m *Model) provForm(it provItem) []section {
	cfg := &m.store.Config
	switch {
	case it.provider != "":
		return m.providerForm(it.provider)
	case it.profile != "":
		if p, ok := cfg.ProfileNamed(it.profile); ok {
			return m.profileForm(p)
		}
	case it.folder != "":
		if r, ok := cfg.RuleFor(state.ExpandHome(it.folder)); ok {
			return []section{{title: "Folder", rows: []setting{m.folderRow(r.Path, r.Profile)}}}
		}
	}
	return nil
}

// providerHead is provider id's name, whether it can run, the account
// or key it's paid with, and its limits.
func (m *Model) providerHead(id string, w int) []string {
	cfg := &m.store.Config
	pr, key := agent.Billed(id)
	k := m.provKind(id)
	u := m.useOf(k)
	label := func(s string) string { return dim(fit(s, 10)) }
	name := glyph(agent.Kind(pr)) + " " + paint(cText+bold, provLabel(id))
	if strings.EqualFold(id, cfg.Default().Name) {
		name += paint(cOrange, "  ★ the default")
	}
	out := []string{name}
	if hint := agent.Hint(k); hint != "" && !agent.Runs(k) {
		out = append(out, label("can't run")+faint(hint))
	}
	if paysByKey(id) {
		if cfg.HasAPIKey(pr) {
			out = append(out, label("API key")+paint(cGreen, "✓ kept")+faint(" · $ changes it"))
		} else {
			out = append(out, label("API key")+paint(cYellow, "none yet")+faint(" · $ adds one"))
		}
	}
	if key {
		return out // no account or limits: every token is paid for
	}
	switch why := m.accts.why[string(k)]; {
	case why != "":
		out = append(out, label("account")+paint(cRed, "✗ not signed in")+dim(" · "+why))
	case !switches(k) && u.inUse.q.Email != "":
		out = append(out, label("account")+dim(u.inUse.q.Email))
	}
	q := u.inUse.q
	if len(q.Windows) == 0 && q.Balance == "" && q.Problem == "" {
		out = append(out, label("limits")+m.limits(u.inUse, w-10, 0))
	}
	return append(out, m.windowLines(q, m.snap.At, label)...)
}

// canDo is what rush can do with provider id in its default harness,
// feature by feature, and what the model it starts with takes.
func (m *Model) canDo(id string, w int) []string {
	k := m.provKind(id)
	model := m.startOn(id, string(k)).model
	if p, ok := m.store.Config.ProfileNamed(id); ok && p.Model != "" {
		model = p.Model
	}
	_, key := agent.Billed(id)
	out := []string{"", rule("What it can do", "in "+agent.HarnessLabel(k)+" · "+agent.LevelOf(k).String(), w)}
	out = append(out, featureGrid(k, key, w)...)
	if model != "" {
		out = append(out, "  "+paint(cText, model)+"  "+modelTakes(k, model))
	}
	if models := m.agentModels(k); len(models) > 0 {
		out = append(out, "", rule("Models", "context · price per million tokens in / out", w))
		for _, c := range models {
			out = append(out, modelLine(k, c, w))
		}
	}
	return out
}

// modelLine is one model agent k offers: its name, context window and
// price, each where rush knows it, then what it's for.
func modelLine(k agent.Kind, c agent.Choice, w int) string {
	var facts []string
	n := c.Context
	if windower, ok := agent.As[agent.ContextWindower](k); ok && n == 0 {
		n = windower.ContextWindow(c.ID)
	}
	if n > 0 {
		facts = append(facts, tokens(n))
	}
	if pr, ok := agent.As[agent.Pricer](k); ok {
		in, okIn := pr.Cost(c.ID, usage.TokenUsage{Input: 1_000_000})
		out, okOut := pr.Cost(c.ID, usage.TokenUsage{Output: 1_000_000})
		switch {
		case okIn && okOut && in == 0 && out == 0:
			facts = append(facts, "free")
		case okIn && okOut:
			facts = append(facts, fmt.Sprintf("$%g / $%g", in, out))
		}
	}
	line := "  " + paint(cText, fit(c.ID, 24)) + " " + dim(fit(strings.Join(facts, " · "), 22))
	if room := w - 50; room > 12 && c.Note != "" {
		line += " " + faint(ansi.Truncate(c.Note, room, "…"))
	}
	return line
}

// featureGrid is what rush can do with agent k, feature by feature, in as
// many columns as fit: lit where it can, faint where it can't, ◌ where
// it's planned. What's said of the ones it can follows, a line each.
// Paid with a key, it has no accounts to switch, sign in to or read.
func featureGrid(k agent.Kind, key bool, w int) []string {
	const cell = 24
	cols := max(1, (w-2)/cell)
	all := agent.AllFeatures()
	rows := (len(all) + cols - 1) / cols
	var out, notes []string
	for r := range rows {
		line := "  "
		for c := range cols {
			i := c*rows + r // down the columns, so related features stay together
			if i >= len(all) {
				break
			}
			f := all[i]
			st := agent.FeatureOf(k, f.Feature)
			if key && slices.Contains([]agent.Feature{agent.FeatureSwitch, agent.FeatureSignIn, agent.FeatureQuota}, f.Feature) {
				st = agent.No
			}
			switch st.Is {
			case agent.StateYes:
				line += paint(cGreen, "✓ ") + paint(cText, fit(f.Label, cell-2))
			case agent.StatePlanned:
				line += paint(cYellow, "◌ ") + dim(fit(f.Label, cell-2))
			default:
				line += faint("– " + fit(f.Label, cell-2))
			}
			if st.Note != "" && st.Is == agent.StateYes {
				notes = append(notes, f.Label+": "+st.Note)
			}
		}
		out = append(out, line)
	}
	for _, n := range notes {
		out = append(out, "  "+faint(n))
	}
	return out
}

// providerForm is provider id's accounts, the harnesses it runs in, what
// its sessions start with in the default one, what it does at a limit
// and its folders, then what its default harness adds.
func (m *Model) providerForm(id string) []section {
	cfg := &m.store.Config
	k := m.provKind(id)
	p, _ := cfg.ProfileNamed(id)
	var secs []section
	if acct, ok := m.accountSection(k); ok && !paysByKey(id) {
		secs = append(secs, acct)
	}
	if auth, ok := m.nativeAuthSection(k); ok {
		secs = append(secs, auth)
	}
	secs = append(secs, m.harnessSection(id))
	st := m.startSection(id, k)
	if pr, _ := agent.Billed(id); pr == "ollama" {
		st.rows = append(st.rows, m.ollamaModelSettings(m.startOn(id, string(k)).model)...)
		secs = append(secs, st, m.decisionSection())
	} else {
		secs = append(secs, st)
	}
	own := m.profileForm(p) // at a limit, and its folders
	if _, key := agent.Billed(id); key {
		own = own[1:] // a key has no limit to reach
	}
	secs = append(secs, own...)
	return append(secs, m.agentSections(k)[1:]...)
}

// harnessSection is every harness provider id runs in, ★ the one new
// sessions start in; Harnesses says why the rest can't.
func (m *Model) harnessSection(id string) section {
	cfg := &m.store.Config
	sec := section{title: "Harnesses", note: "where its sessions can run; ★ is where new ones start"}
	def := agent.HarnessOf(m.provKind(id))
	for _, h := range agent.HarnessKinds() {
		k, warn, why := agent.Compat(id, h)
		if k == "" {
			continue
		}
		hn := agent.HarnessLabel(h)
		sec.rows = append(sec.rows, setting{
			label: hn,
			line: func(int) string {
				mark, now := "  ", faint("enter makes it the default")
				switch {
				case why != "":
					now = faint(why)
				case h == def:
					mark, now = paint(cOrange, "★ "), dim("the default")
				case warn != "":
					now = paint(cYellow, "⚠ ") + dim(warn)
				}
				return mark + glyph(h) + " " + paint(cText, fit(hn, 14)) + levelChip(k) + " " + now
			},
			key: func(s string) (tea.Cmd, bool) {
				if s != "*" && s != "enter" && s != "space" {
					return nil, false
				}
				if why == "" && h != def {
					cfg.SetRunsIn(id, string(h))
					_ = m.store.SaveConfig()
				}
				return nil, true
			},
			keys: []string{"enter", "make default"},
			about: func() (string, string, string) {
				what := provLabel(id) + " in " + hn + ": " + levelWords[agent.LevelOf(k)] + ". #new " + agent.HarnessWord(h) + "@" + agent.ProviderWord(id) + " starts one here once; new ones start in the default, ★."
				switch {
				case why != "":
					return hn, what, why + "."
				case h == def:
					return hn, what, "The default: new " + provLabel(id) + " sessions start here."
				case warn != "":
					return hn, what, "It " + warn + ", on that harness's own sign-in. enter makes it the default."
				}
				return hn, what, "enter makes it the default."
			},
		})
	}
	return sec
}

// accountSection is agent k's accounts, the one in use marked; none when
// rush can't switch it.
func (m *Model) accountSection(k agent.Kind) (section, bool) {
	if !switches(k) {
		return section{}, false
	}
	sec := section{title: "Account", note: "one at a time, shared by all its sessions"}
	rows := accountsOf(m.accountRows(), k)
	if why := m.accts.why[string(k)]; why != "" {
		// Signed in as no one: its sessions fail until it is again.
		sec.rows = append(sec.rows, setting{
			label: "not signed in",
			line: func(w int) string {
				return paint(cRed, "✗ "+agentName(string(k))+" is signed out") + dim(" · l signs it in")
			},
			key: func(s string) (tea.Cmd, bool) {
				if s == "l" || s == "enter" {
					return m.addAccount(k), true
				}
				return nil, false
			},
			keys: []string{"l", "sign in again"},
			about: func() (string, string, string) {
				return "Not signed in", agentName(string(k)) + " isn't signed in, so its sessions fail to start or stop at their first request.", why
			},
		})
	}
	for i := range rows {
		sec.rows = append(sec.rows, m.accountRow(rows[i]))
	}
	if all, n := together(rows); n > 1 {
		sec.rows = append(sec.rows, m.togetherRow(all, n))
	}
	if chain := fleet.Chain(m.snap.Logins, m.snap.At); k == loginsKind && len(chain) > 1 {
		sec.rows = append(sec.rows, m.chainRow(chain))
	}
	sec.rows = append(sec.rows, setting{
		label: "+ add an account",
		line:  func(int) string { return faint("+ add an account") },
		key: func(s string) (tea.Cmd, bool) {
			if s == "enter" || s == "right" || s == "a" {
				return m.addAccount(k), true
			}
			return nil, false
		},
		keys: []string{"enter", "sign in"},
		about: func() (string, string, string) {
			return "Add an account", "Signs " + agentName(string(k)) + " in to another account, for rush to keep and switch to when one runs low.", ""
		},
	})
	return sec, true
}

// accountRow is one of an agent's accounts: enter switches to it.
func (m *Model) accountRow(r acctRow) setting {
	keys := []string{"enter", "switch to", "a", "add", "r", "rename", "l", "sign in again", "d", "forget"}
	if r.login != nil && expired(r.q) {
		keys[5] = "refresh the sign-in"
	}
	if rs := r.q.Resets; rs != nil && rs.Available > 0 {
		keys = append(keys, "u", "use a reset")
	}
	return setting{
		label: r.name(),
		line: func(w int) string {
			mark := faint("○ ")
			if r.current {
				mark = paint(cOrange, "● ")
			}
			use := m.limits(r, 26, 26) + resetsChip(r.q)
			if m.accts.why[string(r.kind)] != "" && !r.current {
				// Its usage reads stale only because the agent is signed out.
				use = paint(cYellow, "! ") + dim("signed out · l signs back in to it")
			}
			return mark + paint(cText, fit(r.name(), 16)) + dim(fit(r.email(), max(0, min(28, w-72)))) + use
		},
		key: func(s string) (tea.Cmd, bool) {
			switch {
			case s == "u":
				return m.useReset(&r), true
			case s == "a":
				return m.addAccount(r.kind), true
			case r.login != nil && slices.Contains([]string{"enter", "r", "l", "d", "x"}, s):
				return m.loginKey(*r.login, s), true
			case r.login == nil && slices.Contains([]string{"enter", "r", "l", "d", "x"}, s):
				return m.signInKey(r, s), true
			}
			return nil, false
		},
		keys: keys,
		about: func() (string, string, string) {
			now := "Kept by rush: enter switches to it."
			if r.current {
				now = "In use: new " + agentName(string(r.kind)) + " sessions run on it."
			}
			if resets := resetsText(r.q, m.snap.At); resets != "" {
				now += "\n" + resets
			}
			now += resetCredits(r.q, m.snap.At)
			return r.name(), acctWho(r), now
		},
	}
}

// resetsText is when each of a reading's windows resets, by the clock
// and how long from now: "5h resets 17:00, in 3h · 7d resets Thu 09:00,
// in 2d".
func resetsText(q usage.Quota, now time.Time) string {
	var out []string
	for i := range q.Windows {
		win := &q.Windows[i]
		if !win.ResetsAt.After(now) {
			continue
		}
		when := win.ResetsAt.Local().Format("15:04")
		if win.ResetsAt.Sub(now) > 20*time.Hour {
			when = win.ResetsAt.Local().Format("Mon 15:04")
		}
		out = append(out, win.Label+" resets "+when+", in "+roughly(win.ResetsAt.Sub(now)))
	}
	return strings.Join(out, " · ")
}

// together is an agent's accounts as one: each window's use averaged
// over the accounts with a reading of it, resetting when the first of
// them does; and how many accounts had a reading.
func together(rows []acctRow) (usage.Quota, int) {
	var all usage.Quota
	count := map[string]int{}
	n := 0
	for k := range rows {
		r := &rows[k]
		if len(r.q.Windows) == 0 {
			continue
		}
		n++
		if r.q.FetchedAt.After(all.FetchedAt) {
			all.FetchedAt = r.q.FetchedAt
		}
		for j := range r.q.Windows {
			win := &r.q.Windows[j]
			i := slices.IndexFunc(all.Windows, func(w usage.Window) bool { return w.Label == win.Label })
			if i < 0 {
				all.Windows = append(all.Windows, usage.Window{Label: win.Label, Name: win.Name, ResetsAt: win.ResetsAt})
				i = len(all.Windows) - 1
			}
			a := &all.Windows[i]
			a.Percent += win.Percent // summed here, averaged below
			if a.ResetsAt.IsZero() || !win.ResetsAt.IsZero() && win.ResetsAt.Before(a.ResetsAt) {
				a.ResetsAt = win.ResetsAt
			}
			count[win.Label]++
		}
	}
	for i := range all.Windows {
		all.Windows[i].Percent /= float64(count[all.Windows[i].Label])
	}
	for i := range rows {
		if rs := rows[i].q.Resets; rs != nil {
			if all.Resets == nil {
				all.Resets = &usage.Resets{}
			}
			all.Resets.Available += rs.Available
		}
	}
	return all, n
}

// togetherRow is an agent's accounts added up: how much room they have
// between them before rush has nowhere left to switch to.
func (m *Model) togetherRow(all usage.Quota, n int) setting {
	return setting{
		label: "together",
		line: func(w int) string {
			return faint("Σ ") + paint(cSub, fit("together", 16)) + dim(fit(fmt.Sprintf("%d accounts", n), max(0, min(28, w-72)))) + m.limits(acctRow{q: all}, 26, 26) + resetsChip(all)
		},
		key: func(string) (tea.Cmd, bool) { return nil, false },
		about: func() (string, string, string) {
			left := make([]string, 0, len(all.Windows))
			for i := range all.Windows {
				left = append(left, fmt.Sprintf("%s: %.1f accounts' worth left", all.Windows[i].Label, float64(n)*(100-all.Windows[i].Percent)/100))
			}
			now := strings.Join(left, " · ")
			if resets := resetsText(all, m.snap.At); resets != "" {
				now += "\nSoonest: " + resets
			}
			if rs := all.Resets; rs != nil && rs.Available > 0 {
				now += fmt.Sprintf("\n↺ %d limit reset%s earned between them: open an account to use one", rs.Available, plural(rs.Available))
			}
			return "All accounts together", fmt.Sprintf("Each limit's use averaged over the %d accounts with a reading, so 50%% means half their combined room is left. It resets when the first of them does.", n), now
		},
	}
}

// chainRow is the order rush spends the logins in: each with its week's
// room and when that's lost, or how long until it has room again.
func (m *Model) chainRow(chain []fleet.Link) setting {
	node := func(k fleet.Link, plain bool) string {
		w, _ := fleet.Week(k.Quota)
		left := fmt.Sprintf("%.0f%% · %s", 100-w.Percent, roughly(fleet.Left(w, m.snap.At)))
		name := paint(cText, k.Name)
		if k.Current {
			name = paint(cOrange, k.Name)
		}
		if plain {
			name = k.Name
		}
		if k.Wait > 0 {
			return dim("⏸"+roughly(k.Wait)+" ") + name + " " + faint(left)
		}
		return name + " " + dim(left)
	}
	return setting{
		label: "switch order",
		line: func(w int) string {
			parts := make([]string, len(chain))
			for i, k := range chain {
				parts[i] = node(k, false)
			}
			return faint("⇢ ") + paint(cSub, fit("switch order", 16)) + fit(strings.Join(parts, faint(" ▸ ")), max(0, w-18))
		},
		key: func(string) (tea.Cmd, bool) { return nil, false },
		about: func() (string, string, string) {
			var lines []string
			for i, k := range chain {
				lines = append(lines, fmt.Sprintf("%d. %s", i+1, node(k, true)))
			}
			return "Switch order", "Room an account doesn't use before its week resets is lost, so rush spends first whichever would lose the most soonest: its week's room left (%) over the time until it resets. It moves on when the one in use is nearly out, or early when another would lose twice as fast. ⏸ is how long until a full one has room again.", strings.Join(lines, "\n")
		},
	}
}

// acctWho is who an account is: its email, organisation, role and plan,
// as far as they're known.
func acctWho(r acctRow) string {
	var who []string
	add := func(vs ...string) {
		for _, v := range vs {
			if v = strings.ReplaceAll(v, "_", " "); v != "" && !slices.Contains(who, v) {
				who = append(who, v)
			}
		}
	}
	if r.login != nil {
		u := r.login.Usage
		add(r.login.Email, u.Org, u.Role, u.Plan, u.Billing)
		if u.Extra {
			add("extra usage on")
		}
	} else {
		add(r.email(), firstNonEmpty(r.q.Plan, r.acct.Plan))
	}
	if len(who) == 0 {
		return "Who it is isn't known yet."
	}
	return strings.Join(who, " · ")
}

// askAPIKey asks for provider p's API key, which pays for its models per
// token in any harness that speaks its API, and picks "API key" over the
// subscription where a session can be either. "none" forgets it.
func (m *Model) askAPIKey(p string) tea.Cmd {
	name := agent.ProviderLabel(p)
	if agent.KeyEnv(p) == "" {
		m.flash(name+" takes no API key", true)
		return nil
	}
	what := name + " API key"
	if m.store.Config.HasAPIKey(p) {
		what += " (none forgets it)"
	}
	m.askSecret(what, func(v string) tea.Cmd {
		if v == "none" {
			v = ""
		}
		return later(func() error { return state.PutAPIKey(p, v) }, func(m *Model, err error) tea.Cmd {
			if err != nil {
				m.flash(err.Error(), true)
				return nil
			}
			m.store.Config.MarkAPIKey(p, v != "")
			_ = m.store.SaveConfig()
			if v == "" {
				m.flash(name+"'s API key is forgotten", false)
			} else {
				m.flash(name+"'s API key is kept in the keychain · #use picks it per session", false)
			}
			return nil
		})
	})
	return nil
}

// resetsChip is how many limit resets an account has earned, when any.
func resetsChip(q usage.Quota) string {
	if q.Resets == nil || q.Resets.Available == 0 {
		return ""
	}
	return "  " + paint(cGreen, fmt.Sprintf("↺ %d", q.Resets.Available))
}

// resetCredits says an account's earned resets, each with what it does
// and when it runs out, for its about: "" when it has none.
func resetCredits(q usage.Quota, now time.Time) string {
	rs := q.Resets
	if rs == nil || rs.Available == 0 {
		return ""
	}
	var b strings.Builder
	fmt.Fprintf(&b, "\n↺ %d limit reset%s earned: each resets its limits at once · u uses one", rs.Available, plural(rs.Available))
	for _, c := range rs.Credits {
		b.WriteString("\n  • " + firstNonEmpty(c.Title, "a limit reset"))
		if c.About != "" {
			b.WriteString(": " + c.About)
		}
		if !c.Expires.IsZero() {
			b.WriteString(" · runs out " + c.Expires.Local().Format("Mon 2 Jan") + ", in " + roughly(c.Expires.Sub(now)))
		}
	}
	return b.String()
}

// useReset spends one of an account's earned resets, once you say so. A
// reset goes to the account its agent is signed in to, so another one is
// switched to first.
func (m *Model) useReset(r *acctRow) tea.Cmd {
	rs := r.q.Resets
	ad, _ := agent.Get(r.kind)
	sp, ok := ad.(agent.ResetSpender)
	switch {
	case rs == nil || rs.Available == 0:
		m.flash(r.name()+" has no limit resets", false)
		return nil
	case !ok:
		m.flash(agentName(string(r.kind))+" can't spend resets through rush", true)
		return nil
	case !r.current:
		m.flash("a reset goes to the account in use: enter switches to "+r.name()+", then u", false)
		return nil
	}
	p, found := m.profileOf(ad)
	if !found {
		return nil
	}
	left := rs.Available - 1
	m.confirmThen(fmt.Sprintf("Use one of %s's %d limit resets now? Its limits go back to empty; %d left after.", r.name(), rs.Available, left), func() tea.Cmd {
		m.flash("using a reset on "+r.name()+"…", false)
		src, _ := ad.(agent.QuotaSource)
		return later(func() resetDone {
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()
			var d resetDone
			if d.words, d.err = sp.UseReset(ctx, p, ""); src == nil {
				return d
			}
			// Read again at once, so the limits shown are the reset ones.
			if q, err := src.Quota(ctx, p, agent.Account{Kind: p.Kind}); err == nil {
				d.q = quotaMsg{p.Dir: q}
				_ = usage.Record(host.QuotasPath(), host.QuotaKey(p), q)
				if q.Account != "" {
					d.q[q.Account] = q
					_ = usage.Record(host.QuotasPath(), q.Account, q)
				}
			}
			return d
		}, func(m *Model, d resetDone) tea.Cmd {
			if d.err != nil {
				m.flash("couldn't use a reset on "+r.name()+": "+d.err.Error(), true)
			} else {
				m.flash(r.name()+": "+d.words, false)
			}
			return d.q.applyTo(m)
		})
	})
	return nil
}

// resetDone is what came of spending a reset, and the limits read after.
type resetDone struct {
	words string
	err   error
	q     quotaMsg
}
