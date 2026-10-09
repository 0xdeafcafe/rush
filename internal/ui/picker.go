package ui

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// picker is a small dropdown: one of an agent's pull requests, the folder
// a new session starts in or an agent works in, or what to do with a link.
type picker struct {
	title  string
	note   string // after the title, dimmed
	about  string // under the title, wrapped: what it's about
	prs    []agent.PR
	dirs   []string
	query  []rune          // narrows dirs, or is a folder of its own
	known  map[string]bool // dirs that are projects rush knows
	disk   []string        // folders on disk for diskQ: see scanDisk
	diskQ  string
	moving string // the agent a folder is chosen for; "" is new sessions
	acts   []linkAct
	img    string   // the image file the acts are for, shown above them
	shot   []string // img drawn, once it's made
	big    bool     // img drawn as big as the window allows
	cursor int
}

// openDirPicker is ctrl+l with nothing to move: the folder new sessions
// start in.
func (m *Model) openDirPicker() tea.Cmd {
	m.picker = &picker{title: "Start new sessions in", dirs: m.folderChoices(), known: m.knownProjects()}
	start := m.startDir()
	for i, d := range m.picker.shownDirs() {
		if d == start {
			m.picker.cursor = i
		}
	}
	return m.scanDisk()
}

// openMovePicker is ctrl+l on an agent: the same folders, and the one
// chosen is where it's told to work from now on.
func (m *Model) openMovePicker(a *fleet.Agent) tea.Cmd {
	m.picker = &picker{
		title: "Move " + oneLine(a.DisplayName) + " to", note: "now in " + tildify(agentDir(a)),
		dirs: m.folderChoices(), known: m.knownProjects(), moving: a.Key,
	}
	return m.scanDisk()
}

// knownProjects are the folders Projects lists.
func (m *Model) knownProjects() map[string]bool {
	out := map[string]bool{}
	for _, p := range m.projects() {
		if filepath.IsAbs(p.key) {
			out[p.key] = true
		}
	}
	return out
}

// scanDisk looks on disk, off the UI's goroutine, for folders the picker's
// text could mean: a typed path's subfolders, or else those beside the
// projects rush knows, so one it doesn't know yet is a pick away.
func (m *Model) scanDisk() tea.Cmd {
	p := m.picker
	if p == nil || p.dirs == nil {
		return nil
	}
	q := strings.TrimSpace(string(p.query))
	var parents []string
	for k := range p.known {
		parents = append(parents, filepath.Dir(k))
	}
	return later(func() []string { return diskDirs(q, parents) }, func(m *Model, dirs []string) tea.Cmd {
		if m.picker == p && strings.TrimSpace(string(p.query)) == q {
			p.disk, p.diskQ = dirs, q
		}
		return nil
	})
}

// diskDirs are the folders q could mean: with a path, those in its folder
// that start as its last part does; else those in parents whose path holds q.
func diskDirs(q string, parents []string) []string {
	match := func(name string) bool { return strings.Contains(strings.ToLower(name), strings.ToLower(q)) }
	if strings.HasPrefix(q, "/") || strings.HasPrefix(q, "~") {
		dir, base := filepath.Split(expand(q))
		parents = []string{filepath.Clean(dir)}
		match = func(name string) bool { return strings.HasPrefix(strings.ToLower(name), strings.ToLower(base)) }
	}
	seen := map[string]bool{}
	var out []string
	for _, par := range parents {
		if seen[par] {
			continue
		}
		seen[par] = true
		ents, _ := os.ReadDir(par)
		for _, e := range ents {
			if d := filepath.Join(par, e.Name()); e.IsDir() && !strings.HasPrefix(e.Name(), ".") && match(e.Name()) {
				out = append(out, d)
			}
		}
	}
	sort.Strings(out)
	return out[:min(len(out), 200)] // ponytail: capped; type more of a path to narrow
}

// agentDir is where an agent works now, as far as its transcript says.
func agentDir(a *fleet.Agent) string {
	if a.Spend.Dir != "" {
		return a.Spend.Dir
	}
	return a.Cwd
}

// folderChoices are the folders worth offering: where new sessions have
// started, then every folder an agent ran in, their repositories, and
// those repositories' Claude worktrees.
func (m *Model) folderChoices() []string {
	out := m.startDirs()
	seen := map[string]bool{}
	for _, d := range out {
		seen[d] = true
	}
	var rest []string
	add := func(p string) {
		if p != "" && !seen[p] {
			seen[p] = true
			rest = append(rest, p)
		}
	}
	for _, a := range m.snap.Agents {
		add(a.Repo)
		for _, wt := range m.worktreesOf(a.Repo) {
			add(wt)
		}
		add(agentDir(a))
	}
	sort.Strings(rest)
	return append(out, rest...)
}

