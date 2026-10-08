package ui

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
)

// --- /permissions and /hooks ---

// settingsFile is one of the settings files a session reads.
type settingsFile struct {
	label, path string
}

// settingsFiles are the files a session of k's on p in cwd reads, in the
// order it reads them; none if k's settings aren't files rush edits.
func settingsFiles(k agent.Kind, p agent.Profile, cwd string) []settingsFile {
	sf, ok := agent.As[agent.SettingsFiler](k)
	if !ok {
		return nil
	}
	var files []settingsFile
	for _, f := range sf.SettingsFiles(p, cwd) {
		files = append(files, settingsFile{f.Label, f.Path})
	}
	return files
}

var ruleKinds = []string{"allow", "ask", "deny"}

// permRule is one permission rule and the file it's in.
type permRule struct {
	text string
	file settingsFile
}

// permSheet is /permissions: the allow, ask and deny rules from every
// settings file, each marked with its file; add one to any file, or take
// one out.
type permSheet struct {
	files  []settingsFile
	rules  [3][]permRule
	mode   string
	tab    int
	cur    [3]int
	adding bool
	input  []rune
	pos    int
	target int // which file a new rule goes to
	// The files are read, and changed, off the UI: read is what's out,
	// and loaded says the first read has landed.
	read   *pending[permRead]
	loaded bool
	err    string
}

func (m *Model) openPermissions(c *hostConn, a *fleet.Agent) tea.Cmd {
	kind, acct, cwd := sessionAgent(c), a.Acct, firstNonEmpty(c.sess.Info.Cwd, a.Cwd)
	p := &permSheet{}
	p.read = goPending(func() permRead { return readRules(settingsFiles(kind, acct, cwd)) })
	p.adopt()
	m.sheet = p
	return p.read.wait()
}

// permRead is every rule in the files, as read off the UI.
type permRead struct {
	files []settingsFile
	rules [3][]permRule
	mode  string
	err   string
}

func readRules(files []settingsFile) permRead {
	r := permRead{files: files}
	for _, f := range files {
		s, err := settingsfile.Load(f.path)
		if err != nil {
			r.err = err.Error()
			continue
		}
		for i, kind := range ruleKinds {
			var list []string
			s.Get("permissions."+kind, &list)
			for _, rule := range list {
				r.rules[i] = append(r.rules[i], permRule{rule, f})
			}
		}
		if m := s.String("permissions.defaultMode"); m != "" {
			r.mode = m + " (" + f.label + ")"
		}
	}
	return r
}

// adopt takes in what was read, once it has landed.
func (p *permSheet) adopt() {
	r, ok := p.read.take()
	if !ok {
		return
	}
	if !p.loaded {
		p.target = len(r.files) - 1
	}
	p.read, p.loaded = nil, true
	p.files, p.rules, p.mode = r.files, r.rules, r.mode
	if r.err != "" {
		p.err = r.err
	}
}

// change edits one file's list for the tab's kind and reads everything
// again, off the UI; one change waits for the one before.
func (p *permSheet) change(f settingsFile, edit func([]string) []string) tea.Cmd {
	key, files, prev := "permissions."+ruleKinds[p.tab], p.files, p.read
	p.read = goPending(func() permRead {
		if prev != nil {
			<-prev.done
		}
		s, err := settingsfile.Load(f.path)
		if err == nil {
			var list []string
			s.Get(key, &list)
			list = edit(list)
			var v any = list
			if len(list) == 0 {
				v = nil
			}
			if err = s.Set(key, v); err == nil {
				err = s.Save()
			}
		}
		r := readRules(files)
		if err != nil {
			r.err = "couldn't save " + tildify(f.path) + ": " + err.Error()
		}
		return r
	})
	p.adopt()
	return p.read.wait()
}

