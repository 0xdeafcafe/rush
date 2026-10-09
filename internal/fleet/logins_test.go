package fleet

import (
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

func loginAt(id string, current bool, fiveHour, sevenDay float64) LoginView {
	return LoginView{
		ID:      id,
		Name:    id,
		Current: current,
		Quota: claude.Usage{
			FetchedAt: time.Now(),
			FiveHour:  claude.Window{Present: true, Percent: fiveHour},
			SevenDay:  claude.Window{Present: true, Percent: sevenDay},
		}.Quota(id),
	}
}

func TestNextLogin(t *testing.T) {
	for _, tc := range []struct {
		name    string
		logins  []LoginView
		stopped bool
		want    string // "" for no switch
	}{
		{"room left", []LoginView{loginAt("a", true, 60, 40), loginAt("b", false, 0, 0)}, false, ""},
		{"idle at 96% runs on", []LoginView{loginAt("a", true, 96, 40), loginAt("b", false, 10, 20)}, false, ""},
		{"5h nearly out", []LoginView{loginAt("a", true, 99, 40), loginAt("b", false, 10, 20)}, false, "b"},
		{"7d nearly out", []LoginView{loginAt("a", true, 10, 99), loginAt("b", false, 10, 20)}, false, "b"},
		{"most week to lose wins", []LoginView{loginAt("a", true, 99, 40), loginAt("b", false, 50, 20), loginAt("c", false, 5, 30)}, false, "b"},
		{"others nearly out too", []LoginView{loginAt("a", true, 99, 40), loginAt("b", false, 95, 20)}, false, ""},
		{"out, the others nearly", []LoginView{loginAt("a", true, 100, 27), loginAt("b", false, 98, 12), loginAt("c", false, 0, 97)}, false, "c"},
		{"out, the others too", []LoginView{loginAt("a", true, 100, 27), loginAt("b", false, 100, 12)}, false, ""},
		{"stopped, the others nearly out", []LoginView{loginAt("a", true, 90, 40), loginAt("b", false, 97, 20)}, true, "b"},
		{"stopped switches below the threshold", []LoginView{loginAt("a", true, 80, 40), loginAt("b", false, 90, 20)}, true, "b"},
		{"only one", []LoginView{loginAt("a", true, 99, 99)}, true, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := NextLogin(tc.logins, tc.stopped)
			if tc.want == "" {
				if ok {
					t.Fatalf("switched to %s, want no switch", got.ID)
				}
				return
			}
			if !ok || got.ID != tc.want {
				t.Fatalf("got %q (%v), want %q", got.ID, ok, tc.want)
			}
		})
	}
}

// Room left in a week that resets in hours is lost unless it's spent now;
// one that resets in days can wait.
func TestNextLoginSpendsWhatResetsSoonest(t *testing.T) {
	week := func(l LoginView, in time.Duration) LoginView {
		for i := range l.Quota.Windows {
			if l.Quota.Windows[i].ID == "seven_day" {
				l.Quota.Windows[i].ResetsAt = time.Now().Add(in)
			}
		}
		return l
	}
	for _, tc := range []struct {
		name   string
		logins []LoginView
		want   string
	}{
		{"hours beats days", []LoginView{week(loginAt("a", true, 12, 2), 72*time.Hour), week(loginAt("b", false, 0, 77), 4*time.Hour)}, "b"},
		{"days stays on hours", []LoginView{week(loginAt("a", true, 12, 77), 4*time.Hour), week(loginAt("b", false, 0, 2), 72*time.Hour)}, ""},
		{"too little to move for", []LoginView{week(loginAt("a", true, 12, 2), 72*time.Hour), week(loginAt("b", false, 0, 95), time.Hour)}, ""},
		{"nearly out takes the most urgent", []LoginView{week(loginAt("a", true, 99, 2), 72*time.Hour), week(loginAt("b", false, 0, 0), 100*time.Hour), week(loginAt("c", false, 0, 60), 10*time.Hour)}, "c"},
		{"out of 5h for now", []LoginView{week(loginAt("a", true, 12, 2), 72*time.Hour), week(loginAt("b", false, 100, 77), 4*time.Hour)}, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := NextLogin(tc.logins, false)
			if tc.want == "" {
				if ok {
					t.Fatalf("switched to %s, want no switch", got.ID)
				}
				return
			}
			if !ok || got.ID != tc.want {
				t.Fatalf("got %q (%v), want %q", got.ID, ok, tc.want)
			}
		})
	}
}

