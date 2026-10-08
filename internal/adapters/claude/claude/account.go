package claude

import (
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Account is one Claude Code config home. The default account is ~/.claude
// with its state in ~/.claude.json; any other lives wherever CLAUDE_CONFIG_DIR
// points and keeps its state file inside that directory.
type Account struct {
	Name      string `json:"name"`
	ConfigDir string `json:"configDir"`
}

// Kind is Claude Code's among rush's agents.
const Kind agent.Kind = "claude"

// AccountOf is a profile as the Claude config folder it is.
func AccountOf(p agent.Profile) Account {
	return Account{Name: p.Name, ConfigDir: p.Dir}
}

// Profile is the config folder as rush's own profile of Claude Code.
func (a Account) Profile() agent.Profile {
	return agent.Profile{Kind: Kind, Name: a.Name, Dir: a.ConfigDir}
}

func DefaultAccount() Account {
	home, _ := os.UserHomeDir()
	return Account{Name: "default", ConfigDir: filepath.Join(home, ".claude")}
}

func (a Account) IsDefault() bool {
	return a.ConfigDir == DefaultAccount().ConfigDir
}

func (a Account) JobsDir() string     { return filepath.Join(a.ConfigDir, "jobs") }
func (a Account) ProjectsDir() string { return filepath.Join(a.ConfigDir, "projects") }
func (a Account) RosterPath() string  { return filepath.Join(a.ConfigDir, "daemon", "roster.json") }
func (a Account) PRCachePath() string { return filepath.Join(a.ConfigDir, "gh-pr-status-cache.json") }

func (a Account) StatePath() string {
	if a.IsDefault() {
		return a.ConfigDir + ".json"
	}
	return filepath.Join(a.ConfigDir, ".claude.json")
}

// Env is what a child claude process needs to act as this account.
// inherited are the markers a Claude Code session sets for what it runs.
// rush started from inside one would pass them on, and the sessions rush
// starts would think they're that session's children: Claude Code then
// saves no transcript for them, among other things.
var inherited = map[string]bool{
	"CLAUDECODE": true, "CLAUDE_CODE_CHILD_SESSION": true, "CLAUDE_CODE_SESSION_ID": true,
	"CLAUDE_CODE_ENTRYPOINT": true, "CLAUDE_CODE_SESSION_ATTENDED": true, "CLAUDE_CODE_EXECPATH": true,
	"CLAUDE_CODE_MESSAGING_SOCKET": true, "CLAUDE_CODE_MESSAGING_TOKEN": true,
	"CLAUDE_PID": true, "CLAUDE_JOB_DIR": true, "CLAUDE_CODE_VERSION": true,
}

func (a Account) Env() []string {
	var env []string
	for _, e := range os.Environ() {
		name, _, _ := strings.Cut(e, "=")
		if name == "CLAUDE_CONFIG_DIR" || inherited[name] {
			continue // another account's, or the session rush was started from
		}
		env = append(env, e)
	}
	if a.IsDefault() {
		return env
	}
	return append(env, "CLAUDE_CONFIG_DIR="+a.ConfigDir)
}

type Window struct {
	Present  bool
	Percent  float64
	ResetsAt time.Time
}

type Usage struct {
	AccountID string // the signed-in account's uuid
	Email     string
	Org       string
	Plan      string
	FiveHour  Window
	SevenDay  Window
	FetchedAt time.Time
	Problem   string // why no fresh reading: not signed in, expired, rate-limited
	Fetched   bool   // read from Anthropic by rush, not Claude Code's cache
	Role      string
	Billing   string
	OrgType   string
	Extra     bool
}

// Used is the fuller of the two windows, in percent: how close the account
// is to being stopped.
func (u Usage) Used() float64 {
	var p float64
	for _, w := range []Window{u.FiveHour, u.SevenDay} {
		if w.Present {
			p = max(p, w.Percent)
		}
	}
	return p
}

// Since empties the windows that have reset since the reading was made:
// nothing has been used of them yet, as far as the reading knows.
func (u Usage) Since(now time.Time) Usage {
	for _, w := range []*Window{&u.FiveHour, &u.SevenDay} {
		if w.Present && !w.ResetsAt.IsZero() && now.After(w.ResetsAt) {
			*w = Window{Present: true}
		}
	}
	return u
}

// LiveUsage is the plan usage a Claude Code session reports as it runs (a
// rate_limit_event's info): the same windows the usage endpoint gives,
// fresh with every request.
func LiveUsage(info []byte, at time.Time) (Usage, bool) {
	var r struct {
		Windows map[string]*struct {
			Utilization *float64 `json:"utilization"`
			ResetsAt    int64    `json:"resetsAt"`
		} `json:"unifiedWindows"`
	}
	if jsonx.Unmarshal(info, &r) != nil {
		return Usage{}, false
	}
	u := Usage{FetchedAt: at, Fetched: true}
	for name, w := range map[string]*Window{"five_hour": &u.FiveHour, "seven_day": &u.SevenDay} {
		if raw := r.Windows[name]; raw != nil && raw.Utilization != nil {
			*w = Window{Present: true, Percent: *raw.Utilization * 100}
			if raw.ResetsAt > 0 {
				w.ResetsAt = time.Unix(raw.ResetsAt, 0)
			}
		}
	}
	return u, u.FiveHour.Present || u.SevenDay.Present
}

type usageFile struct {
	OAuthAccount *struct {
		AccountUUID      string `json:"accountUuid"`
		EmailAddress     string `json:"emailAddress"`
		OrganizationName string `json:"organizationName"`
		OrganizationRole string `json:"organizationRole"`
		OrganizationType string `json:"organizationType"`
		ExtraUsage       bool   `json:"hasExtraUsageEnabled"`
		BillingType      string `json:"billingType"`
		SeatTier         string `json:"seatTier"`
		UserRateLimit    string `json:"userRateLimitTier"`
	} `json:"oauthAccount"`
	Cached *struct {
		FetchedAtMs int64 `json:"fetchedAtMs"`
		Utilization struct {
			FiveHour *rawWindow `json:"five_hour"`
			SevenDay *rawWindow `json:"seven_day"`
		} `json:"utilization"`
	} `json:"cachedUsageUtilization"`
}

type rawWindow struct {
	Utilization float64 `json:"utilization"`
	ResetsAt    string  `json:"resets_at"`
}

func (w *rawWindow) window() Window {
	if w == nil {
		return Window{}
	}
	t, _ := time.Parse(time.RFC3339, w.ResetsAt)
	return Window{Present: true, Percent: w.Utilization, ResetsAt: t}
}

// ReadUsage reads the plan usage Claude Code already cached for this account.
// It never touches credentials or the network.
func ReadUsage(a Account) (Usage, error) {
	b, err := os.ReadFile(a.StatePath())
	if err != nil {
		return Usage{}, err
	}
	var f usageFile
	if err := jsonx.Unmarshal(b, &f); err != nil {
		return Usage{}, err
	}
	var u Usage
	if o := f.OAuthAccount; o != nil {
		u.AccountID, u.Email, u.Org = o.AccountUUID, o.EmailAddress, o.OrganizationName
		u.Role, u.Billing, u.OrgType, u.Extra = o.OrganizationRole, o.BillingType, o.OrganizationType, o.ExtraUsage
		for _, v := range []string{o.UserRateLimit, o.SeatTier, o.OrganizationName, o.BillingType} {
			if v != "" {
				u.Plan = v
				break
			}
		}
	}
	if c := f.Cached; c != nil {
		u.FiveHour = c.Utilization.FiveHour.window()
		u.SevenDay = c.Utilization.SevenDay.window()
		u.FetchedAt = time.UnixMilli(c.FetchedAtMs)
	}
	return u, nil
}
