package plugin

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
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

func TestInstallRecordsWhereItCameFrom(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	repo := t.TempDir()
	dir := filepath.Join(repo, ".rush", "plugins", "demo")
	os.MkdirAll(dir, 0o700)
	os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"demo","version":"1","command":["demo"]}`), 0o600)
	os.WriteFile(filepath.Join(dir, "install.sh"), []byte(`set -eu
mkdir -p "$RUSH_PLUGIN_DIR"
printf '#!/bin/sh\n' > "$RUSH_PLUGIN_DIR/demo"
chmod +x "$RUSH_PLUGIN_DIR/demo"
cp plugin.json "$RUSH_PLUGIN_DIR/"
`), 0o700)
	for _, args := range [][]string{
		{"init", "-q"},
		{"remote", "add", "origin", "https://someone:secret@github.com/acme/haven.git"},
		{"add", "."},
		{"-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"},
	} {
		if out, err := exec.Command("git", append([]string{"-C", repo}, args...)...).CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	if err := Install(InProjects([]string{repo})[0]); err != nil {
		t.Fatal(err)
	}
	p, err := Load(filepath.Join(Root(), "demo"))
	if err != nil {
		t.Fatal(err)
	}
	o := p.Origin
	if o.Project != repo || o.Remote != "github.com/acme/haven" || len(o.Commit) != 40 || o.Uncommitted || o.Version != "1" {
		t.Fatalf("origin: %+v", o)
	}
	if f := p.From(); !strings.Contains(f, "github.com/acme/haven") || strings.Contains(f, "secret") || !strings.Contains(f, o.Commit[:7]) {
		t.Fatalf("From: %q", f)
	}
	d, _ := Digest(p.Dir)
	if o.ChangedSinceInstall(d) {
		t.Fatal("not changed yet")
	}
	os.WriteFile(filepath.Join(p.Dir, "extra"), []byte("x"), 0o600)
	if d, _ := Digest(p.Dir); !o.ChangedSinceInstall(d) {
		t.Fatal("changed since install")
	}

	byHand := filepath.Join(Root(), "other")
	os.MkdirAll(byHand, 0o700)
	if f := (Plugin{Dir: byHand, Origin: originOf("other", byHand)}).From(); !strings.Contains(f, "by hand") {
		t.Fatalf("by hand: %q", f)
	}
	link := filepath.Join(Root(), "linked")
	os.Symlink(dir, link)
	if f := (Plugin{Dir: link, Origin: originOf("linked", link)}).From(); !strings.HasPrefix(f, "Linked to") {
		t.Fatalf("link: %q", f)
	}
}

func TestCleanRemote(t *testing.T) {
	for in, want := range map[string]string{
		"git@github.com:acme/haven.git":       "github.com/acme/haven",
		"https://tok@github.com/acme/haven":   "github.com/acme/haven",
		"ssh://git@gitlab.com/acme/haven.git": "gitlab.com/acme/haven",
		"":                                    "",
	} {
		if got := cleanRemote(in); got != want {
			t.Errorf("cleanRemote(%q) = %q, want %q", in, got, want)
		}
	}
}
