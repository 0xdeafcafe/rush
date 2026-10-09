package plugin

import (
	"os"
	"path/filepath"
	"testing"
)

func TestInProjectsOffersInstallsAndSkips(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	repo := t.TempDir()
	dir := filepath.Join(repo, ".rush", "plugins", "demo")
	os.MkdirAll(dir, 0o700)
	os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"demo","version":"1","command":["demo"]}`), 0o600)
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("no install.sh, no offer: %+v", got)
	}
	// install.sh builds the plugin where rush says, as haven's does.
	os.WriteFile(filepath.Join(dir, "install.sh"), []byte(`set -eu
mkdir -p "$RUSH_PLUGIN_DIR"
printf '#!/bin/sh\n' > "$RUSH_PLUGIN_DIR/demo"
chmod +x "$RUSH_PLUGIN_DIR/demo"
cp plugin.json "$RUSH_PLUGIN_DIR/"
`), 0o700)
	got := InProjects([]string{repo, repo})
	if len(got) != 1 || got[0].Name != "demo" || got[0].Update || got[0].Project != repo {
		t.Fatalf("one offer, once: %+v", got)
	}
	if err := Install(got[0]); err != nil {
		t.Fatal(err)
	}
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("installed at that version: %+v", got)
	}
	if len(Pending()) != 1 {
		t.Fatal("what it built waits on approval")
	}
	os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"demo","version":"2","command":["demo"]}`), 0o600)
	got = InProjects([]string{repo})
	if len(got) != 1 || !got[0].Update {
		t.Fatalf("a new version is an update: %+v", got)
	}
	if err := Skip(got[0]); err != nil {
		t.Fatal(err)
	}
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("not now at version 2: %+v", got)
	}
}
