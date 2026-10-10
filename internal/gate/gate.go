// Package gate queues intensive programs (tsc, go, cargo…) across rush's
// sessions: at most N run at once in a scope, with a pause between
// starts. It's files and flock under state.Dir()/gate, no daemon: a slot
// or a place in the queue frees itself when its holder dies, however it
// dies.
package gate

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/state"
)

// HeldEnv is set in a gated program's environment, to its scope: a gate
// inside it lets everything straight through, so nothing waits on itself.
const HeldEnv = "RUSH_GATE_HELD"

// poll is how often a waiter looks again.
const poll = 250 * time.Millisecond

// Root is where the gate keeps its scopes and shims.
func Root() string { return filepath.Join(state.Dir(), "gate") }

// BinDir is the folder of shims sessions find first on PATH.
func BinDir() string { return filepath.Join(Root(), "bin") }

// A Wait is where a waiter stands, told each time it changes.
type Wait struct {
	Label          string
	Running, Ahead int
	// Load is the load average holding it back while a slot is free, or 0.
	Load float64
}

// Acquire waits for one of r.Parallel slots in the scope key, first come
// first served, and sleeps out r.Stagger since the scope's last start. The
// release it returns frees the slot. waiting, if set, hears each change
// while it waits.
func Acquire(ctx context.Context, r Rule, key, label, name string, waiting func(Wait)) (release func(), err error) {
	dir := filepath.Join(Root(), hash(key))
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	if _, err := os.Stat(filepath.Join(dir, "scope")); err != nil {
		_ = os.WriteFile(filepath.Join(dir, "scope"), []byte(label), 0o600)
	}
	me, ticket, err := takeTicket(dir, name)
	if err != nil {
		return nil, err
	}
	defer func() { _ = os.Remove(ticket); _ = me.Close() }()
	last := Wait{Label: label, Running: -1}
	for {
		ahead := ticketsAhead(dir, filepath.Base(ticket))
		load := busy(r, dir)
		if ahead == 0 && load == 0 {
			if slot := takeSlot(dir, r.Parallel, name); slot != nil {
				if err := stagger(ctx, dir, r.Stagger); err != nil {
					_ = slot.Close()
					return nil, err
				}
				return func() { _ = slot.Close() }, nil
			}
		}
		if w := (Wait{Label: label, Running: held(dir, r.Parallel), Ahead: ahead, Load: load}); !same(w, last) && waiting != nil {
			last = w
			waiting(w)
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(poll):
		}
	}
}

// busy is the load average when the machine is too busy for another run
// in dir's scope to start, or 0. One always may, so nothing waits on load
// it can't change.
func busy(r Rule, dir string) float64 {
	if r.Busy <= 0 || held(dir, r.Parallel) == 0 {
		return 0
	}
	if l := load1(); l > r.Busy*float64(runtime.NumCPU()) {
		return l
	}
	return 0
}

// same is whether a waiter would tell w as it told last: load moves each
// look, so only whether it's the reason counts.
func same(w, last Wait) bool {
	return w.Label == last.Label && w.Running == last.Running && w.Ahead == last.Ahead && (w.Load > 0) == (last.Load > 0)
}

var seq atomic.Int64

// takeTicket joins the queue: a file named by when, held with flock so
// it's seen as gone the moment its holder is. It's locked under a hidden
// name first, so no one sees it unheld.
func takeTicket(dir, name string) (*os.File, string, error) {
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
	_, _ = f.WriteString(holderLine(name))
	path := filepath.Join(dir, "ticket-"+base)
	if err := os.Rename(tmp, path); err != nil {
		_ = f.Close()
		_ = os.Remove(tmp)
		return nil, "", err
	}
	return f, path, nil
}

// ticketsAhead counts the live tickets before mine, removing dead ones.
func ticketsAhead(dir, mine string) int {
	n := 0
	for _, t := range tickets(dir) {
		if t == mine {
			break
		}
		if live(filepath.Join(dir, t), true) {
			n++
		}
	}
	return n
}

func tickets(dir string) []string {
	ents, _ := os.ReadDir(dir)
	var out []string
	for _, e := range ents {
		if strings.HasPrefix(e.Name(), "ticket-") {
			out = append(out, e.Name())
		}
	}
	sort.Strings(out)
	return out
}

// live says whether someone holds path's lock. A dead one is removed, if
// asked.
func live(path string, prune bool) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()
	if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		return true
	}
	if prune {
		_ = os.Remove(path)
	}
	return false
}

