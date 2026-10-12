package gate

import (
	"fmt"
	"sort"
	"time"
)

// The gate shares the machine as an OS would, not as a queue: a run's CPU
// is never held back, only lowered as it ages; memory is the one thing a
// run waits for, and waiters go fairly by agent. The decisions are here,
// as pure functions; gate.go and watch.go do what they say.

// A Ticket is one gated run, waiting or running, as its files have it.
type Ticket struct {
	File    string // its file's name, which sorts by when it was taken
	PID     int    // its holder's
	Name    string
	Session string // its agent: RUSH_SESSION, else the agent's pid
	Since   time.Time
	Memory  uint64 // what it's expected to need
	Running bool
	State
}

// State is what a running ticket's holder last said of its run.
type State struct {
	Nice      int
	Footprint uint64 // bytes its processes hold
	PausedAt  time.Time
	Why       string // why it's paused; "" when it isn't
}

// Paused is whether its run is stopped for memory.
func (s State) Paused() bool { return !s.PausedAt.IsZero() }

// Order is the waiters in ts in the order they go: the one whose agent has
// the fewest runs going first, the oldest ticket of those first.
func Order(ts []Ticket) []Ticket {
	running := map[string]int{}
	var waiting []Ticket
	for _, t := range ts {
		if t.Running {
			running[t.Session]++
		} else {
			waiting = append(waiting, t)
		}
	}
	sort.SliceStable(waiting, func(i, j int) bool {
		a, b := waiting[i], waiting[j]
		if ra, rb := running[a.Session], running[b.Session]; ra != rb {
			return ra < rb
		}
		return a.File < b.File
	})
	return waiting
}

// Room is the memory there is for one more run: what's available, less
// the reserve, less what the runs going are expected to take yet (their
// estimate past what they hold now), so starts in a row don't each count
// the same free memory.
func Room(ts []Ticket, avail, reserve uint64) uint64 {
	take := reserve
	for _, t := range ts {
		if t.Running && t.Memory > t.Footprint {
			take += t.Memory - t.Footprint
		}
	}
	if avail <= take {
		return 0
	}
	return avail - take
}

// Admit is whether next may start beside ts: always when nothing gated
// runs, else when the room covers what it's expected to need.
func Admit(next Ticket, ts []Ticket, avail, reserve uint64) bool {
	for _, t := range ts {
		if t.Running {
			return Room(ts, avail, reserve) >= next.Memory
		}
	}
	return true
}

// NiceAt is the nice a run gets once it has run for d: short checks keep
// the CPU, and a long run makes way for them, though it keeps the CPU no
// one else wants.
func NiceAt(d time.Duration) int {
	switch {
	case d >= 10*time.Minute:
		return 15
	case d >= 2*time.Minute:
		return 10
	case d >= 30*time.Second:
		return 5
	}
	return 0
}

// Pressure levels, as kern.memorystatus_vm_pressure_level has them; 0 is
// unknown.
const (
	PressureNormal   = 1
	PressureWarn     = 2
	PressureCritical = 4
)

// A Look is what a holder's watchdog sees of its run each tick.
type Look struct {
	Ran       time.Duration // running, its time paused left out
	Footprint uint64
	// HasAgent is whether an agent's program started it; AgentAlive
	// whether that's still running. Orphaned is whether it's been handed
	// to launchd, and SessionAlive whether its rush session still runs.
	HasAgent, AgentAlive, Orphaned, SessionAlive bool
	Pressure                                     int
	Paused                                       bool
	// Newest is whether it's the newest-started run going unpaused, and
	// Others how many other runs go unpaused.
	Newest bool
	Others int
	// SincePause is since any run on the machine was last paused.
	SincePause time.Duration
}

// An Act is what the watchdog does to a run.
type Act int

const (
	Carry Act = iota
	Stop
	Pause
	Resume
)

// Tick is how often a holder looks at its run.
const Tick = 2 * time.Second

// Judge is what the watchdog does to a run of r, as l sees it, and why.
// A run its agent left, or past its time or its memory, is stopped. Under
// critical memory pressure the newest run going is paused, one a tick and
// never the last, so the oldest work finishes; back to normal, it goes on.
func Judge(r Rule, l Look) (Act, string) {
	switch {
	case l.HasAgent && !l.AgentAlive:
		return Stop, "the agent that started it is gone"
	case !l.HasAgent && l.Orphaned && !l.SessionAlive:
		return Stop, "it was left running with no session to wait on it"
	case r.Limit > 0 && l.Ran > r.Limit:
		return Stop, fmt.Sprintf("it ran past its %s limit", r.Limit)
	case r.Memory > 0 && l.Footprint > r.Cap():
		return Stop, fmt.Sprintf("it held %s, past its %s cap (three times the %s it's expected to need)",
			Size(l.Footprint), Size(r.Cap()), Size(r.Memory))
	case l.Paused && l.Pressure <= PressureNormal:
		return Resume, "memory is back to normal"
	case !l.Paused && l.Pressure == PressureCritical && l.Newest && l.Others > 0 && l.SincePause >= Tick:
		return Pause, "the machine is critically short of memory"
	}
	return Carry, ""
}
