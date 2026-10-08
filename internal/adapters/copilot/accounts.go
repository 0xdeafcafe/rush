package copilot

import (
	"errors"
	"os/exec"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Copilot's accounts are the GitHub accounts gh is signed in to. Which one
// Copilot runs on is rush's to say (its config's Using), not gh's active
// account, so switching Copilot leaves the rest of your gh alone.

func accountKey(login string) string { return "copilot:gh:" + login }

func accountOf(login string) agent.Account {
	return agent.Account{Kind: Kind, ID: login, Key: accountKey(login), Name: login, Email: login}
}

// chosen is the GitHub account rush runs Copilot on; empty is gh's
// active one. It's read from rush's config at most every half minute.
func chosen() string {
	chosenMu.Lock()
	defer chosenMu.Unlock()
	if time.Since(chosenAt) > 30*time.Second {
		chosenVal, chosenAt = state.Load().Config.Using[string(Kind)], time.Now()
	}
	return chosenVal
}

var (
	chosenMu  sync.Mutex
	chosenVal string
	chosenAt  time.Time
)

// ghLogins are the accounts gh is signed in to on github.com, and which
// is active.
func ghLogins() (logins []string, active string, err error) {
	out, err := exec.Command(gh(), "auth", "status", "-h", "github.com", "--json", "hosts").Output()
	if err != nil {
		return nil, "", errors.New("copilot: gh isn't signed in to GitHub (gh auth login)")
	}
	var st struct {
		Hosts map[string][]struct {
			Login  string `json:"login"`
			Active bool   `json:"active"`
			State  string `json:"state"`
		} `json:"hosts"`
	}
	if err := jsonx.Unmarshal(out, &st); err != nil {
		return nil, "", err
	}
	for _, h := range st.Hosts["github.com"] {
		logins = append(logins, h.Login)
		if h.Active {
			active = h.Login
		}
	}
	return logins, active, nil
}

// Known are the GitHub accounts gh is signed in to.
func (Adapter) Known() []agent.Account { return Logins() }

// ReadsAnyAccount: any gh account's premium requests can be read.
func (Adapter) ReadsAnyAccount() {}

// Logins are the GitHub accounts gh is signed in to.
func Logins() []agent.Account {
	logins, _, _ := ghLogins()
	var out []agent.Account
	for _, l := range logins {
		out = append(out, accountOf(l))
	}
	return out
}

// Current is the GitHub account Copilot runs on.
func (Adapter) Current(agent.Profile) (agent.Account, error) {
	if c := chosen(); c != "" {
		return accountOf(c), nil
	}
	_, active, err := ghLogins()
	if err != nil {
		return agent.Account{}, err
	}
	if active == "" {
		return agent.Account{}, errors.New("copilot: gh has no active GitHub account")
	}
	return accountOf(active), nil
}

// Switch checks gh has a sign-in for a; rush then records a as the
// account Copilot runs on.
func (Adapter) Switch(_ agent.Profile, a agent.Account) error {
	logins, _, err := ghLogins()
	if err != nil {
		return err
	}
	for _, l := range logins {
		if l == a.ID {
			chosenMu.Lock()
			chosenVal, chosenAt = a.ID, time.Now()
			chosenMu.Unlock()
			forgetTokens()
			return nil
		}
	}
	return errors.New("gh isn't signed in as " + a.ID + "; sign in to it again")
}

// SignIn adds a GitHub account to gh; done puts back the account gh had
// active before, so only Copilot moves when you switch to the new one.
func (Adapter) SignIn(agent.Profile) (*exec.Cmd, func() (agent.Account, error), error) {
	before, was, _ := ghLogins()
	cmd := exec.Command(gh(), "auth", "login", "-h", "github.com", "--web")
	done := func() (agent.Account, error) {
		after, active, err := ghLogins()
		if err != nil {
			return agent.Account{}, err
		}
		added := active
		known := map[string]bool{}
		for _, l := range before {
			known[l] = true
		}
		for _, l := range after {
			if !known[l] {
				added = l
			}
		}
		if added == "" {
			return agent.Account{}, errors.New("not signed in")
		}
		if was != "" && was != active {
			_ = exec.Command(gh(), "auth", "switch", "-h", "github.com", "-u", was).Run()
		}
		forgetTokens()
		return accountOf(added), nil
	}
	return cmd, done, nil
}

// Forget leaves gh's sign-in be: it's gh's, and other things use it.
func (Adapter) Forget(agent.Account) error { return nil }

var (
	_ agent.Accounts        = Adapter{}
	_ agent.Known           = Adapter{}
	_ agent.AnyAccountQuota = Adapter{}
)
