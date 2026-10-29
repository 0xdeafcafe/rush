package ui

import (
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
	"github.com/0xdeafcafe/rush/internal/state"
)

// What an agent itself reads, on its page: the keys of its settings file
// its adapter offers (defaults, permissions, hooks), its env block (the
// variables every session starts with, subagents included), and the
// definitions its sessions can start as.

// agentProfile is the profile k's page is about: the one its sessions run
// in.
func (m *Model) agentProfile(k agent.Kind) (agent.Profile, bool) {
	ad, ok := agent.Get(k)
	if !ok {
		return agent.Profile{}, false
	}
	return m.profileOf(ad)
}

// agentDefs are the definitions k's sessions can start as, from its own
// folder and every repository an agent has worked in. They're read off
// the UI: none until they're in.
func (m *Model) agentDefs(k agent.Kind) []agent.AgentDef {
	key, ok := m.defsKey(k)
	if !ok {
		return nil
	}
	defs, _ := agentDefsRead.get(key)
	return defs
}

// defsAt is what agentDefs are read for: the agent, its profile, and the
// folders looked in, joined.
type defsAt struct {
	kind  agent.Kind
	prof  agent.Profile
	roots string
}

func (m *Model) defsKey(k agent.Kind) (defsAt, bool) {
	if _, ok := agent.As[agent.Definer](k); !ok {
		return defsAt{}, false
	}
	p, _ := m.agentProfile(k)
	roots := make([]string, 0, len(m.snap.Agents)+1)
	roots = append(roots, m.launchDir)
	for _, a := range m.snap.Agents {
		roots = append(roots, a.Repo)
	}
	return defsAt{k, p, strings.Join(roots, "\x00")}, true
}

var agentDefsRead = newMemo(0, func(k defsAt) []agent.AgentDef {
	def, _ := agent.As[agent.Definer](k.kind)
	return def.AgentDefs(k.prof, strings.Split(k.roots, "\x00"))
})

// defsChanged has k's definitions read again.
func (m *Model) defsChanged(k agent.Kind) {
	if key, ok := m.defsKey(k); ok {
		agentDefsRead.stale(key)
	}
}

// definitionsSection lists the definitions k's new sessions can start as,
// the one they do marked; none if k has none.
func (m *Model) definitionsSection(k agent.Kind) (section, bool) {
	def, ok := agent.As[agent.Definer](k)
	if !ok {
		return section{}, false
	}
	sec := section{title: "Agent definitions", advanced: true, note: "what new sessions start as (--agent)"}
	newAgent := func() {
		m.ask("new agent name", "", func(v string) tea.Cmd {
			name := strings.ToLower(strings.Join(strings.Fields(v), "-"))
			p, _ := m.agentProfile(k)
			return sheetDo(func() (string, error) { return def.NewAgentDef(p, name) }, func(m *Model, path string, err error) tea.Cmd {
				if err != nil {
					m.fail("couldn't make " + name + ": " + err.Error())
					return nil
				}
				m.defsChanged(k)
				return editFile(path)
			})
		})
	}
	for _, a := range sameDefsOnce(m.agentDefs(k)) {
		sec.rows = append(sec.rows, m.definitionRow(k, a, newAgent))
	}
	sec.title = fmt.Sprintf("Agent definitions (%d)", len(sec.rows))
	return sec, true
}

// sameDefsOnce lists a definition once however many places have it: each
// worktree of a repository has the same files, and would list them all
// again. The first place stays its own, the rest are counted.
func sameDefsOnce(defs []agent.AgentDef) []agent.AgentDef {
	type key struct{ name, model, desc string }
	at := map[key]int{}
	more := map[int]int{}
	var out []agent.AgentDef
	for _, a := range defs {
		k := key{a.Name, a.Model, a.Desc}
		if i, ok := at[k]; ok {
			more[i]++
			continue
		}
		at[k] = len(out)
		out = append(out, a)
	}
	for i, n := range more {
		out[i].Scope = fmt.Sprintf("%s +%d", out[i].Scope, n)
	}
	return out
}

