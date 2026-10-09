package ui

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// loginsMsg is the logins signed in where rush looks, and why ~/.claude's
// couldn't be kept, if it couldn't.
type loginsMsg struct {
	found    []state.FoundLogin
	restored *state.Restored
	imported bool // the older folders' logins were all taken in
	err      error
}

// switchedMsg is ~/.claude signed in as another login, or why it isn't.
type switchedMsg struct {
	to      state.Login
	why     string
	resumed int // rush sessions that carried on at once
	waiting int // rush sessions moving over once their turn ends
	err     error
}

// addedLoginMsg is a login just signed in to from Accounts.
type addedLoginMsg struct {
	name string
	l    state.Login
	err  error
}

// switchGap is how long after a switch rush waits before switching on its
// own again: the readings of both logins need time to catch up. earlyGap
// is how long before it moves early (the one in use not nearly out) for a
// login whose room would be lost sooner, so two never trade places back
// and forth.
const (
	switchGap = 10 * time.Minute
	earlyGap  = time.Hour
)

// findLogins keeps ~/.claude's sign-in in the vault and reports the logins
// found; offline (--soak) leaves the keychain alone.
func (m *Model) findLogins() tea.Cmd {
	if m.offline {
		return nil
	}
	cfg := m.store.Config
	k, ok := state.Logins()
	if !ok {
		return nil
	}
	return func() tea.Msg {
		found, restored, imported, err := k.FindLogins(cfg)
		return loginsMsg{found, restored, imported, err}
	}
}

// fetchLoginUsage refreshes the plan usage of every saved login but the
// one in use, whose reading is ~/.claude's: see fetchUsage.
func (m *Model) fetchLoginUsage() []tea.Cmd {
	path := filepath.Join(state.Dir(), "usage.json")
	cfg, offline := m.store.Config, m.offline
	k, ok := state.Logins()
	if !ok {
		return nil
	}
	var cmds []tea.Cmd
	for _, lg := range cfg.Logins {
		if m.isCurrent(lg.ID) {
			continue
		}
		cmds = append(cmds, func() tea.Msg {
			return usageMsg{key: lg.UsageKey(), u: k.RefreshLogin(path, cfg, lg, offline)}
		})
	}
	return cmds
}

// onLogins saves logins not saved yet, and who each saved one is now. A
// new one's usage is read at once, so a switch it makes possible needn't
// wait for the next minute.
func (m *Model) onLogins(msg loginsMsg) tea.Cmd {
	found := msg.found
	if msg.err != nil && !m.keepFailed {
		m.flash("rush can't switch account: "+msg.err.Error(), true)
	}
	m.keepFailed = msg.err != nil
	var fetch []tea.Cmd
	if r := msg.restored; r != nil {
		was, now := m.loginNamed(r.Was), m.loginNamed(r.Now)
		if r.Err != nil {
			m.flash("a Claude Code started on "+was+" signed ~/.claude back in as it, and rush couldn't switch to "+now+" again: "+r.Err.Error(), true)
		} else {
			m.flash("a Claude Code started on "+was+" signed ~/.claude back in as it · rush switched to "+now+" again", false)
		}
		fetch = append(fetch, m.fetchUsage())
	}
	cfg := &m.store.Config
	changed, added := false, false
	for _, f := range found {
		i := m.loginIndex(f.Login.ID)
		if i < 0 {
			f.Login.Name = m.loginName(f)
			cfg.Logins = append(cfg.Logins, f.Login)
			changed, added = true, true
			continue
		}
		l := &cfg.Logins[i]
		if l.Email != f.Login.Email || l.Org != f.Login.Org || string(l.Profile) != string(f.Login.Profile) {
			l.Email, l.Org, l.Profile = f.Login.Email, f.Login.Org, f.Login.Profile
			changed = true
		}
	}
	if msg.imported && (!cfg.FoldersImported || len(cfg.OldFolders()) > 0) {
		// Older folders' sign-ins are accounts now and their past
		// sessions are in ~/.claude: taken in once, so one you forget
		// stays forgotten. The config as it was is kept aside.
		if !cfg.FoldersImported {
			state.KeepBefore("before-accounts")
		} else {
			state.KeepBefore("before-folders")
		}
		cfg.FoldersImported, cfg.Folders, changed = true, cfg.RootFolder(), true
	}
	if cfg.Active != "" {
		// Which folder new sessions started in is rush's choice now.
		cfg.Active, changed = "", true
	}
	if changed {
		_ = m.store.SaveConfig()
		if m.dialog != nil {
			m.loadDialog()
		}
	}
	if added {
		fetch = append(fetch, m.fetchLoginUsage()...)
	}
	return tea.Batch(fetch...)
}

