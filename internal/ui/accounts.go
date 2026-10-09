package ui

import (
	"fmt"
	"os/exec"
	"reflect"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Every installed agent (a provider: Claude Code, Codex, Copilot…) can
// be signed in as one account at a time. Every agent runs from its own
// home (~/.claude, ~/.codex…); an account is a sign-in rush puts in that
// home when you switch to it. Settings › Providers shows them.

// accountsState is what Accounts knows beyond the snapshot.
type accountsState struct {
	summary map[string]agent.AccountSummary
	// now is the account each agent other than Claude Code is signed in
	// as, by kind: its id.
	now map[string]string
	// why is why who an agent is signed in as isn't known, by kind.
	why map[string]string
	// spill is the agent new sessions run instead of the default
	// profile's first, while every account of that is nearly out.
	spill string
	// switchedAt is when rush last switched an agent's account itself.
	switchedAt map[string]time.Time
	// profile is the profile picked for the next session started from the
	// Prompt (#profile): it goes once used.
	profile string
	// handedOff are the sessions a usage limit stopped that rush handed
	// to another provider, or tried to, by key: each is handed on once.
	handedOff map[string]bool
	// renew is why a login's sign-in couldn't be refreshed, and when, or
	// "" while it's being refreshed, by login id.
	renew map[string]renewNote
	// catalogs is every installed agent's models, read once off the UI
	// for #new's words (readCatalogs).
	catalogs *pending[map[string][]agent.Choice]
}

type renewNote struct {
	why string
	at  time.Time
}

// loginsKind is the provider whose accounts are Config.Logins, switched in
// its one home by the vault rather than through its adapter.
var loginsKind = agent.Kind(state.LoginsKind)

// ready makes the maps, for a Model made without New.
func (s *accountsState) ready() {
	if s.summary == nil {
		s.summary = map[string]agent.AccountSummary{}
	}
	if s.now == nil {
		s.now, s.why, s.switchedAt = map[string]string{}, map[string]string{}, map[string]time.Time{}
	}
	if s.handedOff == nil {
		s.handedOff = map[string]bool{}
	}
	if s.renew == nil {
		s.renew = map[string]renewNote{}
	}
}

// acctMsg is a message about accounts.
type acctMsg interface{ applyTo(m *Model) tea.Cmd }

// acctRow is an agent, or one of its accounts.
type acctRow struct {
	kind    agent.Kind
	head    bool             // the agent's own line
	login   *fleet.LoginView // one of Claude Code's accounts
	acct    agent.Account    // another agent's
	current bool
	q       usage.Quota
}

func (r acctRow) name() string {
	switch {
	case r.head:
		return agentName(string(r.kind))
	case r.login != nil:
		return r.login.Name
	}
	return r.acct.Name
}

func (r acctRow) email() string {
	if r.login != nil {
		return r.login.Email
	}
	return firstNonEmpty(r.acct.Email, r.q.Email)
}

// agentOrder is every installed agent, in the default profile's order,
// which is the order new sessions move on through when an agent's
// accounts are all nearly out. Those it doesn't list follow: Claude Code,
// then by name.
func (m *Model) agentOrder() []agent.Adapter {
	inst := agent.InstalledAll()
	by := map[string]agent.Adapter{}
	for _, a := range inst {
		if agent.CurrentKind(a.Kind()) != a.Kind() {
			continue
		}
		by[string(a.Kind())] = a
	}
	var out []agent.Adapter
	for _, k := range m.store.Config.Default().Kinds() {
		if a, ok := by[k]; ok {
			out = append(out, a)
			delete(by, k)
		}
	}
	var rest []agent.Adapter
	for _, a := range inst {
		if _, ok := by[string(a.Kind())]; ok {
			rest = append(rest, a)
		}
	}
	sort.SliceStable(rest, func(i, j int) bool { return rest[i].Kind() == loginsKind && rest[j].Kind() != loginsKind })
	return append(out, rest...)
}

// profileOf is the home an agent runs from.
func (m *Model) profileOf(a agent.Adapter) (agent.Profile, bool) {
	if a.Kind() == loginsKind {
		acct := m.store.Config.ActiveAccount()
		return agent.Profile{Kind: loginsKind, Name: acct.Name, Dir: acct.ConfigDir}, true
	}
	ps := agent.ProfilesOf(a)
	if len(ps) == 0 {
		return agent.Profile{}, false
	}
	return ps[0], true
}

// acctKey is where an account's limits are kept.
func acctKey(kind, id string) string {
	if kind == "copilot" {
		return "copilot:gh:" + id
	}
	return kind + ":" + id
}

// signInAccount is a kept account as the agent's.
func signInAccount(s state.SignIn) agent.Account {
	return agent.Account{Kind: agent.Kind(s.Kind), ID: s.ID, Key: acctKey(s.Kind, s.ID), Name: s.Name, Email: s.Email, Plan: s.Plan}
}

// accountFrame holds read-only results only while View renders. It is cleared
// before processing another update, so account/config changes need no invalidation.
type accountFrame struct {
	rows, inUse     []acctRow
	rowsOK, inUseOK bool
	notes           []accountFrameNote
}

type accountFrameNote struct {
	kind agent.Kind
	text string
}

// accountRows are each installed agent, then its accounts.
func (m *Model) accountRows() []acctRow {
	if m.drawing && m.accountFrame.rowsOK {
		return m.accountFrame.rows
	}
	out := make([]acctRow, 0, 16+len(m.snap.Logins))
	for _, ad := range m.agentOrder() {
		k := ad.Kind()
		head := acctRow{kind: k, head: true}
		switch _, switches := ad.(agent.Accounts); {
		case k == loginsKind:
			if len(m.snap.Accounts) > 0 {
				head.q = m.snap.Accounts[0].Quota
			}
			out = append(out, head)
			for i := range m.snap.Logins {
				lv := &m.snap.Logins[i]
				out = append(out, acctRow{kind: k, login: lv, current: lv.Current, q: lv.Quota})
			}
		case switches:
			p, _ := m.profileOf(ad)
			out = append(out, head)
			for _, s := range m.store.Config.SignInsOf(string(k)) {
				a := signInAccount(s)
				r := acctRow{kind: k, acct: a, current: m.accts.now[string(k)] == s.ID, q: m.quotas[a.Key]}
				if pq, ok := m.quotas[p.Dir]; r.current && ok && (pq.Account == "" || pq.Account == a.Key) && !pq.FetchedAt.Before(r.q.FetchedAt) {
					r.q = pq
				}
				out = append(out, r)
			}
		default:
			// An agent rush can't switch: its one sign-in is its own.
			if p, ok := m.profileOf(ad); ok {
				head.q = m.quotas[p.Dir]
			}
			out = append(out, head)
		}
	}
	if m.drawing {
		m.accountFrame.rows, m.accountFrame.rowsOK = out, true
	}
	return out
}

// switches is whether rush can switch agent k between accounts.
func switches(k agent.Kind) bool {
	if k == loginsKind {
		return true
	}
	ad, _ := agent.Get(k)
	_, ok := ad.(agent.Accounts)
	return ok
}

// accountsOf are the account rows of agent k.
func accountsOf(rows []acctRow, k agent.Kind) []acctRow {
	var out []acctRow
	for _, r := range rows {
		if r.kind == k && !r.head {
			out = append(out, r)
		}
	}
	return out
}

// startKind is the agent new sessions from the Prompt run: their
// profile's first, unless every account of it is nearly out and the
// profile moves on to the next.
func (m *Model) startKind() string { return m.startKindIn(m.startDir()) }

// startKindIn is the agent a new session in dir runs.
func (m *Model) startKindIn(dir string) string {
	// A frame asks it for the top bar, and Providers for every
	// row: nothing changes while one is drawn, so it's worked out once.
	if mk := m.kindMemo; m.drawing && mk.ok && mk.dir == dir {
		return mk.kind
	}
	k := m.store.Config.DefaultAgent()
	if p, ok := m.startPick(dir); ok {
		k = p.Kind
	}
	k = string(agent.CurrentKind(agent.Kind(k)))
	if m.drawing {
		m.kindMemo = kindMemo{dir: dir, kind: k, ok: true}
	}
	return k
}

// kindMemo is startKindIn's answer while a frame is drawn.
type kindMemo struct {
	dir, kind string
	ok        bool
}

// startAccount is the agent and account new sessions start on, for the
// top bar.
func (m *Model) startAccount() string {
	k := m.startKind()
	if agent.Kind(k) == loginsKind {
		return m.inUse()
	}
	name := agentName(k)
	for _, s := range m.store.Config.SignInsOf(k) {
		if s.ID == m.accts.now[k] {
			return name + " · " + s.Name
		}
	}
	return name
}

// startQuota is the limits of the account new sessions start on, when
// they run an agent other than Claude Code.
func (m *Model) startQuota() (usage.Quota, bool) {
	k := m.startKind()
	if agent.Kind(k) == loginsKind {
		return usage.Quota{}, false
	}
	for _, r := range m.accountRows() {
		if string(r.kind) == k && (r.current || r.head && !switches(r.kind)) {
			return r.q, true
		}
	}
	return usage.Quota{}, true
}

// findSignIns asks each agent rush can switch who it's signed in as, and
// which accounts it knows of itself; one that runs on a key its program
// keeps, whether it has it.
func (m *Model) findSignIns() tea.Cmd {
	if m.offline {
		return nil
	}
	var cmds []tea.Cmd
	for _, ad := range agent.InstalledAll() {
		if agent.CurrentKind(ad.Kind()) != ad.Kind() {
			continue
		}
		if kc, ok := ad.(agent.KeyChecker); ok {
			p, found := m.profileOf(ad)
			_, native := ad.(agent.Authenticator)
			if !found && native {
				p = agent.Profile{Kind: ad.Kind()}
			}
			if found || native {
				cmds = append(cmds, func() tea.Msg {
					// Native login may have created or migrated its config home since
					// the nonblocking profile snapshot was last refreshed.
					if _, native := ad.(agent.Authenticator); native {
						if profiles := ad.Profiles(); len(profiles) > 0 {
							p = profiles[0]
						}
					}
					msg := signInsMsg{kind: string(ad.Kind()), err: kc.CheckKey(p)}
					if info, ok := ad.(agent.AccountSummaryReader); ok {
						v := info.AccountSummary(p)
						msg.summary = &v
					}
					return msg
				})
			}
			continue
		}
		acc, ok := ad.(agent.Accounts)
		if info, isInfo := ad.(agent.AccountSummaryReader); !ok && isInfo {
			// signed in by its own program, with nothing to switch: only what it says of itself
			if p, found := m.profileOf(ad); found {
				cmds = append(cmds, func() tea.Msg {
					v := info.AccountSummary(p)
					return signInsMsg{kind: string(ad.Kind()), summary: &v}
				})
			}
			continue
		}
		if !ok || ad.Kind() == loginsKind {
			continue
		}
		p, ok := m.profileOf(ad)
		if !ok {
			continue
		}
		cmds = append(cmds, func() tea.Msg {
			msg := signInsMsg{kind: string(ad.Kind())}
			msg.cur, msg.err = acc.Current(p)
			if kn, ok := ad.(agent.Known); ok {
				msg.known = kn.Known()
			}
			return msg
		})
	}
	return tea.Batch(cmds...)
}

// signInsMsg is who an agent is signed in as, and the accounts it knows.
type signInsMsg struct {
	summary *agent.AccountSummary
	kind    string
	cur     agent.Account
	err     error
	known   []agent.Account
}

func (msg signInsMsg) applyTo(m *Model) tea.Cmd {
	m.accts.ready()
	if msg.summary != nil {
		m.accts.summary[msg.kind] = *msg.summary
	}
	cfg := &m.store.Config
	before := append([]state.SignIn(nil), cfg.SignIns...)
	note := func(a agent.Account) {
		if a.ID != "" {
			cfg.NoteSignIn(state.SignIn{Kind: msg.kind, ID: a.ID, Email: a.Email, Plan: a.Plan}, a.Name)
		}
	}
	for _, a := range msg.known {
		note(a)
	}
	if msg.err != nil {
		m.accts.why[msg.kind] = msg.err.Error()
		delete(m.accts.now, msg.kind)
	} else {
		note(msg.cur)
		m.accts.now[msg.kind] = msg.cur.ID
		delete(m.accts.why, msg.kind)
	}
	if !reflect.DeepEqual(before, cfg.SignIns) {
		_ = m.store.SaveConfig()
	}
	return nil
}

// switchAccount signs another agent's home in as a.
func (m *Model) switchAccount(a agent.Account, why string) tea.Cmd {
	ad, ok := agent.Get(a.Kind)
	if !ok {
		return nil
	}
	acc, ok := ad.(agent.Accounts)
	p, found := m.profileOf(ad)
	if !ok || !found {
		m.flash("rush can't switch "+ad.Name()+"'s account", true)
		return nil
	}
	return func() tea.Msg {
		return acctSwitchedMsg{to: a, why: why, err: acc.Switch(p, a)}
	}
}

// acctSwitchedMsg is another agent's home signed in as to, or why not.
type acctSwitchedMsg struct {
	to  agent.Account
	why string
	err error
}

func (msg acctSwitchedMsg) applyTo(m *Model) tea.Cmd {
	m.accts.ready()
	k := string(msg.to.Kind)
	if msg.err != nil {
		m.flash("couldn't switch to "+msg.to.Name+": "+msg.err.Error(), true)
		return nil
	}
	m.accts.now[k] = msg.to.ID
	m.accts.switchedAt[k] = time.Now()
	if ad, _ := agent.Get(msg.to.Kind); ad != nil {
		if _, own := ad.(agent.Known); own {
			// Which of its accounts the agent runs on is rush's to keep.
			cfg := &m.store.Config
			if cfg.Using == nil {
				cfg.Using = map[string]string{}
			}
			cfg.Using[k] = msg.to.ID
			_ = m.store.SaveConfig()
		}
	}
	text := "new " + agentName(k) + " sessions run on " + msg.to.Name
	if msg.why != "" {
		text = msg.why + " · " + text
	}
	m.flash(text, false)
	return m.fetchQuotas()
}

// addAccount signs in to another account of agent k.
func (m *Model) addAccount(k agent.Kind) tea.Cmd {
	if k == loginsKind {
		m.ask("login name", "", m.addLogin)
		return nil
	}
	ad, _ := agent.Get(k)
	acc, ok := ad.(agent.Accounts)
	p, found := m.profileOf(ad)
	if !ok || !found {
		if _, native := ad.(agent.Authenticator); native {
			return m.nativeSignIn(k)
		}
		m.flash(harnessName(string(k))+" does not expose a sign-in flow to rush", true)
		return nil
	}
	// Making the sign-in (its home, its command) touches the disk: done
	// off the UI goroutine.
	start := func() (*exec.Cmd, func(error) tea.Msg, error) {
		cmd, done, err := acc.SignIn(p)
		if err != nil {
			return nil, nil, err
		}
		return cmd, func(err error) tea.Msg {
			if err != nil {
				return acctAddedMsg{kind: k, err: err}
			}
			a, err := done()
			return acctAddedMsg{kind: k, a: a, err: err}
		}, nil
	}
	if k == "codex" { // prints its link like Claude Code's; gh's asks in the terminal
		return m.signIn(agentName(string(k)), start)
	}
	return inTerminal(start)
}

// acctAddedMsg is an account just signed in to from Accounts.
type acctAddedMsg struct {
	kind agent.Kind
	a    agent.Account
	err  error
}

func (msg acctAddedMsg) applyTo(m *Model) tea.Cmd {
	if msg.err != nil {
		m.flash("didn't add a "+agentName(string(msg.kind))+" account: "+msg.err.Error(), true)
		return nil
	}
	cfg := &m.store.Config
	known := false
	for _, s := range cfg.SignInsOf(string(msg.kind)) {
		known = known || s.ID == msg.a.ID
	}
	s := cfg.NoteSignIn(state.SignIn{Kind: string(msg.kind), ID: msg.a.ID, Email: msg.a.Email, Plan: msg.a.Plan}, msg.a.Name)
	_ = m.store.SaveConfig()
	if known {
		m.flash("signed in to "+s.Name+" again", false)
	} else {
		m.flash("added "+s.Name+" · enter switches to it", false)
	}
	return m.fetchQuotas()
}

// forgetAccount drops another agent's account, after asking.
func (m *Model) forgetAccount(r acctRow) {
	if r.current {
		m.flash("switch to another account before forgetting "+r.name(), true)
		return
	}
	d := m.dialog
	m.confirmThen(fmt.Sprintf("Forget %s (%s)? rush drops its saved sign-in; sessions already on it keep going.", r.name(), firstNonEmpty(r.email(), agentName(string(r.kind)))), func() tea.Cmd {
		m.store.Config.ForgetSignIn(string(r.kind), r.acct.ID)
		_ = m.store.SaveConfig()
		d.cursor = max(0, d.cursor-1)
		if ad, ok := agent.Get(r.kind); ok {
			if acc, ok := ad.(agent.Accounts); ok {
				gone := r.acct
				return func() tea.Msg { _ = acc.Forget(gone); return nil }
			}
		}
		return nil
	})
}

// signInKey handles a key on another agent's account.
func (m *Model) signInKey(r acctRow, s string) tea.Cmd {
	switch s {
	case "enter":
		if r.current {
			m.flash("already on "+r.name(), false)
			return nil
		}
		return m.switchAccount(r.acct, "")
	case "r":
		m.ask("rename "+r.name(), r.name(), func(v string) tea.Cmd {
			m.renameSignIn(string(r.kind), r.acct.ID, v)
			return nil
		})
	case "l":
		return m.addAccount(r.kind)
	case "d", "x":
		m.forgetAccount(r)
	}
	return nil
}

// renameSignIn renames another agent's account.
func (m *Model) renameSignIn(kind, id, v string) {
	for i, s := range m.store.Config.SignIns {
		if s.Kind == kind && s.ID == id {
			m.store.Config.SignIns[i].Name = v
		}
	}
	_ = m.store.SaveConfig()
}

// loginKey handles a key on one of Claude Code's accounts.
func (m *Model) loginKey(lv fleet.LoginView, s string) tea.Cmd {
	switch s {
	case "enter":
		if lv.Current {
			m.flash("already on "+lv.Name, false)
			return nil
		}
		return m.switchLogin(lv.Login, "")
	case "r":
		if expired(lv.Quota) {
			return m.renewLogin(lv)
		}
		m.ask("rename "+lv.Name, lv.Name, func(v string) tea.Cmd {
			for i, l := range m.store.Config.Logins {
				if l.ID == lv.ID {
					m.store.Config.Logins[i].Name = v
				}
			}
			_ = m.store.SaveConfig()
			m.refresh()
			return nil
		})
	case "l":
		return m.addLogin(lv.Name)
	case "d", "x":
		if lv.Current {
			m.flash("switch to another account before forgetting "+lv.Name, true)
			return nil
		}
		m.confirmThen(fmt.Sprintf("Forget %s (%s)? rush drops its saved sign-in; sessions already on it keep going.", lv.Name, lv.Email), func() tea.Cmd {
			var keep []state.Login
			for _, l := range m.store.Config.Logins {
				if l.ID != lv.ID {
					keep = append(keep, l)
				}
			}
			m.store.Config.Logins = keep
			_ = m.store.SaveConfig()
			m.refresh()
			m.dialog.cursor = max(0, m.dialog.cursor-1)
			id := lv.ID
			return func() tea.Msg { // the keychain and its home
				if k, ok := state.Logins(); ok {
					_ = k.ForgetLogin(id)
				}
				return nil
			}
		})
	}
	return nil
}

// kindName is an agent's name, short enough for a table.
func kindName(k agent.Kind) string {
	if ad, ok := agent.Get(k); ok && len(ad.Name()) <= 9 {
		return ad.Name()
	}
	s := string(k)
	if s == "" {
		return s
	}
	return strings.ToUpper(s[:1]) + s[1:]
}

// limits is an account's first two windows, each a labelled meter, or
// its balance, or why there are none.
func (m *Model) limits(r acctRow, w1, w2 int) string {
	q := r.q
	q.Problem = m.problem(r)
	if len(q.Windows) == 0 {
		msg := faint("no reading")
		switch {
		case q.Balance != "":
			msg = paint(cText, q.Balance)
			if q.Problem != "" {
				msg = paint(cYellow, q.Balance+" · "+q.Problem)
			}
		case q.Problem != "":
			msg = paint(cYellow, q.Problem)
		case r.kind != loginsKind:
			ad, _ := agent.Get(r.kind)
			if _, ok := ad.(agent.QuotaSource); !ok {
				msg = faint("doesn't report its limits")
			} else if q.FetchedAt.IsZero() && r.current {
				msg = faint("fetching…")
			} else if q.FetchedAt.IsZero() {
				msg = faint("read while it's in use")
			}
		case q.FetchedAt.IsZero():
			msg = faint("fetching…")
		}
		return fit(msg, w1+w2)
	}
	stale := time.Since(q.FetchedAt) > 3*usage.Every
	if stale {
		// Say why it's old, not only that it is: a sign-in gone, a limit on asking.
		why := "not read since " + q.FetchedAt.Local().Format("15:04")
		if q.Problem != "" {
			why = q.Problem + " · last read " + q.FetchedAt.Local().Format("15:04")
		}
		return fit(paint(cYellow, "! ")+dim(why), w1+w2)
	}
	var out string
	for i, cw := range []int{w1, w2} {
		if i >= len(q.Windows) {
			out += fit("", cw)
			continue
		}
		win := q.Windows[i]
		pct := fmt.Sprintf("%3.0f%%", win.Percent)
		cell := faint(fit(win.Label, 3)) + bar(win.Percent) + " " + paint(cText, pct) + resetIn(win.ResetsAt, m.snap.At, false)
		out += fit(cell, cw)
	}
	return out
}

// expired is whether a reading failed on an expired sign-in.
func expired(q usage.Quota) bool { return strings.Contains(q.Problem, usage.Expired) }

// problem is why an account has no fresh reading, as its row says it: a
// Claude Code login's expired sign-in says r refreshes it, and one just
// refreshed how that went.
func (m *Model) problem(r acctRow) string {
	if r.login == nil {
		return r.q.Problem
	}
	if n, ok := m.accts.renew[r.login.ID]; ok && n.why == "" {
		return "refreshing the sign-in…"
	} else if ok && time.Since(n.at) < 2*time.Minute {
		return "couldn't refresh: " + n.why
	}
	if expired(r.q) {
		return usage.Expired + " · r refreshes it"
	}
	return r.q.Problem
}

// windowLines are a reading's windows, each with when it resets.
func (m *Model) windowLines(q usage.Quota, now time.Time, label func(string) string) []string {
	var out []string
	for _, win := range q.Windows {
		s := label(win.Label) + bar(win.Percent) + " " + paint(cText, fmt.Sprintf("%.0f%%", win.Percent))
		if !win.ResetsAt.IsZero() {
			when := win.ResetsAt.Local().Format("15:04")
			if win.ResetsAt.Sub(now) > 20*time.Hour {
				when = win.ResetsAt.Local().Format("Mon 15:04")
			}
			if win.ResetsAt.After(now) {
				s += dim("  resets " + when + " · in " + dur(win.ResetsAt.Sub(now)))
			} else {
				s += faint("  reset at " + when + ", since this reading")
			}
		}
		out = append(out, s)
	}
	if q.Balance != "" {
		out = append(out, label("balance")+paint(cText, q.Balance))
	}
	if len(q.Windows) == 0 && q.Problem != "" {
		out = append(out, label("limits")+paint(cYellow, q.Problem))
	}
	if !q.FetchedAt.IsZero() {
		source := "read"
		switch q.Source {
		case usage.Fetched:
			source = "fetched"
		case usage.Live:
			source = "reported by a session"
		}
		out = append(out, label("usage")+faint(source+" at "+q.FetchedAt.Local().Format("15:04")))
	}
	return out
}
