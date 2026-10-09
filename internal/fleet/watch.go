package fleet

import (
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fswait"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
	"github.com/0xdeafcafe/rush/internal/state"
)

// watching is what lets a Load that follows nothing new reuse the last
// reading: most refreshes find every file as it was, and only the
// processes' CPU and memory to read again.
type watching struct {
	w        *fswait.Watcher
	every    time.Duration // a full reading at least this often
	settle   bool          // the next Load may reuse
	stale    bool          // something was handed in since: read afresh
	last     *Snapshot     // the last full reading, its agents copied
	lastFull time.Time
	// What changed since the last load, drained once as it begins (load
	// seq): a file watched and not changed since a cache looked at it
	// needn't be looked at again (see fresh).
	seq     uint64
	chg     map[string]bool
	all     bool              // it can't tell what changed: look at everything
	touched map[string]uint64 // the load each path was last told changed in
	allAt   uint64            // the last load that couldn't tell
	sweep   bool              // this load trusts no watch: what one missed heals
	swept   time.Time         // the last sweep
}

// sweepEvery is how often a load looks at everything, watched or not.
const sweepEvery = 30 * time.Second

// Watch has the loader watch what it reads, so a Load after Settle reuses
// its last reading when none of it changed. It reads everything afresh at
// least every "every", for what isn't watched (other agents' folders, the
// time-based parts of a row).
func (l *Loader) Watch(every time.Duration) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.watching = &watching{every: every} // its watcher starts with the first load
}

// Settle lets the next Load reuse the last reading if nothing it reads has
// changed since. A refresh on a timer settles; one after something you did
// doesn't, so it always reads afresh.
func (l *Loader) Settle() {
	l.inMu.Lock()
	defer l.inMu.Unlock()
	l.in.settle = true
}

// changedSince marks the last reading stale: spend, usage or a nudge was
// handed in, which only a full Load takes up.
func (l *Loader) changedSince() {
	if l.watching != nil {
		l.watching.stale = true
	}
}

// reuse is the last reading with the processes sampled again, when nothing
// watched has changed; ok is false when Load should read afresh.
func (l *Loader) reuse(now time.Time) (*Snapshot, bool) {
	wt := l.watching
	settle := wt != nil && wt.settle
	if wt != nil {
		wt.settle = false
	}
	if !settle || wt.last == nil || wt.stale || now.Sub(wt.lastFull) >= wt.every || wt.sweep || wt.all || len(wt.chg) > 0 {
		return nil, false
	}
	tab := proc.Snapshot(l.prevTab)
	snap := &Snapshot{At: now, Accounts: wt.last.Accounts, Logins: wt.last.Logins, Agents: make([]*Agent, 0, len(wt.last.Agents))}
	for _, a := range wt.last.Agents {
		if a.PID != 0 && tab.Procs[a.PID] == nil {
			return nil, false // a process ended: its row changes
		}
		c := *a
		l.sample(tab, &c)
		c.Temp = l.Temp.Bytes(c.Key)
		snap.Agents = append(snap.Agents, &c)
	}
	snap.Machine = l.machine(tab, snap)
	l.prevTab, snap.Table = tab, tab
	sort.SliceStable(snap.Agents, func(i, j int) bool { return snap.Agents[i].Age(now) < snap.Agents[j].Age(now) })
	return snap, true
}

// drain takes what changed since the last load, once, as one begins.
func (l *Loader) drain(now time.Time) {
	wt := l.watching
	if wt == nil {
		return
	}
	wt.seq++
	wt.chg, wt.all = wt.w.Changes()
	if wt.touched == nil {
		wt.touched = map[string]uint64{}
	}
	for p := range wt.chg {
		wt.touched[p] = wt.seq
	}
	if wt.all {
		wt.allAt = wt.seq
	}
	wt.sweep = wt.all || now.Sub(wt.swept) >= sweepEvery
}

// checked is the load a cache looking at its file now notes, for fresh
// to ask after; 0 when nothing is watched, which fresh never trusts.
func (l *Loader) checked() uint64 {
	if wt := l.watching; wt != nil && !l.quick {
		return wt.seq
	}
	return 0
}

// began starts a full Load: what changed up to now, it reads.
func (l *Loader) began(now time.Time) {
	l.subWatch = l.subWatch[:0]
	if wt := l.watching; wt != nil {
		wt.stale, wt.settle = false, false
		if wt.sweep && !l.quick {
			wt.swept = now
		}
	}
}

// fresh is whether path is watched and hasn't changed since a cache
// looked at it in load at (its checked()), which drained what changed
// before it looked: what it read then still holds,
// without looking again. A sweep trusts nothing, nor a load after one that
// couldn't tell what changed.
func (l *Loader) fresh(path string, at uint64) bool {
	wt := l.watching
	return wt != nil && wt.w != nil && at != 0 && !wt.sweep && !l.quick &&
		wt.allAt <= at && wt.touched[path] <= at && wt.w.Watching(path)
}

// read keeps a full Load's reading for reuse, and watches what it read.
func (l *Loader) read(snap *Snapshot, hosted []host.Info) {
	wt := l.watching
	if wt == nil {
		return
	}
	last := &Snapshot{At: snap.At, Accounts: snap.Accounts, Logins: snap.Logins, Agents: make([]*Agent, len(snap.Agents))}
	for i, a := range snap.Agents {
		c := *a
		last.Agents[i] = &c
	}
	wt.last, wt.lastFull = last, snap.At
	wt.w.Watch(l.watchPaths(snap, hosted))
	for p := range wt.touched {
		if !wt.w.Watching(p) {
			delete(wt.touched, p) // watched again, it's told changed again
		}
	}
}

// watchPaths is what a full Load read that can change under it: the
// folders sessions and jobs register in, the hosts' info, the settings it
// reads, and each live agent's transcript and subagents.
func (l *Loader) watchPaths(snap *Snapshot, hosted []host.Info) []string {
	p := l.store.Config.ActiveAccount().Profile()
	paths := append([]string{state.Dir(), host.Root(), l.UsagePath}, l.subWatch...)
	for _, info := range hosted {
		// Its info.json is replaced on each change, a stopped one's too
		// when it's started again: the folder says so.
		paths = append(paths, filepath.Join(host.Root(), info.ID))
	}
	var jobs []string
	for _, a := range snap.Agents {
		if !a.Live() && a.PID == 0 && !a.Busy() {
			continue
		}
		if a.ID != "" && !a.Rush && !a.Interactive {
			jobs = append(jobs, a.ID)
		}
		if p := a.TranscriptPath; p != "" {
			sess := strings.TrimSuffix(p, ".jsonl")
			paths = append(paths, p, filepath.Dir(p), sess, filepath.Join(sess, "subagents"))
		}
	}
	if k, ok := agent.As[agent.JobKeeper](p.Kind); ok {
		paths = append(paths, k.Watched(p, jobs)...)
	}
	return paths
}