// isCurrent is whether ~/.claude is signed in as the login id, as far as
// the last look found.
func (m *Model) isCurrent(id string) bool {
	for _, l := range m.snap.Logins {
		if l.ID == id {
			return l.Current
		}
	}
	return false
}

// loginNamed is a login's name, by its uuid.
func (m *Model) loginNamed(id string) string {
	for _, l := range m.store.Config.Logins {
		if l.ID == id {
			return l.Name
		}
	}
	return "another account"
}

// inUse is the name of the login ~/.claude is signed in as, or the
// folder's name before rush knows which it is.
func (m *Model) inUse() string {
	for _, l := range m.snap.Logins {
		if l.Current && l.Name != "" {
			return l.Name
		}
	}
	return m.store.Config.ActiveAccount().Name
}

// inUseOf is the name of the account agent k is signed in as.
func (m *Model) inUseOf(k string) string {
	if agent.Kind(k) == loginsKind {
		return m.inUse()
	}
	for _, s := range m.store.Config.SignInsOf(k) {
		if s.ID == m.accts.now[k] {
			return s.Name
		}
	}
	return ""
}

func (m *Model) loginIndex(id string) int {
	for i, l := range m.store.Config.Logins {
		if l.ID == id {
			return i
		}
	}
	return -1
}

// loginName is what a newly found login is called: its folder's name, or
// its email's name for ~/.claude's, unless another login has it already.
func (m *Model) loginName(f state.FoundLogin) string {
	name := f.Name
	if name == "" || name == homeName() {
		name, _, _ = strings.Cut(f.Login.Email, "@")
	}
	for _, l := range m.store.Config.Logins {
		if strings.EqualFold(l.Name, name) {
			return f.Login.Email
		}
	}
	if name == "" {
		return f.Login.ID[:8]
	}
	return name
}

// autoSwitch signs ~/.claude in as another login when the one in use is
// nearly out of its 5-hour or weekly usage, or a rush session was
// stopped by a limit on it, unless the default profile waits and so do
// the sessions a limit stopped. With no login left with room, sessions
// whose profile hands on go to the next provider.
func (m *Model) autoSwitch() tea.Cmd {
	cfg := m.store.Config
	if m.offline || m.switching || time.Since(m.switchedAt) < switchGap {
		return nil
	}
	root := cfg.ActiveAccount()
	stopped := false
	for _, a := range m.snap.Agents {
		if a.Rush && a.Account == root.Name && strings.HasPrefix(a.Detail, "usage limit") && m.sessionProfile(a).Limit() != state.LimitWait {
			stopped = true
		}
	}
	if cfg.Default().Limit() == state.LimitWait && !stopped {
		return nil
	}
	if stopped && m.hasRoom() && len(m.snap.Logins) > 1 {
		// Stopped under a login rush has switched away from since (by
		// another rush, or before this one could say): the one in use has
		// room, so they carry on in it, and no other login is picked.
		if time.Since(m.resumedAt) < time.Minute {
			return nil
		}
		m.resumedAt = time.Now()
		return func() tea.Msg {
			n, _ := reloginHosts(root.Name, cfg)
			if n == 0 {
				return nil
			}
			return doneMsg{text: fmt.Sprintf("%d stopped by a usage limit carry on, on the account now in use", n)}
		}
	}
	to, ok := fleet.NextLogin(m.snap.Logins, stopped)
	if len(m.snap.Logins) < 2 || !ok {
		if stopped && !m.hasRoom() {
			return m.handOffStopped()
		}
		return nil
	}
	// Early: the one in use still has room, so only sessions that start
	// anew move; one with a warm cache stays until it cools (host.Info.Home).
	early := !stopped && m.hasRoom()
	if early && time.Since(m.switchedAt) < earlyGap {
		return nil
	}
	why := "a session hit a usage limit"
	for _, l := range m.snap.Logins {
		switch {
		case !l.Current || stopped:
		case m.hasRoom():
			// Neither nearly out: to's room would be lost sooner.
			w, _ := fleet.Week(to.Quota)
			c, _ := fleet.Week(l.Quota)
			why = fmt.Sprintf("%s's %.0f%% left resets in %s, %s's in %s", to.Name, 100-w.Percent, roughly(fleet.Left(w, time.Now())), l.Name, roughly(fleet.Left(c, time.Now())))
		default:
			why = fmt.Sprintf("%s was at %.0f%%", l.Name, l.Quota.Used(""))
		}
	}
	return m.switchLogin(to.Login, why, !early)
}

