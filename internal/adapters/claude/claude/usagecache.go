package claude

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/netwatch"
)

// UsageEvery is how old rush's last reading of an account may get before
// it asks Anthropic again. Every rush process shares the readings, so
// restarts, several views and --soak runs don't each ask.
const UsageEvery = 5 * time.Minute

// FetchedUsage is rush's last good reading of an account, and until when
// Anthropic asked it not to ask again.
type FetchedUsage struct {
	Usage Usage     `json:"usage"`
	Wait  time.Time `json:"wait,omitzero"`
}

// LoadFetchedUsage reads the readings rush keeps at path, by config folder.
func LoadFetchedUsage(path string) map[string]FetchedUsage {
	out := map[string]FetchedUsage{}
	if b, err := os.ReadFile(path); err == nil {
		_ = jsonx.Unmarshal(b, &out)
	}
	return out
}

// SaveFetchedUsage records one account's entry, keeping the others.
func SaveFetchedUsage(path, key string, f FetchedUsage) error {
	return updateFetchedUsage(path, func(all map[string]FetchedUsage) bool {
		all[key] = f
		return true
	})
}

// RecordUsage keeps a reading made elsewhere (a session's, as it runs)
// under key, unless the one there is newer; when Anthropic had said to
// wait, the wait stands.
func RecordUsage(path, key string, u Usage) error {
	return updateFetchedUsage(path, func(all map[string]FetchedUsage) bool {
		f := all[key]
		if !u.FetchedAt.After(f.Usage.FetchedAt) {
			return false
		}
		f.Usage = u
		all[key] = f
		return true
	})
}

// RecordLiveUsage keeps a running Claude Code's own reading of its plan
// usage as startedAs's, the login a's folder was signed in as when it
// started, but only while the folder still is: once it's switched to
// another, Claude Code picks up the new sign-in as it goes, and whose
// reading it sent can't be told, so it's left out rather than written
// onto the wrong login. The folder's name alone won't do: a Claude Code
// started as startedAs writes its name back over a switch, beside the
// other login's sign-in it has since picked up, so the sign-in has to be
// startedAs's too.
func RecordLiveUsage(path string, a Account, startedAs string, u Usage) error {
	if startedAs == "" || SignedInAs(a) != startedAs {
		return nil
	}
	if id, err := signInOwner(a); err != nil || id != startedAs {
		return nil
	}
	u.AccountID = startedAs
	return RecordUsage(path, Login{ID: startedAs}.UsageKey(), u)
}

// signInOwner is whose the sign-in a's folder holds is, asked of
// Anthropic once per token.
var signInOwner = func(a Account) (string, error) {
	raw, err := readCreds(a)
	if err != nil {
		return "", err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return Owner(ctx, raw)
}

// updateFetchedUsage changes the readings at path under a lock: every
// rush process and every session's host writes them.
func updateFetchedUsage(path string, change func(map[string]FetchedUsage) bool) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	if lf, err := os.OpenFile(path+".lock", os.O_CREATE|os.O_RDWR, 0o600); err == nil {
		defer lf.Close()
		_ = syscall.Flock(int(lf.Fd()), syscall.LOCK_EX)
	}
	all := LoadFetchedUsage(path)
	if !change(all) {
		return nil
	}
	b, err := jsonx.Marshal(all)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".usage-*")
	if err != nil {
		return err
	}
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	tmp.Close()
	return os.Rename(tmp.Name(), path)
}

// RefreshUsage is an account's plan usage, asked of Anthropic only when the
// reading shared through path is older than UsageEvery and Anthropic hasn't
// said to wait; offline never asks. Every rush process goes through here,
// so however many are open an account is asked once per UsageEvery.
// Readings are kept by the login the folder is signed in as, so a
// session's own readings and the login's fetch are the same one.
func RefreshUsage(path string, a Account, offline bool) Usage {
	who := SignedInAs(a)
	key := a.ConfigDir
	if who != "" {
		key = Login{ID: who}.UsageKey()
	}
	return RefreshUsageFor(path, key, offline, func(ctx context.Context) (Usage, error) {
		u, err := orRenew(key, func() (Usage, error) { return FetchUsage(ctx, a) }, func() error { return Renew(ctx, a) })
		if err == nil && who != "" && u.AccountID != who {
			// Its sign-in is another account's than it says: the
			// reading isn't who's. FindLogins puts it right.
			return Usage{}, errors.New("signed in as another account than it says")
		}
		return u, err
	})
}

// UsageKey is where a reading of the account a is signed in as is kept.
func UsageKey(a Account, u Usage) string {
	if u.AccountID != "" {
		return Login{ID: u.AccountID}.UsageKey()
	}
	return a.ConfigDir
}

// UsageJob is what asking Anthropic for plan usage is called where rush
// shows what waits on the network.
const UsageJob = "Claude plan usage"

// RefreshUsageFor is RefreshUsage for readings kept under key, fetched by
// fetch.
func RefreshUsageFor(path, key string, offline bool, fetch func(context.Context) (Usage, error)) Usage {
	if !offline {
		// One asks at a time, so the others find its reading.
		unlock := lockKey(path, key)
		defer unlock()
	}
	f := LoadFetchedUsage(path)[key]
	u, now := f.Usage, time.Now()
	if now.Before(f.Wait) {
		u.Problem = "rate-limited to " + f.Wait.Local().Format("15:04")
	}
	if offline || now.Before(f.Wait) || now.Sub(u.FetchedAt) < UsageEvery {
		return u
	}
	if !netwatch.Run(UsageJob) {
		return u // the network's down: the last reading, until it's back
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	got, err := fetch(ctx)
	netwatch.Done(UsageJob, err)
	var rl *ErrRateLimited
	switch {
	case err == nil:
		u = got
		_ = RecordUsage(path, key, got)
	case errors.As(err, &rl):
		_ = SaveFetchedUsage(path, key, FetchedUsage{Usage: f.Usage, Wait: rl.Until})
		u.Problem = "rate-limited to " + rl.Until.Local().Format("15:04")
	default:
		u.Problem = err.Error()
	}
	return u
}

// lockKey holds the lock on asking Anthropic about key, across every
// rush process.
func lockKey(path, key string) func() {
	sum := sha256.Sum256([]byte(key))
	dir := path + ".locks"
	if os.MkdirAll(dir, 0o700) != nil {
		return func() {}
	}
	f, err := os.OpenFile(filepath.Join(dir, hex.EncodeToString(sum[:8])), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return func() {}
	}
	_ = syscall.Flock(int(f.Fd()), syscall.LOCK_EX)
	return func() { f.Close() }
}
