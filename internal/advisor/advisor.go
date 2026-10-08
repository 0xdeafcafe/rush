// Package advisor watches how you use your agents and says what would cut
// tokens or time. It works in two passes: a cheap model (Haiku) reads a
// digest rush works out itself and proposes candidates, and now and then
// an expensive one (Opus) checks the ones worth money against the
// transcripts before they reach you. It's opt-in: nothing runs until you
// turn it on.
package advisor

import (
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/efficiency"
	"github.com/0xdeafcafe/rush/internal/state"
)

const (
	// Gap is the least time between passes.
	Gap = 3 * time.Hour
	// MinRequests is how many new requests make a pass worth running.
	MinRequests = 150
	// ReviewAt is the dollars a week a candidate must be worth for Opus to
	// check it: a review costs cents to a dollar or so.
	ReviewAt = 2.0
	// Tries is how many reviews may fail on one candidate before it's left
	// alone, so one that times out can't take every day's reviews.
	Tries = 2
	// ReviewsPerDay caps the Opus reviews in any 24 hours.
	ReviewsPerDay = 3
	// Keep is how many findings are kept, rejected and put away ones
	// included: they're what stops a finding being proposed again.
	Keep = 24
	// lockStale is when a pass's lock is taken to be left by a process
	// that died: longer than a pass can run.
	lockStale = 45 * time.Minute
)

// What a Finding's Status can be.
const (
	Candidate = "candidate" // Haiku's, not checked
	Confirmed = "confirmed" // Opus checked it and it holds
	Rejected  = "rejected"  // Opus checked it and it doesn't; kept so it isn't proposed again
)

// Finding is something the advisor noticed.
type Finding struct {
	ID       string    `json:"id"`
	Title    string    `json:"title"`
	Detail   string    `json:"detail"`
	Evidence []string  `json:"evidence,omitempty"`
	Weekly   float64   `json:"weekly,omitzero"` // dollars a week at stake, roughly
	Fix      string    `json:"fix,omitempty"`   // a saver's ID
	Open     string    `json:"open,omitempty"`  // a file to change
	Status   string    `json:"status"`
	Note     string    `json:"note,omitempty"` // the reviewer's
	Tries    int       `json:"tries,omitzero"` // reviews that failed
	At       time.Time `json:"at"`
}

// Record is what the advisor keeps between runs.
type Record struct {
	LastRun  time.Time   `json:"lastRun,omitzero"`
	Runs     int         `json:"runs,omitzero"`
	Reviews  []time.Time `json:"reviews,omitempty"`
	Spent    float64     `json:"spent,omitzero"` // what the advisor itself has cost
	Err      string      `json:"err,omitempty"`  // why the last pass failed
	Findings []Finding   `json:"findings,omitempty"`
	// Dismissed are findings you put away: never shown or proposed again.
	Dismissed []string `json:"dismissed,omitempty"`
}

// Dir is where the advisor keeps its record, and where its passes run.
func Dir() string { return filepath.Join(state.Dir(), "advisor") }

func recordPath() string { return filepath.Join(Dir(), "record.json") }

// Load reads the record; a missing one is empty. One that can't be read
// is kept aside, and the record starts as if a pass had just run, so a
// broken file can't let one run at once or reset the day's reviews.
func Load() *Record {
	r := &Record{}
	b, err := os.ReadFile(recordPath())
	if err != nil {
		return r
	}
	if err := jsonx.Unmarshal(b, r); err != nil {
		_ = os.Rename(recordPath(), recordPath()+".bad")
		return &Record{LastRun: time.Now(), Err: "its record couldn't be read, and is kept as record.json.bad"}
	}
	return r
}

// Enabled is whether the advisor is on in the config on disk: another
// rush may have turned it off. RUSH_ADVISOR decides it for this run.
func Enabled() bool {
	if on, ok := state.EnvBool("advisor"); ok {
		return on
	}
	var c struct {
		Advisor bool `json:"advisor"`
	}
	b, err := os.ReadFile(filepath.Join(state.Dir(), "config.json"))
	return err == nil && jsonx.Unmarshal(b, &c) == nil && c.Advisor
}

// Begin marks a pass as started, before it spends anything: if rush quits
// halfway, the next one still waits out the gap.
func Begin(now time.Time) error {
	r := Load()
	r.LastRun = now
	return r.Save()
}

// Reserve counts a review against the day's cap before it runs, for the
// same reason.
func Reserve(now time.Time) error {
	r := Load()
	r.Reviews = append(r.Reviews, now)
	return r.Save()
}

// Save writes the record.
func (r *Record) Save() error {
	if err := os.MkdirAll(Dir(), 0o700); err != nil {
		return err
	}
	b, err := jsonx.MarshalIndent(r)
	if err != nil {
		return err
	}
	// A temp file of its own, so two rushes saving at once can't write
	// into each other's.
	f, err := os.CreateTemp(Dir(), "record.*.tmp")
	if err != nil {
		return err
	}
	_, werr := f.Write(b)
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr != nil {
		_ = os.Remove(f.Name())
		return werr
	}
	return os.Rename(f.Name(), recordPath())
}

// Due is whether a pass should run now, given how many requests your
// agents have made since the last one.
func (r *Record) Due(now time.Time, newReqs int64) bool {
	if r.LastRun.IsZero() {
		return newReqs > 0
	}
	return now.Sub(r.LastRun) >= Gap && newReqs >= MinRequests
}

