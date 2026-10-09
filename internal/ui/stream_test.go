package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

func TestStreamNewestAtBottom(t *testing.T) {
	t0 := time.Now().Add(-time.Hour)
	threads := []community.Thread{
		{ID: "b", Title: "second question", Messages: []community.Message{{Text: "second question", At: t0.Add(2 * time.Minute)}}},
		{ID: "a", Title: "first question", Messages: []community.Message{{Text: "first", At: t0}, {Text: "a late reply", At: t0.Add(5 * time.Minute)}}},
	}
	m := &Model{store: &state.Store{}}
	m.store.Config.Feed = true
	m.stream.posts = streamOf(threads)
	lines, keys := m.streamLines(60, 40)
	text := ansi.Strip(strings.Join(lines, "\n"))
	first, second, reply := strings.Index(text, "first question"), strings.Index(text, "second question"), strings.Index(text, "a late reply")
	if !(first >= 0 && first < second && second < reply) {
		t.Fatalf("want oldest at the top, newest at the bottom:\n%s", text)
	}
	if !strings.HasPrefix(text, "── the feed 🐓 ") {
		t.Fatalf("posts should sit under the feed rule:\n%s", text)
	}
	if keys[len(keys)-1] != streamKeyPrefix+"a/1" {
		t.Fatalf("last row should open the late reply, got %q", keys[len(keys)-1])
	}
	// Short on room, the oldest go off the top first.
	lines, _ = m.streamLines(60, 6)
	if text := ansi.Strip(strings.Join(lines, "\n")); !strings.Contains(text, "a late reply") || strings.Contains(text, "first question\n") && strings.Index(text, "first") < strings.Index(text, "second") {
		t.Fatalf("small room should keep the newest:\n%s", text)
	}
}

// The side shows each post on one line, one @, and an agent repeating
// itself once.
func TestStreamOneLineNoRepeats(t *testing.T) {
	t0 := time.Now().Add(-time.Hour)
	me := community.Author{Name: "hub-header-sims"}
	tip := func(id string, at time.Duration) community.Thread {
		return community.Thread{ID: id, Title: "Tip: verify sim env", Author: me, Messages: []community.Message{{Text: "Tip: verify sim env", Author: me, At: t0.Add(at)}}}
	}
	m := &Model{store: &state.Store{}}
	m.store.Config.Feed = true
	m.stream.posts = streamOf([]community.Thread{tip("a", 0), tip("b", time.Minute)})
	lines, _ := m.streamLines(60, 40)
	text := ansi.Strip(strings.Join(lines, "\n"))
	if strings.Count(text, "Tip: verify sim env") != 1 || strings.Contains(text, "@@") || len(lines) != 2 {
		t.Fatalf("want one one-line post under the rule, with one @:\n%s", text)
	}
}

// The Settings row turns Twotter on and off, off by default.
func TestTwatterSettingsRow(t *testing.T) {
	m := &Model{store: &state.Store{}}
	row := func() setting {
		for _, sec := range m.generalSections() {
			for _, r := range sec.rows {
				if r.label == "The feed" {
					return r
				}
			}
		}
		t.Fatal("no Twotter row in General")
		return setting{}
	}
	if r := row(); r.value != "off" {
		t.Fatalf("default should be off, got %q", r.value)
	}
	row().set("on")
	if !m.store.Config.Feed || row().value != "on" {
		t.Fatal("the row did not turn Twotter on")
	}
	row().set("off")
	if m.store.Config.Feed {
		t.Fatal("the row did not turn Twotter off")
	}
}

// The timeline docks at the very foot of the list, on the prompt, and the
// list is clipped above it rather than pushing it down.
func TestStreamDocksAtTheFoot(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.store.Config.Feed = true
	t0 := time.Now().Add(-time.Hour)
	var threads []community.Thread
	for i := 0; i < 12; i++ {
		title := "post number " + string(rune('a'+i))
		threads = append(threads, community.Thread{ID: title, Title: title, Messages: []community.Message{{Text: title, At: t0.Add(time.Duration(i) * time.Minute)}}})
	}
	m.stream.posts = streamOf(threads)
	rows := strings.Split(ansi.Strip(m.render()), "\n")
	top, bottom, lastAgent := -1, -1, -1
	for i, r := range rows {
		switch {
		case strings.Contains(r, "── the feed 🐓 "):
			top = i
		case top >= 0 && strings.Contains(r, "post number "):
			bottom = i
		case strings.Contains(r, "agent number "):
			lastAgent = i
		}
	}
	if top < 0 || bottom < 0 {
		t.Fatalf("no docked timeline:\n%s", strings.Join(rows, "\n"))
	}
	if n := bottom - top; n != streamDockPosts {
		t.Fatalf("want %d docked posts, got %d", streamDockPosts, n)
	}
	if !strings.Contains(strings.Join(rows[top:bottom+1], "\n"), "post number l") || strings.Contains(strings.Join(rows[top:bottom+1], "\n"), "post number a ") {
		t.Fatalf("the dock should keep the newest posts:\n%s", strings.Join(rows[top:bottom+1], "\n"))
	}
	if under := []rune(rows[bottom+1]); strings.TrimSpace(string(under[:min(len(under), m.listW)])) == "" {
		t.Fatalf("the dock should sit right on the prompt, no gap:\n%s", strings.Join(rows[top:bottom+3], "\n"))
	}
	if lastAgent >= top || top-lastAgent > 2 {
		t.Fatalf("the list should fill to the dock and stop: last agent row %d, dock at %d", lastAgent, top)
	}
	m.store.Config.Feed = false
	if strings.Contains(ansi.Strip(m.render()), "the feed") {
		t.Fatal("off, there is no dock")
	}
}

