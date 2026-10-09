package host

import (
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
)

func TestRestartDue(t *testing.T) {
	old := time.Now().Add(-time.Hour)
	installed := BinStamp{Mod: time.Now()}
	idle := Info{State: "idle", Exe: BinStamp{Mod: old}}
	with := func(f func(*Info)) Info { i := idle; f(&i); return i }
	for name, c := range map[string]struct {
		info Info
		want bool
	}{
		"idle on an older rush":      {idle, true},
		"working":                    {with(func(i *Info) { i.State = "working" }), false},
		"a question waiting":         {with(func(i *Info) { i.State, i.Needs = "blocked", "Bash" }), false},
		"something queued":           {with(func(i *Info) { i.Queue = []string{"next"} }), false},
		"background work":            {with(func(i *Info) { i.Background = []Task{{ID: "1"}} }), false},
		"stopped by a limit":         {with(func(i *Info) { i.Limit = &Limit{} }), false},
		"already on the new rush":    {with(func(i *Info) { i.Exe = installed }), false},
		"from before it was stamped": {with(func(i *Info) { i.Exe = BinStamp{} }), false},
	} {
		if got := RestartDue(c.info, installed); got != c.want {
			t.Errorf("%s: RestartDue = %v, want %v", name, got, c.want)
		}
	}
}

// An idle host on an older rush than the installed one is restarted; the
// restarted host is on the new binary, and a sweep not asked to restart
// only counts it.
func TestSweepRestartsIdleHost(t *testing.T) {
	bin := setup(t)
	acct := claude.Account{Name: "t", ConfigDir: t.TempDir()}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin, Account: acct.Profile(), SessionID: "sweep-0000-aaaa"})
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if c, err := Dial(cfg.ID); err == nil {
			c.Stop()
			c.Close()
		}
	}()
	old, _ := ReadInfo(cfg.ID)
	if old.Exe.Mod.IsZero() {
		t.Fatal("the host didn't record the binary it started from")
	}
	newer := BinStamp{Mod: old.Exe.Mod.Add(time.Hour)}
	var l Lister
	if stale, restarted := l.Sweep(newer, false); stale != 1 || restarted != 0 {
		t.Fatalf("count only: stale %d restarted %d", stale, restarted)
	}
	if stale, restarted := l.Sweep(newer, true); stale != 0 || restarted != 1 {
		t.Fatalf("restart: stale %d restarted %d", stale, restarted)
	}
	info, _ := ReadInfo(cfg.ID)
	if info.HostPID == old.HostPID || !alive(info.HostPID) || info.SessionID != cfg.SessionID {
		t.Fatalf("restarted: %+v (was pid %d)", info, old.HostPID)
	}
}

// A session idle while its subagents run isn't quiet: restarting its host
// would stop them all.
func TestSubagentsKeepItBusy(t *testing.T) {
	i := Info{State: "idle", Tasks: 3}
	if i.Quiet() || RestartDue(i, BinStamp{Mod: time.Now()}) {
		t.Fatal("restarted with its subagents running")
	}
	if i.Tasks = 0; !i.Quiet() {
		t.Fatal("idle with nothing running isn't quiet")
	}
}
