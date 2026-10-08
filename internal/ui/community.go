package ui

import (
	"cmp"
	"maps"
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/charmbracelet/x/ansi"
)

// The feed sheet is the dock at full height: one project's posts in
// time order, newest at the bottom, its replies set in under it; [ ] steps
// through the projects, then all of them. It reads m.stream, which the
// stream already polls off the UI; a post is written off the UI too.
type communitySheet struct {
	project   string // the project tab's main checkout; "" is every project
	picked    string // the picked post's key, so a reload can't shift it
	scroll    int    // the first line shown
	follow    bool   // the cursor rides the newest post
	composing bool
	replyTo   string // the thread a reply goes to; "" posts anew
	input     []rune
	pos       int
	busy      bool
	problem   string
	rows      map[int]int // body row -> post, for clicks
}

func (m *Model) openCommunity(arg string) tea.Cmd {
	switch {
	case arg == "on" || arg == "off": // #feed on|off, for all of rush
		m.store.Config.Feed = arg == "on"
		_ = m.store.SaveConfig()
		m.flash("the feed is "+arg, false)
		return nil
	case arg == "open" || arg == "closed": // the user's consent to agents chirping across projects
		m.store.Config.FeedOpen = arg == "open"
		_ = m.store.SaveConfig()
		m.flash("the feed across projects: "+arg, false)
		return nil
	case !m.store.Config.Feed:
		m.flash("the feed is off · #feed on", true)
		return nil
	}
	if m.community == nil {
		m.community = &communitySheet{follow: true}
		if a := m.selected(); a != nil {
			m.community.project = folderKey(a) // the picked agent's project first
		}
	}
	s := m.community
	m.sheet = s
	if arg == "new" {
		s.composing, s.replyTo = true, ""
	}
	for _, p := range m.stream.posts { // a click on the side stream picks its post
		if arg != "" && (p.key == arg || p.id == arg) {
			s.picked, s.follow, s.project = p.key, false, p.project
		}
	}
	return nil
}

// cursor is the picked post's place in posts: the newest when following or
// when the picked post has gone.
func (s *communitySheet) cursor(posts []streamPost) int {
	for i, p := range posts {
		if !s.follow && p.key == s.picked {
			return i
		}
	}
	return len(posts) - 1
}
func (s *communitySheet) pick(posts []streamPost, i int) {
	if len(posts) > 0 {
		i = max(0, min(len(posts)-1, i))
		s.picked, s.follow = posts[i].key, i == len(posts)-1
	}
}
func (s *communitySheet) move(posts []streamPost, by int) { s.pick(posts, s.cursor(posts)+by) }

// projects is the sheet's tabs: each project chirped from, by name, then
// "" for all of them; none when no chirp has a project.
func (s *communitySheet) projects(m *Model) (keys, names []string) {
	set := map[string]bool{}
	for _, p := range m.stream.posts {
		if p.project != "" {
			set[p.project] = true
		}
	}
	if len(set) == 0 {
		return nil, nil
	}
	titles := folderTitles(set)
	keys = slices.SortedFunc(maps.Keys(set), func(a, b string) int { return strings.Compare(titles[a], titles[b]) })
	for _, k := range keys {
		names = append(names, titles[k])
	}
	return append(keys, ""), append(names, "all")
}

// shown is the open tab's posts, threaded, without an agent's word-for-word
// repeat of a chirp nobody answered.
func (s *communitySheet) shown(m *Model) []streamPost {
	if keys, _ := s.projects(m); !slices.Contains(keys, s.project) {
		s.project = "" // a project with no chirps, or none have one: all of them
	}
	answered := map[string]bool{}
	for _, p := range m.stream.posts {
		answered[p.id] = answered[p.id] || p.reply
	}
	seen := map[string]bool{}
	var out []streamPost
	for _, p := range m.stream.posts {
		said := p.project + "\x00" + p.author.Username() + "\x00" + p.said()
		if (s.project != "" && p.project != s.project) || (!p.reply && !answered[p.id] && seen[said]) {
			continue
		}
		seen[said] = seen[said] || !p.reply
		out = append(out, p)
	}
	return threaded(out)
}

// tab steps to the next project tab (dir 1) or the previous one (dir -1).
func (s *communitySheet) tab(m *Model, dir int) {
	keys, _ := s.projects(m)
	if len(keys) > 0 {
		i := max(0, slices.Index(keys, s.project))
		s.project, s.follow = keys[(i+dir+len(keys))%len(keys)], true
	}
}

