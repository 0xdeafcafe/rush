package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// --- what rush says ---
//
// rush says three kinds of thing, each in its own place:
//
//   - an answer to a key you pressed (said, or refused): the line under
//     the box, in plain text, gone in a few seconds;
//   - something that broke (failed): the same line in red, kept there
//     until you next press a key;
//   - something rush did on its own (news: a clean-up, a worktree kept,
//     work landed): never that line, and never beside a conversation,
//     since it isn't that agent's: under the feed at the foot of the
//     list, and a count in the header, until you've seen it.
//
// All of them go in the log, which #log opens.

type sayKind int

const (
	saySaid sayKind = iota
	sayRefused
	sayFailed
	sayNews
)

// logLine is one thing rush said, as the log keeps it.
type logLine struct {
	at   time.Time
	kind sayKind
	text string
	// ref is where a click on it goes (newsRef), for news.
	ref  string
	seen bool
}

// logKept is how many things the log keeps.
const logKept = 50

// flashFor is how long an answer stays on the line under the box.
const flashFor = 6 * time.Second

// flash answers a key you pressed: refused when it couldn't be done as
// asked, which is said the same way, with the way forward in its words.
func (m *Model) flash(s string, refused bool) {
	k := saySaid
	if refused {
		k = sayRefused
	}
	m.say(k, s, "")
}

// fail says something broke: in red, until the next key.
func (m *Model) fail(s string) {
	m.say(sayFailed, s, "")
	m.react(fxError)
}

// news says something rush did on its own, ref where a click on it goes.
// It doesn't take the line under the box: you may be typing there.
func (m *Model) news(s, ref string) {
	m.say(sayNews, s, ref)
}

// noted logs something rush did on its own that's expected, such as a
// queue going out: #log has it, the column and the header don't.
func (m *Model) noted(s string) {
	m.news(s, "")
	m.said[len(m.said)-1].seen = true
}

func (m *Model) say(k sayKind, s, ref string) {
	s = convo.KeyWord(s)
	now := time.Now()
	if k != sayNews {
		m.status, m.statusErr, m.statusAt = s, k == sayFailed, now
	}
	m.said = append(m.said, logLine{at: now, kind: k, text: s, ref: ref, seen: k != sayNews})
	if n := len(m.said) - logKept; n > 0 {
		m.said = append(m.said[:0], m.said[n:]...)
	}
}

// statusShown is whether the line under the box shows what rush said
// rather than its keys: an answer for a few seconds, a failure until a key.
func (m *Model) statusShown() bool {
	if m.status == "" {
		return false
	}
	return m.statusErr || m.snap.At.Sub(m.statusAt) < flashFor
}

// keyClearsFailure takes a failure off the line under the box: you've
// pressed a key since, so you've seen it. The log still has it.
func (m *Model) keyClearsFailure() {
	if m.statusErr {
		m.status, m.statusErr = "", false
	}
}

// unseenNews is the news you haven't seen yet, newest first.
func (m *Model) unseenNews() []int {
	var out []int
	for i := len(m.said) - 1; i >= 0; i-- {
		if s := m.said[i]; s.kind == sayNews && !s.seen {
			out = append(out, i)
		}
	}
	return out
}

// seeNews marks all news seen.
func (m *Model) seeNews() {
	for i := range m.said {
		m.said[i].seen = true
	}
}

// Refs a news item's click goes to.
const (
	newsRefProjects = "projects:" // then the project's folder
	newsKeyPrefix   = "rushnews:"
)

// openNewsRef goes where a news item points.
func (m *Model) openNewsRef(ref string) tea.Cmd {
	if dir, ok := strings.CutPrefix(ref, newsRefProjects); ok {
		m.goView(placeProjects)
		m.pickInProjects(dir)
		return m.projectsOpen()
	}
	return nil
}

// newsHead is the header's count of news unseen, "" with none.
func (m *Model) newsHead() string {
	n := len(m.unseenNews())
	if n == 0 {
		return ""
	}
	return paint(cSub, fmt.Sprintf("◇ %d from rush", n)) + dim(" · #log")
}

