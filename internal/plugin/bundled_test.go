package plugin

import (
	"io"
	"testing"
)

func TestBundledOnUntilTurnedOff(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	defer RegisterBundle(Bundle{Manifest: Manifest{Name: "bundle-test", Command: []string{"rush"}, UI: []string{UINotify}},
		Run: func(io.ReadWriteCloser) error { return nil }})()
	if _, ok := Enabled()["bundle-test"]; !ok || !BundledOn("bundle-test") {
		t.Fatal("a bundled plugin should be on without approval")
	}
	p, err := Verify("bundle-test")
	if err != nil || !p.Bundled || p.Dir != "" {
		t.Fatalf("verify: %+v %v", p, err)
	}
	if err := SetBundled("bundle-test", false); err != nil {
		t.Fatal(err)
	}
	if _, ok := Enabled()["bundle-test"]; ok || BundledOn("bundle-test") {
		t.Fatal("turned off, it shouldn't run")
	}
	if _, err := Verify("bundle-test"); err == nil {
		t.Fatal("a bundled plugin turned off shouldn't verify")
	}
	if err := SetBundled("bundle-test", true); err != nil || !BundledOn("bundle-test") {
		t.Fatalf("on again: %v", err)
	}
	if err := SetBundled("not-bundled", false); err == nil {
		t.Fatal("only a bundled plugin can be turned off")
	}
}

// A plugin's entitlements gate whether it ever runs, bundled or installed:
// Enabled() leaves out one this system doesn't meet.
func TestEnabledSkipsUnmetRequirements(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	defer RegisterBundle(Bundle{Manifest: Manifest{Name: "bundle-never", Command: []string{"rush"},
		Requires: Requires{OS: []string{"never-an-os"}}}, Run: func(io.ReadWriteCloser) error { return nil }})()
	if _, ok := Enabled()["bundle-never"]; ok {
		t.Fatal("a bundled plugin whose OS isn't this one should never be enabled")
	}

	dir := install(t, Manifest{Name: "p", Command: []string{"bin"}, Requires: Requires{Bin: []string{"never-a-real-binary-xyz"}}}, nil)
	p, err := Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := Approve(p); err != nil {
		t.Fatal(err)
	}
	if _, ok := Enabled()["p"]; ok {
		t.Fatal("an approved plugin whose binary is missing should never be enabled")
	}
}

func TestBundledManifestIsChecked(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Fatal("a bundled plugin asking for the network should be refused")
		}
	}()
	RegisterBundle(Bundle{Manifest: Manifest{Name: "bundle-net", Command: []string{"rush"}, Network: []string{"example.com:443"}}})
}

// An Optional bundle is off until turned on, and on, it still doesn't run
// without what it requires.
func TestOptionalBundle(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	defer RegisterBundle(Bundle{Optional: true, Manifest: Manifest{Name: "bundle-opt", Command: []string{"rush"}},
		Run: func(io.ReadWriteCloser) error { return nil }})()
	defer RegisterBundle(Bundle{Optional: true, Manifest: Manifest{Name: "bundle-needs", Command: []string{"rush"},
		Requires: Requires{Bin: []string{"never-a-real-binary-xyz"}}}, Run: func(io.ReadWriteCloser) error { return nil }})()
	if _, ok := Enabled()["bundle-opt"]; ok || BundledOn("bundle-opt") || BundlesOn()["bundle-opt"] {
		t.Fatal("an optional bundle should be off until turned on")
	}
	for _, n := range []string{"bundle-opt", "bundle-needs"} {
		if err := SetBundled(n, true); err != nil {
			t.Fatal(err)
		}
	}
	if _, ok := Enabled()["bundle-opt"]; !ok || !BundlesOn()["bundle-opt"] {
		t.Fatal("turned on, it should run")
	}
	if _, ok := Enabled()["bundle-needs"]; ok || !BundledOn("bundle-needs") {
		t.Fatal("on, but missing what it requires, it shouldn't run")
	}
	if err := SetBundled("bundle-opt", false); err != nil || BundledOn("bundle-opt") {
		t.Fatalf("off again: %v", err)
	}
}

func TestServiceNames(t *testing.T) {
	ok := Manifest{Name: "a", Command: []string{"rush"}, Provides: []string{"cleanup.report"}, Uses: []string{"x.y-z"}}
	if err := ok.validateServices(); err != nil {
		t.Fatal(err)
	}
	for _, bad := range []string{"cleanup", "Cleanup.Report", "a..b", ".a"} {
		m := Manifest{Name: "a", Provides: []string{bad}}
		if m.validateServices() == nil {
			t.Errorf("%q should be refused", bad)
		}
	}
	mcp := Manifest{Name: "a", Protocol: "mcp", Provides: []string{"a.b"}}
	if mcp.validateServices() == nil {
		t.Error("an MCP plugin can't provide")
	}
}
