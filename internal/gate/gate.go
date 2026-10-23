// Package gate shares the machine between the intensive programs (tsc,
// go, cargo…) rush's agents run, as docs/gate-sharing.md has it: a run
// waits only for memory, fairly by agent, and its holder lowers its
// priority as it ages and watches it. It's files and flock under
// state.Dir()/gate, no daemon: a ticket frees itself when its holder dies,
// however it dies.
package gate

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/proc"
	"github.com/0xdeafcafe/rush/internal/state"
)

// HeldEnv is set in a gated program's environment: a gate inside it lets
// everything straight through, so nothing waits on itself.
const HeldEnv = "RUSH_GATE_HELD"

// poll is how often a waiter looks again.
const poll = 500 * time.Millisecond

// Root is where the gate keeps its tickets and shims.
func Root() string { return filepath.Join(state.Dir(), "gate") }

// BinDir is the folder of shims sessions find first on PATH.
func BinDir() string { return filepath.Join(Root(), "bin") }

// runs is the one folder of tickets: memory is the machine's, so the
// whole system shares it.
func runs() string { return filepath.Join(Root(), "system") }

// Memory reads what the machine has: available (free, inactive and
// purgeable) and in all, in bytes. Tests set it.
var Memory = memory

// PressureLevel reads the kernel's memory pressure level. Tests set it.
var PressureLevel = pressure

// An Ask is a run asking to start.
type Ask struct {
	Name    string
	Rule    Rule
	Session string  // its agent: RUSH_SESSION, else the agent's pid
	Reserve float64 // the share of memory kept free of gated runs
}

// A Wait is where a waiter stands, told each time it changes.
type Wait struct {
	Ahead, Running int
	// Need is the memory it waits for, and Room what there is, when it's
	// next and memory holds it; both 0 while others go first.
	Need, Room uint64
}

// A Run is a ticket that got in: its holder keeps it until Release.
type Run struct {
	f      *os.File
	path   string
	ask    Ask
	paused time.Duration // in all, so far
	w      watchState
}

// ticketName is a ticket's or run's file as Tickets names it.
func ticketName(path string) string {
	return "ticket-" + strings.TrimPrefix(strings.TrimPrefix(filepath.Base(path), "run-"), "ticket-")
}

// Acquire waits until a's run may start, and is its ticket. waiting, if
// set, hears each change while it waits.
func Acquire(ctx context.Context, a Ask, waiting func(Wait)) (*Run, error) {
	dir := runs()
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	f, path, err := takeTicket(dir, a)
	if err != nil {
		return nil, err
	}
	last := Wait{Running: -1}
	for {
		run, w, err := tryStart(dir, f, path, a)
		if err != nil || run != nil {
			if err != nil {
				_ = os.Remove(path)
				_ = f.Close()
			}
			return run, err
		}
		if !same(w, last) && waiting != nil {
			last = w
			waiting(w)
		}
		select {
		case <-ctx.Done():
			_ = os.Remove(path)
			_ = f.Close()
			return nil, ctx.Err()
		case <-time.After(poll):
		}
	}
}

// same is whether a waiter would tell w as it told last: the room moves
// each look, so only whether memory is the reason counts.
func same(w, last Wait) bool {
	return w.Ahead == last.Ahead && w.Running == last.Running && (w.Need > 0) == (last.Need > 0)
}

// tryStart starts the run when it's next by the fair order and memory
// lets it, else says where it stands. Starting is done under the folder's
// lock, so two waiters never both judge themselves next.
func tryStart(dir string, f *os.File, path string, a Ask) (*Run, Wait, error) {
	lock, err := os.OpenFile(filepath.Join(dir, "admit"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, Wait{}, err
	}
	defer lock.Close()
	// ponytail: a blocking lock, held only while a waiter decides.
	if err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX); err != nil {
		return nil, Wait{}, err
	}
	ts := Tickets()
	mine := filepath.Base(path)
	order := Order(ts)
	w := Wait{}
	for _, t := range ts {
		if t.Running {
			w.Running++
		}
	}
	for _, t := range order {
		if t.File == mine {
			break
		}
		w.Ahead++
	}
	if w.Ahead > 0 {
		return nil, w, nil
	}
	me := Ticket{File: mine, Memory: a.Rule.Memory}
	if w.Running > 0 {
		// Memory it can't read lets the run start, as without a gate.
		if avail, total := Memory(); total > 0 {
			reserve := uint64(float64(total) * a.Reserve)
			if !Admit(me, ts, avail, reserve) {
				w.Need, w.Room = me.Memory, Room(ts, avail, reserve)
				return nil, w, nil
			}
		}
	}
	running := filepath.Join(dir, "run-"+strings.TrimPrefix(mine, "ticket-"))
	if err := os.Rename(path, running); err != nil {
		return nil, w, err
	}
	return &Run{f: f, path: running, ask: a}, w, nil
}

// Release frees the run's place.
func (r *Run) Release() {
	_ = os.Remove(r.path + ".state")
	_ = os.Remove(r.path)
	_ = r.f.Close()
}

var seq atomic.Int64