// newsDock is what rush did on its own that you haven't seen, under the
// feed at the foot of the list, newest at the bottom like the feed, one
// row each, each a click to where it's about. Seen once #log has been
// opened or one's clicked.
func (m *Model) newsDock(w int) (lines, keys []string) {
	idx := m.unseenNews()
	if len(idx) == 0 || w < 24 {
		return nil, nil
	}
	const most = 3
	title := " what rush did "
	lines = []string{faint("──") + paint(cText+bold, title) + faint(strings.Repeat("─", max(0, w-3-cellw.String(title))))}
	keys = []string{newsKeyPrefix + "all"}
	if len(idx) > most {
		lines, keys = append(lines, " "+fit(faint(fmt.Sprintf("and %d more · #log", len(idx)-most)), w-2)), append(keys, newsKeyPrefix+"all")
		idx = idx[:most]
	}
	for n := len(idx) - 1; n >= 0; n-- {
		i := idx[n]
		what, why, _ := strings.Cut(m.said[i].text, ": ")
		row := paint(cSub, "◇ ") + paint(cText, what)
		if why != "" {
			row += dim(" · " + why)
		}
		lines, keys = append(lines, " "+fit(row, w-2)), append(keys, fmt.Sprintf("%s%d", newsKeyPrefix, i))
	}
	return lines, keys
}

// clickNews takes a click on a row of newsDock.
func (m *Model) clickNews(key string) tea.Cmd {
	what, _ := strings.CutPrefix(key, newsKeyPrefix)
	if what == "all" {
		m.openLog()
		return nil
	}
	var i int
	if _, err := fmt.Sscan(what, &i); err != nil || i < 0 || i >= len(m.said) {
		return nil
	}
	m.said[i].seen = true
	return m.openNewsRef(m.said[i].ref)
}

// --- #log ---

// logSheet is the last things rush said, newest first: answers, failures
// and what it did on its own. Enter on one with somewhere to go goes there.
type logSheet struct{ cur int }

func (m *Model) openLog() {
	m.sheet = &logSheet{}
}

func (l *logSheet) width(m *Model) int { return min(110, m.w-4) }

// newest is the log's lines, newest first.
func (l *logSheet) newest(m *Model) []logLine {
	out := make([]logLine, len(m.said))
	for i, s := range m.said {
		out[len(out)-1-i] = s
	}
	return out
}

func (l *logSheet) body(m *Model, w, h int) []string {
	lines := l.newest(m)
	out := []string{sheetTitle("Log", fmt.Sprintf("the last %d things rush said", len(lines)), w), ""}
	if len(lines) == 0 {
		out = append(out, dim("nothing yet"))
	}
	l.cur = min(l.cur, max(0, len(lines)-1))
	from, to := window(len(lines), l.cur, max(1, h-5))
	for i, s := range lines[from:to] {
		mark, c := dim("· "), cText
		switch s.kind {
		case sayFailed:
			mark, c = paint(cRed, "✗ "), cRed
		case sayNews:
			mark = paint(cSub, "◇ ")
			if !s.seen {
				c = cText + bold
			}
		}
		line := faint(s.at.Local().Format("15:04")+"  ") + mark + paint(c, s.text)
		if s.ref != "" {
			line += dim("  ›")
		}
		out = append(out, sheetRow(line, from+i == l.cur, w))
	}
	return append(out, "", keysFit(w, "↑↓", "choose", "enter", "go to it", "esc", "close"))
}

func (l *logSheet) key(m *Model, _ tea.KeyPressMsg, s string) tea.Cmd {
	lines := l.newest(m)
	switch s {
	case "up", "k", "shift+tab":
		l.cur = max(0, l.cur-1)
	case "down", "j", "tab":
		l.cur = min(max(0, len(lines)-1), l.cur+1)
	case "enter":
		if l.cur < len(lines) && lines[l.cur].ref != "" {
			m.sheet = nil
			m.seeNews()
			return m.openNewsRef(lines[l.cur].ref)
		}
	case "esc", "ctrl+c", "q":
		m.sheet = nil
		m.seeNews()
	}
	return nil
}

// flashTone is a plugin's word: a failure when its tone is bad.
func (m *Model) flashTone(s string, bad bool) {
	if bad {
		m.fail(s)
		return
	}
	m.flash(s, false)
}