func dockModel(texts ...string) *Model {
	m := &Model{store: &state.Store{}}
	m.store.Config.Feed = true
	t0 := time.Now().Add(-time.Hour)
	names := []string{"Worker", "a-much-longer-name", "Bo"}
	for i, text := range texts {
		id := string(rune('a' + i))
		m.stream.posts = append(m.stream.posts, streamPost{key: id + "/0", id: id, title: text, author: community.Author{Name: names[i%3]}, at: t0.Add(time.Duration(i) * time.Minute)})
	}
	return m
}

// Quiet, the dock is faded; a new post or the pointer on it lights it.
func TestStreamFadesUnlessNewOrHovered(t *testing.T) {
	m := dockModel("old news", "older news")
	quiet, _ := m.streamLines(60, 8)
	m.hover = streamKeyPrefix + "a/0"
	hovered, _ := m.streamLines(60, 8)
	m.hover = ""
	m.stream.posts[1].at = time.Now()
	fresh, _ := m.streamLines(60, 8)
	if quiet[1] == fresh[1] || quiet[0] == hovered[0] {
		t.Fatal("a quiet dock should render faded, and light up when new or hovered")
	}
	if !strings.Contains(fresh[1], handleColor(m.stream.posts[0])) {
		t.Fatal("fresh, a handle wears its own colour")
	}
}

// Handles pad to one column so the text lines up; ages right-align.
func TestStreamColumnsAlign(t *testing.T) {
	m := dockModel("first thing", "second thing", "third")
	lines, _ := m.streamLines(70, 8)
	col, end := -1, -1
	for _, l := range lines[1:] {
		l = ansi.Strip(l)
		c := max(strings.Index(l, "first"), strings.Index(l, "second"), strings.Index(l, "third"))
		if col >= 0 && (c != col || cellw.String(strings.TrimRight(l, " ")) != end) {
			t.Fatalf("columns do not line up:\n%s", ansi.Strip(strings.Join(lines, "\n")))
		}
		col, end = c, cellw.String(strings.TrimRight(l, " "))
	}
	if handleColor(m.stream.posts[0]) != handleColor(m.stream.posts[0]) {
		t.Fatal("an author's colour should be stable")
	}
}

// A long post ends in …; hovered, it scrolls across its own row, so the
// dock keeps its rows and none of them move.
func TestStreamHoverTickersLongPost(t *testing.T) {
	long := "the build cache was stale because the lockfile changed under it and nothing told the other agents so here is the fix in full"
	m := dockModel("short one", "another", long)
	before, _ := m.streamLines(50, 8)
	if l := ansi.Strip(before[len(before)-1]); !strings.Contains(l, "…") || strings.Contains(l, "in full") {
		t.Fatalf("a long post should end in …: %q", l)
	}
	m.hover, m.hoverAt = streamKeyPrefix+"c/0", time.Now()
	rest, keys := m.streamLines(50, 8)
	if len(rest) != len(before) || ansi.Strip(rest[1]) != ansi.Strip(before[1]) || !m.tickerOn {
		t.Fatalf("hovered, the post should stay on its row and tick:\n%s", ansi.Strip(strings.Join(rest, "\n")))
	}
	m.hoverAt = time.Now().Add(-time.Duration(tickerPause+10) * tickerEvery)
	moved, _ := m.streamLines(50, 8)
	if l := ansi.Strip(moved[len(moved)-1]); strings.Contains(l, "the build") || !strings.Contains(l, "cache was stale") {
		t.Fatalf("after a while the post should have scrolled along: %q", l)
	}
	if keys[len(keys)-1] != m.hover {
		t.Fatalf("the ticking row should keep the hover: %q", keys)
	}
}

// A post scrolls once, wide runes and all, and rests on its end: the
// row never overfills, and the tick loop stops when nothing moves.
func TestTickerScrollsOnceThenStops(t *testing.T) {
	text := "日本語の投稿 then plain words to the end"
	for step := 0; step < 60; step++ {
		got, more := ticker(text, 12, step)
		if cellw.String(got) > 12 {
			t.Fatalf("step %d overfills: %q", step, got)
		}
		if !more {
			if !strings.HasSuffix(text, got) {
				t.Fatalf("stopped short of the end: %q", got)
			}
			m := dockModel(text)
			m.hover, m.tickerPending = streamKeyPrefix+"a/0", true
			if _, cmd := m.Update(tickerMsg{}); cmd != nil {
				t.Fatal("with nothing scrolling the ticker should stop")
			}
			return
		}
	}
	t.Fatal("the ticker never reached the end")
}

// Twotter answers to its old names too.
func TestTwotterAliases(t *testing.T) {
	for _, name := range []string{"twotter", "twatter", "twitter", "twattr", "community"} {
		if fleetAliases[name] != "feed" {
			t.Fatalf("#%s should open Twotter", name)
		}
	}
}

// The dock signs a run of one author's posts once, @handle first.
func TestStreamSignsARunOnce(t *testing.T) {
	m := dockModel("first thing", "second thing", "third")
	m.stream.posts[1].author = m.stream.posts[0].author
	lines, _ := m.streamLines(70, 8)
	rows := strings.Split(ansi.Strip(strings.Join(lines[1:], "\n")), "\n")
	if !strings.HasPrefix(rows[0], " @worker") || strings.TrimSpace(rows[1])[:6] != "second" || !strings.HasPrefix(rows[2], " @bo") {
		t.Fatalf("want the second post by the same author unsigned:\n%s", strings.Join(rows, "\n"))
	}
}
