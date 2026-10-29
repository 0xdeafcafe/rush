package host

import (
	"os"
	"path/filepath"
	"strings"
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
	if err := ensure(cfg.ID, reviveNote, ""); err != nil {
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

// A host gone after its turn ended, its background agents still at work,
// is lost too: they went with it, and it's told to start their work again.
// Its shells and monitors alone don't bring it back, and a reading of it
// shows nothing running.
func TestLoseCutAgents(t *testing.T) {
	agents := Info{ID: "a", State: "idle", Tasks: 2, Background: []Task{
		{ID: "1", Type: "local_agent", Label: "Merge the branches"},
		{ID: "2", Type: "local_bash", Label: "cargo test"},
	}}
	lose(&agents)
	if !agents.Lost || agents.State != "stopped" || len(agents.Background) != 0 || agents.Tasks != 0 {
		t.Fatalf("idle with an agent at work: %+v", agents)
	}
	if got := reviveText(agents); !strings.HasPrefix(got, cutAgentsNote) || !strings.Contains(got, "Merge the branches") || strings.Contains(got, "cargo test") || !IsRushNote(got) {
		t.Fatalf("told: %q", got)
	}

	shells := Info{ID: "b", State: "idle", Background: []Task{{ID: "1", Type: "monitor_mcp"}, {ID: "2", Type: "local_bash"}}}
	if lose(&shells); shells.Lost || len(shells.Background) != 0 {
		t.Fatalf("idle with only a shell and a monitor: %+v", shells)
	}

	turn := Info{ID: "c", State: "working", Background: []Task{{ID: "1", Type: "local_agent", Label: "x"}}}
	if lose(&turn); !turn.Lost || reviveText(turn) != reviveNote {
		t.Fatalf("mid-turn with an agent: %+v", turn)
	}

	asleep := Info{ID: "d", State: "idle", Sleeping: true, Background: []Task{{ID: "1", Type: "local_agent"}}}
	if lose(&asleep); asleep.Lost || asleep.State != "idle" {
		t.Fatalf("asleep: %+v", asleep)
	}

	remote := Info{ID: "e", State: "idle", Remote: "other", Background: []Task{{ID: "1", Type: "local_agent"}}}
	if lose(&remote); remote.Lost {
		t.Fatalf("another machine's: %+v", remote)
	}
}
