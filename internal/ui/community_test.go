package ui

import (
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/charmbracelet/x/ansi"
)

func communityApply(t *testing.T, m *Model, cmd tea.Cmd) tea.Cmd {
	t.Helper()
	if cmd == nil {
		return nil
	}
	msg, ok := cmd().(sheetMsg)
	if !ok {
		t.Fatal("expected async board result")
	}
	return msg.apply(m)
}

func feedModel(t *testing.T) *Model {
	t.Helper()
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(100, 35)
	now := time.Now()
	m.stream.posts = []streamPost{
		{key: "a/0", id: "a", title: "go test hangs on fswait", author: community.Author{Name: "Worker"}, at: now.Add(-time.Minute)},
		{key: "b/0", id: "b", title: "flaky lint", author: community.Author{Name: "Other"}, at: now.Add(-30 * time.Second)},
		{key: "a/1", id: "a", title: "go test hangs on fswait", author: community.Author{SessionID: "s1"}, text: "-race and a 5s timeout found it", at: now, reply: true},
	}
	return m
}

func TestFeedSwitch(t *testing.T) {
	m := feedModel(t)
	if m.openCommunity(""); m.sheet != nil {
		t.Fatal("the sheet opened while the feed is off")
	}
	m.openCommunity("on")
	if !m.store.Config.Feed {
		t.Fatal("#feed on did not turn it on")
	}
	if m.openCommunity(""); m.sheet == nil {
		t.Fatal("the sheet did not open once on")
	}
	m.openCommunity("off")
	if lines, _ := m.streamLines(80, 30); m.store.Config.Feed || lines != nil {
		t.Fatal("#feed off left the stream showing")
	}
}

func TestFeedTimelineIsOneView(t *testing.T) {
	m := feedModel(t)
	m.openCommunity("on")
	m.openCommunity("")
	s := m.community
	text := ansi.Strip(strings.Join(s.body(m, 90, 30), "\n"))
	first, reply, other := strings.Index(text, "go test hangs"), strings.Index(text, "↳ @"), strings.Index(text, "flaky lint")
	if first < 0 || reply < first || other < reply || !strings.Contains(text, "-race and a 5s") {
		t.Fatalf("posts should read in time order, each reply set in under its post:\n%s", text)
	}
	if c := s.cursor(threaded(m.stream.posts)); c != 2 {
		t.Fatalf("the cursor should start on the newest post, at %d", c)
	}
	for _, key := range []string{"enter", "space"} {
		s.composing, s.picked, s.follow = false, "a/1", false
		s.key(m, tea.KeyPressMsg{}, "up")
		if s.key(m, tea.KeyPressMsg{}, key); !s.composing || s.replyTo != "a" {
			t.Fatalf("%s should reply to the picked post: %+v", key, s)
		}
	}
	m.Update(tea.PasteMsg{Content: "board draft"})
	if string(s.input) != "board draft" {
		t.Fatal("paste should land in the reply")
	}
	s.key(m, tea.KeyPressMsg{}, "esc")
	if s.key(m, tea.KeyPressMsg{}, "esc"); m.sheet != nil {
		t.Fatal("esc should close the sheet")
	}
}

func TestFeedPostRendersWithinBounds(t *testing.T) {
	for _, size := range [][2]int{{44, 24}, {140, 45}} {
		m := feedModel(t)
		m.stream.posts[0].title = "question\x1b[2J" + strings.Repeat(" long", 60)
		m.openCommunity("on")
		m.openCommunity("new")
		lines := m.community.body(m, size[0]-10, size[1]-6)
		if len(lines) > size[1]-6 {
			t.Fatalf("%v clips controls", size)
		}
		for _, line := range lines {
			if ansi.StringWidth(line) > size[0]-10 || strings.Contains(line, "\x1b[2J") {
				t.Fatalf("bad line: %q", line)
			}
		}
	}
}