// ReviewsLeft is how many Opus reviews the day's cap still allows.
func (r *Record) ReviewsLeft(now time.Time) int {
	n := 0
	for _, t := range r.Reviews {
		if now.Sub(t) < 24*time.Hour {
			n++
		}
	}
	return max(0, ReviewsPerDay-n)
}

// Shown are the findings to show: confirmed first, then candidates, most
// at stake first within each.
func (r *Record) Shown() []Finding {
	var out []Finding
	for _, f := range r.Findings {
		if f.Status != Rejected && !slices.Contains(r.Dismissed, f.ID) {
			out = append(out, f)
		}
	}
	slices.SortStableFunc(out, func(a, b Finding) int {
		if (a.Status == Confirmed) != (b.Status == Confirmed) {
			if a.Status == Confirmed {
				return -1
			}
			return 1
		}
		switch {
		case a.Weekly > b.Weekly:
			return -1
		case a.Weekly < b.Weekly:
			return 1
		}
		return 0
	})
	return out
}

// Pending are the candidates still waiting on a review, not put away, and
// not failed too often.
func (r *Record) Pending() []Finding {
	var out []Finding
	for _, f := range r.Findings {
		if f.Status == Candidate && f.Tries < Tries && !slices.Contains(r.Dismissed, f.ID) {
			out = append(out, f)
		}
	}
	return out
}

// Settled are the findings Opus has decided on, or you put away: a pass
// doesn't propose or check them again.
func (r *Record) Settled() []string {
	out := slices.Clone(r.Dismissed)
	for _, f := range r.Findings {
		if f.Status != Candidate {
			out = append(out, f.ID)
		}
	}
	return out
}

// Known are the titles a pass mustn't propose again: everything kept,
// rejected and dismissed included.
func (r *Record) Known() []string {
	var out []string
	for _, f := range r.Findings {
		out = append(out, f.Title)
	}
	return out
}

// Dismiss puts a finding away for good.
func (r *Record) Dismiss(id string) {
	if !slices.Contains(r.Dismissed, id) {
		r.Dismissed = append(r.Dismissed, id)
	}
}

// Merge takes a pass's result in, and reports the findings that are newly
// confirmed. Its reviews were counted as they started (Reserve).
func (r *Record) Merge(res Result) []Finding {
	r.LastRun, r.Runs = res.At, r.Runs+1
	r.Spent += res.Spent
	r.Err = ""
	if res.Err != nil {
		r.Err = res.Err.Error()
	}
	var fresh []Finding
	for _, f := range res.Findings {
		i := slices.IndexFunc(r.Findings, func(o Finding) bool { return o.ID == f.ID })
		switch {
		case i < 0:
			r.Findings = append(r.Findings, f)
		case r.Findings[i].Status == Candidate:
			r.Findings[i] = f
		default:
			continue // Opus has settled it
		}
		if f.Status == Confirmed && !slices.Contains(r.Dismissed, f.ID) {
			fresh = append(fresh, f)
		}
	}
	// Over Keep, the oldest go: rejected and put away ones first, then
	// guesses, then what Opus confirmed.
	if len(r.Findings) > Keep {
		rank := func(f Finding) int {
			switch {
			case f.Status == Rejected || slices.Contains(r.Dismissed, f.ID):
				return 0
			case f.Status == Candidate:
				return 1
			}
			return 2
		}
		slices.SortStableFunc(r.Findings, func(a, b Finding) int {
			if ra, rb := rank(a), rank(b); ra != rb {
				return rb - ra
			}
			return b.At.Compare(a.At)
		})
		r.Findings = r.Findings[:Keep]
	}
	cut := time.Now().Add(-24 * time.Hour)
	r.Reviews = slices.DeleteFunc(r.Reviews, func(t time.Time) bool { return t.Before(cut) })
	return fresh
}

// Lock claims the next pass for this process, so two rushes open at once
// don't both run one (and both spend the day's reviews). ok is false when
// another holds it; unlock lets it go.
func Lock() (unlock func(), ok bool) {
	if err := os.MkdirAll(Dir(), 0o700); err != nil {
		return nil, false
	}
	p := filepath.Join(Dir(), "pass.lock")
	f, err := os.OpenFile(p, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
	if os.IsExist(err) {
		if st, serr := os.Stat(p); serr == nil && time.Since(st.ModTime()) > lockStale {
			_ = os.Remove(p)
			f, err = os.OpenFile(p, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
		}
	}
	if err != nil {
		return nil, false
	}
	fmt.Fprintf(f, "%d\n", os.Getpid())
	_ = f.Close()
	return func() { _ = os.Remove(p) }, true
}

// Active is whether any of p's transcripts changed since t: a look at
// file times, far cheaper than reading the figures, so a quiet day costs
// nothing.
func Active(p agent.Profile, since time.Time) bool {
	for _, path := range efficiency.Transcripts(p) {
		if st, err := os.Stat(path); err == nil && st.ModTime().After(since) {
			return true
		}
	}
	return false
}

// idOf names a finding by its title, so the same one proposed twice is one.
func idOf(title string) string {
	s := strings.Join(strings.Fields(strings.ToLower(title)), " ")
	h := sha1.Sum([]byte(s))
	return hex.EncodeToString(h[:6])
}