// definitionRow is one definition: enter starts new sessions as it.
func (m *Model) definitionRow(k agent.Kind, a agent.AgentDef, newAgent func()) setting {
	d := m.dialog
	cfg := &m.store.Config
	used := a.Name == cfg.Dispatch.Agent || cfg.Dispatch.Agent == "" && a.Path == ""
	return setting{
		label: a.Name,
		line: func(w int) string {
			mark := faint("○")
			if used {
				mark = paint(cOrange, "●")
			}
			model := ""
			if a.Model != "" && a.Model != "inherit" {
				model = " · " + modelWord(state.LoginsKind, a.Model)
			}
			return mark + " " + paint(cText, fit(a.Name, 22)) + faint(fit(a.Scope+model, 24)) + dim(fit(a.Desc, max(10, w-48)))
		},
		key: func(s string) (tea.Cmd, bool) {
			switch s {
			case "enter":
				name := a.Name
				if a.Path == "" {
					name = ""
				}
				cfg.Dispatch.Agent = name
				_ = m.store.SaveConfig()
				m.flash("new sessions start as "+a.Name, false)
			case "e":
				if a.Path != "" {
					return editFile(a.Path), true
				}
			case "n":
				newAgent()
			case "x", "d":
				if a.Path != "" {
					m.confirmThen("Delete the agent "+a.Name+" ("+tildify(a.Path)+")?", func() tea.Cmd {
						path := a.Path
						return sheetDo(func() (struct{}, error) { return struct{}{}, os.Remove(path) }, func(m *Model, _ struct{}, err error) tea.Cmd {
							if err != nil {
								m.fail("couldn't delete " + a.Name + ": " + err.Error())
								return nil
							}
							m.defsChanged(k)
							m.loadDialog()
							d.cursor = max(0, d.cursor-1)
							return nil
						})
					})
				}
			default:
				return nil, false
			}
			return nil, true
		},
		keys: []string{"enter", "start as it", "e", "edit", "n", "new", "d", "delete"},
		about: func() (string, string, string) {
			meta := a.Scope
			if a.Model != "" {
				meta += " · model " + a.Model
			}
			if a.Path != "" {
				meta += " · " + tildify(a.Path)
			}
			now := "enter makes new sessions start as it."
			if used {
				now = "New sessions start as it."
			}
			return a.Name, meta + ". " + a.Desc, now
		},
	}
}

// fileSections are what k itself reads, if its adapter says: its settings
// file, then its env block.
func (m *Model) fileSections(k agent.Kind) []section {
	pg, ok := agent.As[agent.SettingsPager](k)
	if !ok || !agent.Supports(k, agent.FeatureSettings) {
		return nil
	}
	s := m.agentSettings(k)
	if s == nil {
		return nil
	}
	page := pg.SettingsPage()
	secs := []section{m.settingsFileSection(page, s)}
	if page.EnvKey != "" {
		secs = append(secs, m.envSection(k, page, s))
	}
	return secs
}

// agentSettings is k's own settings file, read off the UI: nil until it's
// in. It's read again every few seconds while the page is open, and taken
// up when the file changed, by rush or anything else, so the page never
// shows what it no longer says.
func (m *Model) agentSettings(k agent.Kind) *settingsfile.File {
	d := m.dialog
	p, _ := m.agentProfile(k)
	files, _ := settingsFilesRead.get(filesAt{k, p})
	if len(files) == 0 {
		return nil
	}
	path := files[0].path
	r, ok := settingsRead.get(path)
	if !ok {
		if agentSettingsFor.d == d && agentSettingsFor.path == path {
			return d.settings
		}
		return nil
	}
	if d.settings != nil && agentSettingsFor.d == d && agentSettingsFor.path == path && r.mod.Equal(agentSettingsFor.mod) {
		return d.settings
	}
	if r.err != nil && (agentSettingsFor.d != d || agentSettingsFor.path != path) {
		m.fail("couldn't read " + tildify(path) + ": " + r.err.Error())
	}
	d.settings = r.file
	agentSettingsFor.d, agentSettingsFor.path, agentSettingsFor.mod = d, path, r.mod
	return d.settings
}