// takeTicket joins the waiters: a file named by when, held with flock so
// it's seen as gone the moment its holder is. It's locked under a hidden
// name first, so no one sees it unheld.
func takeTicket(dir string, a Ask) (*os.File, string, error) {
	base := fmt.Sprintf("%020d-%d-%d", time.Now().UnixNano(), os.Getpid(), seq.Add(1))
	tmp := filepath.Join(dir, ".t-"+base)
	f, err := os.OpenFile(tmp, os.O_CREATE|os.O_EXCL|os.O_RDWR, 0o600)
	if err != nil {
		return nil, "", err
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = f.Close()
		_ = os.Remove(tmp)
		return nil, "", err
	}
	session := strings.ReplaceAll(a.Session, "\n", " ")
	_, _ = fmt.Fprintf(f, "%d\n%s\n%d\n%s\n%d\n", os.Getpid(), a.Name, time.Now().UnixNano(), session, a.Rule.Memory)
	path := filepath.Join(dir, "ticket-"+base)
	if err := os.Rename(tmp, path); err != nil {
		_ = f.Close()
		_ = os.Remove(tmp)
		return nil, "", err
	}
	return f, path, nil
}

// Tickets is every live ticket, waiting and running, oldest first; dead
// ones are removed.
func Tickets() []Ticket {
	dir := runs()
	ents, _ := os.ReadDir(dir)
	var out []Ticket
	for _, e := range ents {
		n := e.Name()
		running := strings.HasPrefix(n, "run-")
		if !running && !strings.HasPrefix(n, "ticket-") || strings.Contains(n, ".") {
			continue
		}
		p := filepath.Join(dir, n)
		if !live(p) {
			_ = os.Remove(p)
			_ = os.Remove(p + ".state")
			continue
		}
		t := readTicket(p)
		t.File, t.Running = strings.TrimPrefix(strings.TrimPrefix(n, "run-"), "ticket-"), running
		if running {
			t.State = readState(p + ".state")
		}
		out = append(out, t)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].File < out[j].File })
	for i := range out {
		out[i].File = "ticket-" + out[i].File
	}
	return out
}

// live says whether someone holds path's lock.
func live(path string) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()
	return syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil
}

func readTicket(path string) Ticket {
	b, _ := os.ReadFile(path)
	l := strings.Split(string(b), "\n")
	var t Ticket
	if len(l) >= 5 {
		t.PID, _ = strconv.Atoi(l[0])
		t.Name, t.Session = l[1], l[3]
		if ns, err := strconv.ParseInt(l[2], 10, 64); err == nil {
			t.Since = time.Unix(0, ns)
		}
		t.Memory, _ = strconv.ParseUint(l[4], 10, 64)
	}
	return t
}

// readState is what a holder last wrote of its run: nice, footprint, when
// it was paused (0 for not) and why.
func readState(path string) State {
	b, _ := os.ReadFile(path)
	l := strings.SplitN(string(b), "\n", 4)
	var s State
	if len(l) >= 4 {
		s.Nice, _ = strconv.Atoi(l[0])
		s.Footprint, _ = strconv.ParseUint(l[1], 10, 64)
		if ns, _ := strconv.ParseInt(l[2], 10, 64); ns > 0 {
			s.PausedAt = time.Unix(0, ns)
		}
		s.Why = strings.TrimSpace(l[3])
	}
	return s
}

// writeState says s of the run, whole, for anyone reading at once.
func (r *Run) writeState(s State) {
	var at int64
	if s.Paused() {
		at = s.PausedAt.UnixNano()
	}
	b := fmt.Sprintf("%d\n%d\n%d\n%s\n", s.Nice, s.Footprint, at, strings.ReplaceAll(s.Why, "\n", " "))
	if os.WriteFile(r.path+".state.tmp", []byte(b), 0o600) == nil {
		_ = os.Rename(r.path+".state.tmp", r.path+".state")
	}
}

// Waiting is whether the gate holder pid waits for its run to start, or
// holds a run it paused: either way the run is waiting, not hung.
func Waiting(pid int) bool {
	for _, t := range Tickets() {
		if t.PID == pid && (!t.Running || t.Paused()) {
			return true
		}
	}
	return false
}

// HeldFor is whether pid's run is gated already: a running ticket's
// holder is pid's child. So it is when node, its preload's holder holding
// its ticket, replaces itself in place (process.execve) with a shim,
// losing HeldEnv: waiting then, it would wait on its own ticket for ever.
func HeldFor(pid int) bool {
	var tab *proc.Table
	for _, t := range Tickets() {
		if !t.Running {
			continue
		}
		if tab == nil {
			tab = proc.Snapshot(nil)
		}
		if p := tab.Procs[t.PID]; p != nil && p.PPID == pid {
			return true
		}
	}
	return false
}

// A Waiter is a waiting ticket, with its place in the order they go (1 is
// next) and why it waits.
type Waiter struct {
	Ticket
	Place int
	Why   string
}

// Status is the runs going, oldest first, and the waiters in the order
// they go, with reserve the share of memory kept free.
func Status(reserve float64) (running []Ticket, waiting []Waiter) {
	ts := Tickets()
	for _, t := range ts {
		if t.Running {
			running = append(running, t)
		}
	}
	for i, t := range Order(ts) {
		w := Waiter{Ticket: t, Place: i + 1, Why: fmt.Sprintf("behind %d", i)}
		if i == 0 {
			w.Why = "starting"
			if avail, total := Memory(); len(running) > 0 && total > 0 {
				res := uint64(float64(total) * reserve)
				if !Admit(t, ts, avail, res) {
					w.Why = fmt.Sprintf("for memory: needs %s, there's %s past what's kept free", Size(t.Memory), Size(Room(ts, avail, res)))
				}
			}
		}
		waiting = append(waiting, w)
	}
	return running, waiting
}
