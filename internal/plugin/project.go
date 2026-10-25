package plugin

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/uithread"

	"github.com/0xdeafcafe/rush/internal/state"
)

// --- plugins a project ships ---

// Offer is a plugin a repository ships in .rush/plugins/<name>, with an
// install.sh that builds it into the folder RUSH_PLUGIN_DIR names.
type Offer struct {
	Name        string
	Description string
	Version     string
	Dir         string // its folder in the repository
	Project     string // the repository
	Update      bool   // another version of it is installed
	BuiltFrom   []string

	// For an update from the commit installed: that commit, the
	// repository's now, and how many commits between changed the plugin.
	Was, Commit string
	Commits     int

	mark string // what "not now" holds at: the sources' hash, else the version
}

func offersSkippedPath() string { return filepath.Join(Root(), "offers-skipped.json") }

// InProjects lists what the repositories at roots ship that isn't
// installed, or is newer than the commit installed (by version, for one
// installed before rush kept the commit), and wasn't told "not now" at it.
func InProjects(roots []string) []Offer {
	uithread.Forbid("plugin.InProjects")
	installed := map[string]Plugin{}
	ps, _ := Installed()
	for _, p := range ps {
		installed[p.Name] = p
	}
	skipped := map[string]string{}
	if b, err := os.ReadFile(offersSkippedPath()); err == nil {
		_ = jsonx.Unmarshal(b, &skipped)
	}
	seen := map[string]bool{}
	var out []Offer
	for _, root := range roots {
		found, _ := filepath.Glob(filepath.Join(root, ".rush", "plugins", "*", "plugin.json"))
		for _, f := range found {
			dir := filepath.Dir(f)
			var m Manifest
			b, err := os.ReadFile(f)
			if err != nil || jsonx.Unmarshal(b, &m) != nil || m.Name != filepath.Base(dir) || !nameRE().MatchString(m.Name) || seen[m.Name] {
				continue
			}
			if _, err := os.Stat(filepath.Join(dir, "install.sh")); err != nil {
				continue
			}
			if _, ok := BundleNamed(m.Name); ok {
				continue
			}
			// A repository's words, drawn on rush's screen: what a terminal would act on goes.
			o := Offer{Name: m.Name, Description: clip(printable(m.Description), 300), Version: clip(printable(m.Version), 20), Dir: dir, Project: root, BuiltFrom: m.BuiltFrom, mark: m.Version}
			if p, has := installed[m.Name]; has {
				switch {
				case p.Origin.Dev:
					continue // development mode keeps it as the files are
				case p.Origin.Tree != "":
					// Kept at the commit it was installed at, until a newer one changes it.
					head, n, ok := p.Origin.newer(root, sources(root, dir, m.BuiltFrom))
					if !ok {
						continue
					}
					o.Was, o.Commit, o.Commits = p.Origin.Commit, head, n
					o.mark = tree(root, "HEAD", sources(root, dir, m.BuiltFrom))
				case p.Version == m.Version:
					continue
				}
				o.Update = true
			}
			if s, ok := skipped[m.Name]; ok && s == o.mark {
				continue
			}
			seen[m.Name] = true
			out = append(out, o)
		}
	}
	return out
}

// InstallScript is what Install runs, to show before it does.
func (o Offer) InstallScript() string { return filepath.Join(o.Dir, "install.sh") }

// Install runs the offer's install.sh, as you and outside the sandbox,
// into Root()/<name>. What it built still waits on approval (Pending).
func Install(o Offer) error {
	uithread.Forbid("plugin.Install")
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, "/bin/sh", o.InstallScript())
	cmd.Dir = o.Dir
	cmd.Env = append(os.Environ(), "RUSH_PLUGIN_DIR="+filepath.Join(Root(), o.Name), "RUSH_HOME="+state.Dir())
	out, err := cmd.CombinedOutput()
	if err != nil {
		lines := strings.Split(strings.TrimSpace(string(out)), "\n")
		return fmt.Errorf("%s: %w: %s", filepath.Base(o.InstallScript()), err, strings.Join(lines[max(0, len(lines)-3):], " · "))
	}
	return recordOrigin(o)
}

// Skip remembers "not now" for the offer as it is: a newer commit of it,
// or a new version, is offered again.
func Skip(o Offer) error {
	uithread.Forbid("plugin.Skip")
	m := map[string]string{}
	if b, err := os.ReadFile(offersSkippedPath()); err == nil {
		_ = jsonx.Unmarshal(b, &m)
	}
	m[o.Name] = o.mark
	if err := os.MkdirAll(Root(), 0o700); err != nil {
		return err
	}
	b, err := jsonx.MarshalIndent(m)
	if err != nil {
		return err
	}
	return os.WriteFile(offersSkippedPath(), b, 0o600)
}