// agentSettingsFor is the dialog, path and file's time d.settings was read
// for.
var agentSettingsFor struct {
	d    *dialog
	path string
	mod  time.Time
}

type filesAt struct {
	kind agent.Kind
	prof agent.Profile
}

// settingsFilesRead is where each agent's settings files are: finding the
// project's looks for its repository on disk.
var settingsFilesRead = newMemo(0, func(k filesAt) []settingsFile { return settingsFiles(k.kind, k.prof, "") })

type settingsLoad struct {
	file *settingsfile.File
	err  error
	mod  time.Time // when the file was last written, as it was read
}

var settingsRead = newMemo(3*time.Second, func(path string) settingsLoad {
	var mod time.Time
	if st, err := os.Stat(path); err == nil {
		mod = st.ModTime()
	}
	s, err := settingsfile.Load(path)
	if err != nil {
		s, _ = settingsfile.Load(filepath.Join(filepath.Dir(path), ".rush-unreadable", filepath.Base(path)))
	}
	return settingsLoad{s, err, mod}
})

// saveSettings writes the changes made to the settings file off the UI,
// one save after another; the Cmd it hands back says how it went, and
// said when it worked. The file is written whether or not the Cmd runs.
func (m *Model) saveSettings(s *settingsfile.File, said string) tea.Cmd {
	d := s.Detach()
	prev := lastSettingsSave
	p := goPending(func() error {
		if prev != nil {
			<-prev.done
		}
		return d.Save()
	})
	lastSettingsSave = p
	return p.then(func(m *Model) tea.Cmd {
		err, _ := p.take()
		switch {
		case err != nil:
			m.fail("couldn't save " + filepath.Base(d.Path) + ": " + err.Error())
		case said != "":
			m.flash(said, false)
		}
		return nil
	})
}

// lastSettingsSave is the save last started, for the next to wait on.
var lastSettingsSave *pending[error]

// openSettingsKey is e on any settings file or env row.
func (m *Model) openSettingsKey(s string) (tea.Cmd, bool) {
	if s == "e" && m.dialog.settings != nil {
		return editFile(m.dialog.settings.Path), true
	}
	return nil, false
}

func (m *Model) settingsFileSection(page agent.SettingsPage, s *settingsfile.File) section {
	file := filepath.Base(s.Path)
	unsetNote := "not set: " + page.Unset + "."
	row := func(r agent.SettingRow, choices ...string) setting {
		return setting{label: r.Label, what: r.What + " (" + file + " " + r.Key + ")", choices: choices, unset: "not set",
			means: map[string]string{"": unsetNote}, key: m.openSettingsKey, keys: []string{"e", "open " + file}}
	}
	save := func(key string, v any) tea.Cmd {
		_ = s.Set(key, v)
		return m.saveSettings(s, "")
	}
	// Each row's change is run, for its Cmd to say if the save failed;
	// set does the same, for what calls it that way.
	both := func(st *setting, run func(string) tea.Cmd) {
		st.run = run
		st.set = func(v string) { _ = run(v) }
	}
	rows := make([]setting, 0, len(page.Rows))
	for _, r := range page.Rows {
		key := r.Key
		var st setting
		switch r.Type {
		case agent.SettingFlag, agent.SettingOff:
			invert := r.Type == agent.SettingOff
			st = row(r, "", "on", "off")
			var b bool
			if s.Get(key, &b) {
				st.value = onOffWord(b != invert)
			}
			both(&st, func(v string) tea.Cmd {
				if v == "" {
					return save(key, nil)
				}
				return save(key, (v == "on") != invert)
			})
		case agent.SettingInt:
			st = row(r, append([]string{""}, r.Choices...)...)
			var n int
			if s.Get(key, &n) {
				st.value = strconv.Itoa(n)
			}
			both(&st, func(v string) tea.Cmd {
				if n, err := strconv.Atoi(v); err == nil {
					return save(key, n)
				}
				return save(key, nil)
			})
		default:
			st = row(r, append([]string{""}, r.Choices...)...)
			st.value = s.String(key)
			both(&st, func(v string) tea.Cmd {
				if v == "" {
					return save(key, nil)
				}
				return save(key, v)
			})
		}
		rows = append(rows, st)
	}
	return section{title: file, advanced: true, note: tildify(s.Path) + " · " + page.Note, rows: rows}
}

