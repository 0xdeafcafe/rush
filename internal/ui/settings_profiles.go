package ui

import (
	"cmp"
	"fmt"
	"slices"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A profile is providers and a policy, not accounts: each provider is
// signed in to one account at a time, shared by all its sessions, and
// rush moves it between them itself. Every installed provider is a
// profile of its own; yours group several. On Providers, a provider's
// page has its own profile's rows, a profile of yours its providers and
// their order too.

// changeProfile edits the profile called name and saves it. A provider's
// own, changed, is kept as one of yours of its name, standing in for it.
func (m *Model) changeProfile(name string, f func(*state.Profile)) {
	cfg := &m.store.Config
	p, ok := cfg.ProfileNamed(name)
	if !ok {
		return
	}
	f(&p)
	cfg.SetProfile(name, p)
	_ = m.store.SaveConfig()
	m.spillTo()
}

// installedProviders are the providers with an agent installed here.
func installedProviders() []string {
	var out []string
	for _, pr := range agent.Providers() {
		if agent.ProviderInstalled(pr) {
			out = append(out, pr)
		}
	}
	return out
}

// ownProfile is whether name is an installed provider's own profile, as
// built in or as you changed it.
func ownProfile(name string) bool {
	return slices.ContainsFunc(state.IDs(), func(id string) bool { return strings.EqualFold(id, name) })
}

// runsInWords is " in Pi" when agent k is its provider in another's
// harness; empty when it's its own.
func runsInWords(k agent.Kind) string {
	if h := agent.HarnessOf(k); h != k {
		return " in " + agentName(string(h))
	}
	return ""
}

// profileHead is the head of one profile, open: its name, and where new
// sessions under it start now.
func (m *Model) profileHead(p state.Profile) []string {
	now := dim("New sessions under it start on ")
	if pick, ok := p.Pick(m.room()); ok {
		now += paint(cText, agentName(pick.Kind))
		if pick.Account.Name != "" {
			now += dim(" · ") + paint(cText, pick.Account.Name)
		}
		if pick.Wait {
			now += paint(cYellow, " and wait: every account is nearly out")
		}
	} else {
		now += paint(cYellow, "nothing: none of its providers is installed")
	}
	return []string{m.profileMark(p.Name) + paint(cText+bold, p.Name) + "  " + m.chain(p), now}
}

// profileMark is ★ for the default profile, room for it otherwise.
func (m *Model) profileMark(name string) string {
	if strings.EqualFold(name, m.store.Config.Default().Name) {
		return paint(cOrange, "★ ")
	}
	return "  "
}

// makeDefaultProfile makes the profile called name the default, and says so.
func (m *Model) makeDefaultProfile(name string) {
	m.store.Config.SetDefaultProfile(name)
	_ = m.store.SaveConfig()
	m.spillTo()
	if ownProfile(name) {
		name = provLabel(name)
	}
	m.flash(name+" is the default: new sessions get it unless you or a folder pick another", false)
}

// profileAgents are a profile's agents in words, in order.
func (m *Model) profileAgents(p state.Profile) string {
	inst := p.Installed()
	if len(inst) == 0 {
		return "none of its providers is installed"
	}
	var names []string
	for _, k := range inst {
		names = append(names, provLabel(p.ID(agent.ProviderOf(agent.Kind(k))))+runsInWords(agent.Kind(k)))
	}
	return strings.Join(names, " → ")
}

// newProfile makes a profile of the default's providers and policy, on
// the account new sessions start on now, named after its harness and
// that account (claudecode:alex) unless you name it otherwise.
func (m *Model) newProfile() {
	k := m.startKind()
	pick, _ := m.startPick(m.startDir())
	name := strings.ToLower(strings.ReplaceAll(agent.HarnessLabel(agent.Kind(k)), " ", ""))
	if a := m.accountOf(agent.Kind(k)); a != "" {
		name += ":" + strings.ToLower(strings.ReplaceAll(a, " ", "-"))
	}
	m.ask("name the new preset", name, func(v string) tea.Cmd {
		cfg := &m.store.Config
		for _, p := range cfg.AllProfiles() {
			if strings.EqualFold(p.Name, v) {
				m.flash("there's a preset called "+p.Name+" already", true)
				return nil
			}
		}
		if _, ok := agent.Get(agent.Kind(strings.ToLower(v))); ok {
			m.flash(v+" is an agent's name: #preset "+v+" runs it", true)
			return nil
		}
		d := cfg.Default()
		cfg.SetProfile("", state.Profile{Name: v, Providers: slices.Clone(d.Providers), Mix: d.Mix, OnLimit: d.OnLimit,
			Billing: d.Billing, Account: pick.Account.ID, Model: d.Model, Effort: d.Effort})
		_ = m.store.SaveConfig()
		m.openItem(provItem{profile: v})
		return nil
	})
}

func (m *Model) renameProfile(name string) {
	m.ask("rename "+name, name, func(v string) tea.Cmd {
		for _, p := range m.store.Config.AllProfiles() {
			if strings.EqualFold(p.Name, v) && !strings.EqualFold(v, name) {
				m.flash("there's a preset called "+p.Name+" already", true)
				return nil
			}
		}
		if _, ok := agent.Get(agent.Kind(strings.ToLower(v))); ok && !strings.EqualFold(v, name) {
			m.flash(v+" is an agent's name: #preset "+v+" runs it", true)
			return nil
		}
		m.changeProfile(name, func(p *state.Profile) { p.Name = v })
		return nil
	})
}

// profileForm is one profile, open. One of yours has its providers in
// order, where each runs and what it starts with; any has what it does
// when they run low and the folders that pick it. A provider's own keeps
// its harnesses and default on the provider's page, so has only those.
func (m *Model) profileForm(p state.Profile) []section {
	name := p.Name
	cfg := &m.store.Config
	change := func(f func(*state.Profile)) { m.changeProfile(name, f) }
	own := ownProfile(name)
	var first []section
	if !own {
		first = append(first, m.profileProviders(p, change), m.profileStart(p, change))
	}

	limit := choiceSetting("When a limit stops a session", p.Limit(),
		"What a running conversation does when a usage limit stops it.",
		[][2]string{
			{state.LimitAccount, "rush moves its provider to another account with room, and the conversation carries on."},
			{state.LimitHandoff, "another account first; when none has room, the conversation is handed to the next provider in the list, with what it was doing."},
			{state.LimitWait, "it waits for the limit to reset."},
		}, func(v string) { change(func(p *state.Profile) { p.OnLimit = v }) })
	limit.names = map[string]string{state.LimitAccount: "switch account", state.LimitHandoff: "hand off", state.LimitWait: "wait"}
	low := []setting{limit}
	if len(p.Providers) > 1 {
		mix := choiceSetting("When the first is out", firstNonEmpty(p.Mix, state.MixStay),
			"What new sessions do once every account of the first provider is nearly out. Running sessions stay where they are.",
			[][2]string{
				{state.MixStay, "new sessions still start on the first provider, and wait for its limits to reset."},
				{state.MixMix, "new sessions start on the next provider in the list with room, until the first's accounts reset."},
			}, func(v string) { change(func(p *state.Profile) { p.Mix = v }) })
		mix.names = map[string]string{state.MixStay: "wait for it", state.MixMix: "move on"}
		low = append([]setting{mix}, low...)
	}

	if own {
		return []section{{title: "When accounts run low", rows: low}, m.folderSection(name)}
	}
	isDef := strings.EqualFold(cfg.Default().Name, name)
	def := choiceSetting("Default", map[bool]string{true: "yes", false: "no"}[isDef],
		"Whether a new session gets this preset when you haven't picked one and its folder has none.",
		[][2]string{{"yes", "it's the default."}, {"no", "sessions get it when you pick it (#preset " + name + ") or its folder does."}},
		func(v string) {
			if v == "yes" {
				cfg.SetDefaultProfile(name)
				m.spillTo()
			}
		})
	rename := setting{label: "Name", value: name, typed: true, what: "What it's called, for #preset and the top bar.",
		run: func(string) tea.Cmd { return nil }}
	rename.key = func(s string) (tea.Cmd, bool) {
		if s == "enter" || s == "right" {
			m.renameProfile(name)
			return nil, true
		}
		return nil, false
	}
	this := []setting{rename, def}
	return append(first,
		section{title: "When accounts run low", rows: low},
		section{title: "This preset", rows: this},
		m.folderSection(name),
	)
}

// profileStart is what the sessions of one of your profiles start with
// on its first provider: how it's paid, the account, the model and the
// effort, each what Settings gives the provider until chosen here.
func (m *Model) profileStart(p state.Profile, change func(func(*state.Profile))) section {
	sec := section{title: "Starts with", note: "on its first provider; unset is what that provider starts with"}
	if len(p.Providers) == 0 {
		return sec
	}
	pr := p.Providers[0]
	k := agent.Kind(p.KindOf(pr))
	pairs := func(list []agent.Choice) [][2]string {
		out := [][2]string{{"", "as " + provLabel(p.ID(pr)) + " starts in " + agent.HarnessLabel(k) + "."}}
		for _, c := range list {
			out = append(out, [2]string{c.ID, c.Note})
		}
		return out
	}
	if agent.Split(pr) {
		st := choiceSetting("Paid with", p.Billing, "How its sessions are paid for: the subscription, or per token with "+agent.ProviderLabel(pr)+"'s API key.",
			[][2]string{{"", "the subscription " + agentName(pr) + " is signed in to."}, {state.BillingKey, "the API key, per token, in any harness you use it in."}},
			func(v string) { change(func(p *state.Profile) { p.Billing = v }) })
		st.unset, st.names = "subscription", map[string]string{state.BillingKey: "API key"}
		sec.rows = append(sec.rows, st)
	}
	if rows := accountsOf(m.accountRows(), k); len(rows) > 1 && p.Billing != state.BillingKey {
		choices := [][2]string{{"", "whichever of its accounts has the most room."}}
		names := map[string]string{}
		for _, r := range rows {
			choices = append(choices, [2]string{r.id(), "starts on " + r.name() + " while it has room."})
			names[r.id()] = r.name()
		}
		st := choiceSetting("Account", p.Account, "The account its sessions start on.", choices,
			func(v string) { change(func(p *state.Profile) { p.Account = v }) })
		st.unset, st.names = "most room", names
		sec.rows = append(sec.rows, st)
	}
	model := choiceSetting("Model", p.Model, "The model its sessions start with.", pairs(m.agentModels(k)),
		func(v string) { change(func(p *state.Profile) { p.Model = v }) })
	model.unset = "provider's"
	sec.rows = append(sec.rows, model)
	if ch, _ := agent.ChoicesOf(k); agent.Supports(k, agent.FeatureEffort) {
		effort := choiceSetting("Effort", p.Effort, "How hard its sessions think before acting.", pairs(ch.Efforts),
			func(v string) { change(func(p *state.Profile) { p.Effort = v }) })
		effort.unset, effort.typed = "provider's", len(ch.Efforts) == 0
		sec.rows = append(sec.rows, effort)
	}
	return sec
}

// profileProviders is a profile of yours' providers, in its order, then the
// installed ones it leaves out: enter takes one in or out, J and K move
// it, and h changes the harness it runs in under this profile.
func (m *Model) profileProviders(p state.Profile, change func(func(*state.Profile))) section {
	sec := section{title: "Providers", note: "new sessions start on the first; the rest are where they can move on to"}
	var provs []string
	for _, pr := range p.Providers {
		if agent.ProviderInstalled(agent.ProviderOf(agent.Kind(pr))) {
			provs = append(provs, pr)
		}
	}
	for _, pr := range installedProviders() {
		if !slices.Contains(provs, pr) {
			provs = append(provs, pr)
		}
	}
	used := 0
	for _, pr := range provs {
		in := slices.Contains(p.Providers, pr)
		sec.rows = append(sec.rows, m.providerRow(p, pr, in, used, change))
		if in {
			used++
		}
	}
	return sec
}

// providerRow is provider pr in profile p: number at+1 of it when in.
func (m *Model) providerRow(p state.Profile, pr string, in bool, at int, change func(func(*state.Profile))) setting {
	k := agent.Kind(p.KindOf(pr))
	many := len(agent.Harnesses(pr)) > 1
	keys := []string{"enter", "use or not", "J/K", "move"}
	if many && in {
		keys = append(keys, "h", "runs in")
	}
	return setting{
		label: agentName(pr),
		line: func(int) string {
			if !in {
				return faint("  ○ ") + glyph(agent.Kind(pr)) + " " + dim(fit(agentName(pr), 18)) + faint("not used")
			}
			n := paint(cOrange, strconv.Itoa(at+1))
			if !agent.Runs(k) {
				return n + faint(" ● ") + glyph(agent.Kind(pr)) + " " + paint(cText, fit(agentName(pr), 18)) + paint(cYellow, "rush can't start its sessions yet")
			}
			where := ""
			switch {
			case many && p.RunsIn[pr] != "":
				where = dim(runsInWords(k))
			case many:
				where = faint(runsInWords(k))
			}
			return n + paint(cOrange, " ● ") + glyph(agent.Kind(pr)) + " " + paint(cText, fit(agentName(pr), 18)) + where
		},
		key: func(s string) (tea.Cmd, bool) {
			switch s {
			case "enter", "space":
				change(func(p *state.Profile) {
					switch {
					case !slices.Contains(p.Providers, pr):
						p.Providers = append(p.Providers, pr)
					case len(p.Providers) > 1:
						p.Providers = slices.DeleteFunc(p.Providers, func(q string) bool { return q == pr })
						delete(p.RunsIn, pr)
					default:
						m.flash("a preset needs one provider", true)
					}
				})
			case "K", "shift+up", "J", "shift+down":
				d := map[string]int{"K": -1, "shift+up": -1, "J": 1, "shift+down": 1}[s]
				change(func(p *state.Profile) {
					i := slices.Index(p.Providers, pr)
					if j := i + d; i >= 0 && j >= 0 && j < len(p.Providers) {
						p.Providers[i], p.Providers[j] = p.Providers[j], p.Providers[i]
						m.dialog.cursor += d
					}
				})
			case "h":
				if !many || !in {
					return nil, false
				}
				m.nextHarness(p, pr, change)
			default:
				return nil, false
			}
			return nil, true
		},
		keys: keys,
		about: func() (string, string, string) {
			what := "The providers this preset's sessions run, in order. New sessions start on the first; the others are where they go when its accounts are out, if the preset moves on or hands off."
			if many {
				what += " " + agentName(pr) + " can run in more than one harness: h chooses which, for this preset."
			}
			if !in {
				return agentName(pr), what, "Not used: enter adds it at the end."
			}
			return agentName(pr), what, fmt.Sprintf("Number %d, running as %s. enter drops it; J and K move it.", at+1, agentName(string(k)))
		},
	}
}

// nextHarness moves provider pr in profile p on to its next harness: its
// usual one, then each other, then back to whatever its own profile says.
func (m *Model) nextHarness(p state.Profile, pr string, change func(func(*state.Profile))) {
	hs := agent.Harnesses(pr)
	opts := make([]string, 1, len(hs)+1)
	for _, hk := range hs {
		opts = append(opts, string(agent.HarnessOf(hk)))
	}
	next := opts[(slices.Index(opts, p.RunsIn[pr])+1)%len(opts)]
	change(func(p *state.Profile) {
		if next == "" {
			delete(p.RunsIn, pr)
			return
		}
		if p.RunsIn == nil {
			p.RunsIn = map[string]string{}
		}
		p.RunsIn[pr] = next
	})
	q, _ := m.store.Config.ProfileNamed(p.Name)
	where := agentName(q.KindOf(pr))
	if next == "" {
		where += ", as " + agentName(pr) + "'s own preset says"
	}
	m.flash(agentName(pr)+" runs as "+where+" under "+p.Name, false)
}

// folderSection is the folders that pick the profile called profile.
func (m *Model) folderSection(profile string) section {
	cfg := &m.store.Config
	sec := section{title: "Folders", note: "a session started in one, or a folder inside it, gets " + profile}
	for _, r := range cfg.FolderRules {
		if strings.EqualFold(r.Profile, profile) {
			sec.rows = append(sec.rows, m.folderRow(r.Path, r.Profile))
		}
	}
	sec.rows = append(sec.rows, setting{
		label: "+ add a folder",
		line:  func(int) string { return faint("+ add a folder") },
		key: func(s string) (tea.Cmd, bool) {
			if s == "enter" || s == "right" || s == "a" {
				m.addFolder(profile)
				return nil, true
			}
			return nil, false
		},
		keys: []string{"enter", "the selected session's folder, or type one"},
		about: func() (string, string, string) {
			return "Add a folder", "Gives a folder a preset, so every session started in it gets that one: a client's repositories on its own account's agent, say.", ""
		},
	})
	return sec
}

// folderRow is the folder rule for path: which profile it gives, which ←→
// change, and x to remove it.
func (m *Model) folderRow(path, to string) setting {
	cfg := &m.store.Config
	var choices [][2]string
	for _, it := range m.provItems() {
		if name := it.provider + it.profile; name != "" {
			p, _ := cfg.ProfileNamed(name)
			choices = append(choices, [2]string{name, m.profileAgents(p) + "."})
		}
	}
	where := tildify(state.ExpandHome(path))
	st := choiceSetting(where, to, "Sessions started here, or in a folder inside it, get this preset, unless you pick another for them. The longest folder that matches wins.",
		choices, func(v string) { cfg.SetRule(path, v) })
	st.key = func(s string) (tea.Cmd, bool) {
		if s != "x" && s != "d" && s != "backspace" {
			return nil, false
		}
		cfg.SetRule(path, "")
		_ = m.store.SaveConfig()
		m.dialog.cursor = max(0, m.dialog.cursor-1)
		return nil, true
	}
	st.keys = []string{"x", "remove"}
	return st
}

// addFolder asks for a folder, the selected session's to start with,
// and gives it profile, or the default.
func (m *Model) addFolder(profile string) {
	cfg := &m.store.Config
	dir := m.startDir()
	if a := m.selected(); a != nil && a.Cwd != "" {
		dir = a.Cwd
	}
	m.ask("folder", tildify(dir), func(v string) tea.Cmd {
		to := cmp.Or(profile, cfg.Default().Name)
		cfg.SetRule(v, to)
		_ = m.store.SaveConfig()
		m.flash(tildify(state.ExpandHome(v))+" gets "+to, false)
		return nil
	})
}