// switchLogin makes to the login new sessions run as, in its home.
// ~/.claude stays signed in as it is. With move, idle rush sessions rest
// so their next message starts on it, and those a limit stopped carry on
// now; without, each follows once its cache has cooled.
func (m *Model) switchLogin(to state.Login, why string, move bool) tea.Cmd {
	k, ok := state.Logins()
	if m.switching || !ok {
		return nil
	}
	m.switching = true
	cfg := m.store.Config
	root := cfg.ActiveAccount()
	return func() tea.Msg {
		if err := k.UseLogin(cfg, to); err != nil {
			return switchedMsg{to: to, err: err}
		}
		if !move {
			return switchedMsg{to: to, why: why}
		}
		_ = host.MarkMoved() // asleep ones too, which can't be told
		resumed, waiting := reloginHosts(root.Name, cfg)
		return switchedMsg{to: to, why: why, resumed: resumed, waiting: waiting}
	}
}

// hasRoom is whether the login in use has a recent reading below where
// rush switches away from it.
func (m *Model) hasRoom() bool {
	for _, l := range m.snap.Logins {
		if l.Current {
			return time.Since(l.Quota.FetchedAt) < 3*usage.Every && !l.Quota.NearlyOut("", usage.Lead, time.Now())
		}
	}
	return false
}

// reloginHosts tells every rush session in the folder named root that it's signed in as
// another account now, and reports how many a usage limit had stopped and
// how many move over once their turn ends (an idle one does at once). A
// host from before rush could switch ignores the message: one of those a
// limit stopped is still stopped after it, so its Claude Code (which holds
// the old sign-in) is stopped, and it's told to continue, which starts a
// fresh one. A session whose profile waits at a limit is moved all the
// same, but left to wait: one from homes on starts in the new home when
// it next runs, and one from before is replaced without a continue.
func reloginHosts(root string, cfg state.Config) (resumed, waiting int) { //nolint:gocritic // Config goes by value, as everywhere
	for _, info := range host.List() {
		if (info.Account != root && info.Account != "") || info.State == "stopped" {
			continue
		}
		wait := info.Limit != nil && cfg.ProfileFor(info.Cwd, info.Profile).Limit() == state.LimitWait
		if !info.Homes && (info.Kind == "" || info.Kind == string(loginsKind)) {
			switch {
			case hostBusy(info):
				go replaceWhenIdle(info.ID)
				waiting++
			case replaceHost(info, !wait) == nil && info.Limit != nil && !wait:
				resumed++
			}
			continue
		}
		if wait {
			continue
		}
		c, err := host.Dial(info.ID)
		if err != nil {
			continue
		}
		err = c.Relogin()
		if err == nil && info.Limit != nil && stillLimited(info.ID) {
			err = restartClaude(c, info, host.LimitContinue)
		}
		switch {
		case err != nil:
		case info.Limit != nil:
			resumed++
		case hostBusy(info):
			waiting++
		}
		c.Close()
	}
	return resumed, waiting
}

func (m *Model) onSwitched(msg switchedMsg) tea.Cmd {
	m.switching = false
	if msg.err != nil {
		m.flash("couldn't switch to "+msg.to.Name+": "+msg.err.Error(), true)
		return nil
	}
	m.switchedAt = time.Now()
	text := "new sessions run on " + msg.to.Name
	if msg.why != "" {
		text = msg.why + " · " + text
	}
	if msg.resumed > 0 {
		text += fmt.Sprintf(" · %d session%s the limit stopped carry on", msg.resumed, plural(msg.resumed))
	}
	if msg.waiting > 0 {
		text += fmt.Sprintf(" · %d busy session%s will switch after the current turn", msg.waiting, plural(msg.waiting))
	}
	m.flash(text, false)
	if m.dialog != nil {
		m.loadDialog()
	}
	return m.fetchUsage()
}

