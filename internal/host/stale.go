package host

import (
	"os"
	"time"

	"github.com/0xdeafcafe/photon/uithread"
)

// BinStamp is a binary as it was on disk: when it was written and how big
// it is. A host records its own at start, so a rush installed over it
// since shows as newer.
type BinStamp struct {
	Mod  time.Time `json:"mod"`
	Size int64     `json:"size"`
}

// StampOf is the stamp of the file at path; zero when it can't be read.
func StampOf(path string) BinStamp {
	fi, err := os.Stat(path)
	if err != nil {
		return BinStamp{}
	}
	return BinStamp{Mod: fi.ModTime(), Size: fi.Size()}
}

// ExeStamp is the stamp of the rush that's running: a host's Exe.
func ExeStamp() BinStamp {
	exe, err := os.Executable()
	if err != nil {
		return BinStamp{}
	}
	return StampOf(exe)
}

// Stale is whether a rush newer than the one this host started from is
// installed. A host from before it recorded one is never stale: it can't
// be told from one that's current.
func (i Info) Stale(installed BinStamp) bool {
	return !i.Exe.Mod.IsZero() && installed.Mod.After(i.Exe.Mod)
}

// Quiet is whether the session is between turns: idle with nothing
// queued, asked, running in the background, stopped by a limit or being
// retried. It's the only time a host may be restarted.
func (i Info) Quiet() bool {
	return i.State == "idle" && len(i.Queue) == 0 && i.Needs == "" &&
		len(i.Background) == 0 && i.Limit == nil && i.Retry == nil
}

// RestartDue is whether a host should be restarted on the installed
// rush now: a newer one is installed and the session is between turns.
func RestartDue(i Info, installed BinStamp) bool { return i.Stale(installed) && i.Quiet() }

// Sweep is how many live sessions' hosts are on an older rush than the
// one installed, and how many of those it restarted (when restart is
// set) because they were between turns. A session that isn't keeps
// running as it is; the next sweep looks again. It reads every session's
// info and restarts hosts, so never on the UI goroutine.
func (l *Lister) Sweep(installed BinStamp, restart bool) (stale, restarted int) {
	uithread.Forbid("host.Lister.Sweep")
	for _, i := range l.List() {
		if i.State == "stopped" || i.Sleeping || !i.Stale(installed) { // a sleeping one wakes on the new binary anyway
			continue
		}
		stale++
		if !restart || !RestartDue(i, installed) {
			continue
		}
		if cfg, err := ReadConfig(i.ID); err != nil || cfg.Owner > 0 {
			continue // a stand-in's host ends with the program that started it
		}
		// Read again: a message may have come since the list was read.
		if now, err := ReadInfo(i.ID); err != nil || !RestartDue(now, installed) {
			continue
		}
		if Restart(i.ID, func(*Config) {}) == nil {
			restarted++
			stale--
		}
	}
	return stale, restarted
}
