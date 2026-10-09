package claude

import (
	"context"
	"os"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// plans are the plan readings Claude Code cached in each folder's state
// file, with the file's time when read.
var plans struct {
	sync.Mutex
	by map[string]planEntry
}

type planEntry struct {
	r   usage.Reading
	mod time.Time
}

// Plan is the plan Claude Code last cached for p's account.
func (Adapter) Plan(p agent.Profile) usage.Reading {
	a := Account(p)
	st, err := os.Stat(a.StatePath())
	if err != nil {
		return usage.Reading{}
	}
	plans.Lock()
	defer plans.Unlock()
	if e, ok := plans.by[a.ConfigDir]; ok && e.mod.Equal(st.ModTime()) {
		return e.r
	}
	u, _ := claude.ReadUsage(a)
	if plans.by == nil {
		plans.by = map[string]planEntry{}
	}
	plans.by[a.ConfigDir] = planEntry{r: u.Reading(), mod: st.ModTime()}
	return u.Reading()
}

// Readings are the readings shared through path; one Anthropic said to
// wait for says until when.
func (Adapter) Readings(path string) map[string]usage.Reading {
	out := map[string]usage.Reading{}
	all := claude.LoadFetchedUsage(path)
	for k := range all {
		f := all[k]
		if time.Now().Before(f.Wait) {
			f.Usage.Problem = "rate-limited to " + f.Wait.Local().Format("15:04")
		}
		out[k] = f.Usage.Reading()
	}
	return out
}

// RefreshPlan is p's plan, asked of Anthropic when the shared reading is
// old enough.
func (Adapter) RefreshPlan(path string, p agent.Profile, offline bool) usage.Reading {
	return claude.RefreshUsage(path, Account(p), offline).Reading()
}

// FindLogins keeps the sign-in ~/.claude holds now in the vault (Claude
// Code replaces it as it refreshes) and reports it. It also takes in the
// folders an older rush was given: each one's login is reported, its
// sign-in put in the vault unless cfg says that was done already, and its
// past sessions copied into ~/.claude, where they're found and resumed
// like any other. imported is whether every older folder was taken in
// whole, and cfg can forget them.
// It fails when ~/.claude's sign-in can't be kept: without a copy rush
// never switches away from it.
//
// A sign-in in ~/.claude that isn't the account it names was put back by
// a Claude Code started before a switch, as it refreshed its own: the
// switch is made again, and restored says so.
func (Adapter) FindLogins(cfg state.Config) (found []state.FoundLogin, restored *state.Restored, imported bool, failed error) { //nolint:gocritic // state.LoginKeeper's signature
	v := claude.TheVault()
	root := claude.Active(cfg)
	lg, owner, ok, err := v.Keep(root)
	if ok && err == nil {
		found = append(found, state.FoundLogin{Login: lg})
	}
	failed = err
	// A home's sign-in is the newest of its login's: the vault keeps a
	// copy, in case the home goes.
	for _, l := range cfg.Logins {
		if h := claude.HomeOf(l.ID); claude.HasHome(h) {
			_, _, _, _ = v.Keep(h)
		}
	}
	if ok && err == nil && owner != "" && putBack(owner, lg.ID) {
		restored = &state.Restored{Was: owner, Now: lg.ID, Err: v.Use(root, lg)}
		if restored.Err != nil {
			// It can't be switched back: it's signed in as owner,
			// so it says so.
			for _, l := range cfg.Logins {
				if l.ID == owner && len(l.Profile) > 0 {
					_ = claude.Name(root, l)
				}
			}
		}
	}
	imported = true
	for _, f := range cfg.OldFolders() {
		a := claude.Account(f)
		if claude.MergeHistory(a, root) != nil {
			imported = false
		}
		if cfg.FoldersImported {
			continue
		}
		lg, cred, ok := claude.Signed(a)
		if !ok {
			continue
		}
		if _, err := v.Get(lg.ID); err != nil {
			if v.Put(lg.ID, cred) != nil {
				imported = false
				continue
			}
		}
		found = append(found, state.FoundLogin{Login: lg, Name: a.Name})
	}
	return found, restored, imported, failed
}

// mismatch is the sign-in found in ~/.claude as another account's than
// it names, and when.
var mismatch struct {
	sync.Mutex
	was, now string
	at       time.Time
}

// putBack is whether ~/.claude signed in as was while naming now is a
// switch put back rather than a sign-in under way (claude /login writes
// the sign-in a moment before the name): it has to be seen twice, at least
// half a minute apart.
func putBack(was, now string) bool {
	mismatch.Lock()
	defer mismatch.Unlock()
	if mismatch.was != was || mismatch.now != now || time.Since(mismatch.at) > 10*time.Minute {
		mismatch.was, mismatch.now, mismatch.at = was, now, time.Now()
		return false
	}
	return time.Since(mismatch.at) >= 30*time.Second
}

// RefreshLogin is a login's plan usage, shared through path like every
// account's. It's asked with the login's freshest sign-in: ~/.claude's
// when it's signed in as it, its home's when it has one, else the one the
// vault kept.
func (Adapter) RefreshLogin(path string, cfg state.Config, lg state.Login, offline bool) usage.Reading { //nolint:gocritic // state.LoginKeeper's signature
	root := claude.Active(cfg)
	if claude.SignedInAs(root) == lg.ID {
		return claude.RefreshUsage(path, root, offline).Reading()
	}
	if h := claude.HomeOf(lg.ID); claude.HasHome(h) {
		return claude.RefreshUsage(path, h, offline).Reading()
	}
	return claude.RefreshUsageFor(path, lg.UsageKey(), offline, func(ctx context.Context) (claude.Usage, error) {
		return claude.RenewKeptOnExpiry(ctx, lg.ID, func() (claude.Usage, error) {
			cred, err := state.Vault().Get(lg.ID)
			if err != nil {
				return claude.Usage{}, claude.ErrNotSignedIn
			}
			return claude.FetchUsageAs(ctx, cred, lg.ID)
		})
	}).Reading()
}

// RenewLogin refreshes a login's freshest sign-in, where RefreshLogin
// reads it: ~/.claude's, its home's, or the vault's.
func (Adapter) RenewLogin(cfg state.Config, lg state.Login) error { //nolint:gocritic // state.LoginKeeper's signature
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Second)
	defer cancel()
	if root := claude.Active(cfg); claude.SignedInAs(root) == lg.ID {
		return claude.Renew(ctx, root)
	}
	if h := claude.HomeOf(lg.ID); claude.HasHome(h) {
		return claude.Renew(ctx, h)
	}
	return claude.RenewKept(ctx, lg.ID)
}

// UsingLogin is the login new sessions run as in its home, when it's one
// cfg keeps.
func (Adapter) UsingLogin(cfg state.Config) string { //nolint:gocritic // state.LoginKeeper's signature
	id := claude.UsingShown() // the fleet's, every reading
	if _, ok := cfg.Login(id); id == "" || !ok {
		return ""
	}
	return id
}

// UseLogin readies l's home and makes it the one new sessions run in.
func (Adapter) UseLogin(cfg state.Config, l state.Login) error { //nolint:gocritic // state.LoginKeeper's signature
	return claude.UseLogin(claude.Active(cfg), l)
}

// AdoptLogin keeps the sign-in just made in p as its login's.
func (Adapter) AdoptLogin(p agent.Profile) (state.Login, error) {
	return claude.AdoptLogin(Account(p))
}

// ForgetLogin drops a login's vault sign-in and its home.
func (Adapter) ForgetLogin(id string) error { return claude.ForgetLogin(id) }

var (
	_ agent.PlanReader  = Adapter{}
	_ state.LoginKeeper = Adapter{}
)
