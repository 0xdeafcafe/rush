package usage

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"os"
	"path/filepath"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/netwatch"
)

// Every is how old a reading may get before it's read again. Every rush
// process and session shares them, so however many are open an account is
// asked once in that time.
const Every = 5 * time.Minute

// Load is the readings kept at path, by key.
func Load(path string) map[string]Quota {
	out := map[string]Quota{}
	if b, err := os.ReadFile(path); err == nil {
		_ = jsonx.Unmarshal(b, &out)
	}
	return out
}

// Record keeps q under key, unless the reading there is newer.
func Record(path, key string, q Quota) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	unlock := lock(path + ".lock")
	defer unlock()
	all := Load(path)
	if old, ok := all[key]; ok && !q.FetchedAt.After(old.FetchedAt) {
		return nil
	}
	all[key] = q
	b, err := jsonx.Marshal(all)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".quotas-*")
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

// Job is what reading other agents' limits is called where rush shows
// what waits on the network.
const Job = "Other agents' limits"

// Refresh is the reading kept under key, read again with fetch when it's
// older than Every; offline never reads. One process reads at a time, and
// the others find its reading. A failed read keeps the last reading, with
// why in Problem.
func Refresh(path, key string, offline bool, fetch func(context.Context) (Quota, error)) Quota {
	if !offline {
		sum := sha256.Sum256([]byte(key))
		unlock := lock(path + ".locks/" + hex.EncodeToString(sum[:8]))
		defer unlock()
	}
	q := Load(path)[key]
	if offline || time.Since(q.FetchedAt) < Every {
		return q
	}
	if !netwatch.Run(Job) {
		return q // the network's down: the last reading, until it's back
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	got, err := fetch(ctx)
	netwatch.Done(Job, err)
	if err != nil {
		q.Problem = err.Error()
		return q
	}
	if got.FetchedAt.IsZero() {
		got.FetchedAt = time.Now()
	}
	if got.Account == "" {
		got.Account = key
	}
	_ = Record(path, key, got)
	return got
}

// lock holds an flock on the file at path until the returned func is
// called; without one it holds nothing.
func lock(path string) func() {
	if os.MkdirAll(filepath.Dir(path), 0o700) != nil {
		return func() {}
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return func() {}
	}
	_ = syscall.Flock(int(f.Fd()), syscall.LOCK_EX)
	return func() { f.Close() }
}
