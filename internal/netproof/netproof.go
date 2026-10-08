// Package netproof is what every rush process knows together about
// whether the API holds up: each session's host and the list keep one
// record per API, of when a request last failed on the connection, how
// long checks have passed unbroken since, and when a real request last got
// an answer.
//
// Sessions an API error stopped wait on it as a group: one check every few
// seconds serves them all, and one of them failing again sets them all
// back. A session whose prompt cache is still warm tries again as soon as
// the API can be reached, since a try costs it little. One whose cache has
// expired re-reads the whole conversation at full price on its next try,
// so it waits for proof the connection is back for good: a real request
// answered since the failure, or checks passing unbroken for Hold, and
// then only one such session goes first (the canary) until one answers.
package netproof

import (
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Every is how often the API is checked while something waits on it; a
// check younger than this is shared, not made again.
var Every = 5 * time.Second

// Hold is how long checks must pass unbroken, with no request failing,
// before a session whose cache has expired may try again.
var Hold = time.Minute

// canaryFor is how long the one cold session that goes first holds the
// way: if it hasn't been answered by then, another may go.
const canaryFor = 10 * time.Minute

// Proof is the record for one API.
type Proof struct {
	Failed   time.Time `json:"failed,omitzero"`   // a request last failed on the connection
	UpSince  time.Time `json:"upSince,omitzero"`  // checks have all passed since; zero while down
	Checked  time.Time `json:"checked,omitzero"`  // the last check
	Answered time.Time `json:"answered,omitzero"` // a real request last got an answer
	Canary   string    `json:"canary,omitempty"`  // the cold session trying first
	CanaryAt time.Time `json:"canaryAt,omitzero"`
}

// Up is whether the last check passed.
func (p Proof) Up() bool { return !p.UpSince.IsZero() }

// Answering is whether a real request got an answer since the last
// failure: the strongest proof.
func (p Proof) Answering() bool { return !p.Answered.IsZero() && !p.Answered.Before(p.Failed) }

// Holding is whether checks have passed unbroken for Hold since the last
// failure.
func (p Proof) Holding(now time.Time) bool {
	return p.Up() && p.UpSince.After(p.Failed) && now.Sub(p.UpSince) >= Hold
}

// Target is the API requests go to: Anthropic's, or the one
// ANTHROPIC_BASE_URL names, in env (KEY=value, the last wins) or else in
// this process's environment.
func Target(env ...string) string {
	base := os.Getenv("ANTHROPIC_BASE_URL")
	for _, kv := range env {
		if v, ok := strings.CutPrefix(kv, "ANTHROPIC_BASE_URL="); ok {
			base = v
		}
	}
	if u, err := url.Parse(base); err == nil && u.Hostname() != "" {
		return u.Scheme + "://" + u.Host
	}
	return "https://api.anthropic.com"
}

// probe is whether target answers HTTP at all: any response, even a
// refusal, says the connection works end to end, not just that a port
// opened.
var probe = func(target string) bool {
	c := &http.Client{Timeout: 5 * time.Second}
	resp, err := c.Head(target)
	if err != nil {
		return false
	}
	_ = resp.Body.Close()
	return true
}

func path(target string) string {
	sum := sha256.Sum256([]byte(target))
	return filepath.Join(state.Dir(), "netproof", hex.EncodeToString(sum[:8])+".json")
}

// update changes target's record under a lock every rush process shares,
// and returns it as changed.
func update(target string, f func(*Proof)) Proof {
	p := path(target)
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		var pr Proof
		f(&pr)
		return pr
	}
	lf, err := os.OpenFile(p+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err == nil {
		_ = syscall.Flock(int(lf.Fd()), syscall.LOCK_EX)
		defer lf.Close()
	}
	pr := Load(target)
	f(&pr)
	if b, err := jsonx.Marshal(pr); err == nil {
		tmp := p + ".tmp"
		if os.WriteFile(tmp, b, 0o600) == nil {
			_ = os.Rename(tmp, p)
		}
	}
	return pr
}

// Load is target's record as last written.
func Load(target string) Proof {
	var p Proof
	if b, err := os.ReadFile(path(target)); err == nil {
		_ = jsonx.Unmarshal(b, &p)
	}
	return p
}

// Check is target's record with a check no older than Every: made now,
// without any lock held, if the last is older.
func Check(target string) Proof {
	now := time.Now()
	if p := Load(target); now.Sub(p.Checked) < Every {
		return p
	}
	up := probe(target)
	return update(target, func(p *Proof) {
		p.Checked = time.Now()
		switch {
		case !up:
			p.UpSince = time.Time{}
		case p.UpSince.IsZero():
			p.UpSince = now
		}
	})
}

// Fail records a request failing on the connection at at: every waiting
// session starts proving it again.
func Fail(target string, at time.Time) {
	update(target, func(p *Proof) {
		if at.After(p.Failed) {
			p.Failed = at
		}
		p.UpSince, p.Canary = time.Time{}, ""
	})
}

var answered sync.Map // target → when this process last recorded an answer

// Answer records a real request getting an answer at at. A process writes
// it at most every few seconds.
func Answer(target string, at time.Time) {
	if v, ok := answered.Load(target); ok && at.Sub(v.(time.Time)) < Every {
		return
	}
	answered.Store(target, at)
	update(target, func(p *Proof) {
		if at.After(p.Answered) {
			p.Answered = at
		}
		p.Canary = ""
	})
}

// Claim asks for id to be the cold session that goes first. It is true
// when no other holds the way, or the one that did has had long enough.
func Claim(target, id string) bool {
	now := time.Now()
	ok := false
	update(target, func(p *Proof) {
		if p.Canary == "" || p.Canary == id || now.Sub(p.CanaryAt) > canaryFor {
			p.Canary, p.CanaryAt, ok = id, now, true
		}
	})
	return ok
}

// MayGo is whether a session that waits on target may try again now:
// warm (its cache hasn't expired) once the API can be reached; cold only
// on proof the connection holds, and then first only if it's the canary.
// id names it for the claim. It makes a check if the last is stale, so it
// must not be called from a UI's update.
func MayGo(target, id string, warm bool) bool {
	p := Check(target)
	switch {
	case !p.Up():
		return false
	case warm, p.Answering():
		return true
	case p.Holding(time.Now()):
		return Claim(target, id)
	}
	return false
}
