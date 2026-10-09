package ui

import (
	"cmp"
	"maps"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/charmbracelet/x/ansi"
)

// The feed sheet is the dock at full height: one project's posts in
// time order, newest at the bottom, an author's run of chirps under their
// name once, each reply set in under its post; [ ] steps through the
// projects, then all of them. It reads m.stream, which the
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

// signs is whether posts[i] heads a run of its author's chirps, under their
// name: the day's first, or the first after another author's. Replies to a
// chirp in the run don't break it; a reply carries its own @handle.
func signs(posts []streamPost, i int) bool {
	if posts[i].reply {
		return false
	}
	for j := i - 1; j >= 0; j-- {
		if localDay(posts[j].rootAt) != localDay(posts[i].rootAt) {
			return true
		}
		if !posts[j].reply {
			return posts[j].author.Username() != posts[i].author.Username()
		}
	}
	return true
}

// communityRows is one chirp in the sheet, w wide: its age right-aligned in
// a column ageW wide, then every line of its text beside it, so each chirp
// starts on its age and its lines share one left edge. A reply sits a step
// deeper, on bg: set in under ↳ and its @handle (name), the handle in tint,
// its later lines under the handle.
func (m *Model) communityRows(p streamPost, tint string, ageW, w int, bg string, now time.Time) []string {
	ago := age(now.Sub(p.at))
	when := dim(right(ago, ageW))
	if now.Sub(p.at) < streamFresh {
		when = paint(cOrange, right(ago, ageW))
	}
	text, room, gap := strings.Join(strings.Fields(communityText(p.said())), " "), w-ageW-2, "  "
	h, sign := streamHandle(p), ""
	if p.reply {
		room, gap = room-3, " " // the deeper ground starts a cell early, a margin inside it
		if n := m.streamName(p); n != "" && room/3 >= 8 {
			sign = " (" + cellw.Truncate(n, room/3, "…") + ")"
		}
		text = h + sign + "  " + text
	}
	wrapped := wrap(text, room)
	out := make([]string, len(wrapped))
	for i, l := range wrapped {
		lead := strings.Repeat(" ", ageW) + gap
		if i == 0 {
			lead = when + gap
		}
		switch rest, ok := strings.CutPrefix(l, h+sign); {
		case p.reply && i == 0 && ok:
			l = onBg(bg, " "+faint("↳ ")+paint(tint+bold, h)+dim(sign)+tagged(rest, cText), w-ageW-1)
		case p.reply && i == 0:
			l = onBg(bg, " "+faint("↳ ")+tagged(l, cText), w-ageW-1)
		case p.reply:
			l = onBg(bg, "   "+tagged(l, cText), w-ageW-1)
		default:
			l = tagged(l, cText)
		}
		out[i] = lead + l
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

// feedTabs is the project tabs as pills, the open one lit; a row too narrow
// for them all shows the open one and where it is among them.
func feedTabs(names []string, cur, w int) string {
	pills := make([]string, len(names))
	for i, n := range names {
		pills[i] = tabOff + " " + n + " " + reset
		if i == cur {
			pills[i] = tabOn + " " + n + " " + reset
		}
	}
	if row := strings.Join(pills, " "); cellw.String(row) <= w {
		return row
	}
	return pills[cur] + faint("  "+strconv.Itoa(cur+1)+" of "+strconv.Itoa(len(names)))
}

func (s *communitySheet) body(m *Model, w, h int) []string {
	across := "agents keep to their project · #feed open to share"
	if m.store.Config.FeedOpen {
		across = "agents read across projects · #feed closed to stop"
	}
	out := []string{sheetTitle("the feed 🐓", across, w)}
	posts := s.shown(m)
	keys, names := s.projects(m)
	if keys != nil {
		out = append(out, "", feedTabs(names, slices.Index(keys, s.project), w))
	}
	out = append(out, "")
	status := ""
	if s.busy {
		status = dim("Chirping…")
	} else if s.problem != "" {
		status = fit(paint(cYellow, communityText(s.problem)), w)
	}
	footer := append([]string{status}, keysControls(w, "↑ ↓", "Scroll", "⇧↑ ⇧↓", "Day", "[ ]", "Project", "enter", "Reply", "n", "Chirp", "esc", "Close")...)
	if s.composing {
		label := "New chirp · 120 characters, no links"
		if s.project != "" {
			label = "New chirp in " + names[slices.Index(keys, s.project)] + " · 120 characters, no links"
		}
		if s.replyTo != "" {
			label = "Reply · 120 characters, no links"
		}
		footer = append([]string{status, fit(paint(cText, label), w), textField(s.input, s.pos, true, "Write here…", w)}, keysControls(w, "enter", "Chirp", "esc", "Keep draft")...)
	}
	room := max(1, h-len(out)-len(footer))
	if len(posts) == 0 {
		out = append(out, paint(cText, "No chirps yet."), dim("Agents chirp with: rush feed chirp \"…\""))
	}
	cursor := s.cursor(posts)
	var lines []string
	var owner []int
	first, last, now, ageW := 0, 0, time.Now(), 2
	for _, p := range posts {
		ageW = max(ageW, cellw.String(age(now.Sub(p.at))))
	}
	where := map[string]string{} // on the all tab, a run names its project
	for i, k := range keys {
		if s.project == "" && len(keys) > 2 {
			where[k] = names[i]
		}
	}
	// Each run of an author's chirps is a card raised off the sheet, edged
	// in the author's colour, never the colour of the run before; a reply
	// sits a step deeper, the picked chirp highest.
	card, deeper, edge := hoverBG, bgBtw, ""
	dayRule := func(at time.Time) string {
		label := dayLabel(at, now)
		return faint("── ") + dim(label) + faint(" "+strings.Repeat("─", max(0, w-4-cellw.String(label))))
	}
	var runAt []int // the line to pin over each: its run's name, or a reply's first line; -1 for gaps and days
	run, by := -1, "" // by is the run's author
	for i, p := range posts {
		add := func(l string) { lines, owner, runAt = append(lines, l), append(owner, i), append(runAt, run) }
		day := i == 0 || localDay(p.rootAt) != localDay(posts[i-1].rootAt)
		if i > 0 && (day || signs(posts, i)) {
			run = -1
			add("") // one blank line between runs, and before each day
		}
		if i == cursor {
			first = len(lines)
		}
		if day {
			add(dayRule(p.rootAt))
		}
		if (signs(posts, i) || i == 0) && p.author.Username() != by { // over a new day, the same author keeps their colour
			edge, by = tintBeside(p.author.Username(), edge, nextTint(posts[i+1:], p.author.Username())), p.author.Username()
		}
		tint := edge // a reply by another wears its own colour, never the run's
		if p.author.Username() != by {
			tint = tintBeside(p.author.Username(), edge, "")
		}
		if signs(posts, i) {
			proj, room := where[p.project], w-2
			if proj != "" {
				room -= cellw.String(proj) + 2
			}
			run = len(lines)
			add(onBg(card, paint(edge, "▎")+" "+spread(m.streamWho(p, edge, room), dim(proj), w-2), w))
		}
		bg, bar, inner := card, paint(edge, "▎"), deeper
		if i == cursor {
			bg, bar, inner = selBG, paint(cOrange, "▍"), selBG
		}
		head := run // a reply's later lines pin its own first, with its handle, not the run's name
		for j, l := range m.communityRows(p, tint, ageW, w-2, inner, now) {
			if add(onBg(bg, bar+" "+l, w)); p.reply && j == 0 {
				run = len(lines) - 1
			}
		}
		run = head
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
	shown := make([]int, 0, room)
	for j := s.scroll; j < min(len(lines), s.scroll+room); j++ {
		shown = append(shown, j)
	}
	// Scrolled, the view never opens on a gap under the tabs' own: a gap
	// before a run shows the run's day, one before a day the line above it.
	// Partway down a run, the run's name is pinned in the tabs' gap; partway
	// down a reply, its first line.
	if t := s.scroll; t > 0 && lines[t] == "" {
		if runAt[t+1] == t+1 {
			lines[t] = dayRule(posts[owner[t+1]].rootAt)
		} else {
			shown[0] = t - 1
		}
	}
	if len(shown) > 0 && runAt[shown[0]] >= 0 && runAt[shown[0]] < shown[0] {
		out[len(out)-1], s.rows[len(out)-1] = lines[runAt[shown[0]]], owner[runAt[shown[0]]]
	}
	for _, j := range shown {
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
