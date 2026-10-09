package usage

import (
	"strings"
	"time"
)

// Expired is a reading's Problem when its sign-in has expired, one rush
// may be able to refresh.
const Expired = "sign-in expired"

// Window is one limit an account's plan puts on it: Claude's five hours
// or week, Codex's primary or secondary, Copilot's month of premium
// requests.
type Window struct {
	ID    string        // the provider's name for it: "five_hour", "seven_day_opus", "primary"
	Label string        // short, for a meter: "5h", "7d", "Opus 7d", "month"
	Name  string        // in words: "5-hour", "weekly", "Opus weekly", "premium requests"
	Span  time.Duration // how long it runs; zero when the provider doesn't say
	// Percent is how much of it is used, 0–100. It is always set, from
	// Used and Limit when the provider counts instead.
	Percent     float64
	Used, Limit float64 // when the provider counts: 212 of 300 requests
	ResetsAt    time.Time
	Scope       Scope
	// Burn is how fast it's been filling lately, in percent an hour,
	// from the reading before (Follow); zero until there's one.
	Burn float64 `json:",omitzero"`
}

// Lead is how far ahead rush looks for an account filling up: past the
// next reading, and the turns already under way on it when it switches.
const Lead = 2 * Every

// Rate is how fast w fills, in percent an hour: its recent burn, or its
// average since it began when that's faster.
func (w Window) Rate(now time.Time) float64 {
	r := w.Burn
	if w.Span > 0 && !w.ResetsAt.IsZero() {
		if gone := w.Span - w.ResetsAt.Sub(now); gone >= 10*time.Minute {
			r = max(r, w.Percent/gone.Hours())
		}
	}
	return r
}

// ahead is how much of lead w can still fill in: none of it past its
// reset, when what's left is gone anyway.
func (w Window) ahead(lead time.Duration, now time.Time) time.Duration {
	if w.ResetsAt.IsZero() {
		return lead
	}
	return max(0, min(lead, w.ResetsAt.Sub(now)))
}

// NearlyOut is whether a window that limits model fills within lead:
// 99% full already, or full before then, and before it resets, as fast as
// it fills.
func (q Quota) NearlyOut(model string, lead time.Duration, now time.Time) bool {
	for _, w := range q.Windows {
		if w.Scope.Covers(model) && (w.Percent >= 99 || w.Percent+w.Rate(now)*w.ahead(lead, now).Hours() >= 100) {
			return true
		}
	}
	return false
}

// SwitchPoint is how full the tightest window for model gets before
// NearlyOut says so: 99%, or sooner the faster it fills.
func (q Quota) SwitchPoint(model string, lead time.Duration, now time.Time) float64 {
	w, _ := q.Tightest(model)
	return max(0, min(99, 100-w.Rate(now)*w.ahead(lead, now).Hours()))
}

// Follow is next with each window's recent burn worked out from prev, an
// earlier reading of the same account at least Every before it: percents
// are whole, so one step a minute apart reads as 60% an hour.
func Follow(prev, next Quota) Quota {
	dt := next.FetchedAt.Sub(prev.FetchedAt)
	ws := make([]Window, len(next.Windows))
	for i, w := range next.Windows {
		p, ok := prev.Window(w.ID)
		switch {
		case !ok || w.Percent < p.Percent: // new, or reset since
		case dt < Every:
			w.Burn = p.Burn // too close together to tell
		default:
			w.Burn = (p.Burn + (w.Percent-p.Percent)/dt.Hours()) / 2
		}
		ws[i] = w
	}
	next.Windows = ws
	return next
}

// Scope is which models a window limits. The zero Scope limits them all.
type Scope struct {
	// Models are matched as substrings of a model's id, so "opus" covers
	// every Opus.
	Models []string
}

// Covers is whether the window limits model. An empty model is any.
func (s Scope) Covers(model string) bool {
	if len(s.Models) == 0 || model == "" {
		return true
	}
	model = strings.ToLower(model)
	for _, m := range s.Models {
		if strings.Contains(model, strings.ToLower(m)) {
			return true
		}
	}
	return false
}

// Source is where a reading came from.
type Source int

const (
	Cached  Source = iota // the agent's own cache of it
	Fetched               // asked of the provider by rush
	Live                  // reported by a session as it ran
)

// Billing is how what a session spends is paid for.
type Billing string

const (
	// Plan is out of a subscription's allowance: its windows fill, and
	// nothing more is charged.
	Plan Billing = "plan"
	// Overage is past the allowance and paid for by use: Claude's extra
	// usage, Codex's credits.
	Overage Billing = "overage"
	// Metered is an API key: every token is paid for.
	Metered Billing = "metered"
)

// Quota is an account's limits as last read. An account belongs to one
// provider, but any number of agents and sessions can spend it.
type Quota struct {
	Account string // the key it is kept under: "claude:login:<uuid>", "codex:chatgpt:<id>"
	Email   string // who the account is, when the reading says
	Plan    string
	// Balance is what's left on a pay-as-you-go account, in its own
	// currency and words: "¥110.00 left". Such an account has no windows.
	Balance   string
	Windows   []Window
	FetchedAt time.Time
	Source    Source
	Problem   string // why there is no fresh reading: not signed in, expired, rate-limited
	// Resets are the limit resets the account has earned; nil when its
	// provider has none, or didn't say.
	Resets *Resets `json:",omitempty"`
}

// Resets are an account's earned limit resets: each one, spent, resets
// the limits it's for at once. Credits has the details the provider gave,
// which may be fewer than Available.
type Resets struct {
	Available int
	Credits   []ResetCredit `json:",omitempty"`
}

// ResetCredit is one earned reset.
type ResetCredit struct {
	ID, Title, About string
	Granted, Expires time.Time // Expires is zero when it doesn't
}

// Tightest is the fullest window that limits model: the one that stops it
// first. ok is false when no window limits it.
func (q Quota) Tightest(model string) (w Window, ok bool) {
	for _, c := range q.Windows {
		if c.Scope.Covers(model) && (!ok || c.Percent > w.Percent) {
			w, ok = c, true
		}
	}
	return w, ok
}

// Used is how full the tightest window for model is, 0–100.
func (q Quota) Used(model string) float64 {
	w, _ := q.Tightest(model)
	return w.Percent
}

// Window is the window called id, if the reading has it.
func (q Quota) Window(id string) (Window, bool) {
	for _, w := range q.Windows {
		if w.ID == id {
			return w, true
		}
	}
	return Window{}, false
}

// Since empties the windows that have reset since the reading was made:
// nothing has been used of them yet, as far as the reading knows.
func (q Quota) Since(now time.Time) Quota {
	ws := make([]Window, len(q.Windows))
	for i, w := range q.Windows {
		if !w.ResetsAt.IsZero() && now.After(w.ResetsAt) {
			w.Percent, w.Used, w.ResetsAt = 0, 0, time.Time{}
		}
		ws[i] = w
	}
	q.Windows = ws
	return q
}