func (p *permSheet) key(m *Model, k tea.KeyPressMsg, s string) tea.Cmd {
	if p.adding {
		switch s {
		case "esc":
			p.adding = false
		case "tab", "shift+tab":
			d := 1
			if s == "shift+tab" {
				d = len(p.files) - 1
			}
			p.target = (p.target + d) % len(p.files)
		case "enter":
			text := strings.TrimSpace(string(p.input))
			p.adding, p.input, p.pos = false, nil, 0
			if text != "" {
				return p.change(p.files[p.target], func(l []string) []string {
					if slices.Contains(l, text) {
						return l
					}
					return append(l, text)
				})
			}
		default:
			p.input, p.pos, _ = edit(p.input, p.pos, k, s)
		}
		return nil
	}
	p.adopt()
	list := p.rules[p.tab]
	cur := &p.cur[p.tab]
	*cur = max(0, min(*cur, len(list)-1))
	if !p.loaded || len(p.files) == 0 {
		if s == "esc" || s == "ctrl+c" || s == "q" {
			m.sheet = nil
		}
		return nil // nothing to change until the files are read
	}
	switch s {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "]", "right":
		p.tab = (p.tab + 1) % 3
	case "[", "left":
		p.tab = (p.tab + 2) % 3
	case "up", "k":
		*cur = roundMove(*cur, -1, len(list))
	case "down", "j":
		*cur = roundMove(*cur, 1, len(list))
	case "a", "+", "enter":
		p.adding, p.err = true, ""
	case "x", "delete", "backspace":
		if len(list) == 0 {
			return nil
		}
		r := list[*cur]
		m.confirm = &confirmation{question: "Take out " + r.text + "?", detail: "from " + r.file.label, onYes: func() tea.Cmd {
			return p.change(r.file, func(l []string) []string { return slices.DeleteFunc(l, func(x string) bool { return x == r.text }) })
		}}
	case "ctrl+e", "e":
		if len(list) > 0 {
			return editFile(list[*cur].file.path)
		}
		return editFile(p.files[0].path)
	}
	return nil
}

func (p *permSheet) body(m *Model, w, h int) []string {
	p.adopt()
	about := "what Claude may do without asking, must ask about, and may never do"
	out := []string{sheetTitle("Permissions", about, w), ""}
	if !p.loaded {
		return append(out, dim("  reading the settings files…"), "", keysFit(w, "esc", "close"))
	}
	out = append(out, sheetTabs([]string{
		fmt.Sprintf("Allow %d", len(p.rules[0])), fmt.Sprintf("Ask %d", len(p.rules[1])), fmt.Sprintf("Deny %d", len(p.rules[2])),
	}, p.tab))
	if p.mode != "" {
		out = append(out, dim("  default mode: "+p.mode))
	}
	out = append(out, "")
	list := p.rules[p.tab]
	cur := max(0, min(p.cur[p.tab], len(list)-1))
	listH := max(3, h-len(out)-6)
	var rows []string
	for i, r := range list {
		line := paint(cText, fit(r.text, max(20, w-28))) + "  " + dim(r.file.label)
		rows = append(rows, sheetRow(line, i == cur, w))
	}
	if len(rows) == 0 {
		rows = append(rows, dim("  none · a adds one, like Bash(npm test:*), Read(./secrets/**) or WebFetch(domain:example.com)"))
	}
	from, to := window(len(rows), cur, listH)
	out = append(out, rows[from:to]...)
	out = append(out, "")
	switch {
	case p.adding:
		out = append(out, "  "+paint(cOrange, ruleKinds[p.tab]+" ❯ ")+textField(p.input, p.pos, true, "Bash(git status:*)", w-40)+"  "+dim("into ")+paint(cText, p.files[p.target].label)+dim(" · tab"))
	case p.err != "":
		out = append(out, "  "+paint(cRed, ansi.Truncate(p.err, w-4, "…")))
	case len(list) > 0:
		out = append(out, "  "+dim(tildify(list[cur].file.path)))
	}
	return append(out, "", keysFit(w, "a", "add", "x", "take out", "[ ]", "allow/ask/deny", "e", "edit the file", "esc", "close"))
}

// hook is one command a hook event runs.
type hook struct {
	event, matcher, command string
	file                    settingsFile
}

// hookSheet is /hooks: every hook by event, from each settings file and
// plugin, with the file it's in; enter opens that file to change it.
type hookSheet struct {
	hooks   []hook
	off     bool // disableAllHooks
	user    string
	cur     int
	loading bool // the settings files are being read, off the UI
}

// hooksRead is the hooks in the settings files, as read off the UI.
type hooksRead struct {
	hooks []hook
	off   bool
	user  string
}