// shownDirs are the folders the typed text matches: the typed text first
// when it's a path, then known projects, other folders agents worked in,
// and what's on disk.
func (p *picker) shownDirs() []string {
	q := strings.TrimSpace(string(p.query))
	typed := ""
	if strings.HasPrefix(q, "/") || strings.HasPrefix(q, "~") {
		typed = expand(q)
	}
	lq := strings.ToLower(q)
	seen := map[string]bool{typed: true}
	var known, other, disk []string
	for _, d := range p.dirs {
		if !seen[d] && strings.Contains(strings.ToLower(tildify(d)), lq) {
			seen[d] = true
			if p.known[d] {
				known = append(known, d)
			} else {
				other = append(other, d)
			}
		}
	}
	for _, d := range p.disk {
		if !seen[d] {
			seen[d] = true
			disk = append(disk, d)
		}
	}
	var out []string
	if typed != "" {
		out = append(out, typed)
	}
	return append(append(append(out, known...), other...), disk...)
}

// dirSection is the heading the picker lists d under.
func (p *picker) dirSection(d string) string {
	switch {
	case p.known[d]:
		return "Projects"
	case slices.Contains(p.dirs, d):
		return "Folders"
	case slices.Contains(p.disk, d):
		return "On disk"
	}
	return "Path"
}

// setStartDir makes dir where new sessions start, kept first in the list
// when it wasn't in it.
func (m *Model) setStartDir(dir string) {
	for {
		for i, d := range m.startDirs() {
			if d == dir {
				m.pickStartDir(i)
				return
			}
		}
		if m.pickedDir == dir {
			return
		}
		m.pickedDir = dir
	}
}

// moveTo tells an agent to work in another folder from now on, rather than
// stopping it to relaunch there; its row follows once it cd's there or
// writes a file there.
func (m *Model) moveTo(a *fleet.Agent, dir string) tea.Cmd {
	switch {
	case a == nil:
		return nil
	case a.Interactive:
		m.flash(a.DisplayName+" is open in another terminal · tell it there", true)
		return nil
	}
	text := moveNote(dir, agentDir(a))
	if a.Rush {
		// Mid-turn, not queued: work it does before reading it lands in the old folder.
		m.flash("sending to "+a.DisplayName+"…", false)
		send := sendHostedID(a.ID, a.DisplayName, text, true)
		if m.store.Overlay.Names[a.Key] != "" {
			return send // a name you gave it stays
		}
		return tea.Sequence(send, retitleHosted(a.ID, dir))
	}
	return m.replyTo(a, text, text)
}

// moveNote asks an agent to carry on in dir. Its shell can go back to
// where it started between commands, so each one cd's first; the leading
// cd is also what tells rush where it works now.
func moveNote(dir, from string) string {
	cd := dir
	if strings.ContainsAny(dir, " '\"$`") {
		cd = "'" + strings.ReplaceAll(dir, "'", `'\''`) + "'"
	}
	return fmt.Sprintf("Please move over to %s and work there from now on (you were in %s). "+
		"Start by running `cd %s`. Your shell may go back to the old folder between commands, so begin each Bash command with `cd %s && ` "+
		"and use absolute paths under %s. Paths from earlier in this conversation point at the old folder.", dir, from, cd, cd, dir)
}

func (p *picker) size() int {
	if p.dirs != nil {
		return len(p.shownDirs())
	}
	if p.acts != nil {
		return len(p.acts)
	}
	return len(p.prs)
}

// openPR opens the selected agent's pull request in the browser, asking
// which one first when it has several.
func (m *Model) openPR(a *fleet.Agent) tea.Cmd {
	if a == nil || len(a.PRs) == 0 {
		m.flash("no pull request found for this agent", true)
		return nil
	}
	if len(a.PRs) == 1 {
		return browse(a.PRs[0].URL)
	}
	m.picker = &picker{title: "Pull requests · " + oneLine(a.DisplayName), prs: a.PRs}
	return nil
}

func browse(url string) tea.Cmd {
	return func() tea.Msg {
		if err := exec.Command("open", url).Run(); err != nil {
			return doneMsg{err: fmt.Errorf("couldn't open %s: %w", url, err)}
		}
		return doneMsg{text: "opened " + url}
	}
}

func (m *Model) pickerKey(k tea.KeyPressMsg, s string) tea.Cmd {
	p := m.picker
	if p.dirs != nil {
		return m.dirPickerKey(k, s)
	}
	switch s {
	case "esc", "q", "ctrl+y", "ctrl+l":
		m.picker = nil
	case "up", "k", "shift+tab":
		p.cursor = roundMove(p.cursor, -1, p.size())
	case "down", "j", "tab":
		p.cursor = roundMove(p.cursor, 1, p.size())
	case "v":
		if p.img != "" {
			p.big = !p.big
			return m.drawShot() // the one shown stays until this lands
		}
	case "enter":
		if p.acts != nil {
			m.picker = nil
			return p.acts[p.cursor].do(m)
		}
		url := p.prs[p.cursor].URL
		m.picker = nil
		return browse(url)
	}
	return nil
}

