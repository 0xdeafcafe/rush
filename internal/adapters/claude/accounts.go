package claude

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// keyPrefix marks a Claude login's account key: the rest is where its
// reading is kept in usage.json.
const keyPrefix = "claude:"

// accountOf is a saved login as rush's own account.
func accountOf(l claude.Login) agent.Account {
	return agent.Account{Kind: Kind, ID: l.ID, Key: keyPrefix + l.UsageKey(), Name: l.Name, Email: l.Email, Org: l.Org}
}

// Accounts are the logins rush keeps, which ~/.claude can be signed in as.
func (a Adapter) Accounts() []agent.Account {
	var out []agent.Account
	for _, l := range a.config().Logins {
		out = append(out, accountOf(l))
	}
	return out
}

func (a Adapter) login(acct agent.Account) (claude.Login, bool) {
	for _, l := range a.config().Logins {
		if keyPrefix+l.UsageKey() == acct.Key {
			return l, true
		}
	}
	return claude.Login{}, false
}

// Current is the login p's sessions run as: the one in use, in its home,
// or the one p is signed in as.
func (a Adapter) Current(p agent.Profile) (agent.Account, error) {
	id := claude.SignedInAs(a.runAs())
	if id == "" {
		return agent.Account{}, claude.ErrNotSignedIn
	}
	for _, l := range a.config().Logins {
		if l.ID == id {
			return accountOf(l), nil
		}
	}
	return agent.Account{Kind: Kind, Key: keyPrefix + claude.Login{ID: id}.UsageKey()}, nil
}

// Switch signs p in as acct, from the sign-in rush keeps for it.
func (a Adapter) Switch(p agent.Profile, acct agent.Account) error {
	l, ok := a.login(acct)
	if !ok {
		return errors.New("rush has no sign-in for " + acct.Name)
	}
	return claude.UseLogin(Account(p), l)
}

// SignIn is Claude Code's own sign-in, in a folder of its own: done keeps
// the sign-in in the vault and removes the folder, so p stays as it is
// until you switch.
func (Adapter) SignIn(p agent.Profile) (*exec.Cmd, func() (agent.Account, error), error) {
	b := make([]byte, 4)
	_, _ = rand.Read(b)
	scratch := claude.Account{Name: "sign-in", ConfigDir: filepath.Join(state.Dir(), "signin-"+hex.EncodeToString(b))}
	c := exec.Command("claude", "auth", "login")
	c.Env = scratch.Env()
	done := func() (agent.Account, error) {
		l, err := claude.AdoptLogin(scratch)
		if err != nil {
			return agent.Account{}, err
		}
		acct := accountOf(l)
		acct.Plan = planOf(l.Profile)
		return acct, nil
	}
	return c, done, nil
}

// planOf is a sign-in's plan, from who Claude Code says it is.
func planOf(prof []byte) string {
	var p struct {
		Tier string `json:"organizationRateLimitTier"`
		Type string `json:"organizationType"`
	}
	_ = jsonx.Unmarshal(prof, &p)
	return firstOf(p.Type, p.Tier)
}

func firstOf(vs ...string) string {
	for _, v := range vs {
		if v != "" {
			return v
		}
	}
	return ""
}

// Forget drops rush's copy of a's sign-in.
func (Adapter) Forget(a agent.Account) error {
	return claude.ForgetLogin(a.ID)
}

// Quota asks Anthropic for acct's limits with the sign-in rush keeps
// for it, whichever folder is signed in as it. It doesn't share or cache the reading: see claude.RefreshUsageFor.
func (a Adapter) Quota(ctx context.Context, _ agent.Profile, acct agent.Account) (usage.Quota, error) {
	id := strings.TrimPrefix(strings.TrimPrefix(acct.Key, keyPrefix), "login:")
	cred, err := state.Vault().Get(id)
	if err != nil {
		return usage.Quota{}, claude.ErrNotSignedIn
	}
	u, err := claude.FetchUsageAs(ctx, cred, id)
	if err != nil {
		return usage.Quota{}, err
	}
	q := u.Quota(acct.Key)
	q.Source = usage.Fetched
	return q, nil
}

// SignedInAs is the login p's folder is signed in as.
func (Adapter) SignedInAs(p agent.Profile) string { return claude.SignedInAs(Account(p)) }

var (
	_ agent.Accounts     = Adapter{}
	_ agent.QuotaSource  = Adapter{}
	_ agent.SignInReader = Adapter{}
)
