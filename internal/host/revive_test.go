package host

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
)

// A host killed mid-turn reads as lost, and comes back on the same
// conversation told to carry on; one that went to sleep is never lost.
func TestReviveLostHost(t *testing.T) {
	bin := setup(t)
	acct := claude.Account{Name: "t", ConfigDir: t.TempDir()}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Binary: bin, Account: acct.Profile()})
	if err != nil {
		t.Fatal(err)
	}
	old, _ := ReadInfo(cfg.ID)
	_ = syscall.Kill(old.HostPID, syscall.SIGKILL)
	for deadline := time.Now().Add(5 * time.Second); alive(old.HostPID) && time.Now().Before(deadline); {
		time.Sleep(20 * time.Millisecond)
	}
	raw, _ := readInfoFile(cfg.ID)
	raw.State = "working" // its last word before it was killed
	b, _ := jsonx.MarshalIndent(raw)
	os.WriteFile(filepath.Join(dir(cfg.ID), "info.json"), b, 0o600)

	lost, _ := ReadInfo(cfg.ID)
	if !lost.Lost || lost.State != "stopped" {
		t.Fatalf("killed mid-turn: %+v", lost)
	}
	if err := ensure(cfg.ID, reviveNote); err != nil {
		t.Fatal(err)
	}
	info, _ := ReadInfo(cfg.ID)
	got, _ := ReadConfig(cfg.ID)
	if info.Lost || info.HostPID == old.HostPID || !alive(info.HostPID) || !got.Resume || got.Prompt != reviveNote {
		t.Fatalf("revived: %+v config %+v", info, got)
	}

	raw.Sleeping = true
	if midTurn(raw) {
		t.Fatal("a sleeping session is not mid-turn")
	}
}
