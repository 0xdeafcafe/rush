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
}

func offersSkippedPath() string { return filepath.Join(Root(), "offers-skipped.json") }

// InProjects lists what the repositories at roots ship that isn't
// installed at that version, and wasn't told "not now" at it.
func InProjects(roots []string) []Offer {
	uithread.Forbid("plugin.InProjects")
	installed := map[string]string{}
	ps, _ := Installed()
	for _, p := range ps {
		installed[p.Name] = p.Version
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
			if err != nil || jsonx.Unmarshal(b, &m) != nil || m.Name != filepath.Base(dir) || !nameRE.MatchString(m.Name) || seen[m.Name] {
				continue
			}
			if _, err := os.Stat(filepath.Join(dir, "install.sh")); err != nil {
				continue
			}
			if _, ok := BundleNamed(m.Name); ok {
				continue
			}
			v, has := installed[m.Name]
			if has && v == m.Version {
				continue
			}
			if s, ok := skipped[m.Name]; ok && s == m.Version {
				continue
			}
			seen[m.Name] = true
			// A repository's words, drawn on rush's screen: what a terminal would act on goes.
			out = append(out, Offer{Name: m.Name, Description: clip(printable(m.Description), 300), Version: clip(printable(m.Version), 20), Dir: dir, Project: root, Update: has})
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
	return nil
}

// Skip remembers "not now" for the offer at its version: a new version
// is offered again.
func Skip(o Offer) error {
	uithread.Forbid("plugin.Skip")
	m := map[string]string{}
	if b, err := os.ReadFile(offersSkippedPath()); err == nil {
		_ = jsonx.Unmarshal(b, &m)
	}
	m[o.Name] = o.Version
	if err := os.MkdirAll(Root(), 0o700); err != nil {
		return err
	}
	b, err := jsonx.MarshalIndent(m)
	if err != nil {
		return err
	}
	return os.WriteFile(offersSkippedPath(), b, 0o600)
}
