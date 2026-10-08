package ui

import (
	"fmt"
	"path/filepath"
	"sort"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// --- Clean up: haven clean, in Projects ---

// cleanSheet is every worktree the agents' repositories have, and what's
// left in /tmp, as a checklist: what can go without losing anything comes
// ticked, anything else can be ticked by hand, and one confirmation takes
// the lot. Nothing goes until you say so.
type cleanSheet struct {
	items []cleanItem
	cur   int
}

// cleanItem is a worktree, something an agent left outside its project,
// or /tmp's untouched scratch when both are nil.
type cleanItem struct {
	wt   *fleet.Worktree
	left *fleet.LeftItem
	on   bool
	busy string // an agent still running in it: it can't be ticked
}

func (m *Model) openCleanSheet() tea.Cmd {
	c := &m.clean
	if c.checked.IsZero() {
		m.flash("still looking at worktrees with git · a moment", false)
		return m.scanWorktrees()
	}
	s := &cleanSheet{}
	for i := range c.wts {
		w := &c.wts[i]
		it := cleanItem{wt: w, on: w.Safe()}
		if a := m.running(w.Agents); a != nil {
			it.busy, it.on = firstNonEmpty(oneLine(a.DisplayName), "an agent"), false
		}
		s.items = append(s.items, it)
	}
	// Ticked first, the biggest of them on top; then the rest by size.
	sort.SliceStable(s.items, func(i, j int) bool {
		a, b := s.items[i], s.items[j]
		if a.on != b.on {
			return a.on
		}
		return a.wt.Size > b.wt.Size
	})
	// What agents left outside their projects: ticked only when it's
	// scratch in a temp folder nothing has touched since; the rest you may
	// tick, unless it's held back (there before, changed since, git's).
	for i := range c.outside {
		l := &c.outside[i]
		if l.Gone {
			continue
		}
		it := cleanItem{left: l, on: l.Safe(), busy: l.Why()}
		if b := m.agentByKey(l.Agent.Key); b != nil && b.PID != 0 {
			it.busy, it.on = firstNonEmpty(oneLine(b.DisplayName), "an agent")+" is running", false
		}
		s.items = append(s.items, it)
	}
	if c.tmp.StaleItems > 0 {
		s.items = append(s.items, cleanItem{on: true})
	}
	if len(s.items) == 0 {
		m.flash("no worktrees, nothing left outside projects and nothing stale in /tmp · nothing to clean", false)
		return nil
	}
	m.sheet = s
	return nil
}

func (s *cleanSheet) width(m *Model) int { return 130 }

// ticked is how much the ticked items free, and how many would lose work.
func (s *cleanSheet) ticked(m *Model) (n, losing int, size int64) {
	for _, it := range s.items {
		if !it.on {
			continue
		}
		n++
		if it.left != nil {
			size += it.left.Size
			continue
		}
		if it.wt == nil {
			size += m.clean.tmp.Stale
			continue
		}
		size += it.wt.Size
		if !it.wt.Safe() {
			losing++
		}
	}
	return n, losing, size
}

func (s *cleanSheet) body(m *Model, w, h int) []string {
	n, losing, size := s.ticked(m)
	about := fmt.Sprintf("%d ticked · %s", n, disk(size))
	if losing > 0 {
		about += fmt.Sprintf(" · %d would lose work", losing)
	}
	out := []string{sheetTitle("Clean up", about, w), ""}
	from, to := window(len(s.items), s.cur, max(1, h-6))
	for i := from; i < to; i++ {
		out = append(out, sheetRow(s.itemLine(m, s.items[i], w-2), i == s.cur, w))
	}
	return append(out, "", dim("branches stay · ticked by default: nothing uncommitted, every commit pushed or in main, no agent running"), "",
		keysFit(w, "↑↓", "move", "space", "tick", "a", "all safe / none", "enter", "remove ticked", "esc", "close"))
}

func (s *cleanSheet) itemLine(m *Model, it cleanItem, w int) string {
	box := faint("[ ] ")
	if it.on {
		box = paint(cGreen, "[x] ")
	}
	if l := it.left; l != nil {
		why := paint(cGreen, "scratch, untouched since")
		switch {
		case it.busy != "":
			box, why = faint(" ·  "), dim(it.busy)
		case !l.Temp:
			why = paint(cYellow, "outside temp: may be meant to stay")
		}
		if l.Shell {
			why += faint(" · from a shell command")
		}
		return fit(box+paint(cText, fmt.Sprintf("%-44s", ansi.Truncate(tildify(l.Path), 43, "…")))+faint(fmt.Sprintf("%7s  ", disk(l.Size)))+
			dim(oneLine(l.Agent.DisplayName)+" · ")+why, w)
	}
	if it.wt == nil {
		t := m.clean.tmp
		return fit(box+paint(cText, fmt.Sprintf("%-28s", "/tmp"))+dim(fmt.Sprintf("%-16s", ""))+faint(fmt.Sprintf("%7s  ", disk(t.Stale)))+
			paint(cGreen, fmt.Sprintf("%d things untouched for a day", t.StaleItems)), w)
	}
	wt := it.wt
	name := ansi.Truncate(filepath.Base(wt.Path), 27, "…")
	repo := ansi.Truncate(filepath.Base(wt.Repo), 15, "…")
	var why string
	switch {
	case it.busy != "":
		box, why = faint(" ·  "), dim(it.busy+" is running in it")
	case wt.Err != "":
		why = paint(cRed, wt.Err)
	case !wt.Safe():
		why = paint(cYellow, "would lose "+wt.Losses())
	case wt.Pushed():
		why = paint(cGreen, "merged or pushed")
	default:
		why = paint(cGreen, "in local main, not pushed")
	}
	return fit(box+paint(cText, fmt.Sprintf("%-28s", name))+dim(fmt.Sprintf("%-16s", repo))+faint(fmt.Sprintf("%7s  ", disk(wt.Size)))+why, w)
}

func (s *cleanSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	switch k {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "up", "k", "shift+tab":
		s.cur = pickerMove(s.cur, len(s.items), "up")
	case "down", "j", "tab":
		s.cur = pickerMove(s.cur, len(s.items), "down")
	case "space", " ", "x":
		if it := &s.items[s.cur]; it.busy == "" {
			it.on = !it.on
		}
	case "a":
		// All that's safe, or none when that's what's ticked already.
		all := true
		for _, it := range s.items {
			all = all && it.on == s.safe(it)
		}
		for i := range s.items {
			s.items[i].on = !all && s.safe(s.items[i])
		}
	case "enter":
		return s.confirm(m)
	}
	return nil
}

func (s *cleanSheet) safe(it cleanItem) bool {
	if it.left != nil {
		return it.busy == "" && it.left.Safe()
	}
	return it.busy == "" && (it.wt == nil || it.wt.Safe())
}

// confirm opens Delete on everything ticked, which says what each takes;
// esc there comes back here.
func (s *cleanSheet) confirm(m *Model) tea.Cmd {
	var picked []doomed
	for _, it := range s.items {
		if it.on {
			if it.left != nil {
				picked = append(picked, doomed{left: it.left, agents: []*fleet.Agent{it.left.Agent}})
				continue
			}
			picked = append(picked, doomed{wt: it.wt, tmp: it.wt == nil})
		}
	}
	if len(picked) == 0 {
		m.flash("nothing ticked · space ticks one, a ticks all that's safe", false)
		return nil
	}
	return m.openDelete(s, picked...)
}

// nudgeClean says, once a day at most, when there's a lot that could go:
// the prompt to open Clean up.
// ponytail: remembered only while rush runs; a restart may say it again.
func (m *Model) nudgeClean() {
	c := &m.clean
	if time.Since(c.nudged) < 24*time.Hour || m.mode == modeProjects {
		return
	}
	var n int
	var size int64
	for i := range c.wts {
		if w := &c.wts[i]; w.Safe() && m.running(w.Agents) == nil {
			n++
			size += w.Size
		}
	}
	size += c.tmp.Stale
	if n < 5 && size < 2<<30 {
		return
	}
	c.nudged = time.Now()
	m.flash(fmt.Sprintf("%s could go without losing anything: %d worktrees merged and clean, %d things in /tmp · Projects, c cleans up", disk(size), n, c.tmp.StaleItems), false)
}
