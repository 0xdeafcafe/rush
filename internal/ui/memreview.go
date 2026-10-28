package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/memory"
)

// The memory review: a project's notes worth a look, one at a time, each
// with why (untidy, or about work that may have moved on), its text, and
// what to do with it: keep it as it is, edit it, merge it into another,
// move it into CLAUDE.md for every project, or forget it.

// memReview is a project's notes to review, read off the UI.
type memReview struct {
	p     agent.Profile
	title string
	dir   string
	items []memReviewItem
	notes []agent.MemoryFile // every note, to merge into
}

type memReviewItem struct {
	f    agent.MemoryFile
	why  string
	text string
	same string // the note it repeats, to merge into
}

// readMemReview reads the notes of p's project in cwd worth a look.
func readMemReview(p agent.Profile, cwd, title string) (memReview, error) {
	mr, ok := agent.As[agent.MemoryReader](p.Kind)
	if !ok {
		return memReview{}, fmt.Errorf("%s keeps no memory rush can read", harnessName(string(p.Kind)))
	}
	mk, _ := agent.As[agent.MemoryKeeper](p.Kind)
	mem := mr.Memory(p, cwd, "")
	r := memReview{p: p, title: title, dir: mem.Dir}
	now := time.Now()
	for _, f := range mem.Files {
		if f.Group != "Auto memory" || f.Name == "MEMORY.md" || f.Missing {
			continue
		}
		r.notes = append(r.notes, f)
		why := memory.Why(f, now)
		if why == "" {
			continue
		}
		it := memReviewItem{f: f, why: why, same: f.Same}
		if mk != nil {
			if n, err := mk.ReadNote(f.Path); err == nil {
				it.text = n.Body
			}
		}
		r.items = append(r.items, it)
	}
	return r, nil
}

// openMemReview reads p's project in cwd and opens its review.
func (m *Model) openMemReview(p agent.Profile, cwd, title string) tea.Cmd {
	m.flash("reading "+title+"'s memory…", false)
	return sheetDo(func() (memReview, error) { return readMemReview(p, cwd, title) },
		func(m *Model, r memReview, err error) tea.Cmd {
			switch {
			case err != nil:
				m.flash(err.Error(), true)
			case len(r.items) == 0:
				m.flash(fmt.Sprintf("nothing in %s's memory to review · %d notes", title, len(r.notes)), false)
			case m.sheet != nil:
				m.flash(fmt.Sprintf("%d of %s's notes worth a look · close this to review them", len(r.items), title), false)
			default:
				m.sheet = &memReviewSheet{r: r, done: map[string]string{}}
			}
			return nil
		})
}

// memReviewSheet goes through a review's notes, one at a time.
type memReviewSheet struct {
	r    memReview
	i    int
	done map[string]string // what was done to each, by path
	// merging picks the note to merge the one under review into.
	merging bool
	pick    int
	busy    bool
	err     string
}

func (s *memReviewSheet) width(*Model) int { return 100 }

func (s *memReviewSheet) cur() *memReviewItem {
	if s.i < len(s.r.items) {
		return &s.r.items[s.i]
	}
	return nil
}

// targets are the notes the one under review can merge into.
func (s *memReviewSheet) targets() []agent.MemoryFile {
	it := s.cur()
	var out []agent.MemoryFile
	for _, f := range s.r.notes {
		if it != nil && f.Path != it.f.Path && !s.gone(f.Path) {
			out = append(out, f)
		}
	}
	return out
}

func (s *memReviewSheet) body(m *Model, w, h int) []string {
	it := s.cur()
	if it == nil {
		out := []string{sheetTitle("Memory review", s.r.title+" · done", w), ""}
		counts := map[string]int{}
		for _, d := range s.done {
			counts[d]++
		}
		for _, d := range []string{"kept", "edited", "merged", "moved to CLAUDE.md", "forgotten"} {
			if counts[d] > 0 {
				out = append(out, "  "+paint(cText, fmt.Sprintf("%d %s", counts[d], d)))
			}
		}
		if len(s.done) == 0 {
			out = append(out, "  "+dim("nothing changed"))
		}
		return append(out, "", keysFit(w, "esc", "close"))
	}
	out := []string{sheetTitle("Memory review", fmt.Sprintf("%s · %d of %d", s.r.title, s.i+1, len(s.r.items)), w), ""}
	f := it.f
	about := firstNonEmpty(f.About, f.Name)
	for _, l := range wrap(about, w-4) {
		out = append(out, "  "+paint(cText+bold, l))
	}
	meta := []string{paint(memTypeColour(f.Kind), firstNonEmpty(f.Kind, "no type")), f.Name}
	if f.Reads > 0 {
		meta = append(meta, fmt.Sprintf("opened %d× · last %s", f.Reads, f.LastRead.Format("2 Jan")))
	}
	if !f.Mod.IsZero() {
		meta = append(meta, "changed "+dur(time.Since(f.Mod))+" ago")
	}
	out = append(out, "  "+dim(strings.Join(meta, " · ")), "  "+paint(cYellow, "! "+it.why), "")
	if s.merging {
		out = append(out, "  "+paint(cText, "Merge it into which note?")+dim(" its text goes below that note's, and this one goes"), "")
		ts := s.targets()
		top := max(0, min(s.pick-(h-14)/2, len(ts)-(h-12)))
		for j := top; j < len(ts) && len(out) < h-3; j++ {
			out = append(out, sheetRow(paint(cText, firstNonEmpty(ts[j].About, ts[j].Name))+"  "+dim(ts[j].Name), j == s.pick, w))
		}
		return append(out, "", keysFit(w, "↑↓", "pick", "enter", "merge", "esc", "back"))
	}
	text := wrap(it.text, w-6)
	room := max(3, h-len(out)-4)
	if len(text) > room {
		text = append(text[:room-1], "…")
	}
	for _, l := range text {
		out = append(out, "    "+dim(l))
	}
	if s.err != "" {
		out = append(out, "", "  "+paint(cRed, s.err))
	}
	out = append(out, "")
	if s.busy {
		return append(out, paint(cYellow, "⋯"))
	}
	pairs := []string{"k", "keep", "e", "edit", "m", "merge", "g", "to CLAUDE.md", "x", "forget", "→", "skip", "esc", "stop"}
	if it.same != "" && !s.gone(it.same) {
		pairs[5] = "merge into " + strings.TrimSuffix(filepath.Base(it.same), ".md")
	}
	return append(out, keysFit(w, pairs...))
}