// renewLogin refreshes a login's expired sign-in in the background, then
// reads its usage again; its row says how it went.
func (m *Model) renewLogin(lv fleet.LoginView) tea.Cmd {
	k, ok := state.Logins()
	m.accts.ready()
	if n, busy := m.accts.renew[lv.ID]; !ok || m.offline || busy && n.why == "" {
		return nil
	}
	m.accts.renew[lv.ID] = renewNote{}
	cfg, lg, path := m.store.Config, lv.Login, filepath.Join(state.Dir(), "usage.json")
	return func() tea.Msg {
		msg := renewedMsg{l: lg, err: k.RenewLogin(cfg, lg)}
		if msg.err == nil {
			msg.u = k.RefreshLogin(path, cfg, lg, false)
		}
		return msg
	}
}

// renewedMsg is a login's sign-in refreshed and its usage read again, or
// why it couldn't be.
type renewedMsg struct {
	l   state.Login
	u   usage.Reading
	err error
}

func (msg renewedMsg) applyTo(m *Model) tea.Cmd {
	m.accts.ready()
	if msg.err != nil {
		m.accts.renew[msg.l.ID] = renewNote{why: msg.err.Error(), at: time.Now()}
		return nil
	}
	delete(m.accts.renew, msg.l.ID)
	m.flash("refreshed "+msg.l.Name+"'s sign-in", false)
	m.loader.SetFetched(msg.l.UsageKey(), msg.u)
	m.refresh()
	return nil
}

// addLogin signs in to an account in a folder of its own, then keeps the
// sign-in in the vault and removes the folder: ~/.claude stays as it is
// until you switch to it.
func (m *Model) addLogin(name string) tea.Cmd {
	lg, ok := agent.As[agent.Loginer](loginsKind)
	k, kok := state.Logins()
	if !ok || !kok {
		m.flash(harnessName(string(loginsKind))+" can't sign in from rush", true)
		return nil
	}
	return m.signIn(harnessName(string(loginsKind))+" · "+name, func() (*exec.Cmd, func(error) tea.Msg, error) {
		b := make([]byte, 4)
		_, _ = rand.Read(b)
		scratch := agent.Profile{Kind: loginsKind, Name: name, Dir: filepath.Join(state.Dir(), "signin-"+hex.EncodeToString(b))}
		return lg.Login(scratch), func(err error) tea.Msg {
			if err != nil {
				return addedLoginMsg{name: name, err: err}
			}
			l, err := k.AdoptLogin(scratch)
			return addedLoginMsg{name: name, l: l, err: err}
		}, nil
	})
}

func (m *Model) onAddedLogin(msg addedLoginMsg) tea.Cmd {
	if msg.err != nil {
		m.flash("didn't sign in to "+msg.name+": "+msg.err.Error(), true)
		return nil
	}
	cfg := &m.store.Config
	msg.l.Name = msg.name
	if i := m.loginIndex(msg.l.ID); i >= 0 {
		msg.l.Name = cfg.Logins[i].Name
		cfg.Logins[i] = msg.l
		m.flash("signed in to "+msg.l.Name+" again ("+msg.l.Email+")", false)
	} else {
		cfg.Logins = append(cfg.Logins, msg.l)
		m.flash("added "+msg.l.Name+" ("+msg.l.Email+") · enter switches to it", false)
	}
	_ = m.store.SaveConfig()
	if m.dialog != nil {
		m.loadDialog()
	}
	return tea.Batch(m.fetchLoginUsage()...)
}

// signedInAs is whether the login with this id is the one signed in, as
// the last reading of the fleet found.
func (m *Model) signedInAs(id string) bool {
	for _, lv := range m.snap.Logins {
		if lv.Current && lv.ID == id {
			return true
		}
	}
	return false
}

// useLogin switches to the account named, or with that email: one of
// the agent new sessions run first, then any other installed agent's.
func (m *Model) useLogin(name string) tea.Cmd {
	k := m.startKind()
	rows := m.accountRows()
	sort.SliceStable(rows, func(i, j int) bool { return string(rows[i].kind) == k && string(rows[j].kind) != k })
	for _, r := range rows {
		if r.head || r.login != nil || !(strings.EqualFold(r.name(), name) || strings.EqualFold(r.email(), name)) {
			continue
		}
		if r.current {
			m.flash("already on "+r.name(), false)
			return nil
		}
		return m.switchAccount(r.acct, "")
	}
	for _, l := range m.store.Config.Logins {
		if strings.EqualFold(l.Name, name) || strings.EqualFold(l.Email, name) {
			if m.signedInAs(l.ID) {
				m.flash("already on "+l.Name, false)
				return nil
			}
			return m.switchLogin(l, "", true)
		}
	}
	m.flash("no account named "+name, true)
	return nil
}

