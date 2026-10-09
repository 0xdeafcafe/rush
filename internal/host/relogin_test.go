package host

import (
	"cmp"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// staleAgent is fakeAgent signed in as whatever its profile's login file
// says: a conn is stale once the file says another login than it started
// on. A "bg" message leaves work in the background for a while after its
// turn.
type staleAgent struct{ fakeAgent }

func init() { agent.Register(staleAgent{}) }

func (staleAgent) Kind() agent.Kind { return "stale" }

func login(dir string) string {
	b, _ := os.ReadFile(filepath.Join(dir, "login"))
	return strings.TrimSpace(string(b))
}

func (staleAgent) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	c, err := fakeAgent{}.Start(ctx, o)
	if err != nil {
		return nil, err
	}
	who := login(o.Profile.Dir)
	f, _ := os.OpenFile(filepath.Join(o.Profile.Dir, "starts.log"), os.O_APPEND|os.O_WRONLY, 0o600)
	f.WriteString("on " + who + "\n")
	f.Close()
	return &staleConn{fakeConn: c.(*fakeConn), dir: o.Profile.Dir, login: who}, nil
}

type staleConn struct {
	*fakeConn
	dir, login string
	bg         bool
}

func (c *staleConn) Close() error {
	f, _ := os.OpenFile(filepath.Join(c.dir, "starts.log"), os.O_APPEND|os.O_WRONLY, 0o600)
	f.WriteString("closed " + c.login + "\n")
	f.Close()
	return c.fakeConn.Close()
}

func (c *staleConn) Stale() bool { return login(c.dir) != c.login }

func (c *staleConn) Send(in agent.Input) error {
	c.bg = in.Text == "bg"
	if in.Text == "limit" { // out on this account: the turn ends at once
		c.events <- event.TurnEnd{Err: "You've hit your session limit · resets 5am (Europe/London)"}
		return nil
	}
	return c.fakeConn.Send(in)
}

func (c *staleConn) Answer(id, option string) error {
	if c.bg {
		c.events <- event.Background{Tasks: []event.BackgroundTask{{ID: "t1", Label: "a build"}}}
		go func() {
			time.Sleep(1500 * time.Millisecond)
			defer func() { _ = recover() }() // closed, stopped early
			c.events <- event.Background{}
		}()
	}
	return c.fakeConn.Answer(id, option)
}

