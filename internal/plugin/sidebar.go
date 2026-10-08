package plugin

import (
	"bytes"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A plugin with the sidebar capability may arrange rush's agent list: its
// own sections, and a name for each agent it knows. It only labels agents
// rush already shows, so it grants the plugin nothing it could not see.
// The broker checks what it sends and writes it to SidebarPath, a file the
// plugin can't reach; the UI reads it from there.

// Limits on a sidebar.
const (
	MaxSidebarSections = 32
	MaxSidebarAgents   = 2000
	maxSidebarTitle    = 64
	maxSidebarName     = 200
)

// Sidebar is how a plugin arranges the agent list.
type Sidebar struct {
	Plugin    string                  `json:"plugin"`
	Title     string                  `json:"title"`
	Sections  []SidebarSection        `json:"sections"`
	Agents    map[string]SidebarAgent `json:"agents"` // by Claude Code session id
	UpdatedAt time.Time               `json:"updatedAt"`
}

// SidebarSection is one section of the list, in the order given.
type SidebarSection struct {
	Title string `json:"title"`
}

// SidebarAgent places one agent, known by its Claude Code session id.
type SidebarAgent struct {
	Name    string `json:"name,omitempty"`
	Section string `json:"section"`
	Order   int    `json:"order"`
}

// Empty says the sidebar arranges nothing.
func (s Sidebar) Empty() bool { return len(s.Sections) == 0 }

// Label is what the list's group-by mode is called: the title, or the
// plugin's name.
func (s Sidebar) Label() string {
	if s.Title != "" {
		return s.Title
	}
	return s.Plugin
}

// sidebarIDRE is a Claude Code session id, or anything as safe.
var sidebarIDRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`)

// ParseSidebar reads and checks what a plugin sent with sidebar.set,
// cleaning every string of what a terminal would act on. Empty params, or
// no sections, clear it.
func ParseSidebar(plugin string, params jsontext.Value) (Sidebar, error) {
	s := Sidebar{Plugin: plugin}
	if p := bytes.TrimSpace(params); len(p) > 0 && !bytes.Equal(p, []byte("null")) {
		var in struct {
			Title    string                  `json:"title"`
			Sections []SidebarSection        `json:"sections"`
			Agents   map[string]SidebarAgent `json:"agents"`
		}
		if err := jsonx.Unmarshal(p, &in, jsonx.RejectUnknown); err != nil {
			return Sidebar{}, err
		}
		s.Title, s.Sections, s.Agents = in.Title, in.Sections, in.Agents
	}
	if err := s.check(); err != nil {
		return Sidebar{}, err
	}
	return s, nil
}

// check validates and cleans s in place.
func (s *Sidebar) check() error {
	if len(s.Sections) == 0 {
		s.Title, s.Sections, s.Agents = "", nil, nil
		return nil
	}
	var err error
	if s.Title, err = cleanLabel("title", s.Title, maxSidebarTitle); err != nil {
		return err
	}
	if len(s.Sections) > MaxSidebarSections {
		return fmt.Errorf("%d sections: at most %d", len(s.Sections), MaxSidebarSections)
	}
	seen := map[string]bool{}
	for i := range s.Sections {
		t, err := cleanLabel("section title", s.Sections[i].Title, maxSidebarTitle)
		if err != nil {
			return err
		}
		if t == "" {
			return errors.New("a section has no title")
		}
		if seen[t] {
			return fmt.Errorf("section %q appears twice", t)
		}
		seen[t] = true
		s.Sections[i].Title = t
	}
	if len(s.Agents) > MaxSidebarAgents {
		return fmt.Errorf("%d agents: at most %d", len(s.Agents), MaxSidebarAgents)
	}
	for id, a := range s.Agents {
		if !sidebarIDRE.MatchString(id) {
			return fmt.Errorf("agent id %q is not a session id", id)
		}
		if a.Name, err = cleanLabel("agent name", a.Name, maxSidebarName); err != nil {
			return err
		}
		if a.Section, err = cleanLabel("section", a.Section, maxSidebarTitle); err != nil {
			return err
		}
		if !seen[a.Section] {
			return fmt.Errorf("agent %s is in section %q, which isn't in sections", id, a.Section)
		}
		s.Agents[id] = a
	}
	return nil
}

// cleanLabel checks a string's length and makes it one plain line.
func cleanLabel(what, s string, max int) (string, error) {
	if !utf8.ValidString(s) {
		return "", fmt.Errorf("%s is not UTF-8", what)
	}
	s = Clean(s)
	if utf8.RuneCountInString(s) > max {
		return "", fmt.Errorf("%s %q is over %d characters", what, s, max)
	}
	return s, nil
}

// Clean makes s one line of plain text: escape sequences go, and so do
// control and direction-changing characters, with line breaks and tabs
// turned into spaces.
func Clean(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); {
		r, n := utf8.DecodeRuneInString(s[i:])
		if r == 0x1b {
			i += escLen(s[i:])
			continue
		}
		i += n
		switch {
		case r == '\n' || r == '\t' || r == '\r':
			b.WriteByte(' ')
		case unicode.IsControl(r), r == utf8.RuneError, unicode.Is(unicode.Bidi_Control, r):
		default:
			b.WriteRune(r)
		}
	}
	return strings.Join(strings.Fields(b.String()), " ")
}

// escLen is the length of the escape sequence s starts with: CSI up to its
// final byte, OSC, DCS and the like up to BEL or ST, else ESC and one more.
func escLen(s string) int {
	if len(s) < 2 {
		return len(s)
	}
	switch s[1] {
	case '[':
		for i := 2; i < len(s); i++ {
			if s[i] >= 0x40 && s[i] <= 0x7e {
				return i + 1
			}
		}
		return len(s)
	case ']', 'P', '_', '^', 'X':
		for i := 2; i < len(s); i++ {
			if s[i] == 0x07 {
				return i + 1
			}
			if s[i] == 0x1b && i+1 < len(s) && s[i+1] == '\\' {
				return i + 2
			}
		}
		return len(s)
	}
	return 2
}

// SidebarRoot holds the sidebars plugins set, one file each. It is apart
// from the plugins' own folders and data folders, so no plugin writes it.
func SidebarRoot() string { return filepath.Join(filepath.Dir(Root()), "plugin-sidebar") }

// SidebarPath is where a plugin's sidebar is kept.
func SidebarPath(name string) string { return filepath.Join(SidebarRoot(), name+".json") }

// SaveSidebar keeps a plugin's sidebar, or removes it when it's empty.
func SaveSidebar(s Sidebar) error {
	if !nameRE.MatchString(s.Plugin) {
		return fmt.Errorf("plugin name %q", s.Plugin)
	}
	if s.Empty() {
		return RemoveSidebar(s.Plugin)
	}
	if s.UpdatedAt.IsZero() {
		s.UpdatedAt = time.Now()
	}
	if err := os.MkdirAll(SidebarRoot(), 0o700); err != nil {
		return err
	}
	b, err := jsonx.Marshal(s)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(SidebarRoot(), "."+s.Plugin+"-*.tmp")
	if err != nil {
		return err
	}
	_, werr := f.Write(b)
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr == nil {
		werr = os.Chmod(f.Name(), 0o600)
	}
	if werr == nil {
		werr = os.Rename(f.Name(), SidebarPath(s.Plugin))
	}
	if werr != nil {
		_ = os.Remove(f.Name())
	}
	return werr
}

// RemoveSidebar removes a plugin's sidebar, if it has one.
func RemoveSidebar(name string) error {
	if !nameRE.MatchString(name) {
		return nil
	}
	if err := os.Remove(SidebarPath(name)); err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

// PruneSidebars removes the sidebars of plugins that aren't approved, or
// were approved without the capability.
func PruneSidebars(approved map[string]Approval) {
	ents, _ := os.ReadDir(SidebarRoot())
	for _, e := range ents {
		name, ok := strings.CutSuffix(e.Name(), ".json")
		if !ok || strings.HasPrefix(e.Name(), ".") {
			continue
		}
		if a, ok := approved[name]; !ok || !a.Manifest.Sidebar {
			_ = RemoveSidebar(name)
		}
	}
}

// Sidebars reads the sidebars of approved plugins with the capability,
// cheaply enough to call every second: a file is read again only when its
// modification time or size changes. One that doesn't parse is left out.
type Sidebars struct {
	approvedMod time.Time
	approved    map[string]Approval
	files       map[string]sidebarFile
}

type sidebarFile struct {
	mod  time.Time
	size int64
	s    Sidebar
	ok   bool
}

// Load is every sidebar in use, by plugin name order.
func (w *Sidebars) Load() []Sidebar {
	if fi, err := os.Stat(approvedPath()); err != nil {
		w.approved, w.approvedMod = nil, time.Time{}
	} else if !fi.ModTime().Equal(w.approvedMod) || w.approved == nil {
		w.approved, w.approvedMod = Approvals(), fi.ModTime()
	}
	if w.files == nil {
		w.files = map[string]sidebarFile{}
	}
	var out []Sidebar
	for _, name := range slices.Sorted(maps.Keys(w.approved)) {
		if !w.approved[name].Manifest.Sidebar {
			delete(w.files, name)
			continue
		}
		fi, err := os.Stat(SidebarPath(name))
		if err != nil {
			delete(w.files, name)
			continue
		}
		f := w.files[name]
		if !fi.ModTime().Equal(f.mod) || fi.Size() != f.size {
			f = sidebarFile{mod: fi.ModTime(), size: fi.Size()}
			f.s, f.ok = readSidebar(name)
			w.files[name] = f
		}
		if f.ok && !f.s.Empty() {
			out = append(out, f.s)
		}
	}
	return out
}

func readSidebar(name string) (Sidebar, bool) {
	b, err := os.ReadFile(SidebarPath(name))
	if err != nil || len(b) > 8<<20 {
		return Sidebar{}, false
	}
	var s Sidebar
	if jsonx.Unmarshal(b, &s) != nil || s.Plugin != name || s.check() != nil {
		return Sidebar{}, false
	}
	return s, true
}
