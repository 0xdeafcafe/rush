// Package menubar is rush's menu bar icon: a small native app that shows
// every account's usage, what's working and who needs you, and answers
// questions from their notification. The app is Swift, built on this Mac
// the first time it's needed; everything it shows comes from `rush
// menubar feed`, which it runs and talks to over stdin and stdout, one JSON
// object per line.
package menubar

import (
	"bufio"
	"bytes"
	"io"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Top is how many working agents the menu lists.
const Top = 5

// State is everything the menu shows, sent whenever it changes.
type State struct {
	Accounts []Account `json:"accounts"`
	Working  []Working `json:"working"`
	More     int       `json:"more"` // working agents past Top
	Waiting  []Waiting `json:"waiting"`
}

type Account struct {
	Name     string `json:"name"`
	Email    string `json:"email,omitempty"`
	Plan     string `json:"plan,omitempty"`
	FiveHour Window `json:"fiveHour"`
	SevenDay Window `json:"sevenDay"`
	Problem  string `json:"problem,omitempty"`
	Live     int    `json:"live"`
	Current  bool   `json:"current,omitzero"`
}

// Window is one of an account's limits. The feed has room for two: the
// first two of the account's that limit every model, which for Claude are
// the 5-hour and weekly ones, and fiveHour and sevenDay are their names in
// it for the app's sake. Label and Name say which they really are.
type Window struct {
	Present  bool      `json:"present"`
	Percent  float64   `json:"percent"`
	ResetsAt time.Time `json:"resetsAt,omitzero"`
	Label    string    `json:"label,omitempty"` // "5h", "7d"
	Name     string    `json:"name,omitempty"`  // "5-hour", "weekly"
}

// windows are a quota's two windows for the feed.
func windows(q usage.Quota) (first, second Window) {
	var out []Window
	for _, w := range q.Windows {
		if len(w.Scope.Models) == 0 {
			out = append(out, Window{Present: true, Percent: w.Percent, ResetsAt: w.ResetsAt, Label: w.Label, Name: w.Name})
		}
	}
	out = append(out, Window{}, Window{})
	return out[0], out[1]
}

// otherAccounts are the accounts of every other agent that reports its
// limits, as every rush last read them.
func otherAccounts(snap *fleet.Snapshot) []Account {
	readings := usage.Load(host.QuotasPath())
	var out []Account
	shown := map[agent.Kind]bool{} // agents whose accounts the feed has
	for i := range snap.Accounts {
		shown[snap.Accounts[i].Profile().Kind] = true
	}
	for _, a := range host.Installed() {
		if _, ok := a.(agent.QuotaSource); !ok || shown[a.Kind()] {
			continue
		}
		q, ok := readings[host.QuotaKey(a.Profiles()[0])]
		if !ok {
			continue
		}
		acct := Account{Name: a.Name(), Email: q.Email, Plan: firstNonEmpty(q.Plan, q.Balance), Problem: q.Problem}
		acct.FiveHour, acct.SevenDay = windows(q)
		for _, ag := range snap.Agents {
			if ag.Kind == string(a.Kind()) && ag.Live() {
				acct.Live++
			}
		}
		out = append(out, acct)
	}
	return out
}

type Working struct {
	Key     string    `json:"key"`
	Name    string    `json:"name"`
	Doing   string    `json:"doing"`
	Repo    string    `json:"repo,omitempty"`
	Account string    `json:"account"`
	Since   time.Time `json:"since"`
}

// Waiting is an agent waiting on you. Kind says how it can be answered
// from outside rush: "question" (pick an option or type an answer),
// "permission" (allow or deny), "limit" (continue at the reset or not),
// or "" (only in rush or its terminal).
type Waiting struct {
	Key     string    `json:"key"`
	Name    string    `json:"name"`
	Needs   string    `json:"needs"`
	Account string    `json:"account"`
	Seen    bool      `json:"seen,omitzero"`
	Kind    string    `json:"kind,omitempty"`
	Req     string    `json:"req,omitempty"` // the request an answer is for
	Header  string    `json:"header,omitempty"`
	Text    string    `json:"text,omitempty"` // the question, whole
	Options []string  `json:"options,omitempty"`
	Since   time.Time `json:"since"`
}

// Op is something the app asks the feed to do.
type Op struct {
	Op     string `json:"op"` // answer, allow, deny, limit, show
	Key    string `json:"key"`
	Req    string `json:"req,omitempty"`
	Answer string `json:"answer,omitempty"` // an option's label or your own words
	Yes    bool   `json:"yes,omitzero"`     // limit: continue at the reset
}

// pending is what a waiting rush session is asking, as its host has it.
type pending struct {
	sig  string // Needs when it was read; a change means ask again
	host string
	req  *asking
	info host.Info
}

// asking is what a session waits on you for: leave for a call, or the
// answers to a question.
type asking struct {
	ID     string
	Reason string          // why it asks, when it says
	Q      *event.Question // set when it's a question
}

// Feed writes the menu's state to out as it changes, every couple of
// seconds at most, and carries out ops read from in until in closes.
func Feed(in io.Reader, out io.Writer) error {
	unlock := hold()
	defer unlock()
	st := state.Load()
	l := fleet.NewLoader(st)
	l.Watch(10 * time.Second) // between full readings, only what changed on disk
	usagePath := filepath.Join(state.Dir(), "usage.json")

	var mu sync.Mutex // guards asked
	asked := map[string]*pending{}

	done := make(chan struct{})
	errs := make(chan string, 8)
	type reading struct {
		key string
		u   usage.Reading
	}
	fetched := make(chan reading, 8)
	go func() {
		defer close(done)
		sc := bufio.NewScanner(in)
		sc.Buffer(make([]byte, 64<<10), 1<<20)
		for sc.Scan() {
			var o Op
			if jsonx.Unmarshal(sc.Bytes(), &o) != nil {
				continue
			}
			if o.Op == "show" {
				if err := Show(o.Key); err != nil {
					errs <- "opening rush: " + err.Error()
				}
				continue
			}
			mu.Lock()
			p := asked[o.Key]
			mu.Unlock()
			if err := do(o, p); err != nil {
				errs <- err.Error()
			}
		}
	}()

	var last []byte
	var usageAt time.Time
	tick := time.NewTicker(2 * time.Second)
	defer tick.Stop()
	for {
		if time.Since(usageAt) > time.Minute {
			usageAt = time.Now()
			if pr, ok := agent.As[agent.PlanReader](agent.Kind(state.LoginsKind)); ok {
				go func() {
					p := st.Config.ActiveAccount().Profile()
					u := pr.RefreshPlan(usagePath, p, false)
					fetched <- reading{fleet.ReadingKey(p, u), u}
				}()
			}
			if k, ok := state.Logins(); ok {
				for _, lg := range st.Config.Logins {
					go func() { fetched <- reading{lg.UsageKey(), k.RefreshLogin(usagePath, st.Config, lg, false)} }()
				}
			}
		}
		for drained := false; !drained; {
			select {
			case r := <-fetched:
				l.SetFetched(r.key, r.u)
			default:
				drained = true
			}
		}
		l.Settle()
		snap := l.Load(true)
		mu.Lock()
		s := build(snap, asked)
		mu.Unlock()
		b, _ := jsonx.Marshal(s)
		if string(b) != string(last) {
			last = b
			if _, err := out.Write(append(b, '\n')); err != nil {
				return err
			}
		}
		select {
		case <-done:
			return nil
		case e := <-errs:
			b, _ := jsonx.Marshal(map[string]string{"error": e})
			if _, err := out.Write(append(b, '\n')); err != nil {
				return err
			}
		case <-tick.C:
		}
	}
}

func build(snap *fleet.Snapshot, asked map[string]*pending) State {
	s := State{Accounts: []Account{}, Working: []Working{}, Waiting: []Waiting{}}
	if len(snap.Logins) > 0 {
		// Accounts are the logins ~/.claude can be signed in as; the one in
		// use has the live agents.
		live := 0
		for _, av := range snap.Accounts {
			live += av.Live
		}
		for _, lv := range snap.Logins {
			u := lv.Usage
			a := Account{Name: lv.Name, Email: lv.Email, Plan: u.Plan, Problem: u.Problem, Current: lv.Current}
			a.FiveHour, a.SevenDay = windows(lv.Quota)
			if lv.Current {
				a.Live = live
			}
			s.Accounts = append(s.Accounts, a)
		}
	}
	for _, av := range snap.Accounts {
		u := av.Usage
		if len(snap.Logins) == 0 {
			a := Account{Name: av.Name, Email: u.Email, Plan: u.Plan, Problem: u.Problem, Live: av.Live, Current: av.Current}
			a.FiveHour, a.SevenDay = windows(av.Quota)
			s.Accounts = append(s.Accounts, a)
		}
	}
	s.Accounts = append(s.Accounts, otherAccounts(snap)...)
	live := map[string]bool{}
	var working []*fleet.Agent
	for _, a := range snap.Agents {
		switch {
		case a.Checking:
		case a.State == "blocked" && a.PID != 0:
			live[a.Key] = true
			w := Waiting{Key: a.Key, Name: a.DisplayName, Needs: oneLine(a.Needs), Account: a.Account, Seen: a.Seen, Since: a.UpdatedAt}
			if w.Needs == "" {
				w.Needs = oneLine(a.Detail)
			}
			if a.Rush {
				answerable(&w, a, asked)
			}
			s.Waiting = append(s.Waiting, w)
		case a.State == "working" && a.PID != 0:
			working = append(working, a)
		}
	}
	for k := range asked {
		if !live[k] {
			delete(asked, k)
		}
	}
	// Longest-running first: the ones you started earliest and are
	// waiting on most.
	sort.SliceStable(working, func(i, j int) bool { return working[i].CreatedAt.Before(working[j].CreatedAt) })
	for i, a := range working {
		if i == Top {
			s.More = len(working) - Top
			break
		}
		s.Working = append(s.Working, Working{Key: a.Key, Name: a.DisplayName, Doing: doing(a), Repo: a.Repo, Account: a.Account, Since: a.CreatedAt})
	}
	return s
}

// doing is what a working agent is doing now, in a few words.
func doing(a *fleet.Agent) string {
	if d := oneLine(a.Detail); d != "" {
		return d
	}
	if a.TranscriptPath != "" {
		if p := agent.ReadPreview(a.Acct.Kind, a.TranscriptPath, 64<<10); p.Doing != "" {
			return oneLine(p.Doing)
		}
	}
	if a.Interactive {
		return "working in a terminal"
	}
	return "working…"
}

// answerable fills in how a rush session's wait can be answered, asking
// its host what it's waiting on when that changed.
func answerable(w *Waiting, a *fleet.Agent, asked map[string]*pending) {
	p := asked[a.Key]
	if p == nil || p.sig != a.Needs {
		p = &pending{sig: a.Needs, host: a.ID}
		p.req, p.info = waitingOn(a.ID)
		asked[a.Key] = p
	}
	switch {
	case p.info.Limit != nil && p.info.Limit.Ask:
		w.Kind = "limit"
	case p.req == nil:
	case p.req.Q != nil:
		qs := p.req.Q.Asks
		w.Req = p.req.ID
		if len(qs) == 0 {
			return
		}
		q := qs[0]
		w.Header, w.Text = q.Header, strings.TrimSpace(q.Text)
		// One single-choice question can be answered from a notification;
		// several, or ticking many, need rush.
		if len(qs) == 1 && !q.Multi && len(q.Options) > 0 {
			w.Kind = "question"
			for _, o := range q.Options {
				w.Options = append(w.Options, o.Label)
			}
		}
	default:
		w.Kind, w.Req = "permission", p.req.ID
		w.Text = strings.TrimSpace(p.req.Reason)
	}
}

func waitingOn(id string) (*asking, host.Info) {
	c, err := host.Dial(id)
	if err != nil {
		return nil, host.Info{}
	}
	defer c.Close()
	w := waits{open: map[string]*asking{}}
	var d host.Decoder
	timeout := time.After(2 * time.Second)
	for {
		select {
		case l, ok := <-c.Lines:
			if !ok {
				return nil, host.Info{}
			}
			if skipped(l) {
				continue
			}
			evs, _ := d.Decode(l)
			for _, ev := range evs {
				if info, ok := ev.(host.InfoEvent); ok {
					return w.last(), info.Info
				}
				w.take(ev)
			}
		case <-timeout:
			return nil, host.Info{}
		}
	}
}

// waits are what a replay asked and hasn't yet settled, in order.
type waits struct {
	open  map[string]*asking
	order []string
}

func (w *waits) take(ev any) {
	switch ev := ev.(type) {
	case event.Approval:
		w.open[ev.ID] = &asking{ID: ev.ID, Reason: ev.Reason}
		w.order = append(w.order, ev.ID)
	case event.Question:
		q := ev
		w.open[ev.ID] = &asking{ID: ev.ID, Q: &q}
		w.order = append(w.order, ev.ID)
	case event.ApprovalCancelled:
		delete(w.open, ev.ID)
	case host.Answered:
		delete(w.open, ev.ID)
	}
}

// last is the latest still waiting, or nil.
func (w *waits) last() *asking {
	for _, id := range slices.Backward(w.order) {
		if r := w.open[id]; r != nil {
			return r
		}
	}
	return nil
}

// skipped is a replay line of Claude Code's that can't be a permission
// request or its cancelling: most of the replay, and its biggest lines
// (whole messages, tool results), so they aren't taken apart for nothing.
func skipped(l []byte) bool {
	return bytes.HasPrefix(l, []byte(`{"type":"`)) &&
		!bytes.HasPrefix(l, []byte(`{"type":"agtop_`)) &&
		!bytes.HasPrefix(l, []byte(`{"type":"control_`))
}

// do carries out an op on the session it names.
func do(o Op, p *pending) error {
	if p == nil {
		return errGone
	}
	c, err := host.Dial(p.host)
	if err != nil {
		return err
	}
	defer c.Close()
	go func() {
		for range c.Lines { // the replay isn't wanted
		}
	}()
	switch o.Op {
	case "limit":
		return c.ContinueAtReset(o.Yes)
	}
	if p.req == nil || p.req.ID != o.Req {
		return errGone
	}
	switch o.Op {
	case "answer":
		q := p.req.Q
		if q == nil || len(q.Asks) == 0 {
			return errGone
		}
		return c.Allow(p.req.ID, host.AnswerInput(q, map[string]string{q.Asks[0].Text: o.Answer}), false)
	case "allow":
		return c.Allow(p.req.ID, nil, false)
	case "deny":
		return c.Deny(p.req.ID, "", false)
	}
	return nil
}

type feedError string

func (e feedError) Error() string { return string(e) }

const errGone = feedError("that question has already been answered")

func oneLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = strings.TrimSpace(s[:i]) + " …"
	}
	return s
}

// exe is the rush binary the app should run.
func exe() string {
	p, err := os.Executable()
	if err != nil {
		return "rush"
	}
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return p
}

func firstNonEmpty(vs ...string) string {
	for _, v := range vs {
		if v != "" {
			return v
		}
	}
	return ""
}
