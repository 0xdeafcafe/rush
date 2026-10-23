package gate

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/proc"
)

// The guard is the gate's last line, as docs/gate-sharing.md rule 6 has
// it: not every heavy program comes through the gate (a language server an
// MCP server starts, a binary run by its path), and the kernel's pressure
// level can stay normal while the machine pages itself to a halt. So when
// the memory available falls to the floor, it stops the largest program an
// agent started, one at a time, until there's room again.

// DefaultFloor is the share of memory, in percent, below which the guard
// stops agents' programs, and the watchdog takes memory to be critical.
const DefaultFloor = 5

// GuardTick is how often the guard looks; one sysctl a look while there's
// memory.
const GuardTick = time.Second

// guardGrace is how long a program the guard asked to stop has before
// it's made to.
const guardGrace = 3 * time.Second

// minVictim is the least a program must hold for the guard to stop it:
// past that, what fills the memory isn't an agent's.
const minVictim = 256 << 20

// MemoryLevel reads the share of memory available, 0 to 100; -1 when it
// can't be read. Tests set it.
var MemoryLevel = memLevel

// Pressure is the memory pressure the gate acts on: the kernel's level,
// raised to critical once the memory available is at floor percent or
// less, and to warning at twice that, since macOS can say normal while it
// pages itself to a halt.
func Pressure(floor int) int {
	if floor <= 0 {
		floor = DefaultFloor
	}
	lvl := PressureLevel()
	switch m := MemoryLevel(); {
	case m < 0:
	case m <= floor:
		return PressureCritical
	case m <= 2*floor && lvl < PressureWarn:
		return PressureWarn
	}
	return lvl
}

// A Victim is the program the guard stops: its pid, what it is, what it
// holds, and the agent session it's under ("" for none known).
type Victim struct {
	PID       int
	Comm      string
	Footprint uint64
	Agent     int
}

// isRush is whether p is one of rush's own processes.
func isRush(p *proc.Proc) bool { return p != nil && filepath.Base(p.Comm) == "rush" }

// Agents is the agents' programs in t: each one a rush host runs, that
// is a process not rush's whose parent and grandparent are.
func Agents(t *proc.Table) []int {
	var out []int
	for pid, p := range t.Procs {
		if isRush(p) {
			continue
		}
		if parent := t.Procs[p.PPID]; isRush(parent) && isRush(t.Procs[parent.PPID]) {
			out = append(out, pid)
		}
	}
	return out
}

// Choose is the largest program in t, its footprints filled, of those
// under an agent and those named (the gate's programs, wherever they run:
// a daemon an agent started detaches to launchd, and its tsgo with it).
// Never rush's own, an agent's program itself, or self; false when none
// holds minVictim.
func Choose(t *proc.Table, agents []int, names map[string]bool, self int) (Victim, bool) {
	var best Victim
	consider := func(pid, agent int) {
		p := t.Procs[pid]
		if pid == agent || pid == self || p == nil || isRush(p) {
			return
		}
		if p.Footprint > best.Footprint {
			best = Victim{PID: pid, Comm: p.Comm, Footprint: p.Footprint, Agent: agent}
		}
	}
	for _, a := range agents {
		for _, pid := range t.Descendants(a) {
			consider(pid, a)
		}
	}
	for _, pid := range Named(t, names) {
		consider(pid, 0)
	}
	return best, best.Footprint >= minVictim
}

// Named is the processes in t whose name is one of names.
func Named(t *proc.Table, names map[string]bool) []int {
	var out []int
	for pid, p := range t.Procs {
		if names[filepath.Base(p.Comm)] {
			out = append(out, pid)
		}
	}
	return out
}

// A Guard watches the machine's memory and stops agents' programs when
// it runs out.
type Guard struct {
	Floor int // percent; 0 for DefaultFloor
	// Names are the gate's programs: stopped wherever they run, not only
	// under an agent.
	Names map[string]bool
	// Tell tells the agent of session why its program was stopped.
	Tell func(session, text string)
	// Log records what the guard did and why.
	Log func(kind string, fields map[string]any)
}