// Neighbouring runs never share a colour, nor orange beside red; an author
// keeps their own colour unless the run before already wears it, and one
// that can't keeps clear of the next run's, so the change stops there.
func TestFeedNeighboursNeverShareATint(t *testing.T) {
	alike := func(a, b string) bool { return a == b || (a == cOrange || a == cRed) && (b == cOrange || b == cRed) }
	names := []string{"ann", "bo", "cy", "di", "ed", "flo", "gus", "hal", "ivy", "jo"}
	for _, a := range names {
		for _, b := range names {
			for _, c := range names {
				before, own, after, got := handleTint(a), handleTint(b), handleTint(c), tintBeside(b, handleTint(a), handleTint(c))
				switch {
				case alike(got, before):
					t.Fatalf("%s beside %s: alike, %q", b, a, got)
				case !alike(own, before) && got != own:
					t.Fatalf("%s should keep its own colour beside %s", b, a)
				case got != own && alike(got, after):
					t.Fatalf("%s, changed, should keep clear of %s after it", b, c)
				}
			}
		}
	}
}

// wrap never splits an @mention at its hyphens.
func TestWrapKeepsMentionsWhole(t *testing.T) {
	for w := 15; w < 30; w++ {
		for _, l := range wrap("ping @scout-dodo-two about it", w) {
			if strings.Contains(l, "@") && !strings.Contains(l, "@scout-dodo-two") {
				t.Fatalf("at %d the mention broke: %q", w, wrap("ping @scout-dodo-two about it", w))
			}
		}
	}
}

func TestFeedTagsMentionsAndHashtags(t *testing.T) {
	line := tagged("ping @worker about #flaky tests", cSub)
	if ansi.Strip(line) != "ping @worker about #flaky tests" {
		t.Fatalf("tagging changed the text: %q", ansi.Strip(line))
	}
	if !strings.Contains(line, cBlue+"@worker"+reset) || !strings.Contains(line, cQueue+"#flaky"+reset) {
		t.Fatalf("mention and hashtag should be coloured: %q", line)
	}
}

func TestFeedDaySeparatorsAndJump(t *testing.T) {
	m := feedModel(t)
	now := time.Now()
	m.stream.posts[0].at = now.AddDate(0, 0, -3)
	m.stream.posts[1].at = now.AddDate(0, 0, -1)
	m.openCommunity("on")
	m.openCommunity("")
	s := m.community
	text := ansi.Strip(strings.Join(s.body(m, 90, 30), "\n"))
	old := "── " + now.AddDate(0, 0, -3).Format("Mon 2 Jan") + " ──"
	if a, b := strings.Index(text, old), strings.Index(text, "── Yesterday ──"); a < 0 || a > b || strings.Contains(text, "── Today") {
		t.Fatalf("want a separator per day, in order:\n%s", text)
	}
	if !strings.Contains(text, "⇧↑ ⇧↓ Day") {
		t.Fatalf("the footer should name the day keys:\n%s", text)
	}
	for _, step := range []struct{ key, want string }{{"shift+up", "a/0"}, {"shift+up", "a/0"}, {"shift+down", "b/0"}, {"shift+down", "b/0"}, {"up", "a/1"}, {"shift+down", "b/0"}} {
		if s.key(m, tea.KeyPressMsg{}, step.key); s.picked != step.want {
			t.Fatalf("%s: picked %q, want %q", step.key, s.picked, step.want)
		}
	}
}

func TestFeedStreamClickOpensSheetAtPost(t *testing.T) {
	m := feedModel(t)
	m.openCommunity("on")
	_, keys := m.streamLines(80, 30)
	clicked := ""
	for _, k := range keys {
		if strings.HasSuffix(k, "b/0") {
			clicked = k
		}
	}
	if clicked == "" || keys[0] != streamKeyPrefix {
		t.Fatalf("stream rows and frame title should carry board keys: %q", keys)
	}
	id, _ := strings.CutPrefix(clicked, streamKeyPrefix)
	if m.openCommunity(id); m.sheet == nil || m.community.cursor(threaded(m.stream.posts)) != 2 {
		t.Fatal("a click on a stream post should open the sheet with that post picked")
	}
	m.stream.posts = append([]streamPost{{key: "z/0", id: "z", at: time.Now().Add(-time.Hour)}}, m.stream.posts...)
	if m.community.picked != "b/0" || m.community.cursor(threaded(m.stream.posts)) != 3 {
		t.Fatal("the pick should follow its post when the board shifts")
	}
}

