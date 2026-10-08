package ui

import (
	"fmt"
	"path/filepath"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// --- Delete: what a removal takes, before anything goes ---

// deleteSheet says exactly what a removal takes: each path, whether git
// removes it as a worktree or it's deleted as a folder, the branch that
// stays, the agents that worked there, and loudly, what's lost for good.
// Each is looked at afresh off the UI; y goes only once that's landed, and
// never while an agent runs in what would go.
type deleteSheet struct {
	items  []doomed
	read   *pending[[]doomed]
	back   sheet // the sheet it was opened from, back on esc
	scroll int
}

// doomed is one thing a removal takes: a worktree, finished agents' temp
// work, or what's untouched in /tmp.
type doomed struct {
	wt     *fleet.Worktree
	agents []*fleet.Agent // whose temp work; whose left is
	tmp    bool
	left   *fleet.LeftItem // what an agent left outside its project

	// Looked at off the UI.
	looked         bool
	files, commits []string // a worktree's uncommitted files, commits nowhere else
	dirs           []fleet.TempDir
	stale          []fleet.ScratchItem
}

// openDelete opens the sheet on what would go, and looks at each afresh.
func (m *Model) openDelete(back sheet, items ...doomed) tea.Cmd {
	s := &deleteSheet{items: items, back: back}
	// Copies, for the look: nothing it reads is the UI's.
	look := make([]doomed, len(items))
	for i := range items {
		d := &look[i]
		*d = items[i]
		if d.wt != nil {
			w := *d.wt
			d.wt = &w
		}
		if d.left != nil {
			l := *d.left
			d.left = &l
		}
		d.agents = copyAgents(d.agents)
	}
	s.read = goPending(func() []doomed {
		for i := range look {
			d := &look[i]
			switch {
			case d.wt != nil:
				d.files, d.commits = d.wt.Doomed()
			case d.left != nil:
				*d.left = d.left.Again()
			case d.tmp:
				d.stale = fleet.StaleScratch()
			default:
				for _, a := range d.agents {
					d.dirs = append(d.dirs, a.TempDirs()...)
				}
			}
			d.looked = true
		}
		return look
	})
	m.sheet = s
	return s.read.wait()
}

func copyAgents(list []*fleet.Agent) []*fleet.Agent {
	out := make([]*fleet.Agent, len(list))
	for i, a := range list {
		b := *a
		out[i] = &b
	}
	return out
}

// adopt takes in the look, once it has landed.
func (s *deleteSheet) adopt() {
	if v, ok := s.read.take(); ok {
		s.items, s.read = v, nil
	}
}

func (s *deleteSheet) width(*Model) int { return 118 }

// size is how much disk an item frees, as last measured.
func (d *doomed) size(m *Model) int64 {
	var n int64
	switch {
	case d.wt != nil:
		return d.wt.Size
	case d.left != nil:
		return d.left.Size
	case d.tmp && !d.looked:
		return m.clean.tmp.Stale
	case d.tmp:
		for _, it := range d.stale {
			n += it.Size
		}
		return n
	}
	for _, a := range d.agents {
		n += a.Temp
	}
	return n
}

// losing is an item that throws away work nowhere else: uncommitted files,
// or commits no branch keeps.
func (d *doomed) losing() bool {
	return d.wt != nil && (d.wt.Changed > 0 || d.wt.Unpushed > 0 && d.wt.Branch == "" || d.wt.Err != "")
}

// blocked is why an item can't go now: an agent running in it.
func (m *Model) blocked(d *doomed) string {
	if d.wt != nil {
		if a := m.running(d.wt.Agents); a != nil {
			return oneLine(a.DisplayName) + " is running in it"
		}
	}
	if d.left != nil && d.looked {
		if why := d.left.Why(); why != "" {
			return why
		}
	}
	for _, a := range d.agents {
		if b := m.agentByKey(a.Key); b != nil && b.PID != 0 {
			return oneLine(b.DisplayName) + " is running; its temp work may be in use"
		}
	}
	return ""
}

// lines are everything the sheet lists, and what it all comes to.
func (s *deleteSheet) lines(m *Model, w int) (out []string, size int64, losing int, blocked bool) {
	for i := range s.items {
		d := &s.items[i]
		size += d.size(m)
		if d.losing() {
			losing++
		}
		if i > 0 {
			out = append(out, "")
		}
		out = append(out, d.head(m, w)...)
		if why := m.blocked(d); why != "" {
			blocked = true
			out = append(out, "   "+paint(cRed+bold, "✗ can't go: "+why)+dim(" · stop it or mark it done first"))
		}
		if !d.looked {
			out = append(out, "   "+paint(cOrange, spinner[m.tick%len(spinner)])+dim(" looking at it again…"))
			continue
		}
		out = append(out, d.found(m, w)...)
	}
	return out, size, losing, blocked
}

// head is what an item is, where, and how it goes.
func (d *doomed) head(m *Model, w int) []string {
	switch {
	case d.wt != nil:
		wt := d.wt
		keeps := "   " + paint(cGreen, "✓ ") + dim("branch ") + paint(cSub, wt.Branch) + dim(" stays, and "+tildify(wt.Repo)+" with it")
		if wt.Branch == "" {
			keeps = "   " + paint(cYellow, "! ") + dim("on no branch (detached): nothing keeps its commits once it goes")
		}
		return []string{kindLine(kindWorktree, filepath.Base(wt.Path), "worktree · git removes it", disk(wt.Size), w),
			"   " + faint(tildify(wt.Path)), keeps}
	case d.left != nil:
		what := "left by " + oneLine(d.left.Agent.DisplayName) + " · deleted"
		note := "scratch in a temp folder"
		if !d.left.Temp {
			note = "outside any temp folder: only because you ticked it"
		}
		return []string{kindLine(kindTemp, tildify(d.left.Path), what, disk(d.size(m)), w), "   " + dim(note+"; looked at again first, and kept if it changed since")}
	case d.tmp:
		n := m.clean.tmp.StaleItems
		if d.looked {
			n = len(d.stale)
		}
		return []string{kindLine(kindTemp, "/tmp", fmt.Sprintf("%d of yours untouched for a day · deleted", n), disk(d.size(m)), w),
			"   " + dim("each is looked at again first; anything touched since stays")}
	}
	what := "temp work of " + oneLine(d.agents[0].DisplayName)
	if len(d.agents) > 1 {
		what = fmt.Sprintf("temp work of %d finished agents", len(d.agents))
	}
	return []string{kindLine(kindTemp, what, "scratch folders · deleted", disk(d.size(m)), w),
		"   " + dim("conversations, their files and any worktree stay")}
}

// found is what the fresh look found: for a worktree, loudly, what's lost
// and who worked there; for the rest, each path that goes.
func (d *doomed) found(m *Model, w int) []string {
	switch {
	case d.wt != nil:
		return d.worktreeLoss(m, w)
	case d.left != nil:
		return nil
	case d.tmp:
		if len(d.stale) == 0 {
			return []string{"   " + dim("nothing is untouched any more; nothing goes")}
		}
		items := make([]string, len(d.stale))
		for i, it := range d.stale {
			items[i] = fit(tildify(it.Path), w-18) + right(disk(it.Size), 8)
		}
		return listed(items, cSub, w)
	}
	items := make([]string, len(d.dirs))
	for i, t := range d.dirs {
		items[i] = tildify(t.Path)
		if t.Keep {
			items[i] = tildify(t.Path) + "  (emptied; the folder stays)"
		}
	}
	return listed(items, cSub, w)
}

func (d *doomed) worktreeLoss(m *Model, w int) []string {
	wt := d.wt
	var out []string
	if wt.Err != "" {
		out = append(out, "   "+paint(cRed+bold, "✗ "+wt.Err+": what it holds can't be told, and goes with it"))
	}
	if n := len(d.files); n > 0 {
		out = append(out, "   "+paint(cRed+bold, fmt.Sprintf("✗ %d uncommitted file%s LOST for good", n, plural(n))))
		out = append(out, listed(d.files, cRed, w)...)
	}
	if n := len(d.commits); n > 0 {
		c := fmt.Sprintf("%d commit%s", n, plural(n))
		if wt.Branch == "" {
			out = append(out, "   "+paint(cRed+bold, "✗ "+c+" on no branch LOST"))
		} else {
			out = append(out, "   "+paint(cYellow+bold, "! "+c+" not pushed or in main")+paint(cYellow, ": only "+wt.Branch+" keeps them"))
		}
		out = append(out, listed(d.commits, cYellow, w)...)
	}
	if wt.Locked {
		out = append(out, "   "+paint(cYellow, "! locked: someone put a lock on it, and removing it ignores that"))
	}
	if wt.Safe() {
		out = append(out, "   "+paint(cGreen, "✓ nothing is lost: every change committed, every commit pushed or in main"))
	}
	for _, k := range wt.Agents {
		if a := m.agentByKey(k); a != nil && a.PID == 0 {
			out = append(out, "   "+dim("worked here: ")+paint(cSub, oneLine(a.DisplayName))+dim(" · its conversation stays; resumed, it has no folder"))
		}
	}
	return out
}

// listed is a list under an item: the first few, and how many more.
func listed(items []string, col string, w int) []string {
	const shown = 8
	var out []string
	for _, l := range items[:min(shown, len(items))] {
		out = append(out, "      "+paint(col, ansi.Truncate(l, w-8, "…")))
	}
	if len(items) > shown {
		out = append(out, "      "+faint(fmt.Sprintf("… %d more", len(items)-shown)))
	}
	return out
}

func (s *deleteSheet) body(m *Model, w, h int) []string {
	s.adopt()
	lines, size, losing, blocked := s.lines(m, w)
	about := fmt.Sprintf("%d thing%s · frees %s", len(s.items), plural(len(s.items)), disk(size))
	out := []string{sheetTitle("Delete", about, w)}
	if losing > 0 {
		out = append(out, paint(cRed+bold, fmt.Sprintf("✗ %d would lose work that exists nowhere else · this can't be undone", losing)))
	}
	out = append(out, "")
	room := max(1, h-len(out)-3)
	s.scroll = max(0, min(s.scroll, len(lines)-room))
	out = append(out, lines[s.scroll:min(len(lines), s.scroll+room)]...)
	if len(lines) > room {
		out = append(out, faint(fmt.Sprintf("  ↑↓ %d more lines", len(lines)-room)))
	}
	out = append(out, "")
	switch {
	case blocked:
		return append(out, keysFit(w, "↑↓", "scroll", "esc", "back"))
	case s.read != nil:
		return append(out, keysFit(w, "↑↓", "scroll", "esc", "cancel"))
	case losing > 0:
		return append(out, keysFit(w, "↑↓", "scroll", "y", "delete, losing it", "esc", "cancel"))
	}
	return append(out, keysFit(w, "↑↓", "scroll", "y", "delete", "esc", "cancel"))
}

func (s *deleteSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	s.adopt()
	switch k {
	case "esc", "ctrl+c", "q", "n":
		m.sheet = s.back
	case "up", "k":
		s.scroll = max(0, s.scroll-1)
	case "down", "j":
		s.scroll++
	case "pgup":
		s.scroll = max(0, s.scroll-10)
	case "pgdown":
		s.scroll += 10
	case "y", "enter":
		return s.remove(m)
	}
	return nil
}

// remove removes everything the sheet lists, as it was looked at: a worktree
// found safe is checked with git once more, one that isn't is forced.
func (s *deleteSheet) remove(m *Model) tea.Cmd {
	if s.read != nil {
		m.flash("still looking · a moment", false)
		return nil
	}
	for i := range s.items {
		if why := m.blocked(&s.items[i]); why != "" {
			m.flash(why+" · stop it or mark it done first", true)
			return nil
		}
	}
	m.sheet = nil
	var cmds []tea.Cmd
	for i := range s.items {
		switch d := &s.items[i]; {
		case d.wt != nil:
			cmds = append(cmds, m.removeWorktree(*d.wt, !d.wt.Safe()))
		case d.left != nil:
			cmds = append(cmds, m.removeLeft(*d.left))
		case d.tmp:
			cmds = append(cmds, m.clearScratch())
		default:
			cmds = append(cmds, m.cleanTemp(d.agents))
		}
	}
	return tea.Batch(cmds...)
}

// Row kinds on the Projects page and the Delete sheet, each with its own
// mark and colour.
const (
	kindProject = iota
	kindFolder
	kindWorktree
	kindTemp
)

// kindMark is a kind's mark, in its colour.
func kindMark(kind int) string {
	switch kind {
	case kindProject:
		return paint(cOrange, "◆")
	case kindFolder:
		return paint(cSub, "◇")
	case kindWorktree:
		return paint(cBlue, "⎇")
	}
	return paint(cYellow, "◌")
}

// kindLine heads an item on the Delete sheet: its mark and name, what it
// is and how it goes, and its size at the right.
func kindLine(kind int, name, what, size string, w int) string {
	head := kindMark(kind) + " " + paint(cText+bold, name) + "  " + dim(what)
	return fit(head, w-8) + right(size, 8)
}