// dirPickerKey is a folder picker's keys: what's typed narrows the list,
// or is a path of its own.
func (m *Model) dirPickerKey(k tea.KeyPressMsg, s string) tea.Cmd {
	p := m.picker
	switch s {
	case "esc", "ctrl+l":
		m.picker = nil
	case "up", "shift+tab":
		p.cursor = roundMove(p.cursor, -1, p.size())
	case "down", "tab":
		p.cursor = roundMove(p.cursor, 1, p.size())
	case "backspace", "ctrl+h":
		if len(p.query) > 0 {
			p.query, p.cursor = p.query[:len(p.query)-1], 0
			return m.scanDisk()
		}
	case "ctrl+u", "super+backspace":
		p.query, p.cursor = p.query[:0], 0
		return m.scanDisk()
	case "enter":
		shown := p.shownDirs()
		if len(shown) == 0 {
			return nil
		}
		dir := shown[min(p.cursor, len(shown)-1)]
		m.picker = nil
		if p.moving != "" {
			return m.moveTo(m.agentByKey(p.moving), dir)
		}
		m.setStartDir(dir)
		m.flash("new sessions start in "+tildify(dir), false)
	default:
		if k.Text != "" && k.Mod&^tea.ModShift == 0 {
			p.query, p.cursor = append(p.query, []rune(k.Text)...), 0
			return m.scanDisk()
		}
	}
	return nil
}

func (m *Model) pickerBody(w int) []string {
	p := m.picker
	title := paint(cText+bold, p.title)
	if p.note != "" {
		title += dim(" · " + p.note)
	}
	out := []string{title, ""}
	if p.about != "" {
		for _, l := range wrap(p.about, w) {
			out = append(out, dim(l))
		}
		out = append(out, "")
	}
	if p.dirs != nil {
		out = append(out, dim("Folder ❯ ")+string(p.query)+paint(cOrange, "▏"), "")
		shown := p.shownDirs()
		if len(shown) == 0 {
			out = append(out, faint(" no folder matches · type a path"))
		}
		room := max(4, m.h-18) // and up to four headings
		top := max(0, min(p.cursor-room+1, len(shown)-room))
		for i := top; i < len(shown) && i < top+room; i++ {
			if sec := p.dirSection(shown[i]); i == top || sec != p.dirSection(shown[i-1]) {
				out = append(out, faint(" "+sec))
			}
			line := paint(cText, tildify(shown[i]))
			if i == p.cursor {
				line = highlight(paint(cOrange, "▍")+line, w)
			} else {
				line = " " + line
			}
			out = append(out, line)
		}
		do := "start new sessions here"
		if p.moving != "" {
			do = "tell it to work here"
		}
		return append(out, "", keys("type", "filter or a path", "↑↓", "choose", "enter", do, "esc", "close"))
	}
	if p.acts != nil {
		if p.shot != nil {
			pad := strings.Repeat(" ", max(0, (w-cellw.String(ansi.Strip(p.shot[0])))/2))
			for _, r := range p.shot {
				out = append(out, pad+r)
			}
			out = append(out, "")
		}
		for i, a := range p.acts {
			line := paint(cText, a.label)
			if i == p.cursor {
				line = highlight(paint(cOrange, "▍")+line, w)
			} else {
				line = " " + line
			}
			out = append(out, line)
		}
		if p.shot != nil {
			size := "bigger"
			if p.big {
				size = "smaller"
			}
			return append(out, "", keys("↑↓", "choose", "enter", "do it", "v", size, "esc", "close"))
		}
		return append(out, "", keys("↑↓", "choose", "enter", "do it", "esc", "close"))
	}
	for i, pr := range p.prs {
		col := cGreen
		switch {
		case pr.State == "MERGED":
			col = cBlue
		case pr.State == "CLOSED" || pr.State == "unknown":
			col = cDim
		case pr.Checks.Failed > 0:
			col = cRed
		}
		checks := ""
		if pr.Checks.Passed+pr.Checks.Failed+pr.Checks.Pending > 0 {
			checks = fmt.Sprintf("  %d✓ %d✗ %d…", pr.Checks.Passed, pr.Checks.Failed, pr.Checks.Pending)
		}
		title := pr.Title
		if title == "" {
			title = strings.TrimPrefix(pr.URL, "https://github.com/")
		}
		line := paint(col, fmt.Sprintf("#%-6d", pr.Number)) + " " + paint(cText, fit(title, w-40)) + "  " + dim(fit(strings.ToLower(pr.State), 8)) + faint(checks)
		if i == p.cursor {
			line = highlight(paint(cOrange, "▍")+line, w)
		} else {
			line = " " + line
		}
		out = append(out, line)
	}
	return append(out, "", keys("↑↓", "choose", "enter", "open in browser", "esc", "close"))
}