// The sheet marks the picked post, colours handles as the dock does, and
// shows every line of a long post.
func TestFeedSheetPicksAndWraps(t *testing.T) {
	m := feedModel(t)
	m.stream.posts[1].title = strings.Repeat("a long flaky lint story ", 12) + "the end"
	m.openCommunity("on")
	m.openCommunity("b/0")
	lines := m.community.body(m, 90, 40)
	text := ansi.Strip(strings.Join(lines, "\n"))
	marked := false
	for _, l := range strings.Split(text, "\n") {
		marked = marked || strings.HasPrefix(l, "▍") && strings.Contains(l, "the end")
	}
	if !marked {
		t.Fatalf("want the picked post marked and wrapped in full:\n%s", text)
	}
	if !strings.Contains(strings.Join(lines, ""), handleColor(m.stream.posts[1])+bold+"@") {
		t.Fatal("handles should wear their colour in the sheet too")
	}
	if !strings.HasPrefix(text, "the feed 🐓") {
		t.Fatalf("the sheet is called the feed:\n%s", text)
	}
}

func TestFeedSplitsByProject(t *testing.T) {
	m := feedModel(t)
	now := time.Now()
	m.stream.posts[0].project, m.stream.posts[2].project = "/src/rush", "/src/rush"
	m.stream.posts[1].project = "/src/haven"
	m.stream.posts = append(m.stream.posts, // a lone repeat, hidden
		streamPost{key: "c/0", id: "c", project: "/src/haven", title: "flaky lint", author: community.Author{Name: "Other"}, at: now})
	m.openCommunity("on")
	m.openCommunity("")
	s := m.community
	s.project = "/src/rush"
	text := ansi.Strip(strings.Join(s.body(m, 90, 30), "\n"))
	if !strings.Contains(text, " haven   rush   all ") || !strings.Contains(text, "go test hangs") || strings.Contains(text, "flaky lint") {
		t.Fatalf("the rush tab should show only rush's chirps, under its tabs:\n%s", text)
	}
	s.key(m, tea.KeyPressMsg{}, "[")
	text = ansi.Strip(strings.Join(s.body(m, 90, 30), "\n"))
	if s.project != "/src/haven" || strings.Contains(text, "go test hangs") || strings.Count(text, "flaky lint") != 1 {
		t.Fatalf("[ should step to haven, its repeat hidden:\n%s", text)
	}
}

// An author's run of chirps is signed once, @handle then their name; another
// author or a new day signs again, and a reply carries its own @handle.
func TestFeedSignsEachRunOnce(t *testing.T) {
	m := feedModel(t)
	now := time.Now()
	hub, other, you := community.Author{SessionID: "s-hub", Name: "Hub Header Sims Integration"}, community.Author{Name: "Other"}, community.Author{Name: "You"}
	m.stream.posts = []streamPost{
		{key: "a/0", id: "a", title: "first tip", author: hub, at: now.Add(-5 * time.Minute)},
		{key: "b/0", id: "b", title: "second tip", author: hub, at: now.Add(-4 * time.Minute)},
		{key: "b/1", id: "b", text: "thanks", author: you, at: now.Add(-3 * time.Minute), reply: true},
		{key: "c/0", id: "c", title: "third tip", author: hub, at: now.Add(-2 * time.Minute)},
		{key: "d/0", id: "d", title: "a word from another", author: other, at: now.Add(-time.Minute)},
		{key: "e/0", id: "e", title: "back again", author: hub, at: now},
	}
	m.openCommunity("on")
	m.openCommunity("")
	text := ansi.Strip(strings.Join(m.community.body(m, 90, 40), "\n"))
	sign := "▎ " + streamHandle(m.stream.posts[0]) + " (Hub Header Sims Integration)"
	if strings.Count(text, sign) != 2 || strings.Count(text, "@other") != 1 {
		t.Fatalf("want hub signed over each of its two runs, once each:\n%s", text)
	}
	if !strings.Contains(text, "↳ @you  thanks") || strings.Index(text, "second tip") > strings.Index(text, "thanks") || strings.Index(text, "thanks") > strings.Index(text, "third tip") {
		t.Fatalf("a reply sits under its chirp with its own handle, inside the run:\n%s", text)
	}
	var gaps []int // the blank lines between the day's rule and the newest chirp
	rows := strings.Split(text, "\n")
	for i, l := range rows[slices.IndexFunc(rows, func(l string) bool { return strings.HasPrefix(l, "── ") }):] {
		if strings.Contains(l, "back again") {
			break
		}
		if strings.TrimSpace(l) == "" {
			gaps = append(gaps, i)
		}
	}
	if len(gaps) != 2 || gaps[1]-gaps[0] != 3 {
		t.Fatalf("want one blank line between runs and none inside one: %v\n%s", gaps, text)
	}
}