// takeSlot is a free slot, locked and labelled, or nil.
func takeSlot(dir string, n int, name string) *os.File {
	for i := range max(n, 1) {
		f, err := os.OpenFile(filepath.Join(dir, "slot-"+strconv.Itoa(i)), os.O_CREATE|os.O_RDWR, 0o600)
		if err != nil {
			continue
		}
		if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
			_ = f.Close()
			continue
		}
		_ = f.Truncate(0)
		_, _ = f.WriteAt([]byte(holderLine(name)), 0)
		return f
	}
	return nil
}

// held counts the slots taken, of the first n.
func held(dir string, n int) int {
	c := 0
	for i := range max(n, 1) {
		if live(filepath.Join(dir, "slot-"+strconv.Itoa(i)), false) {
			c++
		}
	}
	return c
}

// stagger sleeps out the rest of d since the scope's last start, then
// makes this the last, under the start file's own lock.
func stagger(ctx context.Context, dir string, d time.Duration) error {
	if d <= 0 {
		return nil
	}
	f, err := os.OpenFile(filepath.Join(dir, "start"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer f.Close()
	// ponytail: a blocking lock, held at most one stagger by whoever sleeps.
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX); err != nil {
		return err
	}
	b := make([]byte, 32)
	n, _ := f.ReadAt(b, 0)
	if last, err := strconv.ParseInt(strings.TrimSpace(string(b[:n])), 10, 64); err == nil {
		if rest := time.Until(time.Unix(0, last).Add(d)); rest > 0 {
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-time.After(rest):
			}
		}
	}
	_ = f.Truncate(0)
	_, err = f.WriteAt([]byte(strconv.FormatInt(time.Now().UnixNano(), 10)), 0)
	return err
}

func holderLine(name string) string {
	return fmt.Sprintf("%d\n%s\n%d\n", os.Getpid(), name, time.Now().UnixNano())
}

func hash(key string) string {
	s := sha256.Sum256([]byte(key))
	return hex.EncodeToString(s[:8])
}

// Key is the queue a command in dir joins for scope, and how to name it:
// its worktree, its repo (every worktree of it), or the whole system.
// Outside a repo, worktree and repo are dir itself.
func Key(scope, dir string) (key, label string) {
	dir, _ = filepath.Abs(dir)
	switch scope {
	case "system":
		return "system", "the system"
	case "worktree":
		if top := git(dir, "--show-toplevel"); top != "" {
			return "worktree:" + top, top
		}
	default:
		if common := git(dir, "--git-common-dir"); common != "" {
			if !filepath.IsAbs(common) {
				common = filepath.Join(dir, common)
			}
			if r, err := filepath.EvalSymlinks(common); err == nil {
				common = r
			}
			label := common
			if filepath.Base(common) == ".git" {
				label = filepath.Dir(common)
			}
			return "repo:" + common, label
		}
	}
	return "dir:" + dir, dir
}

func git(dir, what string) string {
	out, err := exec.Command("git", "-C", dir, "rev-parse", what).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// A Holder is a program running, or waiting, in a scope.
type Holder struct {
	PID   int
	Name  string
	Since time.Time
}

// A Scope is one queue as it stands.
type Scope struct {
	Label            string
	Running, Waiting []Holder
}

// Status is every scope with someone in it.
func Status() []Scope {
	ents, _ := os.ReadDir(Root())
	var out []Scope
	for _, e := range ents {
		if !e.IsDir() || e.Name() == "bin" {
			continue
		}
		dir := filepath.Join(Root(), e.Name())
		label, _ := os.ReadFile(filepath.Join(dir, "scope"))
		s := Scope{Label: string(label)}
		files, _ := os.ReadDir(dir)
		for _, f := range files {
			p := filepath.Join(dir, f.Name())
			switch {
			case strings.HasPrefix(f.Name(), "slot-") && live(p, false):
				s.Running = append(s.Running, readHolder(p))
			case strings.HasPrefix(f.Name(), "ticket-") && live(p, false):
				s.Waiting = append(s.Waiting, readHolder(p))
			}
		}
		if len(s.Running)+len(s.Waiting) > 0 {
			out = append(out, s)
		}
	}
	return out
}

func readHolder(path string) Holder {
	b, _ := os.ReadFile(path)
	l := strings.SplitN(string(b), "\n", 4)
	var h Holder
	if len(l) >= 3 {
		h.PID, _ = strconv.Atoi(l[0])
		h.Name = l[1]
		if ns, err := strconv.ParseInt(l[2], 10, 64); err == nil {
			h.Since = time.Unix(0, ns)
		}
	}
	return h
}
