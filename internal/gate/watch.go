package gate

import (
	"context"
	"fmt"
	"os"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/proc"
)

// stopGrace is how long a run asked to stop has before it's made to.
const stopGrace = 10 * time.Second

// A Watch is what a holder's watchdog needs of its run.
type Watch struct {
	// Root is the run's top process: its tree, the holder left out, is
	// the run.
	Root int
	// AgentPID is the agent's program that started it, 0 for none.
	AgentPID int
	// SessionAlive is whether its rush session still runs; nil for none.
	SessionAlive func() bool
	// Tell tells the agent why its run was stopped or paused.
	Tell func(string)
	// Log, if set, records what the watchdog did to the run and why.
	Log func(kind string, fields map[string]any)
}

// watchState is the watchdog's, shared with Wake.
type watchState struct {
	mu    sync.Mutex
	root  int
	state State
}

// Watch looks after the run until ctx ends: every Tick it lowers the
// priority of the run's processes as it ages, new ones too, notes what it
// holds, and stops or pauses it as Judge says.
func (r *Run) Watch(ctx context.Context, w Watch) {
	r.w.mu.Lock()
	r.w.root = w.Root
	r.w.mu.Unlock()
	t := time.NewTicker(Tick)
	defer t.Stop()
	start := time.Now()
	var stopping time.Time
	killed := false
	for {
		var now time.Time
		select {
		case <-ctx.Done():
			r.Wake()
			return
		case now = <-t.C:
		}
		tab := proc.Snapshot(nil)
		tree := runTree(tab, w.Root)
		if len(tree) == 0 {
			continue
		}
		if !stopping.IsZero() {
			if now.Sub(stopping) >= stopGrace {
				signalTree(tree, syscall.SIGKILL)
				if !killed {
					killed = true
					w.log("gate-killed", map[string]any{"name": r.ask.Name, "pids": tree, "why": "still running " + stopGrace.String() + " after it was asked to stop"})
				}
			}
			continue
		}
		r.w.mu.Lock()
		st := r.w.state
		r.w.mu.Unlock()
		ran := now.Sub(start) - r.paused
		if st.Paused() {
			ran -= now.Sub(st.PausedAt)
		}
		if !st.Paused() {
			st.Nice = NiceAt(ran)
			for _, pid := range tree {
				renice(pid, st.Nice)
			}
		}
		tab.Fill(nil, []int{w.Root})
		st.Footprint = 0
		for _, pid := range tree {
			if p := tab.Procs[pid]; p != nil {
				st.Footprint += p.Footprint
			}
		}
		l := Look{Ran: ran, Footprint: st.Footprint, Paused: st.Paused(), Pressure: PressureLevel()}
		l.HasAgent = w.AgentPID > 0
		if a := tab.Procs[w.AgentPID]; l.HasAgent && a != nil {
			l.AgentAlive = !proc.Zombie(w.AgentPID)
		}
		top := w.Root
		if p := tab.Procs[top]; p != nil && p.PPID == os.Getpid() {
			top = os.Getpid()
		}
		if p := tab.Procs[top]; p != nil && p.PPID == 1 {
			l.Orphaned = true
			l.SessionAlive = w.SessionAlive != nil && w.SessionAlive()
		}
		if l.Pressure == PressureCritical && !l.Paused {
			l.Newest, l.Others, l.SincePause = r.standing(now)
		}
		act, why := Judge(r.ask.Rule, l)
		if act != Carry {
			w.log(map[Act]string{Stop: "gate-stopped", Pause: "gate-paused", Resume: "gate-resumed"}[act], map[string]any{"name": r.ask.Name, "root": w.Root, "why": why,
				"ranFor": ran.Round(time.Second).String(), "footprint": st.Footprint, "pressure": l.Pressure})
		}
		switch act {
		case Stop:
			signalTree(tree, syscall.SIGTERM)
			if st.Paused() {
				signalTree(tree, syscall.SIGCONT)
			}
			stopping = now
			w.tell(fmt.Sprintf("rush stopped your `%s` run: %s. Don't run it again as it was: run less of it at once, "+
				"or change its limits in the gate's settings.", r.ask.Name, why))
		case Pause:
			signalTree(tree, syscall.SIGSTOP)
			st.PausedAt, st.Why = now, why
			w.tell(fmt.Sprintf("rush paused your `%s` run: %s. It carries on by itself once memory is back to normal; "+
				"it's waiting, not hung, so don't stop it or run it again.", r.ask.Name, why))
		case Resume:
			signalTree(tree, syscall.SIGCONT)
			r.paused += now.Sub(st.PausedAt)
			st.PausedAt, st.Why = time.Time{}, ""
		}
		r.w.mu.Lock()
		r.w.state = st
		r.w.mu.Unlock()
		r.writeState(st)
	}
}

func (w Watch) log(kind string, fields map[string]any) {
	if w.Log != nil {
		w.Log(kind, fields)
	}
}

func (w Watch) tell(s string) {
	if w.Tell != nil {
		w.Tell(s)
	}
}

// Wake lets a paused run go on: one being asked to stop must run to hear
// it.
func (r *Run) Wake() {
	r.w.mu.Lock()
	defer r.w.mu.Unlock()
	if r.w.state.Paused() && r.w.root > 0 {
		signalTree(runTree(proc.Snapshot(nil), r.w.root), syscall.SIGCONT)
	}
}

// standing is, of the runs going unpaused, whether this is the newest
// started and how many others there are; and how long since any run was
// last paused.
func (r *Run) standing(now time.Time) (newest bool, others int, sincePause time.Duration) {
	ts := Tickets()
	mine := ticketName(r.path)
	newest, sincePause = true, time.Duration(1<<62)
	var me Ticket
	for _, t := range ts {
		if t.File == mine {
			me = t
		}
	}
	for _, t := range ts {
		if !t.Running {
			continue
		}
		if t.Paused() {
			sincePause = min(sincePause, now.Sub(t.PausedAt))
			continue
		}
		if t.File == mine {
			continue
		}
		others++
		if t.Since.After(me.Since) || t.Since.Equal(me.Since) && t.File > mine {
			newest = false
		}
	}
	return newest, others, sincePause
}

// runTree is root and everything under it, deepest first, but this
// process: the node preload's holder runs under the run it holds.
func runTree(tab *proc.Table, root int) []int {
	if tab.Procs[root] == nil {
		return nil
	}
	self := os.Getpid()
	var out []int
	for _, pid := range tab.Descendants(root) {
		if pid != self {
			out = append(out, pid)
		}
	}
	return out
}

// signalTree sends sig to every process in tree, its root first, so a
// tree being paused can't start more while it's walked.
func signalTree(tree []int, sig syscall.Signal) {
	for i := len(tree) - 1; i >= 0; i-- {
		_ = proc.Kill(tree[i], sig)
	}
}