func (m *Model) envSection(k agent.Kind, page agent.SettingsPage, s *settingsfile.File) section {
	name := agentName(string(k))
	file := filepath.Base(s.Path)
	env := map[string]string{}
	s.Get(page.EnvKey, &env)
	sec := section{title: "Environment", advanced: true, note: "every session on this account starts with these, subagents included"}
	setEnv := func(v, value string) tea.Cmd {
		var err error
		if value == "" {
			err = s.Set(page.EnvKey+"."+v, nil)
		} else {
			err = s.Set(page.EnvKey+"."+v, value)
		}
		if err != nil {
			m.fail("couldn't set " + v + ": " + err.Error())
			return nil
		}
		return m.saveSettings(s, "saved to "+tildify(s.Path))
	}
	known := map[string]bool{}
	for _, e := range page.Env {
		known[e.Name] = true
		means := map[string]string{"": "not set: " + page.Unset + "."}
		maps.Copy(means, e.Means)
		sec.rows = append(sec.rows, setting{label: e.Label, value: env[e.Name], choices: append([]string{""}, e.Choices...), unset: "not set",
			what: e.What + " (env " + e.Name + ").", means: means,
			set: func(v string) { setEnv(e.Name, v) }, run: func(v string) tea.Cmd { return setEnv(e.Name, v) },
			key: m.openSettingsKey, keys: []string{"e", "open " + file}})
	}
	var extra []string
	for v := range env {
		if !known[v] {
			extra = append(extra, v)
		}
	}
	sort.Strings(extra)
	for _, v := range extra {
		value := env[v]
		sec.rows = append(sec.rows, setting{label: v, value: value, typed: true,
			what: "An environment variable in this account's " + file + " env block. Every " + name + " session on the account starts with it.",
			set:  func(x string) { setEnv(v, x) },
			run:  func(x string) tea.Cmd { return setEnv(v, x) },
			key: func(k string) (tea.Cmd, bool) {
				switch k {
				case "x", "delete", "backspace":
					m.confirm = &confirmation{question: "Remove " + v + "?", detail: "from this account's " + file, onYes: func() tea.Cmd {
						m.flash("removed "+v, false)
						return setEnv(v, "")
					}}
					return nil, true
				}
				return m.openSettingsKey(k)
			},
			keys: []string{"x", "remove it", "e", "open " + file},
		})
	}
	addVar := func() {
		m.ask("NAME=value", "", func(in string) tea.Cmd {
			v, value, ok := strings.Cut(in, "=")
			v = strings.TrimSpace(v)
			if !ok || v == "" || strings.ContainsAny(v, " \t") {
				m.flash("type it as NAME=value", true)
				return nil
			}
			return setEnv(v, strings.TrimSpace(value))
		})
	}
	sec.rows = append(sec.rows, setting{label: "+ add a variable",
		line: func(int) string { return faint("+ add a variable") },
		key: func(k string) (tea.Cmd, bool) {
			if k == "enter" || k == "right" || k == "a" {
				addVar()
				return nil, true
			}
			return m.openSettingsKey(k)
		},
		keys: []string{"enter", "add NAME=value"},
		about: func() (string, string, string) {
			return "Add a variable", "Adds an environment variable to this account's " + file + " env block. Every " + name + " session on the account starts with it, subagents included.", "Press enter and type NAME=value."
		},
	})
	return sec
}
