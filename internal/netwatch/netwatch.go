// Package netwatch is what rush knows about the network: whether the API
// can be reached, which network the machine is on and when that changed,
// how fast bytes are moving, and which of rush's own jobs that need the
// network are waiting for it to come back.
//
// Everything is read on its own goroutine; the UI only ever takes the last
// reading (Now), so a slow or absent network never holds up a frame.
package netwatch

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"sort"
	"strings"
	"sync"
	"time"
)

// How often the API is checked: rarely while the network has been steady,
// more often while it's been shaky, so it can be called steady again on
// evidence, and while it's down from every 5s backing off to every 30s,
// so it's noticed soon after it's back. A new network, or a job failing on
// what looks like the network, checks at once.
var (
	steadyEvery = 2 * time.Minute
	shakyEvery  = 30 * time.Second
	downFirst   = 5 * time.Second
	downMost    = 30 * time.Second
	dialFor     = 4 * time.Second
	sample      = 5 * time.Second
)

// How far back steadiness looks, and how long a connection may take
// before it counts as slow.
var (
	window = 10 * time.Minute
	slow   = time.Second
)

// OnEvent, when set, is told what changed: the API going down or coming
// back, the machine moving network. It's set by package events, which
// netwatch can't import (state imports netwatch); it must not block.
var OnEvent func(kind string, fields map[string]any)

func event(kind string, fields map[string]any) {
	if OnEvent != nil {
		OnEvent(kind, fields)
	}
}

// ErrOffline is what a job gets for asking while the API can't be reached.
var ErrOffline = errors.New("offline · waits for the network")

// State is the last reading.
type State struct {
	Known    bool          // a check has finished
	Up       bool          // the API answered it
	Checking bool          // a check is being made
	Why      string        // why the last check failed
	Since    time.Time     // up, or down, since
	Checked  time.Time     // when the last check finished
	Latency  time.Duration // how long the last check's connection took
	Checks   int
	Target   string // what's checked: host:port

	Network string    // the network the machine is on, as "en0 192.168.1.20"
	Changed time.Time // when that last changed; zero if it hasn't since rush opened
	Changes int
	// Noticed is how long after the network last changed a check said
	// whether the API answers on it; zero until one has.
	Noticed time.Duration

	// RxRate and TxRate are bytes a second in and out, across the
	// machine's own interfaces; Rated is false where they can't be read.
	RxRate, TxRate float64
	Rated          bool

	// Steady is whether, over the last Window, every check answered,
	// none was slow, and the machine stayed on one network; Shaky says
	// what broke it.
	Steady bool
	Shaky  []string
	Window time.Duration

	Jobs []Job // by name
}

// Job is one of rush's jobs that needs the network.
type Job struct {
	Name    string
	Paused  bool      // it asked while the network was down, and waits for it
	Last    time.Time // when it last ran
	Err     string    // how it last failed; empty if it didn't
	Runs    int
	Skipped int // times it was held back while the network was down
}

type watcher struct {
	sync.Mutex
	s       State
	jobs    map[string]*Job
	started bool
	kick    chan struct{}
	subs    []chan struct{}
	count   map[int][2]uint64 // each interface's bytes in and out, as last read
	at      time.Time
	netKey  string
	pending bool    // the network changed and no check has finished since
	hist    []probe // checks within the window, oldest first
	fails   int     // checks failed in a row
}

// probe is one check, kept to judge steadiness.
type probe struct {
	at      time.Time
	up      bool
	latency time.Duration
}

var w = &watcher{jobs: map[string]*Job{}, kick: make(chan struct{}, 1)}

// job is name's record; called with w held.
func (w *watcher) job(name string) *Job {
	j := w.jobs[name]
	if j == nil {
		j = &Job{Name: name}
		w.jobs[name] = j
	}
	return j
}

// wake closes every Changes channel; called with w held.
func (w *watcher) wake() {
	for _, c := range w.subs {
		close(c)
	}
	w.subs = nil
}

// Start begins watching; it is safe to call more than once. A process that
// never calls it (a session's host) lets every job run.
func Start() {
	w.Lock()
	defer w.Unlock()
	if w.started {
		return
	}
	w.started = true
	w.s.Target = target()
	go w.loop()
}

// Now is the last reading.
func Now() State {
	w.Lock()
	defer w.Unlock()
	s := w.s
	s.Jobs = make([]Job, 0, len(w.jobs))
	for _, j := range w.jobs {
		s.Jobs = append(s.Jobs, *j)
	}
	sort.Slice(s.Jobs, func(i, j int) bool { return s.Jobs[i].Name < s.Jobs[j].Name })
	s.Steady, s.Shaky, s.Window = w.steady(time.Now())
	return s
}

