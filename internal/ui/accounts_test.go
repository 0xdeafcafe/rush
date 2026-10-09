package ui

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// fakeAgent is an agent for Accounts to list: installed when its program
// is in the test's PATH, switchable when switched is set.
type fakeAgent struct {
	kind     agent.Kind
	title    string
	dir      string
	switched *[]string
}

func (f fakeAgent) Kind() agent.Kind                                            { return f.kind }
func (f fakeAgent) Name() string                                                { return f.title }
func (fakeAgent) Features() map[agent.Feature]agent.Support                     { return nil }
func (fakeAgent) Level() agent.Level                                            { return agent.LevelPreview }
func (f fakeAgent) Profiles() []agent.Profile                                   { return []agent.Profile{{Kind: f.kind, Dir: f.dir}} }
func (f fakeAgent) Program() (string, []string)                                 { return "rush-fake-" + string(f.kind), nil }
func (fakeAgent) Start(context.Context, agent.StartOptions) (agent.Conn, error) { return nil, nil }

type switchAgent struct{ fakeAgent }

func (s switchAgent) Current(agent.Profile) (agent.Account, error) {
	return agent.Account{Kind: s.kind, ID: "a1", Email: "one@example.com"}, nil
}
func (s switchAgent) Switch(_ agent.Profile, a agent.Account) error {
	*s.switched = append(*s.switched, a.ID)
	return nil
}
func (switchAgent) SignIn(agent.Profile) (*exec.Cmd, func() (agent.Account, error), error) {
	return nil, nil, nil
}
func (switchAgent) Forget(agent.Account) error { return nil }

// accountsModel is a Model with Claude Code (two logins), a switchable
// agent with two accounts, one that signs in by itself, and one that
// isn't installed.
func accountsModel(t *testing.T) (*Model, *[]string) {
	t.Helper()
	t.Setenv("RUSH_HOME", t.TempDir())
	bin := t.TempDir()
	t.Setenv("PATH", bin)
	var switched []string
	fakes := []agent.Adapter{
		fakeAgent{kind: "claude", title: "Claude Code"},
		switchAgent{fakeAgent{kind: "zcodex", title: "ZCodex", dir: "/z/.zcodex", switched: &switched}},
		fakeAgent{kind: "zplain", title: "ZPlain", dir: "/z/.zplain"},
		fakeAgent{kind: "zgone", title: "ZGone", dir: "/z/.zgone"},
	}
	for _, f := range fakes {
		// The real agent of that kind comes back after: the built-in one
		// is what every other test's sessions run.
		if was, ok := agent.Get(f.Kind()); ok {
			t.Cleanup(func() { agent.Register(was) })
		}
		agent.Register(f)
		if f.Kind() != "zgone" {
			name, _ := f.(agent.Programmer).Program()
			if err := os.WriteFile(filepath.Join(bin, name), []byte("#!/bin/sh\n"), 0o755); err != nil {
				t.Fatal(err)
			}
		}
	}
	agent.Recheck()
	t.Cleanup(agent.Recheck)

	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 160, h: 50, lastState: map[string]string{}}
	now := time.Now()
	win := func(p float64) usage.Quota {
		return usage.Quota{FetchedAt: now, Windows: []usage.Window{{ID: "five_hour", Label: "5h", Percent: p}}}
	}
	m.snap = &fleet.Snapshot{At: now, Logins: []fleet.LoginView{
		{Current: true, Quota: win(40)}, {Quota: win(10)},
	}}
	m.snap.Logins[0].Name, m.snap.Logins[0].Email = "work", "me@work.example"
	m.snap.Logins[1].Name, m.snap.Logins[1].Email = "home", "me@home.example"
	m.store.Config.SignIns = []state.SignIn{
		{Kind: "zcodex", ID: "a1", Name: "one", Email: "one@example.com"},
		{Kind: "zcodex", ID: "a2", Name: "two", Email: "two@example.com"},
	}
	m.quotas = map[string]usage.Quota{"zcodex:a1": win(99), "zcodex:a2": win(20)}
	m.accts.ready()
	m.accts.now["zcodex"] = "a1"
	m.rebuild()
	m.openDialog(pageProviders)
	return m, &switched
}