func (m *Model) openHooks(c *hostConn, a *fleet.Agent) tea.Cmd {
	kind, acct, cwd := sessionAgent(c), a.Acct, firstNonEmpty(c.sess.Info.Cwd, a.Cwd)
	hs := &hookSheet{loading: true}
	m.sheet = hs
	read := sheetDo(func() (hooksRead, error) {
		var r hooksRead
		for i, f := range settingsFiles(kind, acct, cwd) {
			if i == 0 {
				r.user = f.path
			}
			s, err := settingsfile.Load(f.path)
			if err != nil {
				continue
			}
			var off bool
			if s.Get("disableAllHooks", &off) && off {
				r.off = true
			}
			var raw jsontext.Value
			if s.Get("hooks", &raw) {
				r.hooks = append(r.hooks, parseHooks(raw, f)...)
			}
		}
		return r, nil
	}, func(m *Model, r hooksRead, _ error) tea.Cmd {
		hs.loading, hs.off, hs.user = false, r.off, r.user
		hs.hooks = append(r.hooks, hs.hooks...) // the plugins' may have landed first
		hs.sort()
		return nil
	})
	// Enabled plugins' hooks come after, read-only.
	plug, ok := agent.As[agent.Plugger](kind)
	if !ok {
		return read
	}
	return tea.Batch(read, sheetDo(func() ([]hook, error) {
		inst, _, err := plug.Plugins(acct, cwd)
		var out []hook
		for _, pl := range inst {
			if !pl.Enabled {
				continue
			}
			path := filepath.Join(pl.InstallPath, "hooks", "hooks.json")
			if b, err := os.ReadFile(path); err == nil {
				var w struct {
					Hooks jsontext.Value `json:"hooks"`
				}
				if jsonx.Unmarshal(b, &w) == nil {
					out = append(out, parseHooks(w.Hooks, settingsFile{"plugin " + pl.Name, path})...)
				}
			}
		}
		return out, err
	}, func(m *Model, more []hook, err error) tea.Cmd {
		hs.hooks = append(hs.hooks, more...)
		hs.sort()
		return nil
	}))
}

func (hs *hookSheet) sort() {
	sort.SliceStable(hs.hooks, func(i, j int) bool { return hs.hooks[i].event < hs.hooks[j].event })
}

func parseHooks(raw jsontext.Value, f settingsFile) []hook {
	var byEvent map[string][]struct {
		Matcher string `json:"matcher"`
		Hooks   []struct {
			Type    string `json:"type"`
			Command string `json:"command"`
			Prompt  string `json:"prompt"`
		} `json:"hooks"`
	}
	if jsonx.Unmarshal(raw, &byEvent) != nil {
		return nil
	}
	var out []hook
	for ev, groups := range byEvent {
		for _, g := range groups {
			for _, h := range g.Hooks {
				out = append(out, hook{ev, g.Matcher, firstNonEmpty(h.Command, h.Prompt, h.Type), f})
			}
		}
	}
	return out
}

func (hs *hookSheet) key(m *Model, k tea.KeyPressMsg, s string) tea.Cmd {
	switch s {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "up", "k":
		hs.cur = roundMove(hs.cur, -1, len(hs.hooks))
	case "down", "j":
		hs.cur = roundMove(hs.cur, 1, len(hs.hooks))
	case "enter", "e", "ctrl+e":
		path := hs.user
		if len(hs.hooks) > 0 {
			h := hs.hooks[hs.cur]
			if strings.HasPrefix(h.file.label, "plugin ") {
				m.flash("that hook is "+h.file.label+"'s: /plugins turns the plugin off", true)
				return nil
			}
			path = h.file.path
		}
		if path == "" {
			return nil
		}
		m.sheet = nil
		return editFile(path)
	}
	return nil
}

func (hs *hookSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Hooks", "commands Claude Code runs around tools, prompts and sessions", w), ""}
	if hs.off {
		out = append(out, paint(cYellow, "  All hooks are off (disableAllHooks): Settings › Claude turns them back on."), "")
	}
	listH := max(3, h-len(out)-6)
	var rows []string
	selAt, last := 0, ""
	for i, hk := range hs.hooks {
		if hk.event != last {
			rows = append(rows, paint(cSub+bold, "  "+hk.event))
			last = hk.event
		}
		if i == hs.cur {
			selAt = len(rows)
		}
		match := hk.matcher
		if match == "" {
			match = "*"
		}
		line := paint(cBlue, fit(match, 14)) + " " + paint(cText, fit(oneLine(hk.command), max(20, w-44))) + "  " + dim(hk.file.label)
		rows = append(rows, sheetRow(line, i == hs.cur, w))
	}
	if len(rows) == 0 {
		rows = append(rows, dim("  no hooks · enter opens your settings.json to add one"))
		if hs.loading {
			rows[0] = dim("  reading the settings files…")
		}
	}
	from, to := window(len(rows), selAt, listH)
	out = append(out, rows[from:to]...)
	out = append(out, "")
	if len(hs.hooks) > 0 {
		out = append(out, "  "+dim(ansi.Truncate(tildify(hs.hooks[hs.cur].file.path), w-4, "…")))
	}
	return append(out, "", keysFit(w, "↑↓", "choose", "enter", "edit its file", "esc", "close"))
}