// steady judges the network over the window; called with w held.
func (w *watcher) steady(now time.Time) (bool, []string, time.Duration) {
	var failed, slowed int
	for _, p := range w.hist {
		if now.Sub(p.at) > window {
			continue
		}
		switch {
		case !p.up:
			failed++
		case p.latency > slow:
			slowed++
		}
	}
	var why []string
	if failed > 0 {
		why = append(why, fmt.Sprintf("%d check%s failed", failed, plural(failed)))
	}
	if slowed > 0 {
		why = append(why, fmt.Sprintf("%d slow to connect", slowed))
	}
	if !w.s.Changed.IsZero() && now.Sub(w.s.Changed) <= window {
		why = append(why, "joined another network")
	}
	return len(why) == 0, why, window
}

func plural(n int) string {
	if n == 1 {
		return ""
	}
	return "s"
}

// Changes gives a channel that's closed the next time the API goes from
// down to up, or the machine joins another network.
func Changes() <-chan struct{} {
	w.Lock()
	defer w.Unlock()
	c := make(chan struct{})
	w.subs = append(w.subs, c)
	return c
}

// Down is whether a check has said the API can't be reached.
func Down() bool {
	w.Lock()
	defer w.Unlock()
	return w.started && w.s.Known && !w.s.Up
}

// Run is a job asking to use the network: false (and the job marked
// paused) while it's down.
func Run(name string) bool {
	w.Lock()
	defer w.Unlock()
	j := w.job(name)
	if w.started && w.s.Known && !w.s.Up {
		if !j.Paused {
			event("net-job-held", map[string]any{"job": name, "why": "the API can't be reached"})
		}
		j.Paused = true
		j.Skipped++
		return false
	}
	j.Paused = false
	return true
}

// Done records how a job that ran went. An error that looks like the
// network has the API checked at once, rather than at the next check.
func Done(name string, err error) {
	w.Lock()
	j := w.job(name)
	j.Last, j.Runs, j.Err = time.Now(), j.Runs+1, ""
	if err != nil {
		j.Err = err.Error()
	}
	w.Unlock()
	if IsNetErr(err) {
		Check()
	}
}

// Do runs f as the job name, unless the network is down.
func Do(name string, f func() error) error {
	if !Run(name) {
		return ErrOffline
	}
	err := f()
	Done(name, err)
	return err
}

// Check has the API checked now.
func Check() {
	select {
	case w.kick <- struct{}{}:
	default:
	}
}

// IsNetErr is whether err looks like the network rather than the other
// end: no route, no DNS, refused, reset, timed out.
func IsNetErr(err error) bool {
	if err == nil || errors.Is(err, ErrOffline) {
		return false
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return true
	}
	if _, ok := errors.AsType[net.Error](err); ok {
		return true
	}
	t := strings.ToLower(err.Error())
	for _, s := range []string{"no such host", "network is unreachable", "connection refused", "connection reset", "i/o timeout", "no route to host", "network is down", "tls handshake timeout"} {
		if strings.Contains(t, s) {
			return true
		}
	}
	return false
}

// target is what's dialled to see whether the API answers: Anthropic's,
// or the one ANTHROPIC_BASE_URL names.
func target() string {
	addr := "api.anthropic.com:443"
	if u, err := url.Parse(os.Getenv("ANTHROPIC_BASE_URL")); err == nil && u.Hostname() != "" {
		port := u.Port()
		if port == "" && u.Scheme == "http" {
			port = "80"
		}
		if port == "" {
			port = "443"
		}
		addr = net.JoinHostPort(u.Hostname(), port)
	}
	return addr
}

// loop samples the interfaces every few seconds, and checks the API when it's
// due, when asked, and when the network changes.
func (w *watcher) loop() {
	tick := time.NewTicker(sample)
	defer tick.Stop()
	checks := make(chan checkResult, 1)
	checking := false
	var next time.Time // zero: check now
	for {
		now := time.Now()
		w.sampleNet(now)
		if !checking && !now.Before(next) {
			checking = true
			w.Lock()
			w.s.Checking = true
			addr := w.s.Target
			w.Unlock()
			go func() { checks <- dial(addr) }()
		}
		select {
		case <-tick.C:
		case <-w.kick:
			if !checking {
				next = time.Time{}
			}
		case r := <-checks:
			checking = false
			next = time.Now().Add(w.took(r))
		}
	}
}

type checkResult struct {
	up      bool
	latency time.Duration
	why     string
}

func dial(addr string) checkResult {
	start := time.Now()
	c, err := net.DialTimeout("tcp", addr, dialFor)
	if err != nil {
		return checkResult{why: shortErr(err)}
	}
	_ = c.Close()
	return checkResult{up: true, latency: time.Since(start)}
}

