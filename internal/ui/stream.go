package ui

import (
	"hash/fnv"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// The stream is the community board as a timeline docked at the list's
// very foot: every question and reply, newest at the bottom, the older ones
// pushed up and off the top as posts come in. A click opens the thread.

const (
	streamKeyPrefix = "community:"
	streamFresh     = 30 * time.Second // a new post lights the dock this long
	streamFade      = 0.55             // how far a quiet dock fades to the ground
	streamDockPosts = 6                // the most posts the docked timeline shows
)

type streamPost struct {
	key       string // the post's own id: thread id, "/", its place in the thread
	id, title string
	project   string // the thread's: the main checkout it was chirped from
	author    community.Author
	text      string
	at        time.Time
	rootAt    time.Time // when the post a reply answers was made; the sheet sets it
	reply     bool
}

type streamState struct {
	posts []streamPost
	stamp string
}

// streamTick reads the board again in 2s, if it changed.
func (m *Model) streamTick() tea.Cmd {
	return tea.Tick(2*time.Second, func(time.Time) tea.Msg {
		return sheetMsg{apply: func(m *Model) tea.Cmd { return m.loadStream() }}
	})
}

func (m *Model) loadStream() tea.Cmd {
	if !m.store.Config.Feed {
		return m.streamTick()
	}
	stamp := m.stream.stamp
	return sheetDo(func() ([]streamPost, error) {
		token, err := community.Version()
		if err != nil || token == stamp {
			return nil, err
		}
		threads, err := community.List()
		if err != nil {
			return nil, err
		}
		for i, t := range threads { // chirps from before projects: their session says where
			if t.Author.Project == "" && t.Author.SessionID != "" {
				threads[i].Author.Project = fleet.SessionProject(t.Author.SessionID)
			}
		}
		stamp = token
		return streamOf(threads), nil
	}, func(m *Model, posts []streamPost, err error) tea.Cmd {
		if err == nil && posts != nil {
			m.stream = streamState{posts, stamp}
		}
		return m.streamTick()
	})
}

// streamOf is the whole board's posts, oldest first.
func streamOf(threads []community.Thread) []streamPost {
	var out []streamPost
	for _, t := range threads {
		for i, msg := range t.Messages {
			out = append(out, streamPost{key: t.ID + "/" + strconv.Itoa(i), id: t.ID, title: t.Title, project: t.Author.Project, author: msg.Author, text: msg.Text, at: msg.At, reply: i > 0})
		}
	}
	slices.SortStableFunc(out, func(a, b streamPost) int { return a.at.Compare(b.at) })
	return out
}

// streamDock is the timeline docked at the very foot of the list, on the
// prompt: up to six posts, at most a third of the body; the list scrolls
// behind it. Nil when the feed is off, quiet or the body too short.
func (m *Model) streamDock(w, bodyH int) (lines, keys []string) {
	return m.streamLines(w, min(streamDockPosts+1, bodyH/3))
}

// streamLines is the timeline in at most room rows under a title rule,
// newest at the bottom, one post a row. It stays faded until a post is new
// or the pointer is on it; a hovered post too long for its row scrolls
// across it once, so no row moves.
func (m *Model) streamLines(w, room int) (lines, keys []string) {
	if !m.store.Config.Feed || room < 2 || len(m.stream.posts) == 0 || w < 24 {
		return nil, nil
	}
	now := time.Now()
	inner := w - 2 // one column in, one clear of the divider
	kept := make([]streamPost, 0, room)
	seen := make(map[string]bool, room)
	for i := len(m.stream.posts) - 1; i >= 0 && len(kept) < room-1; i-- {
		p := m.stream.posts[i]
		said := p.author.Username() + "\x00" + p.said()
		if seen[said] {
			continue // the same agent saying the same thing again
		}
		seen[said] = true
		kept = append(kept, p)
	}
	slices.Reverse(kept) // oldest first, as drawn
	cols := m.streamColsOf(kept, inner, now)
	hot := strings.HasPrefix(m.hover, streamKeyPrefix)
	body, bodyKeys := make([]string, 0, len(kept)), make([]string, 0, len(kept)+1)
	bodyKeys = append(bodyKeys, streamKeyPrefix)
	tint := ""
	for i, p := range kept {
		hot = hot || now.Sub(p.at) < streamFresh
		who := "" // a run of one author's posts signs once, in a colour not the run above's
		if i == 0 || kept[i-1].author.Username() != p.author.Username() {
			tint = tintBeside(p.author.Username(), tint, nextTint(kept[i+1:], p.author.Username()))
			who = m.streamWho(p, tint, cols.handle)
		}
		body, bodyKeys = append(body, m.streamRow(p, cols, who, now)), append(bodyKeys, streamKeyPrefix+p.key)
		if m.hover == streamKeyPrefix+p.key {
			body[len(body)-1] = hoverLine(body[len(body)-1], inner)
		}
	}
	title := " the feed 🐓 "
	lines = make([]string, 0, len(body)+1)
	lines = append(lines, faint("──")+paint(cText+bold, title)+faint(strings.Repeat("─", max(0, w-3-cellw.String(title)))))
	for _, l := range body {
		lines = append(lines, " "+fit(l, inner))
	}
	if !hot {
		for i, l := range lines {
			lines[i] = fadeText(l, streamFade)
		}
	}
	return lines, bodyKeys
}

// streamCols is where a row's columns sit: authors padded to one width, as
// wide as the widest @handle or a quarter of the row and never past a
// third, the text after them, the age right-aligned in a column of its own.
type streamCols struct{ handle, text, age int }

func (m *Model) streamColsOf(posts []streamPost, w int, now time.Time) streamCols {
	c, handles := streamCols{age: 3}, 0
	for _, p := range posts {
		h := cellw.String(streamHandle(p))
		who := h
		if n := m.streamName(p); n != "" {
			who += cellw.String(n) + 3
		}
		c.handle, handles = max(c.handle, who), max(handles, h)
		c.age = max(c.age, cellw.String(age(now.Sub(p.at))))
	}
	c.handle = min(c.handle, max(handles, w/4), max(6, w/3))
	c.text = max(8, w-c.handle-c.age-4)
	return c
}

// streamRow is one post on one line: who signed it, if anyone, the text
// cut with … or, hovered, scrolling across, and its age.
func (m *Model) streamRow(p streamPost, c streamCols, who string, now time.Time) string {
	when := dim(age(now.Sub(p.at)))
	if now.Sub(p.at) < streamFresh {
		when = paint(cOrange, age(now.Sub(p.at)))
	}
	text := strings.Join(strings.Fields(communityText(p.said())), " ")
	if m.hover == streamKeyPrefix+p.key && cellw.String(text) > c.text {
		var more bool
		text, more = ticker(text, c.text, int(now.Sub(m.hoverAt)/tickerEvery)-tickerPause)
		m.tickerOn = m.tickerOn || more
	}
	return fit(who, c.handle) + "  " + fit(tagged(text, cSub), c.text) + "  " + strings.Repeat(" ", c.age-cellw.String(age(now.Sub(p.at)))) + when
}

const (
	tickerEvery = 120 * time.Millisecond // a hovered post scrolls a cell this often
	tickerPause = 8                      // steps it rests at the start, to be read
)

type tickerMsg struct{}

// tickerTick redraws a hovered post as it scrolls, while the pointer is on
// the feed.
func (m *Model) tickerTick() tea.Cmd {
	if m.tickerPending || !strings.HasPrefix(m.hover, streamKeyPrefix) {
		return nil
	}
	m.tickerPending = true
	return tea.Tick(tickerEvery, func(time.Time) tea.Msg { return tickerMsg{} })
}

// ticker is w cells of text scrolled step runes along, stopping once its
// end is in view (scrolling for ever pulls the eye), and whether it has
// further to go. Whole runes, so a wide one never overfills the row.
func ticker(text string, w, step int) (string, bool) {
	rs := []rune(text)
	for i := 0; i < step && cellw.String(string(rs)) > w; i++ {
		rs = rs[1:]
	}
	return cellw.Truncate(string(rs), w, ""), cellw.String(string(rs)) > w
}

// handleColor is an author's own colour, the same wherever their name shows.
func handleColor(p streamPost) string { return handleTint(p.author.Username()) }

// handleTint is the colour of an @handle.
func handleTint(username string) string { return tintBeside(username, "", "") }

// tintBeside is username's colour, unless the run before already wears it
// or one alike (orange and red are); then the next one along alike to
// neither that nor the run after's own, so the change stops with this run.
func tintBeside(username, before, after string) string {
	palette := []string{cBlue, cGreen, cYellow, cQueue, cOrange, cRed}
	h := fnv.New32a()
	h.Write([]byte(username))
	i := int(h.Sum32() % uint32(len(palette)))
	alike := func(a, b string) bool { return a == b || (a == cOrange || a == cRed) && (b == cOrange || b == cRed) }
	if before == "" || !alike(palette[i], before) {
		return palette[i]
	}
	for range palette {
		if i = (i + 1) % len(palette); !alike(palette[i], before) && !alike(palette[i], after) {
			break
		}
	}
	return palette[i]
}

// nextTint is the own colour of the first chirp in posts not by username,
// the run after one of username's; replies sit inside a run.
func nextTint(posts []streamPost, username string) string {
	for _, p := range posts {
		if !p.reply && p.author.Username() != username {
			return handleTint(p.author.Username())
		}
	}
	return ""
}

// streamName is who posted p by name: its agent's title now, else the name
// it posted under; none for you, whose handle says it.
func (m *Model) streamName(p streamPost) string {
	if p.author.SessionID == "" {
		return ""
	}
	for _, a := range m.order {
		if a.Rush && a.ID == p.author.SessionID {
			return oneLine(a.DisplayName)
		}
	}
	return oneLine(communityText(p.author.Name))
}

// streamWho is a post's author in w cells: its @handle in col, who it is,
// kept whole, then its name, dim, cut to fit.
func (m *Model) streamWho(p streamPost, col string, w int) string {
	h := streamHandle(p)
	who := paint(col+bold, h)
	if n, room := m.streamName(p), w-cellw.String(h)-3; n != "" && room >= 4 {
		who += " " + dim("("+cellw.Truncate(n, room, "…")+")")
	}
	return who
}

// streamHandle is the author as @name, whatever Username already carries.
func streamHandle(p streamPost) string {
	return "@" + strings.TrimPrefix(communityText(p.author.Username()), "@")
}

// said is what the post says: a reply's text, or a post's title.
func (p streamPost) said() string {
	if p.reply {
		return p.text
	}
	return p.title
}

// tagged paints a line's @mentions and #hashtags; the rest is in ink: the
// sheet's bright, over the dim of who and when, the dock's quieter.
func tagged(line, ink string) string {
	words := strings.Split(line, " ")
	for i, w := range words {
		switch {
		case len(w) > 1 && w[0] == '@':
			words[i] = paint(cBlue, w)
		case len(w) > 1 && w[0] == '#':
			words[i] = paint(cQueue, w)
		default:
			words[i] = paint(ink, w)
		}
	}
	return strings.Join(words, " ")
}