// A chirp's wrapped lines share its first line's left edge; a reply's sit
// under its @handle. None runs past the row.
func TestFeedRowsWrapAligned(t *testing.T) {
	now := time.Now()
	long := strings.Repeat("words that wrap ", 8)
	for _, p := range []streamPost{
		{title: long, author: community.Author{Name: "Worker"}, at: now},
		{text: long, author: community.Author{Name: "You"}, at: now, reply: true},
	} {
		rows := (&Model{}).communityRows(p, cBlue, 3, 50, selBG, now)
		first := ansi.Strip(rows[0])
		edge := cellw.String(first[:strings.Index(first, "words")])
		if p.reply {
			edge = cellw.String(first[:strings.Index(first, "@")])
		}
		for _, r := range rows[1:] {
			if r = ansi.Strip(r); cellw.String(r)-cellw.String(strings.TrimLeft(r, " ")) != edge || cellw.String(r) > 50 {
				t.Fatalf("wrapped lines should start at column %d and fit 50:\n%s", edge, ansi.Strip(strings.Join(rows, "\n")))
			}
		}
		if len(rows) < 3 {
			t.Fatalf("want it wrapped: %q", rows)
		}
	}
}

// Scrolled to the newest, the newest sits on the footer and the view opens
// right under the tabs' gap on a name or a day: partway down a run, its
// name is pinned in that gap.
func TestFeedViewPinsItsRunsName(t *testing.T) {
	m := feedModel(t)
	now := time.Now()
	m.stream.posts = nil
	for i := range 14 {
		id := string(rune('a' + i))
		m.stream.posts = append(m.stream.posts, streamPost{key: id + "/0", id: id, title: id + " " + strings.Repeat("a long chirp that wraps ", 4), author: community.Author{Name: []string{"Ann", "Bo"}[i/3%2]}, at: now.Add(time.Duration(i-14) * time.Minute)})
		if i%4 == 1 {
			m.stream.posts = append(m.stream.posts, streamPost{key: id + "/1", id: id, text: strings.Repeat("a long reply that wraps ", 5), author: community.Author{Name: "Cy"}, at: now.Add(time.Duration(i-14)*time.Minute + time.Second), reply: true})
		}
	}
	m.openCommunity("on")
	m.openCommunity("")
	posts, pinned := m.community.shown(m), 0
	for h := 14; h < 24; h++ {
		lines := m.community.body(m, 60, h)
		gap, top := ansi.Strip(lines[1]), ansi.Strip(lines[2])
		view := ansi.Strip(strings.Join(lines, "\n"))
		switch {
		case gap == strings.Repeat(" ", 60) && !regexp.MustCompile(`^(▎ @|── )`).MatchString(top):
			t.Fatalf("at %d tall the view opens partway down a run without its name:\n%s", h, view)
		case gap != strings.Repeat(" ", 60):
			pinned++
			pin, top := m.community.rows[1], m.community.rows[2]
			for top > 0 && posts[top].reply && posts[top].key != posts[pin].key {
				top-- // a reply's first line pins its run's name: the run's author
			}
			if !regexp.MustCompile(`^▎ (@|\s*\d+[smhd]\s+↳ @)`).MatchString(gap) || posts[pin].author != posts[top].author {
				t.Fatalf("at %d tall the run's own name should be pinned under the tabs:\n%s", h, view)
			}
		}
		foot := slices.IndexFunc(lines, func(l string) bool { return strings.HasPrefix(ansi.Strip(l), "↑ ↓") })
		if last := ansi.Strip(lines[foot-2]); !strings.Contains(last, "wraps") || !strings.HasPrefix(last, "▍") {
			t.Fatalf("at %d tall the newest should sit on the footer:\n%s", h, view)
		}
	}
	if pinned == 0 {
		t.Fatal("some height should open partway down a run, its name pinned")
	}
}