func rowNames(rows []acctRow) []string {
	var out []string
	for _, r := range rows {
		if !strings.HasPrefix(string(r.kind), "z") && r.kind != "claude" {
			continue // another test's agent
		}
		n := r.name()
		if r.head {
			n = "#" + n
		}
		out = append(out, n)
	}
	return out
}

// Accounts group under their agent, only installed agents are listed,
// in the order you set, and Providers shows a provider's accounts.
func TestAccountsGroupedByAgent(t *testing.T) {
	m, _ := accountsModel(t)
	got := strings.Join(rowNames(m.accountRows()), ",")
	if got != "#Claude Code,work,home,#ZCodex,one,two,#ZPlain" {
		t.Fatalf("rows: %s", got)
	}
	// Its order is the default profile's: Profiles changes it.
	m.store.Config.SetProfile("", state.Profile{Name: "all", Providers: []string{"claude", "zcodex", "zplain"}})
	m.store.Config.SetDefaultProfile("all")
	m.dialog.cursor = 0
	// 3 picks the third provider.
	m.providersKey("3")
	if it := m.provPicked(); it.provider != "zplain" {
		t.Fatalf("3 picked %+v", it)
	}
	m.providersKey("2")
	body := m.providersBody(150)
	for i, l := range body {
		if w := ansi.StringWidth(l); w > 150 {
			t.Fatalf("line %d is %d wide: %s", i, w, ansi.Strip(l))
		}
	}
	plain := ansi.Strip(strings.Join(body, "\n"))
	for _, want := range []string{"✻ Claude Code", "one@example.com", "ZGone"} {
		if strings.Contains(plain, want) != (want != "ZGone") {
			t.Errorf("want %q shown %v in:\n%s", want, want != "ZGone", plain)
		}
	}
	m.setSettingsPage(pageProfiles) // the default profile is marked on Profiles
	if plain := ansi.Strip(strings.Join(m.providersBody(150), "\n")); !strings.Contains(plain, "★ all") {
		t.Errorf("Profiles doesn't mark the default:\n%s", plain)
	}
}

// enter on another agent's account switches its home to it.
func TestAccountsSwitchAnotherAgent(t *testing.T) {
	m, switched := accountsModel(t)
	m.openItem(provItem{provider: "zcodex"})
	for i, r := range flat(m.provForm(m.provPicked())) {
		if r.label == "two" {
			m.dialog.cursor = i
		}
	}
	cmd := m.providersKey("enter")
	if cmd == nil {
		t.Fatal("enter did nothing")
	}
	msg := cmd().(acctMsg)
	msg.applyTo(m)
	if len(*switched) != 1 || (*switched)[0] != "a2" || m.accts.now["zcodex"] != "a2" {
		t.Fatalf("switched %v, now on %q", *switched, m.accts.now["zcodex"])
	}
}

// An account nearly out switches to the one with the most room, unless
// you asked rush to stay put.
func TestAccountsSwitchOnLimit(t *testing.T) {
	m, switched := accountsModel(t)
	cfg := &m.store.Config
	cfg.SetProfile("", state.Profile{Name: "stay", Providers: []string{"claude", "zcodex"}, OnLimit: state.LimitWait})
	cfg.SetDefaultProfile("stay")
	if cmd := m.checkLimits(); cmd != nil {
		if msg := cmd(); msg != nil {
			t.Fatalf("switched while told to stay: %v", msg)
		}
	}
	cfg.SetDefaultProfile("claude")
	cmd := m.checkLimits()
	if cmd == nil {
		t.Fatal("no switch at 99%")
	}
	for _, msg := range flatten(cmd) {
		if am, ok := msg.(acctMsg); ok {
			am.applyTo(m)
		}
	}
	if len(*switched) != 1 || (*switched)[0] != "a2" {
		t.Fatalf("switched %v", *switched)
	}
}

// Across agents: once every account of the default agent is nearly out,
// new sessions run the next agent in your order, and go back after.
func TestAccountsSpillToNextAgent(t *testing.T) {
	m, _ := accountsModel(t)
	m.store.Config.SetProfile("", state.Profile{Name: "mix", Providers: []string{"claude", "zcodex", "zplain"}, Mix: state.MixMix})
	m.store.Config.SetDefaultProfile("mix")
	for i := range m.snap.Logins {
		m.snap.Logins[i].Quota.Windows[0].Percent = 99
	}
	m.accts.switchedAt["zcodex"] = time.Now() // leave it be
	m.checkLimits()
	if m.startKind() != "zcodex" {
		t.Fatalf("new sessions run %q", m.startKind())
	}
	m.snap.Logins[1].Quota.Windows[0].Percent = 30
	m.checkLimits()
	if m.startKind() != "claude" {
		t.Fatalf("new sessions still run %q", m.startKind())
	}
}

