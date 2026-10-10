package host

import (
	"fmt"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// A shell command can hang: a test deadlocked, a prompt nobody will answer,
// a lock never let go. The turn waits on it to its timeout, burning time
// and showing nothing. Rush watches the processes of the commands the turn
// waits on: one that has used no CPU for stuckAfter is stopped, and the
// agent told why, so the turn carries on.
const (
	stuckAfter = 3 * time.Minute
	stuckLook  = 20 * time.Second
)

// stuckNote is what the agent is told of a command rush stopped.
const stuckNote = "rush stopped your command `%s`: its processes used no CPU for %s, so it was hung " +
	"(a deadlock, or waiting on input or a lock). Don't run it again as it was: find why it hung, or run less of it."

// shellCalls keeps the shell commands the main turn waits on, by call:
// those started in the background aren't waited on. Called with mu held.
func (s *server) shellCalls(m event.Message) {
	if m.Parent != "" {
		return
	}
	for _, p := range m.Parts {
		switch {
		case p.Kind == event.ToolCall && p.Call != nil && p.Call.Kind == tool.Shell && !p.Call.Input.Background && m.Role == "assistant":
			if s.waitsOn == nil {
				s.waitsOn = map[string]waited{}
			}
			if _, ok := s.waitsOn[p.Call.ID]; !ok {
				s.waitsOn[p.Call.ID] = waited{cmd: p.Call.Input.Command, at: time.Now()}
			}
		case p.Kind == event.ToolResult && p.Output != nil:
			delete(s.waitsOn, p.Output.CallID)
		}
	}
}

// waited is a shell command the turn waits on.
type waited struct {
	cmd string
	at  time.Time
}

// treeUse is a shell's tree as last looked at: its CPU time and processes,
// and since when neither changed.
type treeUse struct {
	cpu   time.Duration
	n     int
	since time.Time
}

// watchStuck looks every stuckLook at the shells the turn waits on.
func (s *server) watchStuck() {
	t := time.NewTicker(stuckLook)
	defer t.Stop()
	seen := map[int]treeUse{}
	for {
		select {
		case <-s.quit:
			return
		case now := <-t.C:
			s.mu.Lock()
			pid := s.info.ClaudePID
			var from time.Time
			var cmd string
			for _, w := range s.waitsOn {
				if from.IsZero() || w.at.Before(from) {
					from, cmd = w.at, w.cmd
				}
			}
			n := len(s.waitsOn)
			s.mu.Unlock()
			if pid <= 0 || n == 0 {
				clear(seen)
				continue
			}
			tab := proc.Snapshot(nil)
			for _, sh := range idleShells(tab, waitedShells(tab, pid, from), seen, now) {
				if n > 1 {
					cmd = shellCommand(sh)
				}
				stopTree(tab, sh)
				delete(seen, sh)
				s.mu.Lock()
				if s.info.Inbox {
					_ = tell(s.cfg.ID, MainNotice, fmt.Sprintf(stuckNote, short(cmd), stuckAfter))
				}
				s.mu.Unlock()
			}
		}
	}
}

// waitedShells are the agent's shells started since from, a second's grace
// for the clock: the ones the turn waits on, not those it left running in
// the background before.
func waitedShells(tab *proc.Table, agentPID int, from time.Time) []int {
	var out []int
	for _, c := range tab.Descendants(agentPID) {
		p := tab.Procs[c]
		if c == agentPID || p == nil || p.Start.Before(from.Add(-time.Second)) {
			continue
		}
		if parent := tab.Procs[p.PPID]; parent != nil && strings.Contains(proc.CommandLine(p.PPID), "shell-snapshots") {
			continue // inside one already counted
		}
		if strings.Contains(proc.CommandLine(c), "shell-snapshots") {
			out = append(out, c)
		}
	}
	return out
}

// idleShells notes each shell tree's use in seen, and gives those that used
// no CPU and started or ended no process for stuckAfter. One waiting its
// turn at the gate isn't hung.
func idleShells(tab *proc.Table, shells []int, seen map[int]treeUse, now time.Time) []int {
	var out []int
	live := map[int]bool{}
	for _, sh := range shells {
		live[sh] = true
		tab.Fill(nil, []int{sh})
		u := treeUse{since: now}
		gated := false
		for _, c := range tab.Descendants(sh) {
			if p := tab.Procs[c]; p != nil {
				u.cpu += p.CPUTime
				u.n++
				if len(tab.Children[c]) == 0 && strings.Contains(proc.CommandLine(c), " gate ") {
					gated = true
				}
			}
		}
		old, ok := seen[sh]
		if ok && !gated && old.cpu == u.cpu && old.n == u.n {
			u.since = old.since
		}
		seen[sh] = u
		if now.Sub(u.since) >= stuckAfter {
			out = append(out, sh)
		}
	}
	for sh := range seen {
		if !live[sh] {
			delete(seen, sh)
		}
	}
	return out
}

// stopTree ends what a shell runs, deepest first, and the shell with it:
// asked, then made to a few seconds on.
func stopTree(tab *proc.Table, sh int) {
	pids := tab.Descendants(sh)
	for _, p := range pids {
		_ = proc.Kill(p, syscall.SIGTERM)
	}
	time.AfterFunc(3*time.Second, func() {
		for _, p := range pids {
			if proc.Kill(p, 0) == nil {
				_ = proc.Kill(p, syscall.SIGKILL)
			}
		}
	})
}

// shellCommand is what a shell runs, as its command line has it.
func shellCommand(sh int) string {
	line := proc.CommandLine(sh)
	if i := strings.LastIndex(line, "eval "); i >= 0 {
		line = line[i+5:]
	}
	return line
}

// short is a command cut to one line of reasonable length.
func short(cmd string) string {
	cmd = strings.TrimSpace(strings.SplitN(cmd, "\n", 2)[0])
	if r := []rune(cmd); len(r) > 120 {
		cmd = string(r[:119]) + "…"
	}
	return cmd
}
