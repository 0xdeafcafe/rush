// Package health is what every rush process knows together about whether
// each provider's login works: one record per harness and account, kept in
// one file under state.Dir, of when a turn last got an answer and how its
// requests last failed.
//
// Only failures that say something about the provider count (see
// failure.Kind): a login that stopped working makes it failing at once;
// the provider failing for a moment does after Streak failures in a row, or
// two or more spanning StreakWindow, with no answer between. Being busy or
// rate limited never does, and isn't kept. The error kept is
// failure.Summary's, so nothing a token or key could be in reaches the
// file.
//
// Every time in a record only moves forward, and a record's state counts
// only failures newer than its last answer, so writes from several
// processes in any order agree.
package health

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/failure"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Streak is how many transient failures in a row, with no answer between,
// make a provider failing.
const Streak = 3

// StreakWindow is how far apart two transient failures with no answer
// between make it failing too.
const StreakWindow = 5 * time.Minute

// Entry is one harness and account's record.
type Entry struct {
	OK        time.Time   `json:"ok,omitzero"`         // a turn last got an answer
	Failed    time.Time   `json:"failed,omitzero"`     // a request last failed, of any kind
	Error     string      `json:"error,omitempty"`     // that failure, as failure.Summary says it
	Auth      time.Time   `json:"auth,omitzero"`       // the login last stopped working
	Transient []time.Time `json:"transient,omitempty"` // the newest Streak failures for a moment, oldest first
}

// State is how a provider stands.
type State uint8

const (
	Unknown State = iota // nothing has answered or failed since its login
	OK
	Failing
)

// State is how e stands: failing on a login that stopped working since the
// last answer, or a streak of failures for a moment; else OK once it has
// answered.
func (e Entry) State() State {
	if e.Auth.After(e.OK) {
		return Failing
	}
	var streak []time.Time
	for _, at := range e.Transient {
		if at.After(e.OK) {
			streak = append(streak, at)
		}
	}
	if len(streak) >= Streak || len(streak) > 1 && streak[len(streak)-1].Sub(streak[0]) >= StreakWindow {
		return Failing
	}
	if e.OK.IsZero() {
		return Unknown
	}
	return OK
}

// owes is whether an answer at at would change e: a failure newer than its
// last answer to clear, or no answer yet.
func (e Entry) owes(at time.Time) bool {
	return e.OK.IsZero() || e.Failed.After(e.OK) && at.After(e.OK)
}

// merge is e and o together: each time its newest, the newest Streak
// transient failures of both, and the error of the newest failure.
func (e Entry) merge(o Entry) Entry {
	m := Entry{OK: later(e.OK, o.OK), Failed: later(e.Failed, o.Failed), Auth: later(e.Auth, o.Auth), Error: e.Error}
	if !o.Failed.Before(e.Failed) && o.Error != "" {
		m.Error = o.Error
	}
	t := append(slices.Clone(e.Transient), o.Transient...)
	slices.SortFunc(t, time.Time.Compare)
	t = slices.CompactFunc(t, time.Time.Equal)
	if len(t) > Streak {
		t = t[len(t)-Streak:]
	}
	m.Transient = t
	return m
}

func later(a, b time.Time) time.Time {
	if b.After(a) {
		return b
	}
	return a
}

// Key is the record of harness's account: a login's id, or the profile it
// runs under.
func Key(harness, account string) string { return harness + "/" + account }

// Split is Key's harness and account.
func Split(key string) (harness, account string) {
	harness, account, _ = strings.Cut(key, "/")
	return harness, account
}

// Path is the file every rush process shares.
func Path() string { return filepath.Join(state.Dir(), "health.json") }

// read is the file as last written: parsed again only once it changed,
// as a turn's end looks at it every time.
var read struct {
	sync.Mutex
	path string
	mod  time.Time
	size int64
	all  map[string]Entry
}

// Load is every record, by Key. The map is shared: don't change it.
func Load() map[string]Entry {
	p := Path()
	fi, err := os.Stat(p)
	if err != nil {
		return nil
	}
	read.Lock()
	defer read.Unlock()
	if read.path == p && fi.ModTime().Equal(read.mod) && fi.Size() == read.size {
		return read.all
	}
	all := loadFile(p)
	read.path, read.mod, read.size, read.all = p, fi.ModTime(), fi.Size(), all
	return all
}

func loadFile(p string) map[string]Entry {
	var all map[string]Entry
	if b, err := os.ReadFile(p); err == nil {
		_ = jsonx.Unmarshal(b, &all) // a torn or hand-edited file starts over
	}
	for k, e := range all {
		e.Error = failure.Summary(e.Error) // a hand-edited file keeps nothing else either
		all[k] = e
	}
	return all
}

// Answered records a turn of key's getting an answer at at. It writes only
// when that changes the record: a failure to clear, or its first answer.
func Answered(key string, at time.Time) {
	if e, ok := Load()[key]; ok && !e.owes(at) {
		return
	}
	update(func(all map[string]Entry) {
		e := all[key]
		if e.owes(at) {
			all[key] = e.merge(Entry{OK: at})
		}
	})
}

// Failed records a request of key's failing at at on err. A failure that
// says nothing about the provider (a usage limit, the network, the
// prompt) isn't kept, nor one of it busy or rate limiting: that never
// makes it failing, so it writes nothing. It is the state after.
func Failed(key, err string, at time.Time) State {
	f := Entry{Failed: at, Error: failure.Summary(err)}
	switch failure.KindOf(err) {
	case failure.Auth:
		f.Auth = at
	case failure.Transient:
		f.Transient = []time.Time{at}
	case failure.Capacity:
		return Load()[key].State()
	default:
		return Unknown
	}
	var st State
	update(func(all map[string]Entry) {
		e := all[key].merge(f)
		all[key], st = e, e.State()
	})
	return st
}

// Reset forgets harness's records, so each reads Unknown until it next
// answers or fails: its login was added, renewed or removed, and what the
// old one did says nothing about the new. account narrows it to one; ""
// is all of harness's.
func Reset(harness, account string) {
	found := false
	for k := range Load() {
		found = found || matches(k, harness, account)
	}
	if !found {
		return // nothing to forget: no write
	}
	update(func(all map[string]Entry) {
		for k := range all {
			if matches(k, harness, account) {
				delete(all, k)
			}
		}
	})
}

func matches(key, harness, account string) bool {
	h, a := Split(key)
	return h == harness && (account == "" || a == account)
}

// AllFailing is every record failing, by Key.
func AllFailing() map[string]Entry {
	var out map[string]Entry
	for k, e := range Load() {
		if e.State() == Failing {
			if out == nil {
				out = map[string]Entry{}
			}
			out[k] = e
		}
	}
	return out
}

// update changes the file under a lock every rush process shares: read,
// change, then write a temporary file and rename it over.
func update(f func(map[string]Entry)) {
	p := Path()
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		return
	}
	lf, err := os.OpenFile(p+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err == nil {
		_ = syscall.Flock(int(lf.Fd()), syscall.LOCK_EX)
		defer lf.Close()
	}
	all := loadFile(p)
	if all == nil {
		all = map[string]Entry{}
	}
	f(all)
	b, err := jsonx.Marshal(all)
	if err != nil {
		return
	}
	tmp := p + ".tmp"
	if os.WriteFile(tmp, b, 0o600) == nil {
		_ = os.Rename(tmp, p)
	}
}