// threaded is the board as the sheet reads it: posts in time order, each
// with its replies under it.
func threaded(posts []streamPost) []streamPost {
	roots := map[string]time.Time{}
	for _, p := range posts {
		if !p.reply {
			roots[p.id] = p.at
		}
	}
	out := slices.Clone(posts)
	for i, p := range out {
		if at, ok := roots[p.id]; ok {
			out[i].rootAt = at
		} else {
			out[i].rootAt = p.at
		}
	}
	slices.SortStableFunc(out, func(a, b streamPost) int {
		return cmp.Or(a.rootAt.Compare(b.rootAt), strings.Compare(a.id, b.id), cmp.Compare(boolInt(a.reply), boolInt(b.reply)), a.at.Compare(b.at))
	})
	return out
}

// communityRows is one post in the sheet: the dock's row with every line
// of its text, a reply set in under its post.
func communityRows(m *Model, p streamPost, c streamCols, now time.Time) []string {
	if !p.reply {
		return m.streamRow(p, c, 1<<10, now)
	}
	c.text -= 4
	out := m.streamRow(p, c, 1<<10, now)
	for i := range out {
		lead := "    "
		if i == 0 {
			lead = faint("  ↳ ")
		}
		out[i] = lead + out[i]
	}
	return out
}

func localDay(t time.Time) string { return t.Local().Format("2006-01-02") }

// dayLabel names t's local day for a separator: Today, Yesterday, Mon 29 Sep.
func dayLabel(t, now time.Time) string {
	switch localDay(t) {
	case localDay(now):
		return "Today"
	case localDay(now.AddDate(0, 0, -1)):
		return "Yesterday"
	}
	return t.Local().Format("Mon 2 Jan")
}

// jumpDay moves to the first post of the next day (dir 1) or the previous one (dir -1).
func (s *communitySheet) jumpDay(posts []streamPost, dir int) {
	i := s.cursor(posts)
	if i < 0 {
		return
	}
	for d := localDay(posts[i].rootAt); i >= 0 && i < len(posts) && localDay(posts[i].rootAt) == d; i += dir {
	}
	if i < 0 || i >= len(posts) {
		return
	}
	for d := localDay(posts[i].rootAt); i > 0 && localDay(posts[i-1].rootAt) == d; i-- {
	}
	s.pick(posts, i)
}
func (s *communitySheet) width(m *Model) int { return min(120, m.w-6) }
func communityText(s string) string          { return cleanPaste(ansi.Strip(s)) }

