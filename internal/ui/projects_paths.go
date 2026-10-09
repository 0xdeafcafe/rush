package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Folders and files on the Projects page that x deletes on their own: one
// thing in a session's temp folder, opened from Temporary, or a project's
// agent scratch folder (.claude/tmp), emptied.

// pathRow is a thing on disk a row stands for.
type pathRow struct {
	path    string
	temp    *fleet.Agent // the session whose temp folder it's in
	scratch *project     // the project whose agent scratch folder it is
}

// pathInfo is the top of a folder as it was when last looked at: how many
// things are in it and when the newest of them changed.
type pathInfo struct {
	items int
	mod   time.Time
	at    time.Time
}

// peekFor is how long a look at a folder's top stands.
const peekFor = 30 * time.Second

// dirPeeks are folders being looked at, by path.
var dirPeeks = offReads[string, pathInfo]{}

// peekDir is the top of dir: its things, and when the newest changed. Only
// the top is looked at, never walked. It reads the disk: never on the UI.
func peekDir(dir string) pathInfo {
	p := pathInfo{at: time.Now()}
	if st, err := os.Lstat(dir); err == nil {
		p.mod = st.ModTime()
	}
	ents, _ := os.ReadDir(dir)
	p.items = len(ents)
	for _, e := range ents {
		if fi, err := e.Info(); err == nil && fi.ModTime().After(p.mod) {
			p.mod = fi.ModTime()
		}
	}
	return p
}

// peekLine is a folder's top in words: how many things, when last touched.
// It's looked at in the background, and not again for peekFor; until the
// first look is in it says so.
func (m *Model) peekLine(dir string, now time.Time) string {
	if m.work.peeks == nil {
		m.work.peeks = map[string]pathInfo{}
	}
	take := func() {
		if p, ok := dirPeeks.take(dir); ok {
			m.work.peeks[dir] = p
		}
	}
	take()
	p, ok := m.work.peeks[dir]
	if !ok || time.Since(p.at) >= peekFor {
		dirPeeks.start(dir, func() pathInfo { return peekDir(dir) })
		take() // tests read at once
		p, ok = m.work.peeks[dir]
	}
	if !ok {
		return "looking…"
	}
	s := fmt.Sprintf("%d item%s", p.items, plural(p.items))
	if !p.mod.IsZero() {
		s += " · touched " + age(now.Sub(p.mod)) + " ago"
	}
	return s
}

// tempEntry is one thing at the top of a session's temp folder.
type tempEntry struct {
	path string
	mod  time.Time
	dir  bool
}

// tempList is a session's temp folders' tops as last listed.
type tempList struct {
	ents []tempEntry
	at   time.Time
}

// tempLists are sessions' temp folders being listed again, by agent key.
var tempLists = offReads[string, []tempEntry]{}

// tempEntries are what's at the top of a's temp folders, biggest first once
// measured, else newest first; nil until openTemp's listing is in. Listed
// again, in the background, once it's stood peekFor.
func (m *Model) tempEntries(a *fleet.Agent) []tempEntry {
	if ents, ok := tempLists.take(a.Key); ok && m.work.lists != nil {
		m.work.lists[a.Key] = tempList{ents: ents, at: time.Now()}
	}
	l, ok := m.work.lists[a.Key]
	if ok && time.Since(l.at) >= peekFor {
		c := *a // its own copy, for off the UI
		tempLists.start(a.Key, func() []tempEntry { return listTemp(c.TempDirs()) })
	}
	out := slices.Clone(l.ents)
	sizes := m.work.sizes
	sort.SliceStable(out, func(i, j int) bool {
		if a, b := sizes[out[i].path], sizes[out[j].path]; a != b {
			return a > b
		}
		return out[i].mod.After(out[j].mod)
	})
	return out
}

// listTemp is what's at the top of dirs. It reads the disk: never on the UI.
func listTemp(dirs []fleet.TempDir) []tempEntry {
	var out []tempEntry
	for _, d := range dirs {
		ents, _ := os.ReadDir(d.Path)
		for _, e := range ents {
			te := tempEntry{path: filepath.Join(d.Path, e.Name()), dir: e.IsDir()}
			if fi, err := e.Info(); err == nil {
				te.mod = fi.ModTime()
			}
			out = append(out, te)
		}
	}
	return out
}

// tempShownEntries is how many of a session's things it shows opened.
const tempShownEntries = 40

// openTemp shows or hides what's in a session's temp folders: listed in
// the background, then each thing measured once, the first time it's seen.
func (m *Model) openTemp(a *fleet.Agent) tea.Cmd {
	if m.work.opened == nil {
		m.work.opened = map[string]bool{}
	}
	m.work.opened[a.Key] = !m.work.opened[a.Key]
	if !m.work.opened[a.Key] {
		return nil
	}
	c := *a // its own copy, for off the UI
	return later(func() []tempEntry { return listTemp(c.TempDirs()) }, func(m *Model, ents []tempEntry) tea.Cmd {
		if m.work.lists == nil {
			m.work.lists = map[string]tempList{}
		}
		m.work.lists[a.Key] = tempList{ents: ents, at: time.Now()}
		var todo []string
		for _, e := range ents {
			if _, ok := m.work.sizes[e.path]; !ok {
				todo = append(todo, e.path)
			}
		}
		if len(todo) == 0 {
			return nil
		}
		return later(func() map[string]int64 {
			out := make(map[string]int64, len(todo))
			for _, p := range todo {
				out[p] = fleet.DiskUsage([]fleet.TempDir{{Path: p}})
			}
			return out
		}, func(m *Model, got map[string]int64) tea.Cmd {
			if m.work.sizes == nil {
				m.work.sizes = map[string]int64{}
			}
			for p, n := range got {
				m.work.sizes[p] = n
			}
			return nil
		})
	})
}