func (s *memReviewSheet) key(m *Model, k tea.KeyPressMsg, str string) tea.Cmd {
	if s.busy {
		return nil
	}
	it := s.cur()
	if it == nil {
		if str == "esc" || str == "enter" || str == "q" || str == "ctrl+c" {
			m.sheet = nil
		}
		return nil
	}
	if s.merging {
		ts := s.targets()
		switch str {
		case "up", "k":
			s.pick = max(0, s.pick-1)
		case "down", "j":
			s.pick = max(0, min(len(ts)-1, s.pick+1))
		case "enter":
			if s.pick >= 0 && s.pick < len(ts) {
				return s.merge(m, ts[s.pick].Path)
			}
		case "esc":
			s.merging = false
		}
		return nil
	}
	f := it.f
	mk, _ := agent.As[agent.MemoryKeeper](s.r.p.Kind)
	switch str {
	case "esc", "q", "ctrl+c":
		m.sheet = nil
		m.dropMemCache()
	case "right", "n", "tab", "down":
		s.next()
	case "left", "shift+tab", "up":
		s.back()
	case "k", "enter":
		return s.do(m, "kept", func() error { return memory.Keep(f.Path) })
	case "e":
		path := f.Path
		return editorCmd(func() (string, error) { return path, nil }, func(err error) tea.Msg {
			return sheetMsg{apply: func(m *Model) tea.Cmd {
				if err != nil {
					s.err = err.Error()
					return nil
				}
				s.done[path] = "edited"
				s.next()
				return nil
			}}
		})
	case "m":
		if mk == nil {
			return nil
		}
		if it.same != "" && !s.gone(it.same) {
			return s.merge(m, it.same)
		}
		if len(s.targets()) == 0 {
			m.flash("no other note to merge it into", false)
			return nil
		}
		s.merging, s.pick = true, 0
	case "g":
		if mk == nil {
			return nil
		}
		p := s.r.p
		m.confirm = &confirmation{
			question: "Move it into your CLAUDE.md?",
			detail:   "every session in every project loads it then, not just this project's · the note goes",
			onYes:    func() tea.Cmd { return s.do(m, "moved to CLAUDE.md", func() error { return mk.MakeGlobal(p, f.Path) }) },
		}
	case "x", "delete":
		forget := os.Remove
		if mr, ok := agent.As[agent.MemoryReader](s.r.p.Kind); ok {
			forget = mr.ForgetFile
		}
		m.confirm = &confirmation{
			question: "Forget " + f.Name + "?",
			detail:   "its file is deleted, and its line in MEMORY.md",
			onYes:    func() tea.Cmd { return s.do(m, "forgotten", func() error { return forget(f.Path) }) },
		}
	}
	return nil
}

// merge merges the note under review into the one at into.
func (s *memReviewSheet) merge(m *Model, into string) tea.Cmd {
	mk, ok := agent.As[agent.MemoryKeeper](s.r.p.Kind)
	if !ok {
		return nil
	}
	from := s.cur().f.Path
	s.merging = false
	return s.do(m, "merged", func() error { return mk.MergeNotes(into, from) })
}

// do runs f on the note under review off the UI, then moves on.
func (s *memReviewSheet) do(m *Model, what string, f func() error) tea.Cmd {
	s.busy, s.err = true, ""
	path := s.cur().f.Path
	return sheetDo(func() (struct{}, error) { return struct{}{}, f() }, func(m *Model, _ struct{}, err error) tea.Cmd {
		s.busy = false
		if err != nil {
			s.err = err.Error()
			return nil
		}
		s.done[path] = what
		s.next()
		return nil
	})
}

func (s *memReviewSheet) next() {
	s.i++
	for s.i < len(s.r.items) && s.gone(s.r.items[s.i].f.Path) {
		s.i++
	}
}

// back goes to the note before that's still there, if there is one.
func (s *memReviewSheet) back() {
	for j := s.i - 1; j >= 0; j-- {
		if !s.gone(s.r.items[j].f.Path) {
			s.i = j
			return
		}
	}
}

// gone is whether the review forgot, merged or moved the note at path.
func (s *memReviewSheet) gone(path string) bool {
	d := s.done[path]
	return d == "forgotten" || d == "merged" || d == "moved to CLAUDE.md"
}

// dropMemCache has the memory view and Projects read memory again.
func (m *Model) dropMemCache() {
	if m.host != nil {
		m.host.mem = nil
	}
	m.memCounts = nil
}