func TestNextLoginSkipsLoginsWithoutReading(t *testing.T) {
	unread := LoginView{ID: "b"}
	stale := loginAt("c", false, 0, 0)
	stale.Quota.FetchedAt = time.Now().Add(-2 * time.Hour)
	older := loginAt("e", false, 0, 0)
	older.Quota.FetchedAt = time.Now().Add(-40 * time.Minute)
	if got, ok := NextLogin([]LoginView{loginAt("a", true, 100, 55), older}, false); !ok || got.ID != "e" {
		t.Fatalf("got %q (%v), want e: a login not in use only empties", got.ID, ok)
	}
	got, ok := NextLogin([]LoginView{loginAt("a", true, 100, 55), unread, stale, loginAt("d", false, 1, 0)}, true)
	if !ok || got.ID != "d" {
		t.Fatalf("got %q (%v), want d, the only one with a reading", got.ID, ok)
	}
}

func TestNextLoginIgnoresStaleReading(t *testing.T) {
	cur := loginAt("a", true, 99, 40)
	cur.Quota.FetchedAt = time.Now().Add(-time.Hour)
	if _, ok := NextLogin([]LoginView{cur, loginAt("b", false, 0, 0)}, false); ok {
		t.Fatal("switched on an hour-old reading")
	}
}

// A reading made before ~/.claude was signed in as another account is that
// account's: it mustn't be shown, or switched on, as the new one's.
func TestFreshestSkipsOtherAccountsReading(t *testing.T) {
	l := NewLoader(&state.Store{})
	acct := claude.Account{Name: "default", ConfigDir: "/x"}
	now := time.Now()
	l.SetFetched(acct.ConfigDir, claude.Usage{AccountID: "old", FetchedAt: now, FiveHour: claude.Window{Present: true, Percent: 97}}.Reading())
	l.takeIn()
	cached := claude.Usage{AccountID: "new", FetchedAt: now.Add(-time.Minute), FiveHour: claude.Window{Present: true, Percent: 3}}
	if u := l.freshest(acct.ConfigDir, cached.Reading()); u.AccountID != "new" || fiveHour(u) != 3 {
		t.Fatalf("got %s at %.0f%%, want new at 3%%", u.AccountID, fiveHour(u))
	}
	l.SetFetched(acct.ConfigDir, claude.Usage{AccountID: "new", FetchedAt: now, FiveHour: claude.Window{Present: true, Percent: 5}}.Reading())
	l.takeIn()
	if u := l.freshest(acct.ConfigDir, cached.Reading()); fiveHour(u) != 5 {
		t.Fatalf("got %.0f%%, want the newer reading's 5%%", fiveHour(u))
	}
}

func TestLoginsUseOwnReadingAfterSwitch(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	l := NewLoader(&state.Store{})
	now := time.Now()
	cfg := state.Config{Logins: []state.Login{{ID: "a", Name: "a"}, {ID: "b", Name: "b"}}}
	l.SetFetched("login:a", claude.Usage{AccountID: "a", FetchedAt: now.Add(-time.Minute), FiveHour: claude.Window{Present: true, Percent: 97}}.Reading())
	l.SetFetched("login:b", claude.Usage{AccountID: "b", FetchedAt: now.Add(-time.Minute), FiveHour: claude.Window{Present: true, Percent: 4}}.Reading())
	l.takeIn()
	// ~/.claude is now signed in as b, with a fresher reading of its own.
	root := AccountView{Usage: claude.Usage{AccountID: "b", FetchedAt: now, FiveHour: claude.Window{Present: true, Percent: 6}}.Reading()}
	got := l.logins(cfg, root, now)
	if !got[1].Current || fiveHour(got[1].Usage) != 6 {
		t.Fatalf("b: current %v at %.0f%%, want current at 6%%", got[1].Current, fiveHour(got[1].Usage))
	}
	if got[0].Current || fiveHour(got[0].Usage) != 97 {
		t.Fatalf("a: current %v at %.0f%%, want its own 97%%", got[0].Current, fiveHour(got[0].Usage))
	}
}