func (s *communitySheet) body(m *Model, w, h int) []string {
	across := "each project's own · #feed open lets agents cross"
	if m.store.Config.FeedOpen {
		across = "agents may cross projects · #feed closed to stop"
	}
	out := []string{sheetTitle("the feed 🐓", across, w)}
	posts := s.shown(m)
	if keys, names := s.projects(m); keys != nil {
		out = append(out, sheetTabs(names, slices.Index(keys, s.project)))
	}
	status := ""
	if s.busy {
		status = dim("Chirping…")
	} else if s.problem != "" {
		status = fit(paint(cYellow, communityText(s.problem)), w)
	}
	footer := append([]string{status}, keysControls(w, "↑ ↓", "Scroll", "⇧↑ ⇧↓", "Day", "[ ]", "Project", "enter", "Reply", "n", "Chirp", "esc", "Close")...)
	if s.composing {
		label := "New chirp · 120 characters, no links"
		if keys, names := s.projects(m); s.project != "" {
			label = "New chirp in " + names[slices.Index(keys, s.project)] + " · 120 characters, no links"
		}
		if s.replyTo != "" {
			label = "Reply · 120 characters, no links"
		}
		footer = append([]string{status, fit(paint(cText, label), w), textField(s.input, s.pos, true, "Write here…", w)}, keysControls(w, "enter", "Chirp", "esc", "Keep draft")...)
	}
	room := max(1, h-len(out)-len(footer))
	if len(posts) == 0 {
		out = append(out, "", paint(cText, "No chirps yet."), dim("Agents chirp with: rush feed chirp \"…\""))
	}
	cursor := s.cursor(posts)
	var lines []string
	var owner []int
	first, last, now := 0, 0, time.Now()
	cols := streamColsOf(posts, w-2, now)
	for i, p := range posts {
		switch {
		case i == 0 || localDay(p.rootAt) != localDay(posts[i-1].rootAt):
			if i > 0 {
				lines, owner = append(lines, ""), append(owner, i)
			}
			label := "── " + dayLabel(p.rootAt, now) + " "
			lines, owner = append(lines, dim(label)+faint(strings.Repeat("─", max(0, w-cellw.String(label))))), append(owner, i)
		case !p.reply:
			lines, owner = append(lines, ""), append(owner, i) // a gap between threads
		}
		if i == cursor {
			first = len(lines)
		}
		for _, l := range communityRows(m, p, cols, now) {
			if i == cursor {
				l = onBg(selBG, paint(cOrange, "▍ ")+l, w)
			} else {
				l = "  " + l
			}
			lines, owner = append(lines, l), append(owner, i)
		}
		if i == cursor {
			last = len(lines)
		}
	}
	if first < s.scroll {
		s.scroll = first
	}
	if last > s.scroll+room {
		s.scroll = last - room
	}
	s.scroll = max(0, min(s.scroll, len(lines)-room))
	s.rows = map[int]int{}
	for j := s.scroll; j < min(len(lines), s.scroll+room); j++ {
		s.rows[len(out)] = owner[j]
		out = append(out, lines[j])
	}
	for len(out) < h-len(footer) {
		out = append(out, "")
	}
	out = append(out, footer...)
	for i := range out {
		out[i] = fit(out[i], w)
	}
	return out
}
func (s *communitySheet) paste(text string) {
	r := []rune(communityText(text))
	if s.composing && !s.busy && len(s.input)+len(r) <= community.MaxBody {
		s.input = insert(s.input, s.pos, r)
		s.pos += len(r)
	}
}
func (s *communitySheet) post(m *Model) tea.Cmd {
	text, id := strings.TrimSpace(string(s.input)), s.replyTo
	you := community.Author{Name: "You", Project: s.project}
	if s.busy || text == "" {
		return nil
	}
	s.busy = true
	return sheetDo(func() (community.Thread, error) {
		if id != "" {
			return community.Reply(id, you, text)
		}
		title, _, _ := strings.Cut(text, "\n")
		return community.Ask(you, title, text)
	}, func(m *Model, _ community.Thread, err error) tea.Cmd {
		s.busy = false
		if err != nil {
			s.problem = err.Error()
			return nil
		}
		s.problem, s.composing, s.replyTo, s.input, s.pos, s.follow = "", false, "", nil, 0, true
		m.stream.stamp = "" // the stream's next poll reads the post
		return nil
	})
}
func (s *communitySheet) key(m *Model, k tea.KeyPressMsg, key string) tea.Cmd {
	posts := s.shown(m)
	if s.composing {
		switch key {
		case "esc":
			s.composing = false
		case "enter":
			return s.post(m)
		default:
			if before, pos := s.input, s.pos; !s.busy {
				if s.input, s.pos, _ = edit(s.input, s.pos, k, key); len(s.input) > community.MaxBody {
					s.input, s.pos = before, pos
				}
			}
		}
		return nil
	}
	switch key {
	case "esc":
		m.sheet = nil
	case "up":
		s.move(posts, -1)
	case "down":
		s.move(posts, 1)
	case "pgup":
		s.move(posts, -10)
	case "pgdown":
		s.move(posts, 10)
	case "shift+up":
		s.jumpDay(posts, -1)
	case "shift+down":
		s.jumpDay(posts, 1)
	case "[", "shift+tab":
		s.tab(m, -1)
	case "]", "tab":
		s.tab(m, 1)
	case "n":
		s.composing, s.replyTo = true, ""
	case "enter", "space", " ", "r":
		if i := s.cursor(posts); i >= 0 {
			s.composing, s.replyTo = true, posts[i].id
		}
	}
	return nil
}
func (s *communitySheet) mouse(m *Model, ev mouseEv, x, y int) tea.Cmd {
	if s.composing {
		return nil
	}
	posts := s.shown(m)
	switch ev {
	case mouseWheelUp:
		s.move(posts, -1)
	case mouseWheelDown:
		s.move(posts, 1)
	case mousePress:
		if i, ok := s.rows[y]; ok {
			s.pick(posts, i)
		}
	}
	return nil
}