// Signed in as another account, a session starts again on it at its first
// safe point: a turn under way ends first, and what was queued goes to the
// fresh one; an idle one rests at once; one with work in the background
// once that's done.
func TestReloginAtASafePoint(t *testing.T) {
	home := filepath.Dir(setup(t))
	setLogin := func(who string) { _ = os.WriteFile(filepath.Join(home, "login"), []byte(who), 0o600) }
	setLogin("a")
	cfg, err := Spawn(Config{Kind: "stale", Cwd: home, Account: agent.Profile{Kind: "stale", Name: "stale", Dir: home}, Prompt: "hi", IdleStop: Duration(time.Hour)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	starts := func() []string {
		b, _ := os.ReadFile(filepath.Join(home, "starts.log"))
		return strings.Split(strings.TrimSpace(string(b)), "\n")
	}
	isApproval := func(cmd string) func(any) bool {
		return func(ev any) bool { a, ok := ev.(event.Approval); return ok && a.Call.Input.Command == cmd }
	}
	closed := func(who string) {
		t.Helper()
		for range 250 {
			if got := starts(); got[len(got)-1] == "closed "+who {
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("%s wasn't closed: %q", who, starts())
	}

	// Mid-turn: it's marked, the turn ends, and the queue goes to a fresh one.
	ap := next(t, c, isApproval("echo hi")).(event.Approval)
	setLogin("b")
	if err := c.Relogin(); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.Relogin && i.Info.State == "blocked" })
	if err := c.Send("queued"); err != nil {
		t.Fatal(err)
	}
	if err := c.Allow(ap.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	ap = next(t, c, isApproval("echo queued")).(event.Approval)
	// The old one is gone before the fresh one starts.
	if got := starts(); len(got) != 5 || got[1] != "on a" || got[2] != "closed a" || got[3] != "start fake-session" || got[4] != "on b" {
		t.Fatalf("starts: %q", got)
	}

	// Idle: it rests at once.
	if err := c.Allow(ap.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	next(t, c, inState("idle"))
	setLogin("c")
	if err := c.Relogin(); err != nil {
		t.Fatal(err)
	}
	closed("b")

	// Work in the background: it rests once that's done, not before.
	if err := c.Send("bg"); err != nil {
		t.Fatal(err)
	}
	ap = next(t, c, isApproval("echo bg")).(event.Approval)
	if err := c.Allow(ap.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool {
		i, ok := ev.(InfoEvent)
		return ok && i.Info.State == "idle" && len(i.Info.Background) > 0
	})
	setLogin("d")
	asked := time.Now()
	if err := c.Relogin(); err != nil {
		t.Fatal(err)
	}
	closed("c")
	if d := time.Since(asked); d < time.Second {
		t.Fatalf("rested %v after, with work in the background", d)
	}

	// Out on the account it's due to move off, it moves now, work in the
	// background or not, and carries on.
	if err := c.Send("bg"); err != nil {
		t.Fatal(err)
	}
	ap = next(t, c, isApproval("echo bg")).(event.Approval)
	if err := c.Allow(ap.ID, nil, false); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool {
		i, ok := ev.(InfoEvent)
		return ok && i.Info.State == "idle" && len(i.Info.Background) > 0
	})
	setLogin("e")
	if err := c.Relogin(); err != nil {
		t.Fatal(err)
	}
	asked = time.Now()
	if err := c.Send("limit"); err != nil {
		t.Fatal(err)
	}
	next(t, c, isApproval("echo "+LimitContinue))
	if d := time.Since(asked); d > time.Second {
		t.Fatalf("carried on %v after the limit: it waited for the background", d)
	}
	if err := c.Stop(); err != nil {
		t.Fatal(err)
	}
}

func TestIsLimit(t *testing.T) {
	for _, s := range []string{"Claude usage limit reached", "You've hit your session limit · resets 5am", "You've hit your weekly limit"} {
		if !isLimit(strings.ToLower(s)) {
			t.Errorf("%q isn't read as a limit", s)
		}
	}
	if isLimit("api error: 529 overloaded") {
		t.Error("an overload read as a limit")
	}
}

// The view draws LimitContinue as rush's own row by how it starts.
func TestLimitContinueKeepsItsOpening(t *testing.T) {
	if !strings.HasPrefix(LimitContinue, "continue: you're on another account now, with room") {
		t.Fatal("convo.switched no longer knows LimitContinue")
	}
}

// homedAgent runs in a home per login, as Claude Code does: the login in
// use is its profile's login file, and its home is named for it.
type homedAgent struct{ fakeAgent }

func init() { agent.Register(homedAgent{}) }

func (homedAgent) Kind() agent.Kind { return "homed" }

func (homedAgent) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	c, err := fakeAgent{}.Start(ctx, o)
	if err != nil {
		return nil, err
	}
	home := cmp.Or(o.Home, login(o.Profile.Dir))
	f, _ := os.OpenFile(filepath.Join(o.Profile.Dir, "homes.log"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	f.WriteString(home + "\n")
	f.Close()
	return &homedConn{fakeConn: c.(*fakeConn), dir: o.Profile.Dir, home: home}, nil
}

type homedConn struct {
	*fakeConn
	dir, home string
}

func (c *homedConn) Close() error {
	f, _ := os.OpenFile(filepath.Join(c.dir, "homes.log"), os.O_APPEND|os.O_WRONLY, 0o600)
	f.WriteString("rested\n")
	f.Close()
	return c.fakeConn.Close()
}

func (c *homedConn) Home() string { return c.home }
func (c *homedConn) Moved() bool  { return login(c.dir) != c.home }

func (c *homedConn) Answer(id, option string) error {
	c.events <- event.Message{Role: "assistant", ID: "m2", Tokens: &usage.TokenUsage{Input: 1}} // the cache is warm
	return c.fakeConn.Answer(id, option)
}

// Switched early, a session with a warm cache starts again on the login it
// ran as; told to move, or marked moved while asleep, it starts on the one
// in use.
func TestWarmSessionStaysOnItsLogin(t *testing.T) {
	home := filepath.Dir(setup(t))
	setLogin := func(who string) { _ = os.WriteFile(filepath.Join(home, "login"), []byte(who), 0o600) }
	setLogin("a")
	cfg, err := Spawn(Config{Owner: os.Getpid(), Kind: "homed", Cwd: home, Account: agent.Profile{Kind: "homed", Name: "homed", Dir: home}, Prompt: "hi", IdleStop: Duration(100 * time.Millisecond)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	homes := func() []string {
		b, _ := os.ReadFile(filepath.Join(home, "homes.log"))
		return strings.Fields(string(b))
	}
	turn := func(text string) {
		t.Helper()
		if text != "" {
			if err := c.Send(text); err != nil {
				t.Fatal(err)
			}
		}
		ap := next(t, c, func(ev any) bool { _, ok := ev.(event.Approval); return ok }).(event.Approval)
		if err := c.Allow(ap.ID, nil, false); err != nil {
			t.Fatal(err)
		}
		next(t, c, inState("idle"))
		for range 250 { // it rests once idle
			if got := homes(); got[len(got)-1] == "rested" {
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("never rested: %q", homes())
	}
	turn("")
	setLogin("b")
	turn("warm")
	if got := homes(); len(got) != 4 || got[2] != "a" {
		t.Fatalf("a warm session should stay on a: %q", got)
	}
	if err := c.Relogin(); err != nil {
		t.Fatal(err)
	}
	turn("moved")
	if got := homes(); len(got) != 6 || got[4] != "b" {
		t.Fatalf("told to move, it should start on b: %q", got)
	}
	// Asleep, it can't be told: it reads that it was.
	setLogin("c")
	turn("warm")
	if got := homes(); len(got) != 8 || got[6] != "b" {
		t.Fatalf("a warm session should stay on b: %q", got)
	}
	if err := MarkMoved(); err != nil {
		t.Fatal(err)
	}
	turn("marked")
	if got := homes(); len(got) != 10 || got[8] != "c" {
		t.Fatalf("marked moved, it should start on c: %q", got)
	}
	if err := c.Stop(); err != nil {
		t.Fatal(err)
	}
}
