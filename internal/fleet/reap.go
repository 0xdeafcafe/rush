package fleet

import (
	"os"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// Reaper ends what an agent leaves running when it stops: shells, servers
// and watchers its tools started, which the system hands to launchd (pid 1)
// once the agent's process is gone and which would otherwise run for days.
// It only ever ends a process it saw under an agent, by its pid and start
// time, so nothing else is touched.
type Reaper struct {
	seen map[int]seenProc
}

type seenProc struct {
	start time.Time
	agent string // the agent's name, for saying what was ended
	// owner is the Claude Code process it ran under: only once that has
	// exited is it left behind. Something the agent deliberately set
	// running on its own (nohup, a daemon) is left alone while it works.
	owner      int
	ownerStart time.Time
}

// ourPID is rush's own process, asked once: Scan runs on the UI.
var ourPID = os.Getpid()

// Leftover is a process an agent left behind, with everything under it.
type Leftover struct {
	PID   int
	Start time.Time
	Agent string
	Cmd   string
	Procs int
}

// Watch notes the processes under every agent that has one, and returns
// those noted before that have since been orphaned: alive, the same
// process, no longer under any agent, and adopted by launchd.
func (r *Reaper) Watch(tab *proc.Table, agents []*Agent) []Leftover {
	out := r.Scan(tab, agents)
	for i := range out {
		out[i].Describe()
	}
	return out
}

// Describe fills in the leftover's command line, which asks the system.
func (l *Leftover) Describe() { l.Cmd = proc.CommandLine(l.PID) }

// Scan is Watch without the command lines: it only reads tab, so it never
// waits. Describe fills each in.
func (r *Reaper) Scan(tab *proc.Table, agents []*Agent) []Leftover {
	if tab == nil {
		return nil
	}
	if r.seen == nil {
		r.seen = map[int]seenProc{}
	}
	under := map[int]bool{}
	self := ourPID
	for _, a := range agents {
		if a.PID == 0 || a.PID == self {
			continue
		}
		root := tab.Procs[a.PID]
		if root == nil {
			continue
		}
		for _, pid := range tab.Descendants(a.PID) {
			under[pid] = true
			if pid == a.PID {
				continue
			}
			p := tab.Procs[pid]
			if p == nil {
				continue
			}
			if s, ok := r.seen[pid]; ok && s.start.Equal(p.Start) {
				continue
			}
			owner := ownerOf(tab, root, p)
			if owner == nil {
				continue
			}
			r.seen[pid] = seenProc{start: p.Start, agent: a.DisplayName, owner: owner.PID, ownerStart: owner.Start}
		}
	}
	var out []Leftover
	for pid, s := range r.seen {
		p := tab.Procs[pid]
		switch {
		case p == nil || !p.Start.Equal(s.start):
			delete(r.seen, pid) // gone, or the pid was reused
		case under[pid]:
		case p.PPID == 1 && !alive(tab, s.owner, s.ownerStart):
			// Adopted by launchd, so the top of what was left: ending it
			// ends what's under it too.
			delete(r.seen, pid)
			out = append(out, Leftover{PID: pid, Start: s.start, Agent: s.agent, Procs: len(tab.Descendants(pid))})
		}
	}
	return out
}

// End stops a leftover and everything under it: SIGTERM, then SIGKILL for
// whatever is still there after grace. A process that isn't the one noted
// (its pid reused) is left alone.
func (l Leftover) End(grace time.Duration) {
	tab := proc.Snapshot(nil)
	if p := tab.Procs[l.PID]; p == nil || !p.Start.Equal(l.Start) {
		return
	}
	pids := tab.Descendants(l.PID)
	killed := 0
	defer func() {
		events.Log("leftover-ended", map[string]any{"pid": l.PID, "cmd": l.Cmd, "agent": l.Agent, "procs": len(pids), "killed": killed, "why": "its agent exited and left it running"})
	}()
	starts := map[int]time.Time{}
	for _, pid := range pids {
		if p := tab.Procs[pid]; p != nil {
			starts[pid] = p.Start
		}
		_ = proc.Kill(pid, syscall.SIGTERM)
	}
	time.Sleep(grace)
	now := proc.Snapshot(nil)
	for _, pid := range pids {
		if p := now.Procs[pid]; p != nil && p.Start.Equal(starts[pid]) {
			_ = proc.Kill(pid, syscall.SIGKILL)
			killed++
		}
	}
}

// ownerOf is the agent's process p runs under for an agent whose process
// is root: root itself when it is an agent's program, else (a rush
// host) the child of root that p descends from. p itself is never its own
// owner.
func ownerOf(tab *proc.Table, root, p *proc.Proc) *proc.Proc {
	if agent.IsProgram(root.Comm) {
		return root
	}
	for q, i := p, 0; q != nil && i < 64; i++ {
		if q.PPID == root.PID {
			if q.PID == p.PID {
				return nil // the host's own child is the owner, not a leftover
			}
			return q
		}
		q = tab.Procs[q.PPID]
	}
	return nil
}

func alive(tab *proc.Table, pid int, start time.Time) bool {
	p := tab.Procs[pid]
	return p != nil && p.Start.Equal(start)
}