// tempEntryRows are a session's opened temp folder, a row per thing.
func (m *Model) tempEntryRows(a *fleet.Agent, nameW, sizeW int, now time.Time) []workRow {
	ents := m.tempEntries(a)
	if _, ok := m.work.lists[a.Key]; !ok {
		return []workRow{{line: dim("    listing…")}}
	}
	if len(ents) == 0 {
		return []workRow{{line: dim("    empty")}}
	}
	var rows []workRow
	for _, e := range ents[:min(len(ents), tempShownEntries)] {
		name := filepath.Base(e.path)
		if e.dir {
			name += "/"
		}
		size := "…"
		if n, ok := m.work.sizes[e.path]; ok {
			size = disk(n)
		}
		line := "    " + fit(paint(cSub, name), nameW-4) + dim(right(size, sizeW)) + "   " + dim("touched "+age(now.Sub(e.mod))+" ago")
		rows = append(rows, workRow{id: "e" + e.path, path: &pathRow{path: e.path, temp: a}, line: line})
	}
	if n := len(ents) - tempShownEntries; n > 0 {
		rows = append(rows, workRow{line: dim(fmt.Sprintf("    … %d more, the newest and biggest above", n))})
	}
	return rows
}

// sessionEnded is when a session stopped, or that it runs.
func sessionEnded(a *fleet.Agent, now time.Time) string {
	if a.PID != 0 {
		return paint(cYellow, "running")
	}
	return dim("ended " + age(a.Age(now)) + " ago")
}

// askRemovePath asks before deleting the thing a row stands for.
func (m *Model) askRemovePath(p pathRow) tea.Cmd {
	size := ""
	if n, ok := m.work.sizes[p.path]; ok {
		size = ", " + disk(n)
	}
	switch {
	case p.temp != nil:
		if p.temp.PID != 0 {
			m.flash(p.temp.DisplayName+" is still running; its temp work may be in use · stop it first", true)
			return nil
		}
		a := p.temp
		m.confirm = &confirmation{
			question: "Delete " + filepath.Base(p.path) + size + "?",
			detail:   "from " + a.DisplayName + "'s temp work · the rest of it stays",
			onYes: func() tea.Cmd {
				return m.removed(p, func() error { return fleet.RemoveTempEntry(a, p.path) })
			},
		}
	case p.scratch != nil:
		if slices.ContainsFunc(p.scratch.agents, func(a *fleet.Agent) bool { return a.PID != 0 }) {
			m.flash("an agent in "+p.scratch.title+" is running and may be using its scratch · stop it first", true)
			return nil
		}
		m.confirm = &confirmation{
			question: "Empty " + tildify(p.path) + "?",
			detail:   m.peekLine(p.path, time.Now()) + " · what agents kept as scratch here; the folder stays",
			onYes:    func() tea.Cmd { return m.removed(p, func() error { return emptyScratch(p.path) }) },
		}
	}
	return nil
}

// removed runs a deletion, then forgets what was known of the path.
func (m *Model) removed(p pathRow, rm func() error) tea.Cmd {
	freed := m.work.sizes[p.path]
	return later(rm, func(m *Model, err error) tea.Cmd {
		delete(m.work.sizes, p.path)
		delete(m.work.peeks, p.path)
		if a := p.temp; a != nil {
			if l, ok := m.work.lists[a.Key]; ok {
				l.ents = slices.DeleteFunc(slices.Clone(l.ents), func(e tempEntry) bool { return e.path == p.path })
				m.work.lists[a.Key] = l
			}
		}
		if err != nil {
			m.flash(err.Error(), true)
			return nil
		}
		if a := p.temp; a != nil && freed > 0 {
			m.onTemp(tempMsg{a.Key: fleet.TempSize{Bytes: max(0, a.Temp-freed), At: time.Now()}})
		}
		m.flash("deleted "+tildify(p.path), false)
		return nil
	})
}

// emptyScratch empties a project's agent scratch folder, keeping it: only
// ever a tmp folder in one of the agents' project folders.
func emptyScratch(dir string) error {
	if (filepath.Base(dir) != "tmp" || !slices.Contains(agent.ProjectFolders(), filepath.Base(filepath.Dir(dir)))) && filepath.Dir(dir) != host.TempRoot() {
		return fmt.Errorf("won't empty %s", dir)
	}
	ents, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	for _, e := range ents {
		if err := os.RemoveAll(filepath.Join(dir, e.Name())); err != nil {
			return err
		}
	}
	return nil
}

// goesAfter is how long an agent sits untouched before Projects says it
// can go: as long as done work waits before the tidy-up (3h unless set).
func (m *Model) goesAfter() time.Duration {
	if d := m.store.Config.CleanupAfter(); d > 0 {
		return d
	}
	return state.DefaultCleanup
}

// canGo is a project's agent that's done with: stopped, untouched for
// goesAfter, and leaving no work only its worktree has.
func (m *Model) canGo(a *fleet.Agent, now time.Time) bool {
	if a.PID != 0 || a.Live() || a.Busy() || a.NeedsYou() || m.untouched(a, now) < m.goesAfter() {
		return false
	}
	if t := treeOf(a); t != "" {
		wt := m.worktreeAt(t)
		return wt.Safe()
	}
	return true
}