func hostBusy(info host.Info) bool {
	return info.ClaudePID != 0 && (info.State == "working" || info.State == "blocked" || info.State == "starting")
}

// replaceHost stops a host from before homes and starts a new one on the
// same conversation, which runs as the login in use. What it had queued,
// or a continue if a limit stopped it and carryOn, is sent to the new one.
func replaceHost(info host.Info, carryOn bool) error {
	cfg, err := host.ReadConfig(info.ID)
	if err != nil {
		return err
	}
	c, err := host.Dial(info.ID)
	if err != nil {
		return err
	}
	_ = c.Stop()
	c.Close()
	for i := 0; i < 50 && syscall.Kill(info.HostPID, 0) == nil; i++ {
		time.Sleep(100 * time.Millisecond)
	}
	cfg.Resume, cfg.Prompt, cfg.Images = true, "", nil
	if _, err := host.Spawn(cfg); err != nil {
		return err
	}
	text := strings.Join(info.Queue, "\n\n")
	if text == "" && info.Limit != nil && carryOn {
		text = host.LimitContinue
	}
	if text == "" {
		return nil
	}
	if c, err = host.Dial(info.ID); err != nil {
		return err
	}
	defer c.Close()
	return c.Send(text)
}

// replaceWhenIdle replaces a busy host from before homes once its turn is
// done, unless it stops first.
func replaceWhenIdle(id string) {
	for {
		time.Sleep(2 * time.Second)
		info, err := host.ReadInfo(id)
		if err != nil || info.State == "stopped" || syscall.Kill(info.HostPID, 0) != nil {
			return
		}
		if !hostBusy(info) {
			_ = replaceHost(info, true)
			return
		}
	}
}

// stillLimited waits up to 3s for a session a limit stopped to carry on.
func stillLimited(id string) bool {
	for range 30 {
		time.Sleep(100 * time.Millisecond)
		for _, info := range host.List() {
			if info.ID == id && info.Limit == nil {
				return false
			}
		}
	}
	return true
}

// restartClaude stops a session's Claude Code, and sends text if there is
// any, which starts a fresh one resuming the conversation: signed in as
// whoever ~/.claude is now, with the settings and MCP servers as they are
// now. Without text it starts again with your next message.
func restartClaude(c *host.Client, info host.Info, text string) error {
	if pid := info.ClaudePID; pid != 0 {
		_ = syscall.Kill(pid, syscall.SIGTERM)
		gone := false
		for range 50 {
			if syscall.Kill(pid, 0) != nil {
				gone = true
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
		if !gone {
			_ = syscall.Kill(pid, syscall.SIGKILL)
			time.Sleep(200 * time.Millisecond)
		}
	}
	if text == "" {
		return nil
	}
	return c.Send(text)
}

// restart is #restart: the agent's Claude Code starts again, so it picks up
// the account in use and settings changed since. A rush agent keeps its
// place; any other is relaunched on its account.
func (m *Model) restart(a *fleet.Agent, text string) tea.Cmd {
	if !a.Rush {
		return m.relaunch(a, a.Acct)
	}
	if text == "" && (strings.HasPrefix(a.Detail, "usage limit") || strings.HasPrefix(a.Detail, "API error")) {
		text = "continue"
	}
	id, name := a.ID, a.DisplayName
	run := func() tea.Cmd {
		return func() tea.Msg {
			var info host.Info
			for _, i := range host.List() {
				if i.ID == id {
					info = i
				}
			}
			if info.ID == "" || info.State == "stopped" {
				return doneMsg{err: fmt.Errorf("%s isn't running; a message starts it", name)}
			}
			c, err := host.Dial(id)
			if err != nil {
				return doneMsg{err: err}
			}
			defer c.Close()
			if err := restartClaude(c, info, text); err != nil {
				return doneMsg{err: err}
			}
			if text == "" {
				return doneMsg{text: "restarted " + name + " · it starts again with your next message"}
			}
			return doneMsg{text: "restarted " + name}
		}
	}
	if a.State == "working" || a.State == "blocked" && !strings.HasPrefix(a.Detail, "usage limit") {
		m.confirm = &confirmation{question: "Restart " + name + "?", detail: "it's in the middle of a turn · y cuts it off and resumes the conversation", onYes: run}
		return nil
	}
	return run()
}

// homeName is what the logins agent's own home is called.
func homeName() string {
	if h, ok := agent.As[agent.Homer](loginsKind); ok {
		return h.Home().Name
	}
	return ""
}