// flatten runs a command and any batch it makes, for their messages.
func flatten(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if b, ok := msg.(tea.BatchMsg); ok {
		var out []tea.Msg
		for _, c := range b {
			out = append(out, flatten(c)...)
		}
		return out
	}
	return []tea.Msg{msg}
}

type lesserFake struct{ fakeAgent }

func (lesserFake) Lesser() (string, []string, string) {
	return "rush-fake-gh", nil, "install zlesser's CLI"
}

// An agent there only through a lesser program shows in Accounts, but
// picking it for new sessions says what to install rather than failing.
func TestAccountsLesserAgent(t *testing.T) {
	m, _ := accountsModel(t)
	agent.Register(lesserFake{fakeAgent{kind: "ylesser", title: "ZLesser", dir: "/y/.ylesser"}})
	bin := os.Getenv("PATH")
	if err := os.WriteFile(filepath.Join(bin, "rush-fake-gh"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	agent.Recheck()
	found := false
	for _, r := range m.accountRows() {
		found = found || r.head && r.kind == "ylesser"
	}
	if !found {
		t.Fatal("ZLesser isn't in Accounts")
	}
	m.withAgent("ylesser")
	if m.store.Config.DefaultAgent() == "ylesser" {
		t.Fatal("it became the default though it can't run sessions")
	}
	m.openItem(provItem{provider: "ylesser"})
	if !strings.Contains(ansi.Strip(strings.Join(m.providersBody(150), "\n")), "install zlesser's CLI") {
		t.Fatal("no hint on Providers")
	}
}

// Near a switch, the header names the account rush moves on to next.
func TestUpcomingAccountInHeader(t *testing.T) {
	m, _ := accountsModel(t)
	if _, ok := m.upcoming(40); ok {
		t.Fatal("upcoming shown with room left")
	}
	m.snap.Logins[0].Quota.Windows[0].Percent = 85
	if next, ok := m.upcoming(85); !ok || next.name() != "home" {
		t.Fatalf("upcoming = %q, %v", next.name(), ok)
	}
	m.snap.Accounts = []fleet.AccountView{{Current: true}}
	if !strings.Contains(ansi.Strip(m.activeUsage(200)), "home ━──── 10%") {
		t.Fatalf("header: %q", ansi.Strip(m.activeUsage(200)))
	}
	if strings.HasSuffix(strings.TrimSpace(ansi.Strip(m.activeUsage(200))), "│") {
		t.Fatalf("a divider with no other provider after it: %q", ansi.Strip(m.activeUsage(200)))
	}
	m.snap.Logins[1].Quota.Windows[0].Percent = 96
	if _, ok := m.upcoming(85); ok {
		t.Fatal("upcoming shown with nowhere to go")
	}
}

// A session stopped by a limit on a login rush has since switched away
// from carries on in the one now in use, when that one has room, rather
// than setting off a switch to yet another login.
func TestStoppedOnOldLoginStays(t *testing.T) {
	m, _ := accountsModel(t)
	a := &fleet.Agent{Key: "k1", Rush: true, Kind: "claude", Account: m.store.Config.ActiveAccount().Name, Detail: "usage limit · resets 00:20"}
	m.snap.Agents = []*fleet.Agent{a}
	m.autoSwitch()
	if m.resumedAt.IsZero() {
		t.Fatal("stopped sessions weren't moved to the login in use, at 40%")
	}
}

// A borrowed login is listed last, under its own heading, and left out
// of what the accounts have together.
func TestBorrowedLoginsListedApart(t *testing.T) {
	m, _ := accountsModel(t)
	m.snap.Logins[0].Borrowed = true
	sec, _ := m.accountSection(loginsKind)
	var labels []string
	for _, r := range sec.rows {
		labels = append(labels, r.label)
	}
	got := strings.Join(labels, ",")
	if !strings.Contains(got, "home,borrowed,work") || strings.Contains(got, "together") {
		t.Fatalf("rows %s: want home, then borrowed work, and no together", got)
	}
}
