package host

import (
	"testing"
	"time"
)

// A reload's note holds until the sessions are on the rush it named, and
// only a rush on that build acts on it.
func TestReloadWanted(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	installed := BinStamp{Mod: time.Now(), Size: 10}
	if ReloadWanted(installed) {
		t.Fatal("wanted with no reload asked for")
	}
	if err := WantReload(installed); err != nil {
		t.Fatal(err)
	}
	if !ReloadWanted(installed) {
		t.Fatal("not wanted right after a reload asked for it")
	}
	if ReloadWanted(BinStamp{Mod: installed.Mod.Add(time.Minute), Size: 10}) {
		t.Fatal("wanted by a rush on another build")
	}
	if !ReloadWanted(installed) {
		t.Fatal("a rush on another build ended the reload")
	}
	_ = WantReload(installed)
	ReloadDone()
	if ReloadWanted(installed) {
		t.Fatal("still wanted once done")
	}
}