// A session's reading of the login ~/.claude is signed in as, kept by
// login, is the folder's usage as soon as it's newer.
func TestFreshestTakesLoginReading(t *testing.T) {
	l := NewLoader(&state.Store{})
	l.UsagePath = filepath.Join(t.TempDir(), "usage.json")
	acct := claude.Account{Name: "default", ConfigDir: "/x"}
	now := time.Now()
	l.SetFetched(acct.ConfigDir, claude.Usage{AccountID: "a", FetchedAt: now.Add(-4 * time.Minute), FiveHour: claude.Window{Present: true, Percent: 90}}.Reading())
	l.takeIn()
	_ = claude.RecordUsage(l.UsagePath, "login:a", claude.Usage{AccountID: "a", FetchedAt: now, FiveHour: claude.Window{Present: true, Percent: 96}})
	l.syncUsage()
	cached := claude.Usage{AccountID: "a", FetchedAt: now.Add(-time.Hour)}
	if u := l.freshest(acct.ConfigDir, cached.Reading()); fiveHour(u) != 96 {
		t.Fatalf("got %.0f%%, want the session's 96%%", fiveHour(u))
	}
}

// The login in use in its home is the current one, whatever ~/.claude is
// signed in as, and ~/.claude's reading stays with ~/.claude's login.
func TestLoginInUseInItsHome(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	l := NewLoader(&state.Store{})
	now := time.Now()
	cfg := state.Config{Logins: []state.Login{{ID: "a", Name: "a"}, {ID: "b", Name: "b"}}}
	l.SetFetched("login:a", claude.Usage{AccountID: "a", FetchedAt: now, FiveHour: claude.Window{Present: true, Percent: 30}}.Reading())
	l.takeIn()
	if err := claude.SetUsing("a"); err != nil {
		t.Fatal(err)
	}
	root := AccountView{Usage: claude.Usage{AccountID: "b", FetchedAt: now, FiveHour: claude.Window{Present: true, Percent: 80}}.Reading()}
	got := l.logins(cfg, root, now)
	if !got[0].Current || fiveHour(got[0].Usage) != 30 {
		t.Fatalf("a: current %v at %.0f%%, want current at its own 30%%", got[0].Current, fiveHour(got[0].Usage))
	}
	if got[1].Current || fiveHour(got[1].Usage) != 80 {
		t.Fatalf("b: current %v at %.0f%%, want not current at ~/.claude's 80%%", got[1].Current, fiveHour(got[1].Usage))
	}
}

// fiveHour is how much of r's 5-hour window is used.
func fiveHour(r usage.Reading) float64 {
	for i := range r.Windows {
		if r.Windows[i].ID == "five_hour" {
			return r.Windows[i].Percent
		}
	}
	return 0
}

// The screenshot's three: borrowed in use, personal's week ending in 4h
// once its 5h is back in 3m, alex's week full for 42h.
func TestChain(t *testing.T) {
	now := time.Now()
	at := func(l LoginView, fiveIn, weekIn time.Duration) LoginView {
		for i := range l.Quota.Windows {
			switch l.Quota.Windows[i].ID {
			case "five_hour":
				l.Quota.Windows[i].ResetsAt = now.Add(fiveIn)
			case "seven_day":
				l.Quota.Windows[i].ResetsAt = now.Add(weekIn)
			}
		}
		return l
	}
	got := Chain([]LoginView{
		at(loginAt("alex", false, 0, 100), 5*time.Hour, 42*time.Hour),
		at(loginAt("borrowed", true, 12, 2), 4*time.Hour, 72*time.Hour),
		at(loginAt("personal", false, 100, 77), 3*time.Minute, 4*time.Hour),
		{Login: state.Login{ID: "unread"}},
	}, now)
	var names []string
	for _, k := range got {
		names = append(names, k.Name)
	}
	if strings.Join(names, ",") != "borrowed,personal,alex" {
		t.Fatalf("got %v, want borrowed, personal, alex", names)
	}
	if got[1].Wait != 3*time.Minute || got[2].Wait != 42*time.Hour {
		t.Fatalf("waits %v, %v: want 3m, 42h", got[1].Wait, got[2].Wait)
	}
}