// took takes in a check, and says when the next is due.
func (w *watcher) took(r checkResult) time.Duration {
	w.Lock()
	defer w.Unlock()
	now := time.Now()
	s := &w.s
	was, known := s.Up, s.Known
	s.Known, s.Up, s.Checking, s.Checked, s.Checks = true, r.up, false, now, s.Checks+1
	s.Latency, s.Why = r.latency, r.why
	if !known || was != r.up {
		s.Since = now
		if r.up {
			event("net-up", map[string]any{"target": s.Target, "latency": r.latency.String()})
		} else {
			event("net-down", map[string]any{"target": s.Target, "why": r.why})
		}
	}
	if w.pending {
		w.pending = false
		s.Noticed = now.Sub(s.Changed)
	}
	w.hist = append(w.hist, probe{at: now, up: r.up, latency: r.latency})
	for len(w.hist) > 0 && now.Sub(w.hist[0].at) > window {
		w.hist = w.hist[1:]
	}
	if !r.up {
		w.fails++
		return min(downFirst<<(w.fails-1), downMost)
	}
	w.fails = 0
	for _, j := range w.jobs {
		j.Paused = false
	}
	if known && !was {
		w.wake()
	}
	if ok, _, _ := w.steady(now); ok {
		return steadyEvery
	}
	return shakyEvery
}

// sampleNet reads the interfaces' counters, for the rates, and which
// network the machine is on, to notice when that changes.
func (w *watcher) sampleNet(now time.Time) {
	count, ok := counters()
	key, label := network()
	w.Lock()
	defer w.Unlock()
	if ok && !w.at.IsZero() {
		var rx, tx uint64
		for i, c := range count {
			if p, seen := w.count[i]; seen {
				rx, tx = rx+grew(p[0], c[0]), tx+grew(p[1], c[1])
			}
		}
		if d := now.Sub(w.at).Seconds(); d > 0 {
			w.s.RxRate, w.s.TxRate = float64(rx)/d, float64(tx)/d
		}
		w.s.Rated = true
	}
	if ok {
		w.count, w.at = count, now
	}
	if key != w.netKey {
		first := w.netKey == "" && w.s.Network == ""
		w.netKey, w.s.Network = key, label
		if !first {
			event("net-changed", map[string]any{"network": label})
			w.s.Changed, w.s.Changes, w.pending = now, w.s.Changes+1, true
			w.wake()
			select {
			case w.kick <- struct{}{}:
			default:
			}
		}
	}
}

// grew is how far a counter moved from was to is, allowing for one cut to
// 32 bits wrapping; a counter that went back otherwise was reset.
func grew(was, is uint64) uint64 {
	switch {
	case is >= was:
		return is - was
	case was < 1<<32:
		return is + 1<<32 - was
	}
	return 0
}

// network is which network the machine is on: its interfaces that are up
// with an address to the outside, as a key, and the first as a label.
func network() (key, label string) {
	ifs, err := net.Interfaces()
	if err != nil {
		return "", ""
	}
	var parts []string
	for _, in := range ifs {
		if in.Flags&net.FlagUp == 0 || in.Flags&net.FlagLoopback != 0 || tunnel(in.Name) {
			continue
		}
		addrs, err := in.Addrs()
		if err != nil {
			continue
		}
		for _, a := range addrs {
			ipn, ok := a.(*net.IPNet)
			if !ok || !ipn.IP.IsGlobalUnicast() {
				continue
			}
			parts = append(parts, in.Name+" "+ipn.IP.String())
			if label == "" && ipn.IP.To4() != nil {
				label = in.Name + " " + ipn.IP.String()
			}
		}
	}
	sort.Strings(parts)
	if label == "" && len(parts) > 0 {
		label = parts[0]
	}
	if len(parts) == 0 {
		return "none", "no network"
	}
	return strings.Join(parts, ","), label
}

// tunnel is an interface that carries other interfaces' traffic, or
// none of the machine's own: VPNs, AirDrop, bridges for VMs.
func tunnel(name string) bool {
	for _, p := range []string{"utun", "ipsec", "gif", "stf", "awdl", "llw", "bridge", "anpi", "ap", "tun", "tap", "docker", "veth", "br-", "virbr", "vmnet", "lo"} {
		if strings.HasPrefix(name, p) {
			return true
		}
	}
	return false
}

// shortErr is the part of a dial's error that says why.
func shortErr(err error) string {
	t := err.Error()
	if i := strings.LastIndex(t, ": "); i >= 0 {
		t = t[i+2:]
	}
	return t
}
