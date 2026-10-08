package ui

import (
	"hash/fnv"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
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
			out = append(out, streamPost{key: t.ID + "/" + strconv.Itoa(i), id: t.ID, title: t.Title, author: msg.Author, text: msg.Text, at: msg.At, reply: i > 0})
		}
	}
	slices.SortStableFunc(out, func(a, b streamPost) int { return a.at.Compare(b.at) })
	return out
}

// streamDock is the timeline docked at the very foot of the list, on the
// prompt: up to six posts, at most a third of the body; the list scrolls
// behind it. Nil when Twotter is off, quiet or the body too short.
func (m *Model) streamDock(w, bodyH int) (lines, keys []string) {
	return m.streamLines(w, min(streamDockPosts+1, bodyH/3))
}

// streamLines is the timeline in at most room rows under a title rule,
// newest at the bottom, one post a row. It stays faded until a post is new
// or the pointer is on it; the hovered post opens out over its neighbours,
// so the dock keeps its height and the row under the pointer stays put.
func (m *Model) streamLines(w, room int) (lines, keys []string) {
	if !m.store.Config.Feed || room < 2 || len(m.stream.posts) == 0 || w < 24 {
		return nil, nil
	}
	now := time.Now()
	inner := w - 2 // one column in, one clear of the divider
	var kept []streamPost
	seen := map[string]bool{}
	for i := len(m.stream.posts) - 1; i >= 0 && len(kept) < room-1; i-- {
		p := m.stream.posts[i]
		said := p.author.Username() + "\x00" + p.said()
		if seen[said] {
			continue // the same agent saying the same thing again
		}
		seen[said] = true
		kept = append([]streamPost{p}, kept...)
	}
	cols := streamColsOf(kept, inner, now)
	hot, hovered := strings.HasPrefix(m.hover, streamKeyPrefix), -1
	var body, bodyKeys []string
	for i, p := range kept {
		hot = hot || now.Sub(p.at) < streamFresh
		if m.hover == streamKeyPrefix+p.key {
			hovered = i
		}
		body, bodyKeys = append(body, m.streamRow(p, cols, 1, now)[0]), append(bodyKeys, streamKeyPrefix+p.key)
	}
	if hovered >= 0 {
		key := streamKeyPrefix + kept[hovered].key
		full := m.streamRow(kept[hovered], cols, room-1, now)
		for len(body) < len(full) { // too few posts to open over: the dock grows up
			body, bodyKeys = append([]string{""}, body...), append([]string{""}, bodyKeys...)
			hovered++
		}
		at := min(hovered, len(body)-len(full))
		for j, l := range full {
			body[at+j], bodyKeys[at+j] = hoverLine(l, inner), key
		}
	}
	title := " the feed 🐓 "
	lines = []string{faint("──") + paint(cText+bold, title) + faint(strings.Repeat("─", max(0, w-3-cellw.String(title))))}
	for _, l := range body {
		lines = append(lines, " "+fit(l, inner))
	}
	if !hot {
		for i, l := range lines {
			lines[i] = fadeText(l, streamFade)
		}
	}
	return lines, append([]string{streamKeyPrefix}, bodyKeys...)
}

// streamCols is where a row's columns sit: handles padded to one width, the
// text after them, the age right-aligned in a column of its own.
type streamCols struct{ handle, text, age int }

func streamColsOf(posts []streamPost, w int, now time.Time) streamCols {
	c := streamCols{age: 3}
	for _, p := range posts {
		c.handle = max(c.handle, cellw.String(streamHandle(p)))
		c.age = max(c.age, cellw.String(age(now.Sub(p.at))))
	}
	c.handle = min(c.handle, max(6, w/3))
	c.text = max(8, w-c.handle-c.age-4)
	return c
}

// streamRow is one post in at most rows lines: the handle in its colour,
// the text wrapped and the last line cut with …, its age on the first line.
func (m *Model) streamRow(p streamPost, c streamCols, rows int, now time.Time) []string {
	when := dim(age(now.Sub(p.at)))
	if now.Sub(p.at) < streamFresh {
		when = paint(cOrange, age(now.Sub(p.at)))
	}
	text := strings.Join(strings.Fields(communityText(p.said())), " ")
	wrapped := []string{text}
	if rows > 1 {
		if wrapped = wrap(text, c.text); len(wrapped) > rows {
			wrapped = append(wrapped[:rows-1], wrapped[rows-1]+" "+strings.Join(wrapped[rows:], " "))
		}
	}
	pad := strings.Repeat(" ", c.handle+2)
	out := make([]string, len(wrapped))
	for i, l := range wrapped {
		lead := pad
		if i == 0 {
			lead = fit(paint(handleColor(p)+bold, streamHandle(p)), c.handle) + "  "
		}
		out[i] = lead + fit(tagged(l), c.text)
		if i == 0 {
			out[i] += "  " + strings.Repeat(" ", c.age-cellw.String(age(now.Sub(p.at)))) + when
		}
	}
	return out
}

// handleColor is an author's own colour, the same wherever their name shows.
func handleColor(p streamPost) string {
	palette := []string{cBlue, cGreen, cYellow, cQueue, cOrange, cRed}
	h := fnv.New32a()
	h.Write([]byte(p.author.Username()))
	return palette[h.Sum32()%uint32(len(palette))]
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

// tagged paints a line's @mentions and #hashtags; the rest stays plain.
func tagged(line string) string {
	words := strings.Split(line, " ")
	for i, w := range words {
		switch {
		case len(w) > 1 && w[0] == '@':
			words[i] = paint(cBlue, w)
		case len(w) > 1 && w[0] == '#':
			words[i] = paint(cQueue, w)
		default:
			words[i] = paint(cSub, w)
		}
	}
	return strings.Join(words, " ")
}