// Run guards until ctx ends. Only one guard on the machine acts at a
// time: the one holding the guard's lock; the others wait for it.
func (g Guard) Run(ctx context.Context) {
	floor := g.Floor
	if floor <= 0 {
		floor = DefaultFloor
	}
	var lock *os.File
	defer func() {
		if lock != nil {
			_ = lock.Close()
		}
	}()
	t := time.NewTicker(GuardTick)
	defer t.Stop()
	var (
		victim   Victim
		asked    time.Time
		killed   bool
		idleSaid bool
	)
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		if lock == nil {
			lock = takeGuardLock()
			if lock == nil {
				continue
			}
		}
		if victim.PID > 0 {
			if proc.Kill(victim.PID, 0) == nil && !proc.Zombie(victim.PID) {
				if !killed && time.Since(asked) >= guardGrace {
					stopTree(victim.PID, syscall.SIGKILL)
					killed = true
					g.log("guard-killed", map[string]any{"pid": victim.PID, "name": victim.Comm, "why": "still running " + guardGrace.String() + " after it was asked to stop"})
				}
				continue
			}
			victim = Victim{}
			continue // a tick for the memory it held to come back
		}
		level := MemoryLevel()
		if Pressure(floor) != PressureCritical {
			idleSaid = false
			continue
		}
		tab := proc.Snapshot(nil)
		agents := Agents(tab)
		tab.Fill(nil, append(agents, Named(tab, g.Names)...))
		v, ok := Choose(tab, agents, g.Names, os.Getpid())
		if !ok {
			if !idleSaid {
				idleSaid = true
				g.log("guard-short", map[string]any{"level": level, "floor": floor, "why": "memory is short, but no agent's program or gated one holds much of it"})
			}
			continue
		}
		why := fmt.Sprintf("the machine had %d%% of its memory left, at or under the %d%% floor, and it held the most of any agent's program (%s)", level, floor, Size(v.Footprint))
		if level < 0 {
			why = fmt.Sprintf("the kernel said memory was critical, and it held the most of any agent's program (%s)", Size(v.Footprint))
		}
		stopTree(v.PID, syscall.SIGTERM)
		victim, asked, killed = v, time.Now(), false
		session := sessionOf(v.PID, v.Agent)
		g.log("guard-stopped", map[string]any{"pid": v.PID, "name": v.Comm, "footprint": v.Footprint, "level": level, "floor": floor, "session": session, "why": why})
		if g.Tell != nil && session != "" {
			g.Tell(session, fmt.Sprintf("rush stopped `%s` (pid %d), which your session started: %s. "+
				"Don't start it again as it was: run less at once, one typecheck or test run at a time, and scope it to the paths you touched.", v.Comm, v.PID, why))
		}
	}
}

func (g Guard) log(kind string, f map[string]any) {
	if g.Log != nil {
		g.Log(kind, f)
	}
}

// takeGuardLock is the guard's lock, held, or nil while another guard
// holds it.
func takeGuardLock() *os.File {
	if os.MkdirAll(Root(), 0o700) != nil {
		return nil
	}
	f, err := os.OpenFile(filepath.Join(Root(), "guard"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil
	}
	if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		_ = f.Close()
		return nil
	}
	return f
}

// stopTree sends sig to pid and everything under it, waking any the
// gate paused so they hear it.
func stopTree(pid int, sig syscall.Signal) {
	tree := proc.Snapshot(nil).Descendants(pid)
	signalTree(tree, sig)
	signalTree(tree, syscall.SIGCONT)
}

// sessionOf is the rush session pid runs under, from its environment or
// else its agent's; "" when neither says.
func sessionOf(pids ...int) string {
	for _, pid := range pids {
		for _, kv := range proc.Env(pid) {
			if s, ok := strings.CutPrefix(kv, "RUSH_SESSION="); ok && s != "" {
				return s
			}
		}
	}
	return ""
}
