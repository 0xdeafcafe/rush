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

// gitRepo makes a repository shipping the plugin demo, built from its
// folder and lib/, committed.
func gitRepo(t *testing.T) (repo, dir string, run func(...string) string) {
	t.Helper()
	repo = t.TempDir()
	dir = filepath.Join(repo, ".rush", "plugins", "demo")
	os.MkdirAll(dir, 0o700)
	os.MkdirAll(filepath.Join(repo, "lib"), 0o700)
	os.WriteFile(filepath.Join(repo, "lib", "code"), []byte("1"), 0o600)
	os.WriteFile(filepath.Join(repo, "README"), []byte("x"), 0o600)
	os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"demo","version":"1","command":["demo"],"builtFrom":["../../../lib"]}`), 0o600)
	os.WriteFile(filepath.Join(dir, "install.sh"), []byte(`set -eu
mkdir -p "$RUSH_PLUGIN_DIR"
printf '#!/bin/sh\n' > "$RUSH_PLUGIN_DIR/demo"
cat ../../../lib/code >> "$RUSH_PLUGIN_DIR/demo"
chmod +x "$RUSH_PLUGIN_DIR/demo"
cp plugin.json "$RUSH_PLUGIN_DIR/"
`), 0o700)
	run = func(args ...string) string {
		t.Helper()
		out, err := exec.Command("git", append([]string{"-C", repo, "-c", "user.name=t", "-c", "user.email=t@t"}, args...)...).CombinedOutput()
		if err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
		return strings.TrimSpace(string(out))
	}
	run("init", "-q", "-b", "main")
	run("add", ".")
	run("commit", "-qm", "one")
	return repo, dir, run
}

func TestLockedUntilANewerCommitChangesIt(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	repo, _, run := gitRepo(t)
	if err := Install(InProjects([]string{repo})[0]); err != nil {
		t.Fatal(err)
	}
	if o := Origins()["demo"]; o.Tree == "" || len(o.Sources) != 2 || o.Sources[1] != "lib" {
		t.Fatalf("origin: %+v", o)
	}
	// A commit that doesn't touch the plugin's sources isn't an update.
	os.WriteFile(filepath.Join(repo, "README"), []byte("y"), 0o600)
	run("commit", "-qam", "readme")
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("its files didn't change: %+v", got)
	}
	// Nor is an edit not yet committed.
	os.WriteFile(filepath.Join(repo, "lib", "code"), []byte("2"), 0o600)
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("not committed: %+v", got)
	}
	// A commit to what it's built from is, with the version unchanged.
	run("commit", "-qam", "two")
	got := InProjects([]string{repo})
	if len(got) != 1 || !got[0].Update || got[0].Was != Origins()["demo"].Commit || got[0].Commits != 1 {
		t.Fatalf("an update: %+v", got)
	}
	// A checkout behind the commit installed doesn't offer to go back.
	old := run("rev-parse", "HEAD~1")
	if err := Install(got[0]); err != nil {
		t.Fatal(err)
	}
	run("checkout", "-q", old)
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("behind what's installed: %+v", got)
	}
	run("checkout", "-q", "main")
	// "Not now" holds until a newer commit changes it again.
	os.WriteFile(filepath.Join(repo, "lib", "code"), []byte("3"), 0o600)
	run("commit", "-qam", "three")
	got = InProjects([]string{repo})
	if len(got) != 1 {
		t.Fatalf("three: %+v", got)
	}
	Skip(got[0])
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("not now: %+v", got)
	}
	os.WriteFile(filepath.Join(repo, "lib", "code"), []byte("4"), 0o600)
	run("commit", "-qam", "four")
	if got := InProjects([]string{repo}); len(got) != 1 || got[0].Commits != 2 {
		t.Fatalf("four, two commits on from the one installed: %+v", got)
	}
}

func TestDevelopmentModeRebuildsAsFilesChange(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	repo, dir, _ := gitRepo(t)
	if err := SetDev("demo", true); err == nil {
		t.Fatal("not installed from a repository yet")
	}
	if err := Install(InProjects([]string{repo})[0]); err != nil {
		t.Fatal(err)
	}
	p, _ := Load(filepath.Join(Root(), "demo"))
	if err := Approve(p); err != nil {
		t.Fatal(err)
	}
	if len(DevChanged()) != 0 {
		t.Fatal("not in development mode")
	}
	if err := SetDev("demo", true); err != nil {
		t.Fatal(err)
	}
	if _, ok := DevChanged()["demo"]; !ok {
		t.Fatal("turning it on builds it from the files as they are")
	}
	if ok, err := Rebuild("demo"); err != nil || !ok {
		t.Fatalf("rebuilt and approved: %v %v", ok, err)
	}
	if len(DevChanged()) != 0 {
		t.Fatal("built as the files are")
	}
	if got := InProjects([]string{repo}); len(got) != 0 {
		t.Fatalf("development mode isn't offered updates: %+v", got)
	}
	// An edit, not committed, is built and runs without asking.
	os.WriteFile(filepath.Join(repo, "lib", "code"), []byte("edited"), 0o600)
	if _, ok := DevChanged()["demo"]; !ok {
		t.Fatal("a file changed")
	}
	if ok, err := Rebuild("demo"); err != nil || !ok {
		t.Fatalf("rebuilt: %v %v", ok, err)
	}
	if b, _ := os.ReadFile(filepath.Join(Root(), "demo", "demo")); !strings.Contains(string(b), "edited") {
		t.Fatalf("built from the edit: %q", b)
	}
	if len(Pending()) != 0 {
		t.Fatal("approved as it asks for no more")
	}
	p, _ = Load(filepath.Join(Root(), "demo"))
	if !strings.Contains(p.From(), "development mode") {
		t.Fatalf("From: %q", p.From())
	}
	// One that asks for more waits on you.
	os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"demo","version":"1","command":["demo"],"builtFrom":["../../../lib"],"network":["example.com:443"]}`), 0o600)
	if ok, err := Rebuild("demo"); err != nil || ok {
		t.Fatalf("asks for more: %v %v", ok, err)
	}
	if len(Pending()) != 1 {
		t.Fatal("waits on approval")
	}
	if err := SetDev("demo", false); err != nil {
		t.Fatal(err)
	}
	if _, err := Rebuild("demo"); err == nil {
		t.Fatal("off: no rebuilding")
	}
}
