package efficiency

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
)

// Status is how far a saver is set up.
type Status int

const (
	Off     Status = iota
	Partial        // found, but not doing anything yet: rtk's binary without its hook
	On
)

// Found is what detection found of a saver.
type Found struct {
	Status Status
	Parts  []string  // what was found, in words: "hook in settings.json"
	Wants  string    // what's missing when Partial
	Since  time.Time // when it was installed, when that's known
	Value  string    // a setting's value
}

// Env is a home's setup as detection needs it, read once per look.
type Env struct {
	Profile agent.Profile
	Setup
}

// LoadEnv reads a home's settings, plugins and MCP servers, through
// Agent's Source.
func LoadEnv(p agent.Profile) *Env {
	e := &Env{Profile: p}
	if src, ok := source(); ok {
		e.Setup = src.Setup(p)
	}
	if e.Settings == nil {
		e.Settings, _ = settingsfile.Load(os.DevNull)
	}
	if e.Enabled == nil {
		e.Enabled = map[string]bool{}
	}
	if e.Plugins == nil {
		e.Plugins = map[string]time.Time{}
	}
	if e.MCP == nil {
		e.MCP = map[string]bool{}
	}
	return e
}

// searchPath is where programs are looked for: the PATH, and where
// package managers put them, which a GUI-started rush's PATH may miss.
var searchPath = func() []string {
	home, _ := os.UserHomeDir()
	dirs := filepath.SplitList(os.Getenv("PATH"))
	return append(dirs, "/opt/homebrew/bin", "/usr/local/bin", filepath.Join(home, ".local", "bin"),
		filepath.Join(home, ".cargo", "bin"), filepath.Join(home, "go", "bin"))
}

// LookPath finds a program the way detection does.
func LookPath(name string) string {
	if p, err := exec.LookPath(name); err == nil {
		return p
	}
	for _, d := range searchPath() {
		p := filepath.Join(d, name)
		if st, err := os.Stat(p); err == nil && !st.IsDir() && st.Mode()&0o111 != 0 {
			return p
		}
	}
	return ""
}

// Detect looks for a saver in this account.
func (e *Env) Detect(s *Saver) Found {
	if st := s.Setting; st != nil {
		return e.detectSetting(st)
	}
	var f Found
	d := s.Detect
	bin, hook, plugin, mcp := false, false, false, false
	for _, b := range d.Bins {
		if p := LookPath(b); p != "" {
			bin = true
			f.Parts = append(f.Parts, b+" installed")
			if t := brewTime(p); !t.IsZero() {
				f.Since = t
			}
		}
	}
	for _, h := range d.Hooks {
		for _, c := range e.Hooks {
			if strings.Contains(c, h) && !hook {
				hook = true
				f.Parts = append(f.Parts, "hook in settings.json")
			}
		}
	}
	for _, id := range d.Plugins {
		if t, ok := e.Plugins[id]; ok {
			if e.Enabled[id] {
				plugin = true
				f.Parts = append(f.Parts, "plugin "+id+" on")
			} else {
				f.Parts = append(f.Parts, "plugin "+id+" off")
			}
			if !t.IsZero() {
				f.Since = t
			}
		}
	}
	for _, m := range d.MCP {
		for name := range e.MCP {
			if name == m || strings.Contains(name, m) {
				mcp = true
				f.Parts = append(f.Parts, "MCP server "+name)
			}
		}
		for id := range e.Enabled {
			if strings.Contains(id, m) && e.Enabled[id] && !plugin {
				mcp = true
				f.Parts = append(f.Parts, "plugin "+id)
			}
		}
	}
	file := false
	for _, p := range d.Files {
		if _, err := os.Stat(filepath.Join(e.Profile.Dir, p)); err == nil {
			file = true
			f.Parts = append(f.Parts, p)
		}
	}
	any := bin || hook || plugin || mcp || file
	switch {
	case !any:
		f.Status = Off
	case d.Need == "hook" && !hook:
		f.Status, f.Wants = Partial, "its hook isn't in settings.json"
	case d.Need == "mcp" && !mcp:
		f.Status, f.Wants = Partial, "it isn't added as an MCP server"
	case len(d.Plugins) > 0 && !plugin && !hook:
		f.Status, f.Wants = Partial, "the plugin is off"
	default:
		f.Status = On
	}
	return f
}

func (e *Env) detectSetting(st *Setting) Found {
	var f Found
	var raw jsontext.Value
	ok := false
	if st.Env {
		v, set := e.Settings.Env()[st.Key]
		ok, f.Value = set, v
	} else if e.Settings.Get(st.Key, &raw) {
		ok, f.Value = true, strings.Trim(string(raw), `"`)
	}
	if !ok || st.Exact && f.Value != fmt.Sprint(st.Value) {
		return f
	}
	f.Status = On
	f.Parts = []string{fmt.Sprintf("%s = %s", st.Key, f.Value)}
	return f
}

// brewTime is when Homebrew installed the program at p, from its receipt.
func brewTime(p string) time.Time {
	real, err := filepath.EvalSymlinks(p)
	if err != nil || !strings.Contains(real, "/Cellar/") {
		return time.Time{}
	}
	// …/Cellar/<name>/<version>/bin/<name>
	dir := filepath.Dir(filepath.Dir(real))
	b, err := os.ReadFile(filepath.Join(dir, "INSTALL_RECEIPT.json"))
	if err != nil {
		return time.Time{}
	}
	var r struct {
		Time int64 `json:"time"`
	}
	if jsonx.Unmarshal(b, &r) != nil || r.Time == 0 {
		return time.Time{}
	}
	return time.Unix(r.Time, 0)
}

// Pick is the recipe to use: the first whose program is found.
func Pick(rs []Recipe) (*Recipe, string) {
	var missing []string
	for i := range rs {
		if rs[i].Needs == "" || LookPath(rs[i].Needs) != "" {
			return &rs[i], ""
		}
		missing = append(missing, rs[i].Needs)
	}
	if len(missing) == 0 {
		return nil, "nothing to run"
	}
	return nil, "needs " + strings.Join(missing, " or ")
}

// Plan is the commands a recipe runs here, steps already done left out.
func (r *Recipe) Plan() [][]string {
	var out [][]string
	for _, s := range r.Steps {
		if s.Skip != "" && LookPath(s.Skip) != "" {
			continue
		}
		out = append(out, s.Argv)
	}
	return out
}
