// Package pushlater holds an agent's git push until its branch has been
// quiet, so a burst of pushes reaches the remote, and its CI, as one.
//
// The gate's hook rewrites a plain trailing `git push` in a session to
// `rush push-later`, which writes a hold and starts a waiter. The waiter
// sends the newest hold for the branch once Quiet passes with no newer one,
// and tells the session when the push fails. Pushes to main, master or the
// remote's default branch are never held, and RUSH_PUSH_NOW=1 turns it off.
package pushlater

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/state"
)

// Quiet is how long a branch goes without a new push before its held push
// is sent.
const Quiet = 2 * time.Minute

// Off is whether pushes go at once: RUSH_PUSH_NOW is set, or this isn't a
// rush session.
func Off() bool { return os.Getenv("RUSH_PUSH_NOW") != "" || os.Getenv("RUSH_SESSION") == "" }

// trailingPush is a `git push` that ends the command: alone, or after && or
// ; with nothing after it.
var trailingPush = regexp.MustCompile(`(^|&&|;|\n)(\s*)git\s+push((?:\s+[^\s;&|<>()$` + "`" + `]+)*)\s*$`)

// safeFlags are the flags a held push may carry; any other leaves the
// command alone, as do refspecs with a colon or naming a protected branch.
var safeFlags = []string{"-u", "--set-upstream", "-q", "--quiet", "--no-verify", "-v", "--verbose"}

// Rewrite is command with its trailing `git push ...` run as
// `<exe> push-later ...`, when it is one a hold may stand in for.
func Rewrite(command, exe string) (string, bool) {
	m := trailingPush.FindStringSubmatchIndex(command)
	if m == nil {
		return "", false
	}
	args := strings.Fields(command[m[6]:m[7]])
	if len(args) > 2+countFlags(args) {
		return "", false // more than a remote and a branch
	}
	for _, a := range args {
		switch {
		case strings.HasPrefix(a, "-") && !slices.Contains(safeFlags, a):
			return "", false
		case strings.Contains(a, ":") || Protected(a):
			return "", false
		}
	}
	return command[:m[5]] + quote(exe) + " push-later" + command[m[6]:m[7]], true
}

func countFlags(args []string) int {
	n := 0
	for _, a := range args {
		if strings.HasPrefix(a, "-") {
			n++
		}
	}
	return n
}

func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }

// Protected is whether branch is one a push is never held for.
func Protected(branch string) bool { return branch == "main" || branch == "master" }

// A Hold is a push waiting for its branch to go quiet.
type Hold struct {
	Dir     string    `json:"dir"`
	Branch  string    `json:"branch"`
	Args    []string  `json:"args"`
	Session string    `json:"session"`
	At      time.Time `json:"at"`
}

// Key names the hold for dir's branch.
func Key(dir, branch string) string {
	s := sha256.Sum256([]byte(dir + "\x00" + branch))
	return hex.EncodeToString(s[:8])
}

func folder() string { return filepath.Join(state.Dir(), "push-later") }

func path(key string) string { return filepath.Join(folder(), key+".json") }

// Write records h, replacing any older hold for its branch.
func Write(h Hold) (string, error) {
	if err := os.MkdirAll(folder(), 0o700); err != nil {
		return "", err
	}
	key := Key(h.Dir, h.Branch)
	b, err := json.Marshal(h)
	if err != nil {
		return "", err
	}
	tmp := path(key) + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return "", err
	}
	return key, os.Rename(tmp, path(key))
}

// Read is the hold under key.
func Read(key string) (Hold, error) {
	var h Hold
	b, err := os.ReadFile(path(key))
	if err != nil {
		return h, err
	}
	return h, json.Unmarshal(b, &h)
}

// Due is how long a hold made at at still waits, at now.
func Due(at, now time.Time) time.Duration { return max(0, at.Add(Quiet).Sub(now)) }

// A Waiter sends a held push once its branch is quiet. Its clock, sleep
// and push are fields so tests can drive it.
type Waiter struct {
	Now   func() time.Time
	Sleep func(time.Duration)
	Push  func(Hold) error
	Fail  func(Hold, error)
}

// Run waits on the hold under key until it is due, sends it, and keeps on
// while a newer hold came during the push. It returns once none is left.
func (w Waiter) Run(key string) {
	for {
		h, err := Read(key)
		if err != nil {
			return
		}
		if d := Due(h.At, w.Now()); d > 0 {
			w.Sleep(d)
			continue
		}
		err = w.Push(h)
		if now, rerr := Read(key); rerr == nil && !now.At.Equal(h.At) {
			continue // a newer push came while this one went: it carries this one
		}
		_ = os.Remove(path(key))
		if err != nil && w.Fail != nil {
			w.Fail(h, err)
		}
		return
	}
}

// Take removes and returns the hold under key, for a push sent at once.
func Take(key string) (Hold, error) {
	h, err := Read(key)
	if err != nil {
		return h, err
	}
	return h, os.Remove(path(key))
}

// Lock claims the waiter for key: false when a live one has it.
func Lock(key string, alive func(int) bool) bool {
	p := filepath.Join(folder(), key+".pid")
	if b, err := os.ReadFile(p); err == nil {
		var pid int
		if json.Unmarshal(b, &pid) == nil && pid != os.Getpid() && alive(pid) {
			return false
		}
	}
	b, _ := json.Marshal(os.Getpid())
	return os.WriteFile(p, b, 0o600) == nil
}

// Unlock frees key's waiter lock, if this process holds it.
func Unlock(key string) {
	p := filepath.Join(folder(), key+".pid")
	var pid int
	if b, err := os.ReadFile(p); err == nil && json.Unmarshal(b, &pid) == nil && pid == os.Getpid() {
		_ = os.Remove(p)
	}
}

// ErrNone is a --now with nothing held for the branch.
var ErrNone = errors.New("no push is held for this branch")
