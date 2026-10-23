package fleet

import (
	"os"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// compactEvery is how long a row's compaction, as last worked out, holds:
// it reads a process's environment and the settings files it reads.
const compactEvery = 10 * time.Second

type compactEntry struct {
	at time.Time
	c  agent.Compaction
}

// compactions fills in each row's Compaction.
func (l *Loader) compactions(agents []*Agent, hosted []host.Info, now time.Time) {
	byID := make(map[string]*host.Info, len(hosted))
	for i := range hosted {
		byID[hosted[i].ID] = &hosted[i]
	}
	if l.compact == nil {
		l.compact = map[string]compactEntry{}
	}
	listed := make(map[string]bool, len(agents))
	// Rows with no process of their own all read rush's environment:
	// those in one folder on one profile compact alike, for compactEvery.
	if l.compactFolder == nil {
		l.compactFolder = map[string]compactEntry{}
	}
	byFolder := l.compactFolder
	for _, a := range agents {
		listed[a.Key] = true
		if e, ok := l.compact[a.Key]; ok && now.Sub(e.at) < compactEvery {
			a.Compaction = e.c
			continue
		}
		var info *host.Info
		if a.Rush {
			info = byID[a.ID]
		}
		folder := ""
		if info == nil && a.PID == 0 {
			folder = a.Kind + "\x00" + a.Acct.Dir + "\x00" + a.Cwd
		}
		if e, ok := byFolder[folder]; ok && folder != "" && now.Sub(e.at) < compactEvery {
			a.Compaction = e.c
		} else {
			p, env := l.sessionEnv(a, info)
			a.Compaction = agent.CompactionOf(agent.Kind(a.Kind), p, a.Cwd, env)
			if folder != "" {
				byFolder[folder] = compactEntry{now, a.Compaction}
			}
		}
		l.compact[a.Key] = compactEntry{now, a.Compaction}
	}
	for k := range l.compact {
		if !listed[k] {
			delete(l.compact, k)
		}
	}
	for pid, e := range l.envs {
		if p := l.procOf(pid); p == nil || !p.Start.Equal(e.start) {
			delete(l.envs, pid)
		}
	}
	for k, e := range byFolder {
		if now.Sub(e.at) >= compactEvery {
			delete(byFolder, k)
		}
	}
}

// sessionEnv is the profile a's session runs on and the environment its
// agent's process has: read off the process while it runs; for a rush
// session asleep, its host's with what rush starts it with on top; else
// rush's own, which a session it starts inherits.
func (l *Loader) sessionEnv(a *Agent, info *host.Info) (agent.Profile, []string) {
	p := a.Acct
	if info == nil {
		if a.PID != 0 {
			if env := l.envOf(a.PID); env != nil {
				return p, env
			}
		}
		return p, os.Environ()
	}
	if info.ClaudePID != 0 {
		if env := l.envOf(info.ClaudePID); env != nil {
			return p, env
		}
	}
	env := os.Environ()
	if info.HostPID != 0 {
		if e := l.envOf(info.HostPID); e != nil {
			env = e
		}
	}
	if cfg, ok := l.config(*info); ok {
		if cfg.Account.Dir != "" {
			p = cfg.Account
		}
		env = append(env[:len(env):len(env)], cfg.Env...)
	}
	return p, env
}

// procEnv is a process's environment, as it started: one never changes
// while it runs, so it's read once for the process, by its start time.
type procEnv struct {
	start time.Time
	env   []string
}

// envOf is pid's environment, read once while it's the same process.
func (l *Loader) envOf(pid int) []string {
	p := l.procOf(pid)
	if p == nil || p.Start.IsZero() {
		return proc.Env(pid)
	}
	if e, ok := l.envs[pid]; ok && e.start.Equal(p.Start) {
		return e.env
	}
	env := proc.Env(pid)
	if env != nil {
		if l.envs == nil {
			l.envs = map[int]procEnv{}
		}
		l.envs[pid] = procEnv{start: p.Start, env: env}
	}
	return env
}

// procOf is pid in the last process table, if it was there.
func (l *Loader) procOf(pid int) *proc.Proc {
	if l.prevTab == nil {
		return nil
	}
	return l.prevTab.Procs[pid]
}
