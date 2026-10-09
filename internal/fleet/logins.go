package fleet

import (
	"cmp"
	"slices"
	"sort"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// LoginView is one login with its plan usage.
type LoginView struct {
	state.Login
	Usage usage.Reading // who it is, and its plan
	// Quota is the plan's limits, as Usage read them.
	Quota   usage.Quota
	Current bool // the one new sessions run as
}

// logins is every saved login with its usage; root is ~/.claude, whose
// saved reading belongs to the login it's signed in as. The one in use is
// the one rush runs sessions as, in its home, or else ~/.claude's.
func (l *Loader) logins(cfg state.Config, root AccountView, now time.Time) []LoginView {
	var out []LoginView
	var using string
	if l.burns == nil {
		l.burns = map[string]usage.Quota{}
	}
	if k, ok := state.Logins(); ok {
		using = k.UsingLogin(cfg)
	}
	for _, lg := range cfg.Logins {
		v := LoginView{Login: lg, Current: lg.ID != "" && lg.ID == root.Usage.AccountID}
		if using != "" {
			v.Current = using == lg.ID
		}
		f, ok := l.fetched[lg.UsageKey()]
		switch {
		case root.Usage.AccountID == lg.ID && (!ok || root.Usage.FetchedAt.After(f.FetchedAt)):
			v.Usage = root.Usage
			v.Usage.Problem = f.Problem
		default:
			v.Usage = f
		}
		v.Usage.Email, v.Usage.Org = lg.Email, lg.Org
		if root.Usage.AccountID == lg.ID {
			v.Usage.Plan, v.Usage.Role, v.Usage.Billing, v.Usage.OrgType, v.Usage.Extra = root.Usage.Plan, root.Usage.Role, root.Usage.Billing, root.Usage.OrgType, root.Usage.Extra
		}
		v.Usage = v.Usage.Since(now)
		v.Quota = usage.Follow(l.burns[lg.ID], v.Usage.Quota(lg.UsageKey()))
		if prev := l.burns[lg.ID]; v.Quota.FetchedAt.Sub(prev.FetchedAt) >= usage.Every {
			l.burns[lg.ID] = v.Quota // the reading its next is measured from
		}
		out = append(out, v)
	}
	return out
}

// NextLogin is the login to switch to. Room an account doesn't use
// before its week resets is lost, so the one to spend is whichever loses
// the most soonest: the most urgent (Urgency) login that isn't nearly out.
// rush moves to it when the one in use is nearly out, stopped says a
// session already hit a limit on it, or another would lose far more room
// by waiting (its week ends in hours, the one in use's in days). Once the
// one in use is out altogether, any login with room left will do.
func NextLogin(logins []LoginView, stopped bool) (LoginView, bool) {
	var cur *LoginView
	var others []LoginView
	for i := range logins {
		switch q := logins[i].Quota; {
		case logins[i].Current:
			cur = &logins[i]
		case time.Since(q.FetchedAt) < otherFor && len(q.Windows) > 0:
			// Only a login with a reading: one rush can't read (signed
			// out, expired) would look empty. A login not in use only
			// empties, so an older reading of it still holds.
			others = append(others, logins[i])
		}
	}
	if cur == nil || len(others) == 0 {
		return LoginView{}, false
	}
	now := time.Now()
	fresh := time.Since(cur.Quota.FetchedAt) < 3*usage.Every
	if !stopped && !fresh {
		return LoginView{}, false
	}
	sort.SliceStable(others, func(i, j int) bool { return Urgency(others[i].Quota, now) > Urgency(others[j].Quota, now) })
	nearly := cur.Quota.NearlyOut("", usage.Lead, now)
	out := stopped || cur.Quota.Used("") >= 100
	for _, o := range others {
		switch {
		case o.Quota.Used("") >= state.SwitchAt, o.Quota.NearlyOut("", usage.Lead, now):
			continue
		case stopped || nearly:
			return o, true
		case 100-o.Quota.Used("") >= minRoom && Urgency(o.Quota, now) >= switchFor*Urgency(cur.Quota, now):
			return o, true
		}
		return LoginView{}, false // the most urgent with room isn't worth moving for
	}
	if !out {
		return LoginView{}, false
	}
	// Out, and every other nearly: whichever has the most room now.
	best := slices.MinFunc(others, func(a, b LoginView) int { return cmp.Compare(a.Quota.Used(""), b.Quota.Used("")) })
	return best, best.Quota.Used("") < 100
}

// Urgency is how fast q must be spent, in percent an hour, not to lose
// any of the room left in its longest window by the time it resets.
func Urgency(q usage.Quota, now time.Time) float64 {
	w, ok := Week(q)
	if !ok {
		return 0
	}
	return (100 - w.Percent) / max(Left(w, now).Hours(), 0.1)
}

// Left is how long until w resets: all of it when it hasn't begun.
func Left(w usage.Window, now time.Time) time.Duration {
	if w.ResetsAt.IsZero() {
		return w.Span
	}
	return w.ResetsAt.Sub(now)
}

// Week is q's longest window that limits every model: its scarce one,
// whose unspent room is lost when it resets (a shorter one's only waits).
func Week(q usage.Quota) (usage.Window, bool) {
	var w usage.Window
	ok := false
	for _, c := range q.Windows {
		if len(c.Scope.Models) == 0 && (!ok || c.Span > w.Span) {
			w, ok = c, true
		}
	}
	return w, ok
}

// switchFor is how many times more urgent another login must be than the
// one in use, with room still, to move to it; minRoom is how much room it
// must have left, so a few percent about to reset isn't worth the move.
const (
	switchFor = 2.0
	minRoom   = 10.0
)

// otherFor is how old a reading of a login not in use may be and still be
// switched to.
const otherFor = time.Hour

// Link is one login in the order rush spends them, and how long until
// it has room again when a window is full now.
type Link struct {
	LoginView
	Wait time.Duration
}

// Chain is the order rush spends the logins in, as NextLogin picks them:
// the one in use, then the rest by Urgency, a full one last by when it
// has room again. A login without a reading is left out.
func Chain(logins []LoginView, now time.Time) []Link {
	var cur, rest []Link
	for _, l := range logins {
		if len(l.Quota.Windows) == 0 {
			continue
		}
		k := Link{LoginView: l}
		for _, w := range l.Quota.Windows {
			if w.Percent >= state.SwitchAt && w.ResetsAt.After(now) {
				k.Wait = max(k.Wait, w.ResetsAt.Sub(now))
			}
		}
		if l.Current {
			cur = append(cur, k)
		} else {
			rest = append(rest, k)
		}
	}
	slices.SortStableFunc(rest, func(a, b Link) int {
		if c := cmp.Compare(Urgency(b.Quota, now), Urgency(a.Quota, now)); c != 0 {
			return c
		}
		return cmp.Compare(a.Wait, b.Wait)
	})
	return append(cur, rest...)
}
