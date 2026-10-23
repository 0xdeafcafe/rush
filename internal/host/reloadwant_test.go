package host

import (
	"testing"
	"time"
)

// A reload's note holds until the sessions are on the rush it named, and
// ends by itself once another rush is installed.
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
		t.Fatal("still wanted once another rush was installed")
	}
	if ReloadWanted(installed) {
		t.Fatal("the note outlived the rush it named")
	}
	_ = WantReload(installed)
	ReloadDone()
	if ReloadWanted(installed) {
		t.Fatal("still wanted once done")
	}
}
